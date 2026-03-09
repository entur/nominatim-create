//! OSM PBF to Nominatim NDJSON converter.
//!
//! Converts OSM PBF files in 4 passes:
//! 1. Relations pass: Collect admin boundaries and POI relation member way IDs
//! 2. Ways pass: Collect all needed node IDs (streets, admin ways, POI ways, relation member ways)
//! 3. Nodes pass: Fetch node coordinates for all needed nodes
//! 4. Ways+Relations pass: Build indexes, calculate centroids, and convert POIs

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use osmpbf::{Element, ElementReader};

use crate::common::country::Country;
use crate::common::extra::Extra;
use crate::common::geo;
use crate::common::importance::ImportanceCalculator;
use crate::config::Config;
use crate::target::json_writer::JsonWriter;
use crate::target::nominatim_id::NominatimId;
use crate::target::nominatim_place::*;

pub fn convert(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let converter = OsmConverter::new(config.clone());
    converter.convert(input, output, is_appending)
}

// ---------------------------------------------------------------------------
// Coordinate
// ---------------------------------------------------------------------------

/// Geographic coordinate (latitude, longitude).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coordinate {
    pub lat: f64,
    pub lon: f64,
}

impl Coordinate {
    pub fn centroid(&self) -> Vec<f64> {
        vec![round6(self.lon), round6(self.lat)]
    }

    pub fn bbox(&self) -> Vec<f64> {
        vec![round6(self.lon), round6(self.lat), round6(self.lon), round6(self.lat)]
    }
}

fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

// ---------------------------------------------------------------------------
// CoordinateStore – open-addressing hash map storing coords as delta-encoded ints
// ---------------------------------------------------------------------------

pub struct CoordinateStore {
    ids: Vec<i64>,
    delta_lats: Vec<i32>,
    delta_lons: Vec<i32>,
    size: usize,
}

const BASE_LAT: f64 = -90.0;
const BASE_LON: f64 = -180.0;
const COORD_SCALE: f64 = 1e5; // ~1.1 m precision
const LOAD_FACTOR: f64 = 0.7;

impl CoordinateStore {
    pub fn new(initial_capacity: usize) -> Self {
        Self {
            ids: vec![0; initial_capacity],
            delta_lats: vec![0; initial_capacity],
            delta_lons: vec![0; initial_capacity],
            size: 0,
        }
    }

    pub fn put(&mut self, id: i64, coord: Coordinate) {
        if self.size as f64 >= self.ids.len() as f64 * LOAD_FACTOR {
            self.resize();
        }
        let capacity = self.ids.len();
        let mut index = Self::hash(id, capacity);
        while self.ids[index] != 0 && self.ids[index] != id {
            index = (index + 1) % capacity;
        }
        if self.ids[index] == 0 {
            self.size += 1;
        }
        self.ids[index] = id;
        self.delta_lats[index] = ((coord.lat - BASE_LAT) * COORD_SCALE) as i32;
        self.delta_lons[index] = ((coord.lon - BASE_LON) * COORD_SCALE) as i32;
    }

    pub fn get(&self, id: i64) -> Option<Coordinate> {
        let capacity = self.ids.len();
        let mut index = Self::hash(id, capacity);
        while self.ids[index] != 0 {
            if self.ids[index] == id {
                let lat = BASE_LAT + self.delta_lats[index] as f64 / COORD_SCALE;
                let lon = BASE_LON + self.delta_lons[index] as f64 / COORD_SCALE;
                return Some(Coordinate { lat, lon });
            }
            index = (index + 1) % capacity;
        }
        None
    }

    fn hash(id: i64, capacity: usize) -> usize {
        (id.wrapping_mul(2_654_435_761).rem_euclid(capacity as i64)) as usize
    }

    fn resize(&mut self) {
        let old_ids = std::mem::take(&mut self.ids);
        let old_lats = std::mem::take(&mut self.delta_lats);
        let old_lons = std::mem::take(&mut self.delta_lons);
        let new_cap = old_ids.len() * 2;

        self.ids = vec![0; new_cap];
        self.delta_lats = vec![0; new_cap];
        self.delta_lons = vec![0; new_cap];
        self.size = 0;

        for i in 0..old_ids.len() {
            if old_ids[i] != 0 {
                let lat = BASE_LAT + old_lats[i] as f64 / COORD_SCALE;
                let lon = BASE_LON + old_lons[i] as f64 / COORD_SCALE;
                self.put(old_ids[i], Coordinate { lat, lon });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BoundingBox
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
}

impl BoundingBox {
    pub fn contains(&self, coord: &Coordinate) -> bool {
        coord.lat >= self.min_lat
            && coord.lat <= self.max_lat
            && coord.lon >= self.min_lon
            && coord.lon <= self.max_lon
    }

    pub fn area(&self) -> f64 {
        (self.max_lat - self.min_lat) * (self.max_lon - self.min_lon)
    }

    pub fn from_coordinates(coords: &[Coordinate]) -> Option<Self> {
        if coords.is_empty() {
            return None;
        }
        let mut min_lat = f64::MAX;
        let mut max_lat = f64::MIN;
        let mut min_lon = f64::MAX;
        let mut max_lon = f64::MIN;
        for c in coords {
            if c.lat < min_lat {
                min_lat = c.lat;
            }
            if c.lat > max_lat {
                max_lat = c.lat;
            }
            if c.lon < min_lon {
                min_lon = c.lon;
            }
            if c.lon > max_lon {
                max_lon = c.lon;
            }
        }
        Some(Self {
            min_lat,
            max_lat,
            min_lon,
            max_lon,
        })
    }
}

// ---------------------------------------------------------------------------
// AdministrativeBoundary
// ---------------------------------------------------------------------------

pub struct AdministrativeBoundary {
    pub name: String,
    pub admin_level: i32,
    pub ref_code: Option<String>,
    pub country: Country,
    pub centroid: Coordinate,
    pub bbox: Option<BoundingBox>,
    pub boundary_nodes: Vec<Coordinate>,
}

impl AdministrativeBoundary {
    /// Ray-casting algorithm – check if point is inside the polygon.
    pub fn contains_point(&self, coord: &Coordinate) -> bool {
        if self.boundary_nodes.len() < 3 {
            return false;
        }
        let mut inside = false;
        let n = self.boundary_nodes.len();
        let mut j = n - 1;
        for i in 0..n {
            let ci = &self.boundary_nodes[i];
            let cj = &self.boundary_nodes[j];
            if (ci.lon > coord.lon) != (cj.lon > coord.lon)
                && coord.lat
                    < (cj.lat - ci.lat) * (coord.lon - ci.lon) / (cj.lon - ci.lon) + ci.lat
            {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    /// Euclidean distance from the given point to this boundary's centroid.
    pub fn distance_to_point(&self, coord: &Coordinate) -> f64 {
        let d_lat = coord.lat - self.centroid.lat;
        let d_lon = coord.lon - self.centroid.lon;
        (d_lat * d_lat + d_lon * d_lon).sqrt()
    }

    pub fn is_in_bounding_box(&self, coord: &Coordinate) -> bool {
        self.bbox.as_ref().is_some_and(|b| b.contains(coord))
    }
}

// ---------------------------------------------------------------------------
// AdministrativeBoundaryIndex
// ---------------------------------------------------------------------------

pub const ADMIN_LEVEL_COUNTY: i32 = 4;
pub const ADMIN_LEVEL_MUNICIPALITY: i32 = 7;

const CACHE_PRECISION: f64 = 100.0;

pub struct AdministrativeBoundaryIndex {
    counties: Vec<AdministrativeBoundary>,
    municipalities: Vec<AdministrativeBoundary>,
    lookup_cache: HashMap<(i64, i64), (Option<usize>, Option<usize>)>,
}

impl AdministrativeBoundaryIndex {
    pub fn new() -> Self {
        Self {
            counties: Vec::new(),
            municipalities: Vec::new(),
            lookup_cache: HashMap::new(),
        }
    }

    pub fn add_boundary(&mut self, boundary: AdministrativeBoundary) {
        match boundary.admin_level {
            ADMIN_LEVEL_COUNTY => self.counties.push(boundary),
            ADMIN_LEVEL_MUNICIPALITY => self.municipalities.push(boundary),
            _ => {}
        }
    }

    /// Finds both the county and municipality for the given coordinates.
    /// Results are cached with 0.01 degree precision.
    pub fn find_county_and_municipality(
        &mut self,
        coord: &Coordinate,
    ) -> (Option<&AdministrativeBoundary>, Option<&AdministrativeBoundary>) {
        let key = (
            round_cache_coord(coord.lat),
            round_cache_coord(coord.lon),
        );

        // Check cache – we store indices rather than references to satisfy the borrow checker.
        if !self.lookup_cache.contains_key(&key) {
            let county_idx = Self::find_best_match_idx(&self.counties, coord);
            let muni_idx = Self::find_best_match_idx(&self.municipalities, coord);
            self.lookup_cache.insert(key, (county_idx, muni_idx));
        }

        let &(county_idx, muni_idx) = self.lookup_cache.get(&key).unwrap();
        (
            county_idx.map(|i| &self.counties[i]),
            muni_idx.map(|i| &self.municipalities[i]),
        )
    }

    /// 3-tier lookup: ray-casting, bounding box + closest centroid, closest centroid.
    fn find_best_match_idx(
        boundaries: &[AdministrativeBoundary],
        coord: &Coordinate,
    ) -> Option<usize> {
        if boundaries.is_empty() {
            return None;
        }

        // Tier 1: ray-casting polygon containment
        let containing: Vec<usize> = boundaries
            .iter()
            .enumerate()
            .filter(|(_, b)| b.contains_point(coord))
            .map(|(i, _)| i)
            .collect();

        if !containing.is_empty() {
            return containing
                .into_iter()
                .min_by(|&a, &b| {
                    let area_a = boundaries[a].bbox.map_or(f64::MAX, |bb| bb.area());
                    let area_b = boundaries[b].bbox.map_or(f64::MAX, |bb| bb.area());
                    area_a.partial_cmp(&area_b).unwrap_or(std::cmp::Ordering::Equal)
                });
        }

        // Tier 2: bounding box + closest centroid
        let in_bbox: Vec<usize> = boundaries
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_in_bounding_box(coord))
            .map(|(i, _)| i)
            .collect();

        if !in_bbox.is_empty() {
            return in_bbox
                .into_iter()
                .min_by(|&a, &b| {
                    let da = boundaries[a].distance_to_point(coord);
                    let db = boundaries[b].distance_to_point(coord);
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                });
        }

        // Tier 3: closest centroid (last resort)
        boundaries
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = a.distance_to_point(coord);
                let db = b.distance_to_point(coord);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    pub fn get_statistics(&self) -> String {
        format!(
            "Loaded {} counties and {} municipalities",
            self.counties.len(),
            self.municipalities.len()
        )
    }
}

fn round_cache_coord(v: f64) -> i64 {
    (v * CACHE_PRECISION) as i64
}

// ---------------------------------------------------------------------------
// StreetSegment
// ---------------------------------------------------------------------------

pub struct StreetSegment {
    pub name: String,
    pub start: Coordinate,
    pub end: Coordinate,
}

impl StreetSegment {
    /// Perpendicular distance from a point to this line segment, in meters.
    pub fn distance_to_point(&self, point: &Coordinate) -> f64 {
        // Approximate metric scaling at point latitude
        let lat_scale = 111_000.0;
        let lon_scale = 111_000.0 * point.lat.to_radians().cos();

        let px = (point.lon - self.start.lon) * lon_scale;
        let py = (point.lat - self.start.lat) * lat_scale;
        let dx = (self.end.lon - self.start.lon) * lon_scale;
        let dy = (self.end.lat - self.start.lat) * lat_scale;

        let seg_len_sq = dx * dx + dy * dy;
        if seg_len_sq == 0.0 {
            return (px * px + py * py).sqrt();
        }

        let t = ((px * dx + py * dy) / seg_len_sq).clamp(0.0, 1.0);
        let closest_x = t * dx;
        let closest_y = t * dy;
        let dist_x = px - closest_x;
        let dist_y = py - closest_y;
        (dist_x * dist_x + dist_y * dist_y).sqrt()
    }
}

// ---------------------------------------------------------------------------
// StreetIndex – grid-based spatial index
// ---------------------------------------------------------------------------

const GRID_SIZE: f64 = 0.005;
const MAX_SEARCH_RADIUS: i32 = 10;
const MAX_DISTANCE_METERS: f64 = 100.0;

pub const HIGHWAY_TYPES: &[&str] = &[
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "unclassified",
    "residential",
    "living_street",
    "pedestrian",
    "service",
    "road",
];

pub struct StreetIndex {
    segments: Vec<StreetSegment>,
    spatial_index: HashMap<(i32, i32), Vec<usize>>,
    lookup_cache: RefCell<HashMap<(i32, i32), Option<String>>>,
}

const STREET_CACHE_PRECISION: f64 = 1000.0;

impl StreetIndex {
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
            spatial_index: HashMap::new(),
            lookup_cache: RefCell::new(HashMap::new()),
        }
    }

    /// Adds a street's segments to the index.
    pub fn add_street(&mut self, name: &str, coordinates: &[Coordinate]) {
        if coordinates.len() < 2 {
            return;
        }
        for i in 0..coordinates.len() - 1 {
            let start = coordinates[i];
            let end = coordinates[i + 1];
            let segment = StreetSegment {
                name: name.to_string(),
                start,
                end,
            };
            let seg_idx = self.segments.len();
            self.segments.push(segment);

            let min_lat_cell = (start.lat.min(end.lat) / GRID_SIZE) as i32;
            let max_lat_cell = (start.lat.max(end.lat) / GRID_SIZE) as i32;
            let min_lon_cell = (start.lon.min(end.lon) / GRID_SIZE) as i32;
            let max_lon_cell = (start.lon.max(end.lon) / GRID_SIZE) as i32;

            for lat_cell in min_lat_cell..=max_lat_cell {
                for lon_cell in min_lon_cell..=max_lon_cell {
                    self.spatial_index
                        .entry((lat_cell, lon_cell))
                        .or_default()
                        .push(seg_idx);
                }
            }
        }
    }

    /// Finds the nearest street name using expanding ring search, with caching.
    pub fn find_nearest_street(&self, coord: &Coordinate) -> Option<String> {
        let cache_key = (
            (coord.lat * STREET_CACHE_PRECISION) as i32,
            (coord.lon * STREET_CACHE_PRECISION) as i32,
        );
        if let Some(cached) = self.lookup_cache.borrow().get(&cache_key) {
            return cached.clone();
        }
        let result = self.find_nearest_street_uncached(coord);
        self.lookup_cache.borrow_mut().insert(cache_key, result.clone());
        result
    }

    fn find_nearest_street_uncached(&self, coord: &Coordinate) -> Option<String> {
        let lat_cell = (coord.lat / GRID_SIZE) as i32;
        let lon_cell = (coord.lon / GRID_SIZE) as i32;

        let mut nearest_name: Option<&str> = None;
        let mut nearest_distance = f64::MAX;

        for radius in 0..=MAX_SEARCH_RADIUS {
            let mut found_in_ring = false;

            for d_lat in -radius..=radius {
                for d_lon in -radius..=radius {
                    // Only check cells on the ring boundary (skip interior)
                    if radius > 0 && d_lat.abs() != radius && d_lon.abs() != radius {
                        continue;
                    }

                    let cell = (lat_cell + d_lat, lon_cell + d_lon);
                    if let Some(indices) = self.spatial_index.get(&cell) {
                        for &idx in indices {
                            let segment = &self.segments[idx];
                            let distance = segment.distance_to_point(coord);
                            if distance < nearest_distance {
                                nearest_distance = distance;
                                nearest_name = Some(&segment.name);
                                found_in_ring = true;
                            }
                        }
                    }
                }
            }

            // If we found something and the next ring can't possibly be closer, stop
            if found_in_ring
                && nearest_distance < (radius + 1) as f64 * GRID_SIZE * 111_000.0
            {
                break;
            }
        }

        if nearest_distance <= MAX_DISTANCE_METERS {
            nearest_name.map(|s| s.to_string())
        } else {
            None
        }
    }

    pub fn get_statistics(&self) -> String {
        format!(
            "Loaded {} street segments in {} grid cells",
            self.segments.len(),
            self.spatial_index.len()
        )
    }
}

// ---------------------------------------------------------------------------
// OSMPopularityCalculator
// ---------------------------------------------------------------------------

struct POIFilter {
    key: String,
    value: String,
    priority: i32,
}

pub struct OsmPopularityCalculator {
    filters: Vec<POIFilter>,
    default_value: f64,
}

impl OsmPopularityCalculator {
    pub fn new(config: &Config) -> Self {
        let filters = config
            .osm
            .filters
            .iter()
            .map(|f| POIFilter {
                key: f.key.clone(),
                value: f.value.clone(),
                priority: f.priority,
            })
            .collect();
        Self {
            filters,
            default_value: config.osm.default_value,
        }
    }

    /// Returns `default_value * highest_matching_priority`, or 0.0 if nothing matches.
    pub fn calculate_popularity(&self, tags: &BTreeMap<&str, &str>) -> f64 {
        let highest = self
            .filters
            .iter()
            .filter(|f| tags.get(f.key.as_str()) == Some(&f.value.as_str()))
            .map(|f| f.priority)
            .max();

        match highest {
            Some(p) => self.default_value * p as f64,
            None => 0.0,
        }
    }

    /// Returns true if this key/value pair is in the filter list.
    pub fn has_filter(&self, key: &str, value: &str) -> bool {
        self.filters
            .iter()
            .any(|f| f.key == key && f.value == value)
    }
}

// ---------------------------------------------------------------------------
// GeometryCalculator
// ---------------------------------------------------------------------------

fn calculate_centroid(coords: &[Coordinate]) -> Option<Coordinate> {
    if coords.is_empty() {
        return None;
    }
    let n = coords.len() as f64;
    let lat = coords.iter().map(|c| c.lat).sum::<f64>() / n;
    let lon = coords.iter().map(|c| c.lon).sum::<f64>() / n;
    Some(Coordinate { lat, lon })
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

use crate::common::util::titleize;

use crate::common::text::join_osm_values;

/// Extract a 2-letter country code from OSM admin relation tags.
fn extract_country_code(tags: &HashMap<&str, &str>) -> Option<Country> {
    let iso = tags
        .get("ISO3166-2")
        .or_else(|| tags.get("ISO3166-2-lvl4"))
        .or_else(|| tags.get("ISO3166-2:lvl4"))
        .or_else(|| tags.get("is_in:country_code"))
        .or_else(|| tags.get("country_code"));

    if let Some(code) = iso {
        let two_letter = &code[..code.len().min(2)];
        if let Some(c) = Country::parse(Some(two_letter)) {
            return Some(c);
        }
    }

    // If ref is all digits, assume Norway
    if let Some(ref_val) = tags.get("ref")
        && ref_val.chars().all(|c| c.is_ascii_digit()) {
            return Some(Country::no());
        }

    None
}

/// Replace colons with dots (for category IDs).
fn as_category(s: &str) -> String {
    s.replace(':', ".")
}

// ---------------------------------------------------------------------------
// Category / source / layer constants
// ---------------------------------------------------------------------------

const LEGACY_SOURCE_WHOSONFIRST: &str = "legacy.source.whosonfirst";
const LEGACY_LAYER_ADDRESS: &str = "legacy.layer.address";
const OSM_POI: &str = "osm.public_transport.poi";
const LEGACY_CATEGORY_PREFIX: &str = "legacy.category.";
const COUNTRY_PREFIX: &str = "country.";
const COUNTY_ID_PREFIX: &str = "county_gid.";
const LOCALITY_ID_PREFIX: &str = "locality_gid.";
const SOURCE_OSM: &str = "openstreetmap";

const ACCURACY_POINT: &str = "point";
const ACCURACY_POLYGON: &str = "polygon";

const OBJECT_TYPE_NODE: &str = "N";
const OBJECT_TYPE_WAY: &str = "W";
const OBJECT_TYPE_RELATION: &str = "R";

// ---------------------------------------------------------------------------
// OsmEntityConverter
// ---------------------------------------------------------------------------

struct OsmEntityConverter<'a> {
    nodes_coords: &'a CoordinateStore,
    way_centroids: &'a CoordinateStore,
    admin_boundary_index: &'a mut AdministrativeBoundaryIndex,
    street_index: &'a StreetIndex,
    popularity_calculator: &'a OsmPopularityCalculator,
    importance_calc: ImportanceCalculator,
    config: &'a Config,
}

impl<'a> OsmEntityConverter<'a> {
    /// Filter tags to only those matching configured filters (sorted by key for deterministic order).
    fn filter_tags<'t>(&self, tags: &HashMap<&'t str, &'t str>) -> BTreeMap<&'t str, &'t str> {
        tags.iter()
            .filter(|(k, v)| self.popularity_calculator.has_filter(k, v))
            .map(|(&k, &v)| (k, v))
            .collect()
    }

    fn convert_node(
        &mut self,
        id: i64,
        lat: f64,
        lon: f64,
        all_tags: &HashMap<&str, &str>,
    ) -> Option<NominatimPlace> {
        let name = *all_tags.get("name")?;
        if name.is_empty() {
            return None;
        }
        let tags = self.filter_tags(all_tags);
        let coord = Coordinate { lat, lon };
        Some(self.create_place_content(
            id,
            &tags,
            name,
            OBJECT_TYPE_NODE,
            ACCURACY_POINT,
            coord,
            None,
            all_tags,
        ))
    }

    fn convert_way(
        &mut self,
        id: i64,
        all_tags: &HashMap<&str, &str>,
    ) -> Option<NominatimPlace> {
        let name = *all_tags.get("name")?;
        if name.is_empty() {
            return None;
        }
        let coord = self.way_centroids.get(id)?;
        let tags = self.filter_tags(all_tags);
        Some(self.create_place_content(
            id,
            &tags,
            name,
            OBJECT_TYPE_WAY,
            ACCURACY_POLYGON,
            coord,
            None,
            all_tags,
        ))
    }

    fn convert_relation(
        &mut self,
        id: i64,
        member_node_ids: &[i64],
        member_way_ids: &[i64],
        all_tags: &HashMap<&str, &str>,
    ) -> Option<NominatimPlace> {
        let name = *all_tags.get("name")?;
        if name.is_empty() {
            return None;
        }

        let mut member_coords = Vec::new();
        for &nid in member_node_ids {
            if let Some(c) = self.nodes_coords.get(nid) {
                member_coords.push(c);
            }
        }
        for &wid in member_way_ids {
            if let Some(c) = self.way_centroids.get(wid) {
                member_coords.push(c);
            }
        }
        if member_coords.is_empty() {
            return None;
        }

        let centroid = calculate_centroid(&member_coords)?;
        let tags = self.filter_tags(all_tags);

        let fallback_county =
            if tags.get("type") == Some(&"boundary") && tags.get("boundary") == Some(&"administrative") {
                Some(titleize(name))
            } else {
                None
            };

        Some(self.create_place_content(
            id,
            &tags,
            name,
            OBJECT_TYPE_RELATION,
            ACCURACY_POLYGON,
            centroid,
            fallback_county.as_deref(),
            all_tags,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn create_place_content(
        &mut self,
        entity_id: i64,
        tags: &BTreeMap<&str, &str>,
        name: &str,
        object_type: &str,
        accuracy: &str,
        centroid: Coordinate,
        fallback_county: Option<&str>,
        all_tags: &HashMap<&str, &str>,
    ) -> NominatimPlace {
        let (county, municipality) =
            self.admin_boundary_index.find_county_and_municipality(&centroid);

        let country = determine_country(county, municipality, all_tags, &centroid);

        let visible_categories: Vec<String> = {
            let mut cats = vec![
                LEGACY_SOURCE_WHOSONFIRST.to_string(),
                LEGACY_LAYER_ADDRESS.to_string(),
                OSM_POI.to_string(),
                format!("{}poi", LEGACY_CATEGORY_PREFIX),
            ];
            for (_, &v) in tags.iter() {
                cats.push(format!("{}{}", LEGACY_CATEGORY_PREFIX, v));
            }
            cats
        };

        let osm_id = format!("OSM:TopographicPlace:{}", entity_id);

        // Alt names
        let alt_name_keys = ["alt_name", "old_name", "no:name", "loc_name", "short_name"];
        let visible_alt_names: Vec<String> = alt_name_keys
            .iter()
            .filter_map(|&k| tags.get(k).copied())
            .filter(|&v| !v.is_empty() && v != name)
            .map(|s| s.to_string())
            .collect();

        let mut indexed_alt_names: Vec<String> = visible_alt_names.clone();
        indexed_alt_names.push(osm_id.clone());

        let en_name = tags.get("en:name").copied().map(|s| s.to_string());

        let county_gid = county
            .and_then(|c| c.ref_code.as_ref())
            .map(|r| format!("KVE:TopographicPlace:{}", r));
        let locality_gid = municipality
            .and_then(|m| m.ref_code.as_ref())
            .map(|r| format!("KVE:TopographicPlace:{}", r));
        let locality = municipality.map(|m| titleize(&m.name));

        let street = self.street_index.find_nearest_street(&centroid);

        let county_name = county
            .map(|c| titleize(&c.name))
            .or_else(|| fallback_county.map(|s| s.to_string()));

        let address = Address {
            street,
            city: locality.clone(),
            county: county_name,
        };

        let extra = Extra {
            id: Some(osm_id.clone()),
            source: Some(SOURCE_OSM.to_string()),
            accuracy: Some(accuracy.to_string()),
            country_a: country.as_ref().map(|c| c.three_letter_code.clone()),
            county_gid: county_gid.clone(),
            locality: locality.clone(),
            locality_gid: locality_gid.clone(),
            tags: join_osm_values(&visible_categories),
            alt_name: join_osm_values(
                &visible_alt_names,
            ),
            ..Extra::default()
        };

        // Build indexed categories
        let mut indexed_categories = visible_categories.clone();
        if let Some(ref c) = country {
            indexed_categories.push(format!("{}{}", COUNTRY_PREFIX, c.name));
        }
        if let Some(ref gid) = county_gid {
            indexed_categories.push(format!("{}{}", COUNTY_ID_PREFIX, as_category(gid)));
        }
        if let Some(ref gid) = locality_gid {
            indexed_categories.push(format!("{}{}", LOCALITY_ID_PREFIX, as_category(gid)));
        }
        let nominatim_id = NominatimId::Osm.create_from_i64(entity_id);
        let rank_address = self.determine_rank_address(tags);
        let importance = self.calculate_importance(tags);

        let content = PlaceContent {
            place_id: nominatim_id,
            object_type: object_type.to_string(),
            object_id: nominatim_id,
            categories: indexed_categories,
            rank_address,
            importance,
            parent_place_id: Some(0),
            name: Some(Name {
                name: Some(name.to_string()),
                name_en: en_name,
                alt_name: join_osm_values(&indexed_alt_names),
            }),
            housenumber: None,
            address,
            postcode: None,
            country_code: country.map(|c| c.name.clone()),
            centroid: centroid.centroid(),
            bbox: centroid.bbox(),
            extra,
        };

        NominatimPlace {
            type_: "Place".to_string(),
            content: vec![content],
        }
    }

    fn determine_rank_address(&self, tags: &BTreeMap<&str, &str>) -> i32 {
        let ra = &self.config.osm.rank_address;
        if tags.contains_key("boundary") {
            ra.boundary
        } else if tags.contains_key("place") {
            ra.place
        } else if tags.contains_key("road") {
            ra.road
        } else if tags.contains_key("building") {
            ra.building
        } else {
            ra.poi
        }
    }

    fn calculate_importance(&self, tags: &BTreeMap<&str, &str>) -> RawNumber {
        let popularity = self.popularity_calculator.calculate_popularity(tags);
        RawNumber::from_f64_6dp(self.importance_calc.calculate_importance(popularity))
    }
}

fn determine_country(
    county: Option<&AdministrativeBoundary>,
    municipality: Option<&AdministrativeBoundary>,
    tags: &HashMap<&str, &str>,
    coord: &Coordinate,
) -> Option<Country> {
    county
        .map(|c| c.country.clone())
        .or_else(|| municipality.map(|m| m.country.clone()))
        .or_else(|| {
            tags.get("addr:country")
                .and_then(|code| Country::parse(Some(code)))
        })
        .or_else(|| {
            let c = crate::common::coordinate::Coordinate::new(coord.lat, coord.lon);
            geo::get_country(&c)
        })
}

// ---------------------------------------------------------------------------
// Intermediate data collected across passes
// ---------------------------------------------------------------------------

struct AdminRelationData {
    name: String,
    admin_level: i32,
    ref_code: String,
    way_ids: Vec<i64>,
    country: Country,
}

struct StreetWayData {
    name: String,
    node_ids: Vec<i64>,
}

// ---------------------------------------------------------------------------
// OsmConverter – main 4-pass converter
// ---------------------------------------------------------------------------

pub struct OsmConverter {
    config: Config,
}

impl OsmConverter {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Convert an OSM PBF file to Nominatim NDJSON.
    pub fn convert(
        &self,
        input: &Path,
        output: &Path,
        is_appending: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert!(input.exists(), "Input file does not exist: {:?}", input);

        let mut nodes_coords = CoordinateStore::new(500_000);
        let mut way_centroids = CoordinateStore::new(50_000);
        let mut admin_boundary_index = AdministrativeBoundaryIndex::new();
        let mut street_index = StreetIndex::new();
        let popularity_calculator = OsmPopularityCalculator::new(&self.config);

        // =================================================================
        // Pass 1: Relations – collect admin boundaries and POI relation member IDs
        // =================================================================
        eprintln!("Pass 1/4: Scanning relations for admin boundaries and POI relations...");

        let mut admin_relations: Vec<AdminRelationData> = Vec::new();
        let mut poi_relation_member_way_ids: HashSet<i64> = HashSet::new();
        let mut poi_relation_node_ids: HashSet<i64> = HashSet::new();

        {
            let reader = ElementReader::from_path(input)?;
            reader.for_each(|element| {
                if let Element::Relation(relation) = element {
                    let tags: HashMap<&str, &str> =
                        relation.tags().collect();

                    if tags.get("boundary") == Some(&"administrative") {
                        if let Some(admin_level_str) = tags.get("admin_level")
                            && let Ok(admin_level) = admin_level_str.parse::<i32>()
                                && (admin_level == ADMIN_LEVEL_COUNTY
                                    || admin_level == ADMIN_LEVEL_MUNICIPALITY)
                                {
                                    let name = tags.get("name").map(|s| s.to_string());
                                    let ref_code = tags.get("ref").map(|s| s.to_string());
                                    let country = extract_country_code(&tags);

                                    if let (Some(name), Some(ref_code), Some(country)) =
                                        (name, ref_code, country)
                                    {
                                        let way_ids: Vec<i64> = relation
                                            .members()
                                            .filter(|m| m.member_type == osmpbf::RelMemberType::Way)
                                            .map(|m| m.member_id)
                                            .collect();

                                        admin_relations.push(AdminRelationData {
                                            name,
                                            admin_level,
                                            ref_code,
                                            way_ids,
                                            country,
                                        });
                                    }
                                }

                        // Collect node members from admin relations
                        for member in relation.members() {
                            if member.member_type == osmpbf::RelMemberType::Node {
                                poi_relation_node_ids.insert(member.member_id);
                            }
                        }
                    }

                    // Check for POI relation (has name and matching tags)
                    if tags.contains_key("name")
                        && tags
                            .iter()
                            .any(|(k, v)| popularity_calculator.has_filter(k, v))
                    {
                        for member in relation.members() {
                            match member.member_type {
                                osmpbf::RelMemberType::Way => {
                                    poi_relation_member_way_ids.insert(member.member_id);
                                }
                                osmpbf::RelMemberType::Node => {
                                    poi_relation_node_ids.insert(member.member_id);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            })?;
        }

        eprintln!(
            "  Found {} admin boundary relations",
            admin_relations.len()
        );
        eprintln!(
            "  Found {} POI relation member ways",
            poi_relation_member_way_ids.len()
        );

        // =================================================================
        // Pass 2: Ways – collect all required node IDs and way metadata
        // =================================================================
        eprintln!("Pass 2/4: Scanning ways for streets, admin boundaries, and POIs...");

        let admin_way_ids: HashSet<i64> = admin_relations
            .iter()
            .flat_map(|r| r.way_ids.iter().copied())
            .collect();

        let mut street_ways: Vec<StreetWayData> = Vec::new();
        let mut needed_node_ids: HashSet<i64> = poi_relation_node_ids.clone();
        let mut poi_way_ids: HashSet<i64> = HashSet::new();
        let mut admin_way_node_ids: HashMap<i64, Vec<i64>> = HashMap::new();

        {
            let reader = ElementReader::from_path(input)?;
            reader.for_each(|element| {
                if let Element::Way(way) = element {
                    let tags: HashMap<&str, &str> = way.tags().collect();
                    let node_ids: Vec<i64> = way.refs().collect();

                    // Street way?
                    if let Some(name) = tags.get("name")
                        && let Some(highway) = tags.get("highway")
                            && HIGHWAY_TYPES.contains(highway) {
                                street_ways.push(StreetWayData {
                                    name: name.to_string(),
                                    node_ids: node_ids.clone(),
                                });
                                needed_node_ids.extend(&node_ids);
                            }

                    // Admin boundary way?
                    if admin_way_ids.contains(&way.id()) {
                        admin_way_node_ids.insert(way.id(), node_ids.clone());
                        needed_node_ids.extend(&node_ids);
                    }

                    // POI relation member way?
                    if poi_relation_member_way_ids.contains(&way.id()) {
                        needed_node_ids.extend(&node_ids);
                    }

                    // Direct POI way?
                    if tags.contains_key("name")
                        && tags
                            .iter()
                            .any(|(k, v)| popularity_calculator.has_filter(k, v))
                    {
                        poi_way_ids.insert(way.id());
                        needed_node_ids.extend(&node_ids);
                    }
                }
            })?;
        }

        eprintln!("  Found {} street ways", street_ways.len());
        eprintln!("  Found {} POI ways", poi_way_ids.len());
        eprintln!(
            "  Total unique node coordinates needed: {}",
            needed_node_ids.len()
        );

        // =================================================================
        // Pass 3: Nodes – collect coordinates for all needed nodes
        // =================================================================
        eprintln!("Pass 3/4: Collecting node coordinates...");

        {
            let reader = ElementReader::from_path(input)?;
            reader.for_each(|element| {
                match element {
                    Element::Node(node) => {
                        if needed_node_ids.contains(&node.id()) {
                            nodes_coords.put(
                                node.id(),
                                Coordinate {
                                    lat: node.lat(),
                                    lon: node.lon(),
                                },
                            );
                        }
                    }
                    Element::DenseNode(node) => {
                        if needed_node_ids.contains(&node.id) {
                            nodes_coords.put(
                                node.id,
                                Coordinate {
                                    lat: node.lat(),
                                    lon: node.lon(),
                                },
                            );
                        }
                    }
                    _ => {}
                }
            })?;
        }

        // =================================================================
        // Build admin boundary index
        // =================================================================
        eprintln!("  Building administrative boundary index...");
        Self::build_admin_boundary_index(
            &admin_relations,
            &admin_way_node_ids,
            &nodes_coords,
            &mut admin_boundary_index,
        );
        eprintln!("  {}", admin_boundary_index.get_statistics());

        // =================================================================
        // Build street index
        // =================================================================
        eprintln!("  Building street index...");
        Self::build_street_index(&street_ways, &nodes_coords, &mut street_index);
        eprintln!("  {}", street_index.get_statistics());

        // =================================================================
        // Pass 4: Convert POI entities and write output
        // =================================================================
        eprintln!("Pass 4/4: Processing POI entities and writing output...");

        let all_needed_way_ids: HashSet<i64> = poi_way_ids
            .iter()
            .chain(poi_relation_member_way_ids.iter())
            .copied()
            .collect();

        let mut results: Vec<NominatimPlace> = Vec::new();

        {
            // We need to collect way centroid data AND convert POIs in this pass.
            // PBF files are ordered: Nodes -> Ways -> Relations.

            // POI node data: coordinates and tags (order preserves PBF file order)
            struct NodePoiData {
                ids: Vec<i64>,
                coords: HashMap<i64, Coordinate>,
                tags: HashMap<i64, Vec<(String, String)>>,
            }

            let mut node_data = NodePoiData {
                ids: Vec::new(),
                coords: HashMap::new(),
                tags: HashMap::new(),
            };

            // Way data: node IDs and tags (order preserves PBF file order)
            struct WayPassData {
                ids: Vec<i64>,
                way_node_ids: HashMap<i64, Vec<i64>>,
                way_tags: HashMap<i64, Vec<(String, String)>>,
            }

            let mut way_data = WayPassData {
                ids: Vec::new(),
                way_node_ids: HashMap::new(),
                way_tags: HashMap::new(),
            };

            // Relation data (order preserves PBF file order)
            struct RelationPassData {
                ids: Vec<i64>,
                member_node_ids: HashMap<i64, Vec<i64>>,
                member_way_ids: HashMap<i64, Vec<i64>>,
                tags: HashMap<i64, Vec<(String, String)>>,
            }

            let mut rel_data = RelationPassData {
                ids: Vec::new(),
                member_node_ids: HashMap::new(),
                member_way_ids: HashMap::new(),
                tags: HashMap::new(),
            };

            let reader = ElementReader::from_path(input)?;
            reader.for_each(|element| {
                match element {
                    Element::Node(node) => {
                        let tags: HashMap<&str, &str> =
                            node.tags().collect();
                        if tags.contains_key("name")
                            && tags
                                .iter()
                                .any(|(k, v)| popularity_calculator.has_filter(k, v))
                        {
                            let owned_tags: Vec<(String, String)> =
                                node.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                            node_data.ids.push(node.id());
                            node_data.coords.insert(node.id(), Coordinate { lat: node.lat(), lon: node.lon() });
                            node_data.tags.insert(node.id(), owned_tags);
                        }
                    }
                    Element::DenseNode(node) => {
                        let tags: HashMap<&str, &str> =
                            node.tags().collect();
                        if tags.contains_key("name")
                            && tags
                                .iter()
                                .any(|(k, v)| popularity_calculator.has_filter(k, v))
                        {
                            let owned_tags: Vec<(String, String)> =
                                node.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                            node_data.ids.push(node.id);
                            node_data.coords.insert(node.id, Coordinate { lat: node.lat(), lon: node.lon() });
                            node_data.tags.insert(node.id, owned_tags);
                        }
                    }
                    Element::Way(way) => {
                        if all_needed_way_ids.contains(&way.id()) {
                            let node_ids: Vec<i64> = way.refs().collect();
                            way_data.ids.push(way.id());
                            way_data.way_node_ids.insert(way.id(), node_ids);
                            let owned_tags: Vec<(String, String)> =
                                way.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                            way_data.way_tags.insert(way.id(), owned_tags);
                        }
                    }
                    Element::Relation(relation) => {
                        let tags: HashMap<&str, &str> =
                            relation.tags().collect();
                        if tags.contains_key("name")
                            && tags
                                .iter()
                                .any(|(k, v)| popularity_calculator.has_filter(k, v))
                        {
                            let mut member_nodes = Vec::new();
                            let mut member_ways = Vec::new();
                            for member in relation.members() {
                                match member.member_type {
                                    osmpbf::RelMemberType::Node => {
                                        member_nodes.push(member.member_id);
                                    }
                                    osmpbf::RelMemberType::Way => {
                                        member_ways.push(member.member_id);
                                    }
                                    _ => {}
                                }
                            }
                            rel_data.ids.push(relation.id());
                            rel_data.member_node_ids.insert(relation.id(), member_nodes);
                            rel_data.member_way_ids.insert(relation.id(), member_ways);
                            let owned_tags: Vec<(String, String)> = relation
                                .tags()
                                .map(|(k, v)| (k.to_string(), v.to_string()))
                                .collect();
                            rel_data.tags.insert(relation.id(), owned_tags);
                        }
                    }
                }
            })?;

            // Calculate way centroids (must be done before converting ways/relations)
            for &way_id in &way_data.ids {
                if let Some(node_ids) = way_data.way_node_ids.get(&way_id) {
                    let way_node_coords: Vec<Coordinate> = node_ids
                        .iter()
                        .filter_map(|&nid| nodes_coords.get(nid))
                        .collect();
                    if let Some(centroid) = calculate_centroid(&way_node_coords) {
                        way_centroids.put(way_id, centroid);
                    }
                }
            }

            let importance_calc = ImportanceCalculator::new(&self.config.importance);
            let mut converter = OsmEntityConverter {
                nodes_coords: &nodes_coords,
                way_centroids: &way_centroids,
                admin_boundary_index: &mut admin_boundary_index,
                street_index: &street_index,
                popularity_calculator: &popularity_calculator,
                importance_calc,
                config: &self.config,
            };

            // Convert POI nodes in PBF file order
            for &node_id in &node_data.ids {
                if let (Some(&coord), Some(owned_tags)) =
                    (node_data.coords.get(&node_id), node_data.tags.get(&node_id))
                {
                    let tags: HashMap<&str, &str> = owned_tags
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.as_str()))
                        .collect();
                    if let Some(place) =
                        converter.convert_node(node_id, coord.lat, coord.lon, &tags)
                    {
                        results.push(place);
                    }
                }
            }

            // Convert POI ways in PBF file order
            for &way_id in &way_data.ids {
                if poi_way_ids.contains(&way_id)
                    && let Some(owned_tags) = way_data.way_tags.get(&way_id) {
                        let tags: HashMap<&str, &str> = owned_tags
                            .iter()
                            .map(|(k, v)| (k.as_str(), v.as_str()))
                            .collect();
                        if let Some(place) = converter.convert_way(way_id, &tags) {
                            results.push(place);
                        }
                    }
            }

            // Convert POI relations in PBF file order
            for &rel_id in &rel_data.ids {
                if let Some(owned_tags) = rel_data.tags.get(&rel_id) {
                    let tags: HashMap<&str, &str> = owned_tags
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.as_str()))
                        .collect();
                    let member_nodes = rel_data
                        .member_node_ids
                        .get(&rel_id)
                        .map(|v| v.as_slice())
                        .unwrap_or(&[]);
                    let member_ways = rel_data
                        .member_way_ids
                        .get(&rel_id)
                        .map(|v| v.as_slice())
                        .unwrap_or(&[]);

                    if let Some(place) =
                        converter.convert_relation(rel_id, member_nodes, member_ways, &tags)
                    {
                        results.push(place);
                    }
                }
            }
        }

        eprintln!("Finished processing {} entities", results.len());

        // Write output
        JsonWriter::export(&results, output, is_appending)?;

        Ok(())
    }

    fn build_admin_boundary_index(
        admin_relations: &[AdminRelationData],
        admin_way_node_ids: &HashMap<i64, Vec<i64>>,
        nodes_coords: &CoordinateStore,
        index: &mut AdministrativeBoundaryIndex,
    ) {
        for relation in admin_relations {
            let all_node_coords: Vec<Coordinate> = relation
                .way_ids
                .iter()
                .flat_map(|way_id| {
                    admin_way_node_ids
                        .get(way_id)
                        .map(|nids| {
                            nids.iter()
                                .filter_map(|&nid| nodes_coords.get(nid))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .collect();

            if all_node_coords.is_empty() {
                continue;
            }

            let centroid = Coordinate {
                lat: all_node_coords.iter().map(|c| c.lat).sum::<f64>()
                    / all_node_coords.len() as f64,
                lon: all_node_coords.iter().map(|c| c.lon).sum::<f64>()
                    / all_node_coords.len() as f64,
            };
            let bbox = BoundingBox::from_coordinates(&all_node_coords);

            let boundary = AdministrativeBoundary {
                name: relation.name.clone(),
                admin_level: relation.admin_level,
                ref_code: Some(relation.ref_code.clone()),
                country: relation.country.clone(),
                centroid,
                bbox,
                boundary_nodes: all_node_coords,
            };
            index.add_boundary(boundary);
        }
    }

    fn build_street_index(
        street_ways: &[StreetWayData],
        nodes_coords: &CoordinateStore,
        index: &mut StreetIndex,
    ) {
        let mut skipped = 0;

        for street in street_ways {
            let coordinates: Vec<Coordinate> = street
                .node_ids
                .iter()
                .filter_map(|&nid| nodes_coords.get(nid))
                .collect();

            if coordinates.len() >= 2 {
                index.add_street(&street.name, &coordinates);
            } else {
                skipped += 1;
            }
        }

        if skipped > 0 {
            eprintln!(
                "  Warning: Skipped {} streets due to missing node coordinates",
                skipped
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coordinate_store() {
        let mut store = CoordinateStore::new(16);
        store.put(1, Coordinate { lat: 59.9, lon: 10.7 });
        store.put(2, Coordinate { lat: 60.0, lon: 11.0 });

        let c1 = store.get(1).unwrap();
        assert!((c1.lat - 59.9).abs() < 0.001);
        assert!((c1.lon - 10.7).abs() < 0.001);

        let c2 = store.get(2).unwrap();
        assert!((c2.lat - 60.0).abs() < 0.001);
        assert!((c2.lon - 11.0).abs() < 0.001);

        assert!(store.get(999).is_none());
    }

    #[test]
    fn test_bounding_box() {
        let bbox = BoundingBox {
            min_lat: 59.0,
            max_lat: 61.0,
            min_lon: 10.0,
            max_lon: 12.0,
        };
        assert!(bbox.contains(&Coordinate { lat: 60.0, lon: 11.0 }));
        assert!(!bbox.contains(&Coordinate { lat: 62.0, lon: 11.0 }));
        assert!((bbox.area() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn test_bounding_box_from_coordinates() {
        let coords = vec![
            Coordinate { lat: 59.0, lon: 10.0 },
            Coordinate { lat: 61.0, lon: 12.0 },
            Coordinate { lat: 60.0, lon: 11.0 },
        ];
        let bbox = BoundingBox::from_coordinates(&coords).unwrap();
        assert!((bbox.min_lat - 59.0).abs() < 1e-9);
        assert!((bbox.max_lat - 61.0).abs() < 1e-9);
        assert!((bbox.min_lon - 10.0).abs() < 1e-9);
        assert!((bbox.max_lon - 12.0).abs() < 1e-9);
    }

    #[test]
    fn test_ray_casting() {
        // Simple square polygon
        let boundary = AdministrativeBoundary {
            name: "Test".to_string(),
            admin_level: 4,
            ref_code: None,
            country: Country::no(),
            centroid: Coordinate { lat: 60.0, lon: 11.0 },
            bbox: None,
            boundary_nodes: vec![
                Coordinate { lat: 59.0, lon: 10.0 },
                Coordinate { lat: 59.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 10.0 },
            ],
        };
        assert!(boundary.contains_point(&Coordinate { lat: 60.0, lon: 11.0 }));
        assert!(!boundary.contains_point(&Coordinate { lat: 62.0, lon: 11.0 }));
    }

    #[test]
    fn test_street_segment_distance() {
        let segment = StreetSegment {
            name: "Test Street".to_string(),
            start: Coordinate { lat: 60.0, lon: 10.0 },
            end: Coordinate { lat: 60.0, lon: 10.001 },
        };

        // Point on the line
        let dist = segment.distance_to_point(&Coordinate { lat: 60.0, lon: 10.0005 });
        assert!(dist < 1.0);

        // Point offset from the line
        let dist2 = segment.distance_to_point(&Coordinate { lat: 60.001, lon: 10.0005 });
        assert!(dist2 > 50.0);
    }

    #[test]
    fn test_titleize() {
        assert_eq!(titleize("OSLO"), "Oslo");
        assert_eq!(titleize("nordland"), "Nordland");
        assert_eq!(titleize("old town"), "Old Town");
    }

    #[test]
    fn test_calculate_centroid() {
        let coords = vec![
            Coordinate { lat: 59.0, lon: 10.0 },
            Coordinate { lat: 61.0, lon: 12.0 },
        ];
        let c = calculate_centroid(&coords).unwrap();
        assert!((c.lat - 60.0).abs() < 1e-9);
        assert!((c.lon - 11.0).abs() < 1e-9);
    }

    #[test]
    fn test_calculate_centroid_empty() {
        assert!(calculate_centroid(&[]).is_none());
    }

    // ===== OSM Popularity Calculator tests =====

    fn full_test_config() -> crate::config::Config {
        serde_json::from_str(r#"{
            "osm": {
                "defaultValue": 1.0,
                "rankAddress": { "boundary": 10, "place": 20, "road": 26, "building": 28, "poi": 30 },
                "filters": [
                    {"key": "amenity", "value": "hospital", "priority": 9},
                    {"key": "amenity", "value": "cinema", "priority": 1},
                    {"key": "amenity", "value": "restaurant", "priority": 6},
                    {"key": "amenity", "value": "school", "priority": 9},
                    {"key": "tourism", "value": "hotel", "priority": 6},
                    {"key": "tourism", "value": "museum", "priority": 8},
                    {"key": "tourism", "value": "attraction", "priority": 1}
                ]
            },
            "stedsnavn": { "defaultValue": 40.0, "rankAddress": 16 },
            "matrikkel": { "addressPopularity": 20.0, "streetPopularity": 20.0, "rankAddress": 26 },
            "poi": { "importance": 0.5, "rankAddress": 30 },
            "stopPlace": {
                "defaultValue": 50, "rankAddress": 30,
                "stopTypeFactors": { "busStation": 2.0, "metroStation": 2.0, "railStation": 2.0 },
                "interchangeFactors": { "recommendedInterchange": 3.0, "preferredInterchange": 10.0 }
            },
            "groupOfStopPlaces": { "gosBoostFactor": 10.0, "rankAddress": 30 },
            "importance": { "minPopularity": 1.0, "maxPopularity": 1000000000.0, "floor": 0.1 }
        }"#).unwrap()
    }

    #[test]
    fn popularity_base_times_priority() {
        let config = full_test_config();
        let calc = OsmPopularityCalculator::new(&config);
        let hospital = BTreeMap::from([("amenity", "hospital")]); // priority 9
        let cinema = BTreeMap::from([("amenity", "cinema")]); // priority 1
        let h_pop = calc.calculate_popularity(&hospital);
        let c_pop = calc.calculate_popularity(&cinema);
        assert!(h_pop > 0.0);
        assert!(c_pop > 0.0);
        assert_eq!(h_pop / c_pop, 9.0);
    }

    #[test]
    fn multiple_matching_tags_use_highest_priority() {
        let config = full_test_config();
        let calc = OsmPopularityCalculator::new(&config);
        let high_only = BTreeMap::from([("amenity", "hospital")]); // 9
        let both = BTreeMap::from([("amenity", "hospital"), ("tourism", "attraction")]); // 9, 1
        assert_eq!(calc.calculate_popularity(&high_only), calc.calculate_popularity(&both));
    }

    #[test]
    fn unmatched_tags_return_zero() {
        let config = full_test_config();
        let calc = OsmPopularityCalculator::new(&config);
        assert_eq!(calc.calculate_popularity(&BTreeMap::from([("amenity", "bench")])), 0.0);
        assert_eq!(calc.calculate_popularity(&BTreeMap::from([("shop", "convenience")])), 0.0);
        assert_eq!(calc.calculate_popularity(&BTreeMap::from([("foo", "bar")])), 0.0);
    }

    #[test]
    fn empty_tags_return_zero() {
        let config = full_test_config();
        let calc = OsmPopularityCalculator::new(&config);
        assert_eq!(calc.calculate_popularity(&BTreeMap::new()), 0.0);
    }

    #[test]
    fn has_filter_requires_exact_match() {
        let config = full_test_config();
        let calc = OsmPopularityCalculator::new(&config);
        assert!(calc.has_filter("amenity", "hospital"));
        assert!(!calc.has_filter("amenity", "bench"));
        assert!(!calc.has_filter("amenity", "hospitals")); // plural
        assert!(!calc.has_filter("building", "hospital")); // wrong key
    }

    #[test]
    fn different_poi_types_have_different_priorities() {
        let config = full_test_config();
        let calc = OsmPopularityCalculator::new(&config);
        let hospital = calc.calculate_popularity(&BTreeMap::from([("amenity", "hospital")]));
        let hotel = calc.calculate_popularity(&BTreeMap::from([("tourism", "hotel")]));
        let cinema = calc.calculate_popularity(&BTreeMap::from([("amenity", "cinema")]));
        assert!(hospital > 0.0 && hotel > 0.0 && cinema > 0.0);
        assert_ne!(hospital, hotel);
        assert_ne!(hotel, cinema);
        assert_ne!(hospital, cinema);
    }

    // ===== OsmEntityConverter tests =====
    //
    // These test filter_tags, category assignment, alt name extraction,
    // rank_address determination, and importance via convert_node/convert_way.

    fn make_converter<'a>(
        config: &'a Config,
        nodes: &'a CoordinateStore,
        ways: &'a CoordinateStore,
        admin_index: &'a mut AdministrativeBoundaryIndex,
        street_index: &'a StreetIndex,
        pop_calc: &'a OsmPopularityCalculator,
    ) -> OsmEntityConverter<'a> {
        OsmEntityConverter {
            nodes_coords: nodes,
            way_centroids: ways,
            admin_boundary_index: admin_index,
            street_index,
            popularity_calculator: pop_calc,
            importance_calc: ImportanceCalculator::new(&config.importance),
            config,
        }
    }

    fn empty_converter_parts(config: &Config) -> (CoordinateStore, CoordinateStore, AdministrativeBoundaryIndex, StreetIndex, OsmPopularityCalculator) {
        (
            CoordinateStore::new(0),
            CoordinateStore::new(0),
            AdministrativeBoundaryIndex::new(),
            StreetIndex::new(),
            OsmPopularityCalculator::new(config),
        )
    }

    // -- filter_tags --

    #[test]
    fn filter_tags_keeps_only_configured_filters() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let mut tags = HashMap::new();
        tags.insert("amenity", "hospital");
        tags.insert("name", "Oslo Hospital");
        tags.insert("building", "yes");

        let filtered = conv.filter_tags(&tags);
        assert!(filtered.contains_key("amenity"));
        assert!(!filtered.contains_key("name"));
        assert!(!filtered.contains_key("building"));
    }

    #[test]
    fn filter_tags_returns_empty_for_no_matches() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let mut tags = HashMap::new();
        tags.insert("name", "Something");
        tags.insert("building", "yes");

        let filtered = conv.filter_tags(&tags);
        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_tags_returns_sorted_keys() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let mut tags = HashMap::new();
        tags.insert("tourism", "museum");
        tags.insert("amenity", "hospital");

        let filtered = conv.filter_tags(&tags);
        let keys: Vec<&str> = filtered.keys().copied().collect();
        assert_eq!(keys, vec!["amenity", "tourism"]); // alphabetical
    }

    // -- determine_rank_address --

    #[test]
    fn rank_address_boundary_takes_priority() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("boundary", "administrative"), ("place", "city")]);
        assert_eq!(conv.determine_rank_address(&tags), 10);
    }

    #[test]
    fn rank_address_place_when_no_boundary() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("place", "city")]);
        assert_eq!(conv.determine_rank_address(&tags), 20);
    }

    #[test]
    fn rank_address_road() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("road", "residential")]);
        assert_eq!(conv.determine_rank_address(&tags), 26);
    }

    #[test]
    fn rank_address_building() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("building", "yes")]);
        assert_eq!(conv.determine_rank_address(&tags), 28);
    }

    #[test]
    fn rank_address_defaults_to_poi() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("amenity", "hospital")]);
        assert_eq!(conv.determine_rank_address(&tags), 30);
    }

    // -- convert_node integration: categories, alt names, object type, fields --

    #[test]
    fn convert_node_returns_none_without_name() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("amenity", "hospital")]);
        assert!(conv.convert_node(1, 59.9, 10.7, &tags).is_none());
    }

    #[test]
    fn convert_node_returns_none_with_empty_name() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", ""), ("amenity", "hospital")]);
        assert!(conv.convert_node(1, 59.9, 10.7, &tags).is_none());
    }

    #[test]
    fn convert_node_has_correct_object_type() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].object_type, "N");
    }

    #[test]
    fn convert_node_has_point_accuracy() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.accuracy.as_deref(), Some("point"));
    }

    #[test]
    fn convert_node_has_osm_source() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.source.as_deref(), Some("openstreetmap"));
    }

    #[test]
    fn convert_node_categories_include_tag_values() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let cats = &place.content[0].categories;
        assert!(cats.contains(&"legacy.category.hospital".to_string()));
        assert!(cats.contains(&"legacy.source.whosonfirst".to_string()));
        assert!(cats.contains(&"legacy.layer.address".to_string()));
        assert!(cats.contains(&"osm.public_transport.poi".to_string()));
        assert!(cats.contains(&"legacy.category.poi".to_string()));
    }

    #[test]
    fn convert_node_categories_include_multiple_tag_values() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Museum Hotel"),
            ("amenity", "hospital"),
            ("tourism", "museum"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let cats = &place.content[0].categories;
        assert!(cats.contains(&"legacy.category.hospital".to_string()));
        assert!(cats.contains(&"legacy.category.museum".to_string()));
    }

    #[test]
    fn convert_node_categories_exclude_non_filtered_tags() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Something"),
            ("amenity", "hospital"),
            ("building", "yes"),  // not in filters
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let cats = &place.content[0].categories;
        assert!(!cats.contains(&"legacy.category.yes".to_string()));
    }

    #[test]
    fn convert_node_has_correct_name() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Oslo Sykehus"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].name.as_ref().unwrap().name.as_deref(), Some("Oslo Sykehus"));
    }

    #[test]
    fn convert_node_alt_names_from_filtered_tags() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        // alt_name is only extracted from filtered tags, not all_tags
        // Since our filters only have amenity/tourism, alt_name in all_tags won't appear
        // unless it's also in filtered tags
        let tags = HashMap::from([
            ("name", "Oslo Sykehus"),
            ("amenity", "hospital"),
            ("alt_name", "Oslo Hospital"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        // alt_name key is not in filtered tags (no filter for key="alt_name"),
        // so visible_alt_names should be empty, but indexed alt names has the osm_id
        let extra_alt = &place.content[0].extra.alt_name;
        assert!(extra_alt.is_none()); // no visible alt names
    }

    #[test]
    fn convert_node_en_name_from_filtered_tags() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        // en:name is also looked up from filtered tags
        let tags = HashMap::from([
            ("name", "Oslo Sykehus"),
            ("amenity", "hospital"),
            ("en:name", "Oslo Hospital"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        // en:name is not in filters, so name_en should be None
        assert!(place.content[0].name.as_ref().unwrap().name_en.is_none());
    }

    #[test]
    fn convert_node_osm_id_in_extra() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.id.as_deref(), Some("OSM:TopographicPlace:42"));
    }

    #[test]
    fn convert_node_osm_id_in_indexed_alt_names() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        // The indexed alt_name (in name struct) should contain the OSM ID
        let name_alt = place.content[0].name.as_ref().unwrap().alt_name.as_ref().unwrap();
        assert!(name_alt.contains("OSM:TopographicPlace:42"));
    }

    #[test]
    fn convert_node_has_correct_coordinates() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.91, 10.75, &tags).unwrap();
        let centroid = &place.content[0].centroid;
        assert_eq!(centroid.len(), 2);
        assert!((centroid[0] - 10.75).abs() < 1e-6); // lon first
        assert!((centroid[1] - 59.91).abs() < 1e-6); // lat second
    }

    #[test]
    fn convert_node_importance_reflects_priority() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let hospital_tags = HashMap::from([("name", "Hospital"), ("amenity", "hospital")]); // priority 9
        let cinema_tags = HashMap::from([("name", "Cinema"), ("amenity", "cinema")]); // priority 1
        let h = conv.convert_node(1, 59.9, 10.7, &hospital_tags).unwrap();
        let c = conv.convert_node(2, 59.9, 10.7, &cinema_tags).unwrap();

        let h_imp: f64 = h.content[0].importance.0.parse().unwrap();
        let c_imp: f64 = c.content[0].importance.0.parse().unwrap();
        assert!(h_imp > c_imp, "hospital importance ({h_imp}) should be higher than cinema ({c_imp})");
    }

    #[test]
    fn convert_node_with_admin_boundary_has_county_gid() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);

        // Add a county boundary that contains our test point
        admin.add_boundary(AdministrativeBoundary {
            name: "OSLO".to_string(),
            admin_level: ADMIN_LEVEL_COUNTY,
            ref_code: Some("03".to_string()),
            country: Country::no(),
            centroid: Coordinate { lat: 59.9, lon: 10.7 },
            bbox: Some(BoundingBox { min_lat: 59.0, max_lat: 61.0, min_lon: 10.0, max_lon: 12.0 }),
            boundary_nodes: vec![
                Coordinate { lat: 59.0, lon: 10.0 },
                Coordinate { lat: 59.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 10.0 },
            ],
        });

        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.county_gid.as_deref(), Some("KVE:TopographicPlace:03"));
    }

    #[test]
    fn convert_node_with_municipality_has_locality_gid_and_titleized_name() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);

        admin.add_boundary(AdministrativeBoundary {
            name: "OSLO".to_string(),
            admin_level: ADMIN_LEVEL_MUNICIPALITY,
            ref_code: Some("0301".to_string()),
            country: Country::no(),
            centroid: Coordinate { lat: 59.9, lon: 10.7 },
            bbox: Some(BoundingBox { min_lat: 59.0, max_lat: 61.0, min_lon: 10.0, max_lon: 12.0 }),
            boundary_nodes: vec![
                Coordinate { lat: 59.0, lon: 10.0 },
                Coordinate { lat: 59.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 10.0 },
            ],
        });

        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.locality_gid.as_deref(), Some("KVE:TopographicPlace:0301"));
        assert_eq!(place.content[0].extra.locality.as_deref(), Some("Oslo")); // titleized
        assert_eq!(place.content[0].address.city.as_deref(), Some("Oslo"));
    }

    #[test]
    fn convert_node_county_gid_in_categories() {
        let config = full_test_config();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);

        admin.add_boundary(AdministrativeBoundary {
            name: "OSLO".to_string(),
            admin_level: ADMIN_LEVEL_COUNTY,
            ref_code: Some("03".to_string()),
            country: Country::no(),
            centroid: Coordinate { lat: 59.9, lon: 10.7 },
            bbox: Some(BoundingBox { min_lat: 59.0, max_lat: 61.0, min_lon: 10.0, max_lon: 12.0 }),
            boundary_nodes: vec![
                Coordinate { lat: 59.0, lon: 10.0 },
                Coordinate { lat: 59.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 12.0 },
                Coordinate { lat: 61.0, lon: 10.0 },
            ],
        });

        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let cats = &place.content[0].categories;
        assert!(cats.iter().any(|c| c.starts_with("county_gid.") && c.contains("03")));
    }

    // -- extract_country_code --

    #[test]
    fn extract_country_code_from_iso3166_2() {
        let tags = HashMap::from([("ISO3166-2", "NO-03")]);
        let country = extract_country_code(&tags).unwrap();
        assert_eq!(country.name, "no");
    }

    #[test]
    fn extract_country_code_from_country_code_tag() {
        let tags = HashMap::from([("country_code", "NO")]);
        let country = extract_country_code(&tags).unwrap();
        assert_eq!(country.name, "no");
    }

    #[test]
    fn extract_country_code_from_numeric_ref_assumes_norway() {
        let tags = HashMap::from([("ref", "0301")]);
        let country = extract_country_code(&tags).unwrap();
        assert_eq!(country.name, "no");
    }

    #[test]
    fn extract_country_code_returns_none_for_no_tags() {
        let tags: HashMap<&str, &str> = HashMap::new();
        assert!(extract_country_code(&tags).is_none());
    }

    #[test]
    fn extract_country_code_returns_none_for_non_numeric_ref() {
        let tags = HashMap::from([("ref", "abc")]);
        assert!(extract_country_code(&tags).is_none());
    }

    // -- as_category --

    #[test]
    fn as_category_replaces_colons_with_dots() {
        assert_eq!(as_category("KVE:TopographicPlace:03"), "KVE.TopographicPlace.03");
    }

    #[test]
    fn as_category_no_colons_unchanged() {
        assert_eq!(as_category("simple"), "simple");
    }
}
