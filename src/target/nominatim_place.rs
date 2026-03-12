use crate::common::extra::Extra;
use serde::{Serialize, Serializer};

/// A pre-formatted JSON number. Serializes as raw JSON (no quoting).
#[derive(Debug)]
pub struct RawNumber(pub String);

impl RawNumber {
    /// Format with exactly 6 decimal places (matches Kotlin's `.toBigDecimalWithScale(6)`).
    pub fn from_f64_6dp(val: f64) -> Self {
        Self(format!("{:.6}", val))
    }

    /// Use the default float representation (matches Kotlin's `.toBigDecimal()`).
    pub fn from_f64(val: f64) -> Self {
        Self(val.to_string())
    }
}

impl Serialize for RawNumber {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let raw: Box<serde_json::value::RawValue> =
            serde_json::value::RawValue::from_string(self.0.clone()).map_err(S::Error::custom)?;
        (*raw).serialize(s)
    }
}

mod vec_f64_6dp {
    use serde::ser::SerializeSeq;
    use serde::Serializer;

    pub fn serialize<S: Serializer>(vals: &[f64], s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let mut seq = s.serialize_seq(Some(vals.len()))?;
        for val in vals {
            let formatted = format!("{:.6}", val);
            let raw: Box<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(formatted).map_err(S::Error::custom)?;
            seq.serialize_element(&*raw)?;
        }
        seq.end()
    }
}

#[derive(Debug, Serialize)]
pub struct NominatimPlace {
    #[serde(rename = "type")]
    pub type_: String,
    pub content: Vec<PlaceContent>,
}

#[derive(Debug, Serialize)]
pub struct PlaceContent {
    pub place_id: String,
    pub object_type: String,
    pub object_id: i64,
    pub categories: Vec<String>,
    pub rank_address: i32,
    pub importance: RawNumber,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_place_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Name>,
    pub address: Address,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub housenumber: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postcode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    #[serde(serialize_with = "vec_f64_6dp::serialize")]
    pub centroid: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty", serialize_with = "vec_f64_6dp::serialize")]
    pub bbox: Vec<f64>,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Address {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub county: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Name {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "name:en", skip_serializing_if = "Option::is_none")]
    pub name_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NominatimHeader {
    #[serde(rename = "type")]
    pub type_: String,
    pub content: HeaderContent,
}

#[derive(Debug, Serialize)]
pub struct HeaderContent {
    pub version: String,
    pub generator: String,
    pub database_version: String,
    pub data_timestamp: String,
    pub features: Features,
}

#[derive(Debug, Serialize)]
pub struct Features {
    pub sorted_by_country: bool,
    pub has_addresslines: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raw_number_6dp() {
        let r = RawNumber::from_f64_6dp(0.252848);
        assert_eq!(r.0, "0.252848");
    }

    #[test]
    fn test_raw_number_6dp_rounds() {
        let r = RawNumber::from_f64_6dp(0.1234567890);
        assert_eq!(r.0, "0.123457");
    }

    #[test]
    fn test_raw_number_6dp_trailing_zeros() {
        let r = RawNumber::from_f64_6dp(1.0);
        assert_eq!(r.0, "1.000000");
    }

    #[test]
    fn test_raw_number_default() {
        let r = RawNumber::from_f64(0.5);
        assert_eq!(r.0, "0.5");
    }

    #[test]
    fn test_raw_number_serializes_without_quotes() {
        let r = RawNumber::from_f64_6dp(0.252848);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "0.252848");
    }

    #[test]
    fn test_place_serialization_6dp_centroid() {
        let place = NominatimPlace {
            type_: "Place".to_string(),
            content: vec![PlaceContent {
                place_id: "NSR-StopPlace-123".to_string(),
                object_type: "N".to_string(),
                object_id: 0,
                categories: vec!["source.nsr".to_string()],
                rank_address: 30,
                importance: RawNumber::from_f64_6dp(0.252848),
                parent_place_id: None,
                name: Some(Name {
                    name: Some("Test".to_string()),
                    name_en: None,
                    alt_name: None,
                }),
                address: Address {
                    street: None,
                    city: Some("Oslo".to_string()),
                    county: None,
                },
                housenumber: None,
                postcode: Some("0001".to_string()),
                country_code: Some("no".to_string()),
                centroid: vec![10.752212, 59.913946],
                bbox: vec![],
                extra: Extra::default(),
            }],
        };
        let json = serde_json::to_string(&place).unwrap();
        // Verify centroid has 6 decimal places
        assert!(json.contains("10.752212"));
        assert!(json.contains("59.913946"));
        // Verify importance is unquoted
        assert!(json.contains("\"importance\":0.252848"));
        // Verify bbox is omitted when empty
        assert!(!json.contains("bbox"));
    }

    #[test]
    fn test_place_serialization_optional_fields_omitted() {
        let place = PlaceContent {
            place_id: "KVE-PostalAddress-1".to_string(),
            object_type: "N".to_string(),
            object_id: 0,
            categories: vec![],
            rank_address: 26,
            importance: RawNumber::from_f64_6dp(0.230103),
            parent_place_id: None,
            name: None,
            address: Address { street: None, city: None, county: None },
            housenumber: None,
            postcode: None,
            country_code: None,
            centroid: vec![10.0, 59.0],
            bbox: vec![],
            extra: Extra::default(),
        };
        let json = serde_json::to_string(&place).unwrap();
        assert!(!json.contains("parent_place_id"));
        assert!(!json.contains("\"name\""));
        assert!(!json.contains("housenumber"));
        assert!(!json.contains("postcode"));
        assert!(!json.contains("country_code"));
    }

    #[test]
    fn test_header_serialization() {
        let header = NominatimHeader {
            type_: "NominatimDumpFile".to_string(),
            content: HeaderContent {
                version: "0.1.0".to_string(),
                generator: "geocoder".to_string(),
                database_version: "0.3.6-1".to_string(),
                data_timestamp: "2026-01-01T00:00:00+00:00".to_string(),
                features: Features {
                    sorted_by_country: true,
                    has_addresslines: false,
                },
            },
        };
        let json = serde_json::to_string(&header).unwrap();
        assert!(json.contains("\"type\":\"NominatimDumpFile\""));
        assert!(json.contains("\"generator\":\"geocoder\""));
        assert!(json.contains("\"sorted_by_country\":true"));
    }
}
