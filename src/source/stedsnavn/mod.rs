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
use crate::target::nominatim_id::NominatimId;
use crate::target::nominatim_place::*;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::path::Path;

const TARGET_TYPES: &[&str] = &["by", "bydel", "tettsted", "tettsteddel", "tettbebyggelse"];
const ACCEPTED_STATUS: &[&str] = &["vedtatt", "godkjent", "privat", "samlevedtak"];

pub fn convert(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let xml = std::fs::read_to_string(input)?;
    let entries = parse_gml(&xml)?;
    let importance_calc = ImportanceCalculator::new(&config.importance);

    let nominatim_entries: Vec<NominatimPlace> = entries
        .into_iter()
        .map(|e| convert_to_nominatim(&e, config, &importance_calc))
        .collect();

    JsonWriter::export(&nominatim_entries, output, is_appending)?;
    Ok(())
}

struct StedsnavnEntry {
    lokal_id: String,
    stedsnavn: String,
    navneobjekttype: String,
    kommunenummer: String,
    kommunenavn: String,
    fylkesnummer: String,
    fylkesnavn: String,
    coordinates: Vec<(f64, f64)>, // (easting, northing) UTM33
    annen_skrivemaate: Vec<String>,
}

fn parse_gml(xml: &str) -> Result<Vec<StedsnavnEntry>, Box<dyn std::error::Error>> {
    let mut entries = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"featureMember" || e.name().as_ref() == b"gml:featureMember" => {
                if let Some(entry) = parse_feature_member(&mut reader)? {
                    entries.push(entry);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(entries)
}

fn parse_feature_member(reader: &mut Reader<&[u8]>) -> Result<Option<StedsnavnEntry>, Box<dyn std::error::Error>> {
    let mut lokal_id: Option<String> = None;
    let mut navnerom: Option<String> = None;
    let mut stedsnavn: Option<String> = None;
    let mut navneobjekttype: Option<String> = None;
    let mut skrivemaatestatus: Option<String> = None;
    let mut kommunenummer: Option<String> = None;
    let mut kommunenavn: Option<String> = None;
    let mut fylkesnummer: Option<String> = None;
    let mut fylkesnavn: Option<String> = None;
    let mut coordinates: Vec<(f64, f64)> = Vec::new();
    let mut annen_skrivemaate: Vec<String> = Vec::new();
    let mut inside_annen = false;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref()).unwrap_or("");
                match name {
                    "lokalId" | "app:lokalId" => lokal_id = Some(reader.read_text(e.name())?.into_owned()),
                    "navnerom" | "app:navnerom" => navnerom = Some(reader.read_text(e.name())?.into_owned()),
                    "komplettskrivemåte" | "app:komplettskrivemåte" => {
                        let text = reader.read_text(e.name())?.into_owned();
                        if inside_annen {
                            annen_skrivemaate.push(text);
                        } else if stedsnavn.is_none() {
                            stedsnavn = Some(text);
                        }
                    }
                    "navneobjekttype" | "app:navneobjekttype" => navneobjekttype = Some(reader.read_text(e.name())?.into_owned()),
                    "skrivemåtestatus" | "app:skrivemåtestatus" => {
                        let text = reader.read_text(e.name())?.into_owned();
                        if !inside_annen && skrivemaatestatus.is_none() {
                            skrivemaatestatus = Some(text);
                        }
                    }
                    "kommunenummer" | "app:kommunenummer" => kommunenummer = Some(reader.read_text(e.name())?.into_owned()),
                    "kommunenavn" | "app:kommunenavn" => kommunenavn = Some(reader.read_text(e.name())?.into_owned()),
                    "fylkesnummer" | "app:fylkesnummer" => fylkesnummer = Some(reader.read_text(e.name())?.into_owned()),
                    "fylkesnavn" | "app:fylkesnavn" => fylkesnavn = Some(reader.read_text(e.name())?.into_owned()),
                    "annenSkrivemåte" | "app:annenSkrivemåte" => inside_annen = true,
                    "posList" | "gml:posList" => {
                        let text = reader.read_text(e.name())?.into_owned();
                        parse_pos_list(&text, &mut coordinates);
                    }
                    "pos" | "gml:pos" => {
                        let text = reader.read_text(e.name())?.into_owned();
                        parse_pos(&text, &mut coordinates);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref()).unwrap_or("");
                match name {
                    "featureMember" | "gml:featureMember" => break,
                    "annenSkrivemåte" | "app:annenSkrivemåte" => inside_annen = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }

    // Filter
    let is_target = navneobjekttype.as_deref().is_some_and(|t| TARGET_TYPES.contains(&t));
    let has_status = skrivemaatestatus.as_deref().is_some_and(|s| ACCEPTED_STATUS.contains(&s));
    let has_fields = lokal_id.is_some() && navnerom.is_some() && stedsnavn.is_some()
        && kommunenummer.is_some() && kommunenavn.is_some()
        && fylkesnummer.is_some() && fylkesnavn.is_some();

    if is_target && has_status && has_fields {
        Ok(Some(StedsnavnEntry {
            lokal_id: lokal_id.unwrap(),
            stedsnavn: stedsnavn.unwrap(),
            navneobjekttype: navneobjekttype.unwrap(),
            kommunenummer: kommunenummer.unwrap(),
            kommunenavn: kommunenavn.unwrap(),
            fylkesnummer: fylkesnummer.unwrap(),
            fylkesnavn: fylkesnavn.unwrap(),
            coordinates,
            annen_skrivemaate,
        }))
    } else {
        Ok(None)
    }
}

fn parse_pos_list(text: &str, coords: &mut Vec<(f64, f64)>) {
    let parts: Vec<&str> = text.split_whitespace().collect();
    for chunk in parts.chunks(2) {
        if chunk.len() == 2
            && let (Ok(east), Ok(north)) = (chunk[0].parse::<f64>(), chunk[1].parse::<f64>()) {
                coords.push((east, north));
            }
    }
}

fn parse_pos(text: &str, coords: &mut Vec<(f64, f64)>) {
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() >= 2
        && let (Ok(east), Ok(north)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) {
            coords.push((east, north));
        }
}

fn convert_to_nominatim(entry: &StedsnavnEntry, config: &Config, importance_calc: &ImportanceCalculator) -> NominatimPlace {
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
    let nominatim_id = NominatimId::Stedsnavn.create(&entry.lokal_id);

    NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: nominatim_id,
            object_type: "N".to_string(),
            object_id: nominatim_id,
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
