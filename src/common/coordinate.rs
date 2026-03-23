use crate::common::util::round6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coordinate {
    pub lat: f64,
    pub lon: f64,
}

impl Coordinate {
    pub const ZERO: Coordinate = Coordinate { lat: 0.0, lon: 0.0 };

    pub fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }

    /// GeoJSON-style centroid: `[longitude, latitude]` (note: lon first, not lat).
    pub fn centroid(&self) -> Vec<f64> {
        vec![round6(self.lon), round6(self.lat)]
    }

    /// GeoJSON-style bounding box: `[min_lon, min_lat, max_lon, max_lat]`.
    /// For a point, min and max are identical.
    pub fn bbox(&self) -> Vec<f64> {
        vec![round6(self.lon), round6(self.lat), round6(self.lon), round6(self.lat)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coordinate_new() {
        let c = Coordinate::new(59.9139, 10.7522);
        assert_eq!(c.lat, 59.9139);
        assert_eq!(c.lon, 10.7522);
    }

    #[test]
    fn test_coordinate_zero() {
        assert_eq!(Coordinate::ZERO.lat, 0.0);
        assert_eq!(Coordinate::ZERO.lon, 0.0);
    }

    #[test]
    fn test_centroid_format() {
        let c = Coordinate::new(59.9139456789, 10.7522123456);
        let centroid = c.centroid();
        assert_eq!(centroid.len(), 2);
        assert_eq!(centroid[0], 10.752212); // lon first
        assert_eq!(centroid[1], 59.913946); // then lat
    }

    #[test]
    fn test_bbox_format() {
        let c = Coordinate::new(59.9139, 10.7522);
        let bbox = c.bbox();
        assert_eq!(bbox.len(), 4);
        // bbox is [min_lon, min_lat, max_lon, max_lat] — for a point, all the same
        assert_eq!(bbox[0], bbox[2]); // lon == lon
        assert_eq!(bbox[1], bbox[3]); // lat == lat
        assert_eq!(bbox[0], 10.7522);
        assert_eq!(bbox[1], 59.9139);
    }
}
