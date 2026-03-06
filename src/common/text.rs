pub const OSM_TAG_SEPARATOR: &str = ";";

pub fn join_osm_values(values: &[String]) -> Option<String> {
    let filtered: Vec<&str> = values.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    if filtered.is_empty() {
        None
    } else {
        Some(filtered.join(OSM_TAG_SEPARATOR))
    }
}
