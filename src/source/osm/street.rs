use std::cell::RefCell;
use std::collections::HashMap;

use super::coordinate::Coordinate;

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
// StreetIndex -- grid-based spatial index
// ---------------------------------------------------------------------------

/// Grid cell size in degrees (~550 m at 60°N latitude). Streets are bucketed into
/// cells of this size for fast spatial lookup.
const GRID_SIZE: f64 = 0.005;
/// Maximum number of grid rings to expand when searching for the nearest street.
const MAX_SEARCH_RADIUS: i32 = 10;
/// Streets farther than this (in meters) are not considered a match.
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

/// Grid-based spatial index for finding the nearest street to a coordinate.
///
/// The `lookup_cache` uses `RefCell` to allow caching through a shared (`&self`) reference.
/// This is Rust's "interior mutability" pattern -- the borrow rules are checked at runtime
/// instead of compile time, letting `find_nearest_street` cache results without requiring
/// `&mut self`.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
