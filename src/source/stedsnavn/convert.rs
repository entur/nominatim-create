use crate::common::category::*;
use crate::common::coordinate::Coordinate;
use crate::common::country::Country;
use crate::common::extra::Extra;
use crate::common::geo;
use crate::common::importance::ImportanceCalculator;
use crate::common::text::join_osm_values;
use crate::common::util::titleize;
use crate::config::Config;
use crate::target::json_writer::JsonWriter;
use crate::target::nominatim_id::as_place_id;
use crate::target::nominatim_place::*;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::BufReader;
use std::path::Path;

use super::gml::{StedsnavnEntry, parse_feature_member};

pub fn convert_all(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let importance_calc = ImportanceCalculator::new(&config.importance);
    let mut writer = JsonWriter::open(output, is_appending)?;

    let file = std::fs::File::open(input)?;
    let buf_reader = BufReader::new(file);
    let mut reader = Reader::from_reader(buf_reader);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"featureMember" || e.name().as_ref() == b"gml:featureMember" => {
                if let Some(entry) = parse_feature_member(&mut reader)? {
                    let place = convert_to_nominatim(&entry, config, &importance_calc);
                    writer.write_entry(&place)?;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(())
}

pub(crate) fn convert_to_nominatim(entry: &StedsnavnEntry, config: &Config, importance_calc: &ImportanceCalculator) -> NominatimPlace {
    let coord = if !entry.coordinates.is_empty() {
        let avg_east = entry.coordinates.iter().map(|c| c.0).sum::<f64>() / entry.coordinates.len() as f64;
        let avg_north = entry.coordinates.iter().map(|c| c.1).sum::<f64>() / entry.coordinates.len() as f64;
        geo::convert_utm33_to_lat_lon(avg_east, avg_north)
    } else {
        Coordinate::ZERO
    };
    let country = geo::get_country(&coord).unwrap_or_else(Country::no);

    let visible_cats = vec![
        OSM_POI.to_string(),
        "legacy.source.whosonfirst".to_string(),
        "legacy.layer.address".to_string(),
        format!("{LEGACY_CATEGORY_PREFIX}{}", entry.navneobjekttype),
    ];

    let county_gid = format!("KVE:TopographicPlace:{}", entry.fylkesnummer);
    let locality_gid = format!("KVE:TopographicPlace:{}", entry.kommunenummer);

    let mut indexed_cats = visible_cats.clone();
    indexed_cats.push(SOURCE_STEDSNAVN.to_string());
    indexed_cats.push(LAYER_POI.to_string());
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    indexed_cats.push(county_ids_category(&county_gid));
    indexed_cats.push(locality_ids_category(&locality_gid));
    indexed_cats.push(as_category(&entry.lokal_id));

    let visible_alt: Vec<String> = entry.annen_skrivemaate.iter()
        .filter(|s| s.as_str() != entry.stedsnavn)
        .cloned().collect();
    let mut indexed_alt = visible_alt.clone();
    indexed_alt.push(entry.lokal_id.clone());

    let importance = RawNumber::from_f64_6dp(importance_calc.calculate_importance(config.stedsnavn.default_value));
    NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: as_place_id(&entry.lokal_id),
            object_type: "N".to_string(),
            object_id: 0,
            categories: indexed_cats,
            rank_address: config.stedsnavn.rank_address,
            importance,
            parent_place_id: Some(0),
            name: Some(Name {
                name: Some(entry.stedsnavn.clone()),
                name_en: None,
                alt_name: join_osm_values(&indexed_alt),
            }),
            address: Address {
                city: Some(titleize(&entry.kommunenavn)),
                county: Some(entry.fylkesnavn.clone()),
                ..Default::default()
            },
            housenumber: None,
            postcode: None,
            country_code: Some(country.name.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: Extra {
                id: Some(entry.lokal_id.clone()),
                source: Some("kartverket-stedsnavn".to_string()),
                accuracy: Some("point".to_string()),
                country_a: Some(country.three_letter_code),
                county_gid: Some(county_gid),
                locality: Some(entry.kommunenavn.clone()),
                locality_gid: Some(locality_gid),
                tags: join_osm_values(&visible_cats),
                alt_name: join_osm_values(&visible_alt),
                ..Default::default()
            },
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::gml::{TARGET_TYPES, ACCEPTED_STATUS, parse_gml};
    use crate::source::test_helpers::{test_config, test_data_path};

    // ===== Place type tests =====

    #[test]
    fn target_types_recognized() {
        for t in &["by", "bydel", "tettsted", "tettsteddel", "tettbebyggelse"] {
            assert!(TARGET_TYPES.contains(t), "Should recognize {t}");
        }
    }

    #[test]
    fn non_target_types_rejected() {
        for t in &["grend", "fylke", "kommune"] {
            assert!(!TARGET_TYPES.contains(t), "Should reject {t}");
        }
    }

    #[test]
    fn exactly_5_target_types() {
        assert_eq!(TARGET_TYPES.len(), 5);
    }

    // ===== Spelling status tests =====

    #[test]
    fn accepted_statuses_recognized() {
        for s in &["vedtatt", "godkjent", "privat", "samlevedtak"] {
            assert!(ACCEPTED_STATUS.contains(s), "Should accept {s}");
        }
    }

    #[test]
    fn rejected_statuses() {
        for s in &["uvurdert", "avslått", "foreslått", "klage", "historisk"] {
            assert!(!ACCEPTED_STATUS.contains(s), "Should reject {s}");
        }
    }

    #[test]
    fn exactly_4_accepted_statuses() {
        assert_eq!(ACCEPTED_STATUS.len(), 4);
    }

    // ===== GML parsing tests (bydel.gml) =====

    #[test]
    fn bydel_finds_stedsnavn_despite_historisk_alt_spelling() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn bydel_converts_with_grunerloekka_name() {
        let config = test_config();
        let input = test_data_path("bydel.gml");
        let output = std::env::temp_dir().join("test_bydel_output.ndjson");
        convert_all(&config, &input, &output, false).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("Grünerløkka"));
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3); // header + 2 entries
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn bydel_alt_name_populated_for_entry_with_annen_skrivemaate() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        let entry_with_alt = entries.iter().find(|e| !e.annen_skrivemaate.is_empty());
        assert!(entry_with_alt.is_some(), "Should have entry with annenSkrivemåte");
        let place = convert_to_nominatim(entry_with_alt.unwrap(), &config, &importance_calc);
        let alt_name = place.content[0].name.as_ref().unwrap().alt_name.as_ref();
        assert!(alt_name.is_some(), "alt_name should be populated");
    }

    // ===== Conversion field tests =====

    #[test]
    fn converted_entries_have_correct_source() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        for entry in &entries {
            let place = convert_to_nominatim(entry, &config, &importance_calc);
            assert_eq!(place.content[0].extra.source.as_deref(), Some("kartverket-stedsnavn"));
        }
    }

    #[test]
    fn converted_entries_have_point_accuracy() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        for entry in &entries {
            let place = convert_to_nominatim(entry, &config, &importance_calc);
            assert_eq!(place.content[0].extra.accuracy.as_deref(), Some("point"));
        }
    }

    #[test]
    fn converted_entries_have_country_code_no() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        for entry in &entries {
            let place = convert_to_nominatim(entry, &config, &importance_calc);
            assert_eq!(place.content[0].country_code.as_deref(), Some("no"));
        }
    }

    #[test]
    fn converted_entries_have_object_type_n() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        for entry in &entries {
            let place = convert_to_nominatim(entry, &config, &importance_calc);
            assert_eq!(place.content[0].object_type, "N");
        }
    }

    #[test]
    fn converted_entries_have_valid_importance() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        for entry in &entries {
            let place = convert_to_nominatim(entry, &config, &importance_calc);
            let imp: f64 = place.content[0].importance.0.parse().unwrap();
            assert!(imp > 0.0 && imp <= 1.0);
        }
    }

    #[test]
    fn converted_entries_have_valid_rank_address() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        for entry in &entries {
            let place = convert_to_nominatim(entry, &config, &importance_calc);
            assert!(place.content[0].rank_address <= 20);
        }
    }

    #[test]
    fn converted_entries_have_locality_and_county_gid() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        for entry in &entries {
            let place = convert_to_nominatim(entry, &config, &importance_calc);
            let extra = &place.content[0].extra;
            let expected_locality = format!("KVE:TopographicPlace:{}", entry.kommunenummer);
            assert_eq!(extra.locality_gid.as_deref(), Some(expected_locality.as_str()));
            let expected_county = format!("KVE:TopographicPlace:{}", entry.fylkesnummer);
            assert_eq!(extra.county_gid.as_deref(), Some(expected_county.as_str()));
        }
    }

    #[test]
    fn converted_entries_have_coordinates_in_valid_range() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        for entry in &entries {
            let place = convert_to_nominatim(entry, &config, &importance_calc);
            let centroid = &place.content[0].centroid;
            assert_eq!(centroid.len(), 2);
            let lon: f64 = centroid[0].to_string().parse().unwrap();
            let lat: f64 = centroid[1].to_string().parse().unwrap();
            assert!(lon > 0.0 && lon < 20.0, "Norwegian lon: {lon}");
            assert!(lat > 58.0 && lat < 72.0, "Norwegian lat: {lat}");
        }
    }

    #[test]
    fn output_has_county_gid_and_locality_gid_in_categories() {
        let config = test_config();
        let input = test_data_path("bydel.gml");
        let output = std::env::temp_dir().join("test_stedsnavn_gid.ndjson");
        convert_all(&config, &input, &output, false).unwrap();
        let lines: Vec<String> = std::fs::read_to_string(&output).unwrap()
            .lines().skip(1).map(String::from).collect();
        assert!(lines.iter().any(|l| l.contains("county_gid.KVE") && l.contains("locality_gid.KVE")));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn city_names_are_titleized() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        for entry in &entries {
            let place = convert_to_nominatim(entry, &config, &importance_calc);
            if let Some(city) = &place.content[0].address.city {
                let first = city.chars().next().unwrap();
                assert!(first.is_uppercase() || !first.is_alphabetic(), "City should be titleized: {city}");
            }
        }
    }

    #[test]
    fn output_header_present() {
        let config = test_config();
        let input = test_data_path("bydel.gml");
        let output = std::env::temp_dir().join("test_stedsnavn_header.ndjson");
        convert_all(&config, &input, &output, false).unwrap();
        let first_line = std::fs::read_to_string(&output).unwrap().lines().next().unwrap().to_string();
        assert!(first_line.contains("NominatimDumpFile"));
        assert!(first_line.contains("version"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn parsed_entries_have_all_required_fields() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        for entry in &entries {
            assert!(!entry.lokal_id.is_empty());
            assert!(!entry.stedsnavn.is_empty());
            assert!(!entry.navneobjekttype.is_empty());
            assert!(!entry.kommunenummer.is_empty());
            assert!(!entry.kommunenavn.is_empty());
            assert!(!entry.fylkesnummer.is_empty());
            assert!(!entry.fylkesnavn.is_empty());
            assert!(!entry.coordinates.is_empty());
        }
    }

    #[test]
    fn legacy_category_present() {
        let xml = std::fs::read_to_string(test_data_path("bydel.gml")).unwrap();
        let entries = parse_gml(&xml).unwrap();
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);

        for entry in &entries {
            let place = convert_to_nominatim(entry, &config, &importance_calc);
            assert!(place.content[0].categories.iter().any(|c| c.starts_with(LEGACY_CATEGORY_PREFIX)));
        }
    }
}
