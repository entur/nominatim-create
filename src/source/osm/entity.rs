use std::collections::{BTreeMap, HashMap};

use crate::common::category::{LAYER_POI, SOURCE_OSM};
use crate::common::country::Country;
use crate::common::extra::Extra;
use crate::common::geo;
use crate::common::importance::ImportanceCalculator;
use crate::common::text::join_osm_values;
use crate::common::util::titleize;
use crate::config::Config;
use crate::target::nominatim_id::as_place_id;
use crate::target::nominatim_place::*;

use super::admin::AdministrativeBoundary;
use super::admin::AdministrativeBoundaryIndex;
use super::coordinate::Coordinate;
use super::geometry::calculate_centroid;
use super::popularity::OsmPopularityCalculator;
use super::street::StreetIndex;

use super::coordinate::CoordinateStore;

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


const ACCURACY_POINT: &str = "point";
const ACCURACY_POLYGON: &str = "polygon";

pub(crate) const OBJECT_TYPE_NODE: &str = "N";
pub(crate) const OBJECT_TYPE_WAY: &str = "W";
pub(crate) const OBJECT_TYPE_RELATION: &str = "R";

// ---------------------------------------------------------------------------
// OsmEntityConverter
// ---------------------------------------------------------------------------

pub(crate) struct OsmEntityConverter<'a> {
    pub(crate) nodes_coords: &'a CoordinateStore,
    pub(crate) way_centroids: &'a CoordinateStore,
    pub(crate) admin_boundary_index: &'a mut AdministrativeBoundaryIndex,
    pub(crate) street_index: &'a StreetIndex,
    pub(crate) popularity_calculator: &'a OsmPopularityCalculator,
    pub(crate) importance_calc: ImportanceCalculator,
    pub(crate) config: &'a Config,
}

impl<'a> OsmEntityConverter<'a> {
    /// Filter tags to only those matching configured filters (sorted by key).
    pub(crate) fn filter_tags<'t>(
        &self,
        tags: &HashMap<&'t str, &'t str>,
    ) -> BTreeMap<&'t str, &'t str> {
        tags.iter()
            .filter(|(k, v)| self.popularity_calculator.has_filter(k, v))
            .map(|(&k, &v)| (k, v))
            .collect()
    }

    pub(crate) fn convert_node(
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

    pub(crate) fn convert_way(
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

    pub(crate) fn convert_relation(
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

        let member_coords = self.collect_member_coords(member_node_ids, member_way_ids);
        if member_coords.is_empty() {
            return None;
        }

        let centroid = calculate_centroid(&member_coords)?;
        let tags = self.filter_tags(all_tags);

        let fallback_county = if tags.get("type") == Some(&"boundary")
            && tags.get("boundary") == Some(&"administrative")
        {
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

    fn collect_member_coords(
        &self,
        member_node_ids: &[i64],
        member_way_ids: &[i64],
    ) -> Vec<Coordinate> {
        let mut coords = Vec::new();
        for &nid in member_node_ids {
            if let Some(c) = self.nodes_coords.get(nid) {
                coords.push(c);
            }
        }
        for &wid in member_way_ids {
            if let Some(c) = self.way_centroids.get(wid) {
                coords.push(c);
            }
        }
        coords
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
        let osm_id = format!("OSM:PointOfInterest:{}", entity_id);

        let visible_categories = build_visible_categories(tags);
        let (visible_alt_names, indexed_alt_names) = build_alt_names(tags, name, &osm_id);
        let en_name = tags.get("en:name").copied().map(|s| s.to_string());

        let (county_gid, county_name) =
            resolve_county(county, fallback_county);
        let (locality_gid, locality) = resolve_municipality(municipality);

        let street = self.street_index.find_nearest_street(&centroid);

        let address = Address {
            street,
            city: locality.clone(),
            county: county_name,
        };

        let extra = build_extra(
            &osm_id,
            accuracy,
            &country,
            &county_gid,
            &locality,
            &locality_gid,
            &visible_categories,
            &visible_alt_names,
        );

        let indexed_categories = build_indexed_categories(
            &visible_categories,
            &country,
            &county_gid,
            &locality_gid,
        );

        let rank_address = self.determine_rank_address(tags);
        let importance = self.calculate_importance(tags);

        let content = PlaceContent {
            place_id: as_place_id(&osm_id),
            object_type: object_type.to_string(),
            object_id: 0,
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

// ---------------------------------------------------------------------------
// Helper functions (extracted from create_place_content)
// ---------------------------------------------------------------------------

fn build_visible_categories(tags: &BTreeMap<&str, &str>) -> Vec<String> {
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
}

fn build_alt_names(
    tags: &BTreeMap<&str, &str>,
    name: &str,
    osm_id: &str,
) -> (Vec<String>, Vec<String>) {
    let alt_name_keys = ["alt_name", "old_name", "no:name", "loc_name", "short_name"];
    let visible: Vec<String> = alt_name_keys
        .iter()
        .filter_map(|&k| tags.get(k).copied())
        .filter(|&v| !v.is_empty() && v != name)
        .map(|s| s.to_string())
        .collect();

    let mut indexed = visible.clone();
    indexed.push(osm_id.to_string());

    (visible, indexed)
}

fn resolve_county(
    county: Option<&AdministrativeBoundary>,
    fallback_county: Option<&str>,
) -> (Option<String>, Option<String>) {
    let county_gid = county
        .and_then(|c| c.ref_code.as_ref())
        .map(|r| format!("KVE:TopographicPlace:{}", r));
    let county_name = county
        .map(|c| titleize(&c.name))
        .or_else(|| fallback_county.map(|s| s.to_string()));
    (county_gid, county_name)
}

fn resolve_municipality(
    municipality: Option<&AdministrativeBoundary>,
) -> (Option<String>, Option<String>) {
    let locality_gid = municipality
        .and_then(|m| m.ref_code.as_ref())
        .map(|r| format!("KVE:TopographicPlace:{}", r));
    let locality = municipality.map(|m| titleize(&m.name));
    (locality_gid, locality)
}

#[allow(clippy::too_many_arguments)]
fn build_extra(
    osm_id: &str,
    accuracy: &str,
    country: &Option<Country>,
    county_gid: &Option<String>,
    locality: &Option<String>,
    locality_gid: &Option<String>,
    visible_categories: &[String],
    visible_alt_names: &[String],
) -> Extra {
    Extra {
        id: Some(osm_id.to_string()),
        source: Some("openstreetmap".to_string()),
        accuracy: Some(accuracy.to_string()),
        country_a: country.as_ref().map(|c| c.three_letter_code.clone()),
        county_gid: county_gid.clone(),
        locality: locality.clone(),
        locality_gid: locality_gid.clone(),
        tags: join_osm_values(visible_categories),
        alt_name: join_osm_values(visible_alt_names),
        ..Extra::default()
    }
}

fn build_indexed_categories(
    visible_categories: &[String],
    country: &Option<Country>,
    county_gid: &Option<String>,
    locality_gid: &Option<String>,
) -> Vec<String> {
    let mut cats = visible_categories.to_vec();
    cats.push(SOURCE_OSM.to_string());
    cats.push(LAYER_POI.to_string());
    if let Some(c) = country {
        cats.push(format!("{}{}", COUNTRY_PREFIX, c.name));
    }
    if let Some(gid) = county_gid {
        cats.push(format!("{}{}", COUNTY_ID_PREFIX, as_category(gid)));
    }
    if let Some(gid) = locality_gid {
        cats.push(format!("{}{}", LOCALITY_ID_PREFIX, as_category(gid)));
    }
    cats
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

pub(crate) fn determine_country(
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

/// Extract a 2-letter country code from OSM admin relation tags.
pub(crate) fn extract_country_code(tags: &HashMap<&str, &str>) -> Option<Country> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::admin::ADMIN_LEVEL_COUNTY;
    use super::super::admin::ADMIN_LEVEL_MUNICIPALITY;
    use super::super::geometry::BoundingBox;
    use crate::source::test_helpers::test_config_with_osm_filters;

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
        let config = test_config_with_osm_filters();
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
        let config = test_config_with_osm_filters();
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
        let config = test_config_with_osm_filters();
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
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("boundary", "administrative"), ("place", "city")]);
        assert_eq!(conv.determine_rank_address(&tags), 10);
    }

    #[test]
    fn rank_address_place_when_no_boundary() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("place", "city")]);
        assert_eq!(conv.determine_rank_address(&tags), 20);
    }

    #[test]
    fn rank_address_road() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("road", "residential")]);
        assert_eq!(conv.determine_rank_address(&tags), 26);
    }

    #[test]
    fn rank_address_building() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("building", "yes")]);
        assert_eq!(conv.determine_rank_address(&tags), 28);
    }

    #[test]
    fn rank_address_defaults_to_poi() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = BTreeMap::from([("amenity", "hospital")]);
        assert_eq!(conv.determine_rank_address(&tags), 30);
    }

    // -- convert_node integration --

    #[test]
    fn convert_node_returns_none_without_name() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("amenity", "hospital")]);
        assert!(conv.convert_node(1, 59.9, 10.7, &tags).is_none());
    }

    #[test]
    fn convert_node_returns_none_with_empty_name() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", ""), ("amenity", "hospital")]);
        assert!(conv.convert_node(1, 59.9, 10.7, &tags).is_none());
    }

    #[test]
    fn convert_node_has_correct_object_type() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].object_type, "N");
    }

    #[test]
    fn convert_node_has_point_accuracy() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.accuracy.as_deref(), Some("point"));
    }

    #[test]
    fn convert_node_has_osm_source() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test Hospital"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.source.as_deref(), Some("openstreetmap"));
    }

    #[test]
    fn convert_node_categories_include_tag_values() {
        let config = test_config_with_osm_filters();
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
        let config = test_config_with_osm_filters();
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
        let config = test_config_with_osm_filters();
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
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Oslo Sykehus"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].name.as_ref().unwrap().name.as_deref(), Some("Oslo Sykehus"));
    }

    #[test]
    fn convert_node_alt_names_from_filtered_tags() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Oslo Sykehus"),
            ("amenity", "hospital"),
            ("alt_name", "Oslo Hospital"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let extra_alt = &place.content[0].extra.alt_name;
        assert!(extra_alt.is_none()); // no visible alt names
    }

    #[test]
    fn convert_node_en_name_from_filtered_tags() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([
            ("name", "Oslo Sykehus"),
            ("amenity", "hospital"),
            ("en:name", "Oslo Hospital"),
        ]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert!(place.content[0].name.as_ref().unwrap().name_en.is_none());
    }

    #[test]
    fn convert_node_osm_id_in_extra() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        assert_eq!(place.content[0].extra.id.as_deref(), Some("OSM:PointOfInterest:42"));
    }

    #[test]
    fn convert_node_osm_id_in_indexed_alt_names() {
        let config = test_config_with_osm_filters();
        let (nodes, ways, mut admin, streets, pop) = empty_converter_parts(&config);
        let mut conv = make_converter(&config, &nodes, &ways, &mut admin, &streets, &pop);

        let tags = HashMap::from([("name", "Test"), ("amenity", "hospital")]);
        let place = conv.convert_node(42, 59.9, 10.7, &tags).unwrap();
        let name_alt = place.content[0].name.as_ref().unwrap().alt_name.as_ref().unwrap();
        assert!(name_alt.contains("OSM:PointOfInterest:42"));
    }

    #[test]
    fn convert_node_has_correct_coordinates() {
        let config = test_config_with_osm_filters();
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
        let config = test_config_with_osm_filters();
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
        let config = test_config_with_osm_filters();
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
        assert_eq!(place.content[0].extra.county_gid.as_deref(), Some("KVE:TopographicPlace:03"));
    }

    #[test]
    fn convert_node_with_municipality_has_locality_gid_and_titleized_name() {
        let config = test_config_with_osm_filters();
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
        let config = test_config_with_osm_filters();
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
