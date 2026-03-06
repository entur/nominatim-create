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
    pub place_id: i64,
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
