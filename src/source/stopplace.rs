mod convert;
mod popularity;
pub(crate) mod xml;

// Re-export for external use (poi.rs uses read_element_as_string)
pub use xml::read_element_as_string;

pub fn convert(
    config: &crate::config::Config,
    input: &std::path::Path,
    output: &std::path::Path,
    is_appending: bool,
    usage: &crate::common::usage::UsageBoost,
) -> Result<(), Box<dyn std::error::Error>> {
    convert::convert_all(config, input, output, is_appending, usage)
}

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) mod helpers {
        pub use crate::source::test_helpers::{test_config, test_data_path};
        use super::super::xml::*;

        pub fn make_stop_place(id: &str, name: &str, transport_mode: Option<&str>, stop_place_type: Option<&str>) -> StopPlaceXml {
            StopPlaceXml {
                id: id.to_string(),
                name: Some(name.to_string()),
                description: None,
                centroid: Some(CentroidXml { location: LocationXml { longitude: 10.0, latitude: 60.0 } }),
                transport_mode: transport_mode.map(|s| s.to_string()),
                bus_submode: None, tram_submode: None, rail_submode: None,
                metro_submode: None, air_submode: None, water_submode: None, telecabin_submode: None,
                stop_place_type: stop_place_type.map(|s| s.to_string()),
                weighting: None,
                topographic_place_ref: None,
                parent_site_ref: None,
                alternative_names: None,
                tariff_zones: None,
            }
        }

        pub fn make_stop_place_with_submode(id: &str, transport_mode: &str, bus_sub: Option<&str>, rail_sub: Option<&str>, tram_sub: Option<&str>) -> StopPlaceXml {
            let mut sp = make_stop_place(id, "Test Stop", Some(transport_mode), None);
            sp.bus_submode = bus_sub.map(|s| s.to_string());
            sp.rail_submode = rail_sub.map(|s| s.to_string());
            sp.tram_submode = tram_sub.map(|s| s.to_string());
            sp
        }

        pub fn make_stop_place_with_alt_names(id: &str, name: &str, alt_names: Vec<(&str, Option<&str>)>) -> StopPlaceXml {
            let mut sp = make_stop_place(id, name, None, None);
            sp.alternative_names = Some(AlternativeNamesXml {
                names: alt_names.into_iter().map(|(n, nt)| AlternativeNameXml {
                    name_type: nt.map(|s| s.to_string()),
                    name: Some(n.to_string()),
                }).collect(),
            });
            sp
        }
    }
}
