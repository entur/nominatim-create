use crate::common::category::*;
use crate::common::coordinate::Coordinate;
use crate::common::country::Country;
use crate::common::extra::Extra;
use crate::common::geo;
use crate::config::Config;
use crate::target::json_writer::JsonWriter;
use crate::target::nominatim_id::NominatimId;
use crate::target::nominatim_place::*;
use chrono::{Local, NaiveDateTime};
use quick_xml::de::from_str;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Deserialize;
use std::path::Path;

pub fn convert(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let xml = std::fs::read_to_string(input)?;
    let topo_places = parse_topographic_places(&xml)?;
    let now = Local::now().naive_local();

    let entries: Vec<NominatimPlace> = topo_places
        .into_iter()
        .filter(|tp| is_valid(tp, &now))
        .filter_map(|tp| convert_topo_place(config, &tp))
        .collect();

    JsonWriter::export(&entries, output, is_appending)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct TopographicPlaceXml {
    #[serde(rename = "@id")]
    id: Option<String>,
    #[serde(rename = "ValidBetween")]
    valid_between: Option<ValidBetweenXml>,
    #[serde(rename = "Descriptor")]
    descriptor: Option<DescriptorXml>,
    #[serde(rename = "Centroid")]
    centroid: Option<CentroidXml>,
}

#[derive(Debug, Deserialize)]
struct ValidBetweenXml {
    #[serde(rename = "FromDate")]
    from_date: Option<String>,
    #[serde(rename = "ToDate")]
    to_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DescriptorXml {
    #[serde(rename = "Name")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CentroidXml {
    #[serde(rename = "Location")]
    location: LocationXml,
}

#[derive(Debug, Deserialize)]
struct LocationXml {
    #[serde(rename = "Longitude")]
    longitude: f64,
    #[serde(rename = "Latitude")]
    latitude: f64,
}

fn parse_topographic_places(xml: &str) -> Result<Vec<TopographicPlaceXml>, Box<dyn std::error::Error>> {
    let mut places = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == b"TopographicPlace" {
                    let text = crate::source::stopplace::read_element_as_string_pub(&mut reader, "TopographicPlace", e)?;
                    if let Ok(tp) = from_str::<TopographicPlaceXml>(&text) {
                        places.push(tp);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(places)
}

fn is_valid(tp: &TopographicPlaceXml, now: &NaiveDateTime) -> bool {
    let Some(vb) = &tp.valid_between else { return true };
    let from_ok = vb.from_date.as_ref().is_none_or(|d| {
        NaiveDateTime::parse_from_str(d, "%Y-%m-%dT%H:%M:%S").map_or(true, |dt| *now >= dt)
    });
    let to_ok = vb.to_date.as_ref().is_none_or(|d| {
        NaiveDateTime::parse_from_str(d, "%Y-%m-%dT%H:%M:%S").map_or(true, |dt| *now <= dt)
    });
    from_ok && to_ok
}

fn convert_topo_place(config: &Config, tp: &TopographicPlaceXml) -> Option<NominatimPlace> {
    let id = tp.id.as_deref().unwrap_or("");
    let name = tp.descriptor.as_ref()?.name.as_deref().unwrap_or("");
    let centroid_xml = tp.centroid.as_ref()?;
    let coord = Coordinate::new(centroid_xml.location.latitude, centroid_xml.location.longitude);
    let country = geo::get_country(&coord).unwrap_or_else(Country::no);

    let visible_tag = OSM_CUSTOM_POI;
    let indexed_cats = vec![
        visible_tag.to_string(),
        format!("{COUNTRY_PREFIX}{}", country.name),
        as_category(id),
    ];

    let nominatim_id = NominatimId::Poi.create(id);
    Some(NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: nominatim_id,
            object_type: "N".to_string(),
            object_id: nominatim_id,
            categories: indexed_cats,
            rank_address: config.poi.rank_address,
            importance: RawNumber::from_f64(config.poi.importance),
            parent_place_id: None,
            name: Some(Name { name: Some(name.to_string()), name_en: None, alt_name: None }),
            address: Address::default(),
            housenumber: None,
            postcode: None,
            country_code: Some(country.name.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: Extra {
                id: Some(id.to_string()),
                source: Some("custom-poi".to_string()),
                tags: Some(visible_tag.to_string()),
                country_a: Some(country.three_letter_code),
                ..Default::default()
            },
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_config() -> Config {
        serde_json::from_str(r#"{
            "osm": {
                "defaultValue": 1.0,
                "rankAddress": { "boundary": 10, "place": 20, "road": 26, "building": 28, "poi": 30 },
                "filters": []
            },
            "stedsnavn": { "defaultValue": 40.0, "rankAddress": 16 },
            "matrikkel": { "addressPopularity": 20.0, "streetPopularity": 20.0, "rankAddress": 26 },
            "poi": { "importance": 0.5, "rankAddress": 30 },
            "stopPlace": {
                "defaultValue": 50,
                "rankAddress": 30,
                "stopTypeFactors": {},
                "interchangeFactors": {}
            },
            "groupOfStopPlaces": { "gosBoostFactor": 10.0, "rankAddress": 30 },
            "importance": { "minPopularity": 1.0, "maxPopularity": 1000000000.0, "floor": 0.1 }
        }"#).unwrap()
    }

    fn test_data_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-data").join(name)
    }

    fn convert_test_file(suffix: &str) -> Vec<String> {
        let config = test_config();
        let input = test_data_path("poi-test.xml");
        let output = std::env::temp_dir().join(format!("test_poi_{suffix}.ndjson"));
        convert(&config, &input, &output, false).unwrap();
        let lines: Vec<String> = std::fs::read_to_string(&output).unwrap()
            .lines().map(String::from).collect();
        let _ = std::fs::remove_file(&output);
        lines
    }

    #[test]
    fn converts_valid_pois_and_filters_expired_and_future() {
        let lines = convert_test_file("filter");
        let data_lines: Vec<&String> = lines.iter().skip(1).collect(); // skip header
        // Should include: valid (1), always-valid (4), open-ended (5)
        // Should exclude: expired (2), future (3)
        assert_eq!(data_lines.len(), 3);
        assert!(lines.iter().any(|l| l.contains("TEST:TopographicPlace:1")));
        assert!(!lines.iter().any(|l| l.contains("TEST:TopographicPlace:2")));
        assert!(!lines.iter().any(|l| l.contains("TEST:TopographicPlace:3")));
        assert!(lines.iter().any(|l| l.contains("TEST:TopographicPlace:4")));
        assert!(lines.iter().any(|l| l.contains("TEST:TopographicPlace:5")));
    }

    #[test]
    fn poi_has_correct_coordinates_and_categories() {
        let lines = convert_test_file("coords");
        let poi1 = lines.iter().find(|l| l.contains("TEST:TopographicPlace:1")).unwrap();
        assert!(poi1.contains("10.75"));
        assert!(poi1.contains("59.91"));
        assert!(poi1.contains(OSM_CUSTOM_POI));
        assert!(poi1.contains("custom-poi"));
    }

    #[test]
    fn poi_output_contains_coordinates() {
        let lines = convert_test_file("rawcoords");
        let content = lines.join("\n");
        assert!(content.contains("10.75"));
        assert!(content.contains("59.91"));
    }

    #[test]
    fn is_valid_accepts_no_validity_period() {
        let now = Local::now().naive_local();
        let tp = TopographicPlaceXml {
            id: Some("test".to_string()),
            valid_between: None,
            descriptor: None,
            centroid: None,
        };
        assert!(is_valid(&tp, &now));
    }

    #[test]
    fn is_valid_rejects_expired() {
        let now = Local::now().naive_local();
        let tp = TopographicPlaceXml {
            id: Some("test".to_string()),
            valid_between: Some(ValidBetweenXml {
                from_date: Some("2020-01-01T00:00:00".to_string()),
                to_date: Some("2020-12-31T23:59:59".to_string()),
            }),
            descriptor: None,
            centroid: None,
        };
        assert!(!is_valid(&tp, &now));
    }

    #[test]
    fn is_valid_rejects_future() {
        let now = Local::now().naive_local();
        let tp = TopographicPlaceXml {
            id: Some("test".to_string()),
            valid_between: Some(ValidBetweenXml {
                from_date: Some("2099-01-01T00:00:00".to_string()),
                to_date: Some("2099-12-31T23:59:59".to_string()),
            }),
            descriptor: None,
            centroid: None,
        };
        assert!(!is_valid(&tp, &now));
    }

    #[test]
    fn is_valid_accepts_open_ended() {
        let now = Local::now().naive_local();
        let tp = TopographicPlaceXml {
            id: Some("test".to_string()),
            valid_between: Some(ValidBetweenXml {
                from_date: Some("2020-01-01T00:00:00".to_string()),
                to_date: None,
            }),
            descriptor: None,
            centroid: None,
        };
        assert!(is_valid(&tp, &now));
    }
}
