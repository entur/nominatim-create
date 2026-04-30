pub mod belagenhet;
pub mod matrikkel;
pub mod osm;
pub mod poi;
pub mod stedsnavn;
pub mod stopplace;

#[cfg(test)]
pub(crate) mod test_helpers {
    use crate::config::Config;

    /// Shared test config with all sections populated.
    /// Each converter only reads its own section, so extra fields don't matter.
    pub fn test_config() -> Config {
        serde_json::from_str(r#"{
            "osm": {
                "defaultValue": 1.0,
                "rankAddress": { "boundary": 10, "place": 20, "road": 26, "building": 28, "poi": 30 },
                "filters": [{"key": "amenity", "value": "hospital", "priority": 9}]
            },
            "stedsnavn": { "defaultValue": 40.0, "rankAddress": 16 },
            "matrikkel": { "addressPopularity": 20.0, "streetPopularity": 20.0, "rankAddress": 26 },
            "poi": { "importance": 0.5, "rankAddress": 30 },
            "stopPlace": {
                "defaultValue": 50,
                "rankAddress": 30,
                "stopTypeFactors": { "busStation": 2.0, "metroStation": 2.0, "railStation": 2.0 },
                "interchangeFactors": { "recommendedInterchange": 3.0, "preferredInterchange": 10.0 }
            },
            "groupOfStopPlaces": { "rankAddress": 30 },
            "importance": { "minPopularity": 1.0, "maxPopularity": 1000000000.0, "floor": 0.1 }
        }"#).unwrap()
    }

    /// Test config with additional OSM POI filters for popularity/entity tests.
    pub fn test_config_with_osm_filters() -> Config {
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
                "defaultValue": 50,
                "rankAddress": 30,
                "stopTypeFactors": { "busStation": 2.0, "metroStation": 2.0, "railStation": 2.0 },
                "interchangeFactors": { "recommendedInterchange": 3.0, "preferredInterchange": 10.0 }
            },
            "groupOfStopPlaces": { "rankAddress": 30 },
            "importance": { "minPopularity": 1.0, "maxPopularity": 1000000000.0, "floor": 0.1 }
        }"#).unwrap()
    }

    pub fn test_data_path(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-data").join(name)
    }
}
