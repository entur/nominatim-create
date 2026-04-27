use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize, Clone)]
pub struct Config {
    pub osm: OsmConfig,
    pub stedsnavn: StedsnavnConfig,
    pub matrikkel: MatrikkelConfig,
    pub poi: PoiConfig,
    #[serde(rename = "stopPlace")]
    pub stop_place: StopPlaceConfig,
    #[serde(rename = "groupOfStopPlaces")]
    pub group_of_stop_places: GroupOfStopPlacesConfig,
    pub importance: ImportanceConfig,
    #[serde(default)]
    pub belagenhet: BelagenhetConfig,
}

#[derive(Deserialize, Clone)]
pub struct OsmConfig {
    #[serde(rename = "defaultValue")]
    pub default_value: f64,
    #[serde(rename = "rankAddress")]
    pub rank_address: RankAddress,
    pub filters: Vec<PoiFilter>,
}

#[derive(Deserialize, Clone, Copy)]
pub struct RankAddress {
    pub boundary: i32,
    pub place: i32,
    pub road: i32,
    pub building: i32,
    pub poi: i32,
}

#[derive(Deserialize, Clone)]
pub struct PoiFilter {
    pub key: String,
    pub value: String,
    pub priority: i32,
}

#[derive(Deserialize, Clone)]
pub struct StedsnavnConfig {
    #[serde(rename = "defaultValue")]
    pub default_value: f64,
    #[serde(rename = "rankAddress")]
    pub rank_address: i32,
}

#[derive(Deserialize, Clone)]
pub struct MatrikkelConfig {
    #[serde(rename = "addressPopularity")]
    pub address_popularity: f64,
    #[serde(rename = "streetPopularity")]
    pub street_popularity: f64,
    #[serde(rename = "rankAddress")]
    pub rank_address: i32,
}

#[derive(Deserialize, Clone)]
pub struct PoiConfig {
    pub importance: f64,
    #[serde(rename = "rankAddress")]
    pub rank_address: i32,
}

#[derive(Deserialize, Clone)]
pub struct StopPlaceConfig {
    #[serde(rename = "defaultValue")]
    pub default_value: i64,
    #[serde(rename = "rankAddress")]
    pub rank_address: i32,
    #[serde(rename = "stopTypeFactors")]
    pub stop_type_factors: std::collections::HashMap<String, f64>,
    #[serde(rename = "interchangeFactors")]
    pub interchange_factors: std::collections::HashMap<String, f64>,
}

#[derive(Deserialize, Clone)]
pub struct GroupOfStopPlacesConfig {
    #[serde(rename = "gosBoostFactor")]
    pub gos_boost_factor: f64,
    #[serde(rename = "rankAddress")]
    pub rank_address: i32,
    /// Multiplier applied on top of the unclamped importance for GoSPs in [`Self::home_country`].
    /// Lifts major-city groups above the 0-1 importance band so they can outrank near-focus
    /// streets that share the same name prefix (e.g. "Bergen" vs "Bergensgata" when searching
    /// from Oslo). Photon stores importance as a plain double, so values above 1.0 contribute
    /// proportionally more to the final score. GoSPs outside the home country use the regular
    /// clamped importance, which keeps long-distance bus terminals like Berlin ZOB from
    /// outranking Norwegian cities.
    #[serde(rename = "importanceMultiplier", default = "default_gosp_importance_multiplier")]
    pub importance_multiplier: f64,
    /// ISO 3166-1 alpha-2 (lowercase) for the country whose GoSPs receive the importance boost.
    #[serde(rename = "homeCountry", default = "default_gosp_home_country")]
    pub home_country: String,
}

fn default_gosp_importance_multiplier() -> f64 { 1.0 }
fn default_gosp_home_country() -> String { "no".to_string() }

#[derive(Deserialize, Clone)]
pub struct BelagenhetConfig {
    #[serde(rename = "addressPopularity", default = "default_belagenhet_address_pop")]
    pub address_popularity: f64,
    #[serde(rename = "streetPopularity", default = "default_belagenhet_street_pop")]
    pub street_popularity: f64,
    #[serde(rename = "rankAddress", default = "default_belagenhet_rank")]
    pub rank_address: i32,
}

fn default_belagenhet_address_pop() -> f64 { 20.0 }
fn default_belagenhet_street_pop() -> f64 { 20.0 }
fn default_belagenhet_rank() -> i32 { 26 }

impl Default for BelagenhetConfig {
    fn default() -> Self {
        Self {
            address_popularity: default_belagenhet_address_pop(),
            street_popularity: default_belagenhet_street_pop(),
            rank_address: default_belagenhet_rank(),
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
pub struct ImportanceConfig {
    #[serde(rename = "minPopularity")]
    pub min_popularity: f64,
    #[serde(rename = "maxPopularity")]
    pub max_popularity: f64,
    pub floor: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CONFIG: &str = r#"{
        "osm": {
            "defaultValue": 1.0,
            "rankAddress": { "boundary": 10, "place": 20, "road": 26, "building": 28, "poi": 30 },
            "filters": [
                {"key": "amenity", "value": "hospital", "priority": 9}
            ]
        },
        "stedsnavn": { "defaultValue": 40.0, "rankAddress": 16 },
        "matrikkel": { "addressPopularity": 20.0, "streetPopularity": 20.0, "rankAddress": 26 },
        "poi": { "importance": 0.5, "rankAddress": 30 },
        "stopPlace": {
            "defaultValue": 50,
            "rankAddress": 30,
            "stopTypeFactors": { "busStation": 2.0 },
            "interchangeFactors": { "preferredInterchange": 10.0 }
        },
        "groupOfStopPlaces": { "gosBoostFactor": 10.0, "rankAddress": 30 },
        "importance": { "minPopularity": 1.0, "maxPopularity": 1000000000.0, "floor": 0.1 }
    }"#;

    #[test]
    fn test_config_deserializes() {
        let config: Config = serde_json::from_str(TEST_CONFIG).unwrap();
        assert_eq!(config.osm.default_value, 1.0);
        assert_eq!(config.osm.rank_address.boundary, 10);
        assert_eq!(config.osm.rank_address.poi, 30);
        assert_eq!(config.osm.filters.len(), 1);
        assert_eq!(config.osm.filters[0].key, "amenity");
        assert_eq!(config.osm.filters[0].priority, 9);
    }

    #[test]
    fn test_config_stop_place_factors() {
        let config: Config = serde_json::from_str(TEST_CONFIG).unwrap();
        assert_eq!(config.stop_place.default_value, 50);
        assert_eq!(*config.stop_place.stop_type_factors.get("busStation").unwrap(), 2.0);
        assert_eq!(*config.stop_place.interchange_factors.get("preferredInterchange").unwrap(), 10.0);
    }

    #[test]
    fn test_config_importance() {
        let config: Config = serde_json::from_str(TEST_CONFIG).unwrap();
        assert_eq!(config.importance.min_popularity, 1.0);
        assert_eq!(config.importance.max_popularity, 1_000_000_000.0);
        assert_eq!(config.importance.floor, 0.1);
    }

    #[test]
    fn test_config_matrikkel() {
        let config: Config = serde_json::from_str(TEST_CONFIG).unwrap();
        assert_eq!(config.matrikkel.address_popularity, 20.0);
        assert_eq!(config.matrikkel.street_popularity, 20.0);
        assert_eq!(config.matrikkel.rank_address, 26);
    }

    #[test]
    fn test_config_load_missing_file() {
        let result = Config::load(Some(Path::new("/nonexistent/config.json")));
        assert!(result.is_err());
    }
}

impl Config {
    /// Load and parse the converter configuration file.
    ///
    /// Returns `Result<Self, Box<dyn std::error::Error>>` -- this is a common Rust pattern
    /// for CLI tools where the caller only needs to display the error, not match on specific
    /// variants. `Box<dyn Error>` is a trait object that can hold any error type. The `?`
    /// operator below automatically converts specific errors (IO, JSON parse) into this
    /// boxed form and returns early -- similar to a thrown exception, but checked at
    /// compile time.
    pub fn load(path: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.unwrap_or_else(|| Path::new("converter.json"));
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read config file '{}': {e}", path.display()))?;
        let config: Config = serde_json::from_str(&content)
            .map_err(|e| format!("Invalid config '{}': {e}", path.display()))?;
        eprintln!("Loaded configuration from: {}", path.display());
        Ok(config)
    }
}
