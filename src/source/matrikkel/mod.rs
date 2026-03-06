use crate::common::category::*;
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
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub fn convert(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
    stedsnavn_gml: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let importance_calc = ImportanceCalculator::new(&config.importance);
    let kommune_mapping = if let Some(gml_path) = stedsnavn_gml {
        build_kommune_mapping(gml_path)?
    } else {
        HashMap::new()
    };

    let all_addresses = parse_csv(input)?;

    // Pass 1: addresses
    let addresses: Vec<NominatimPlace> = all_addresses
        .iter()
        .map(|a| convert_address(a, config, &importance_calc, &kommune_mapping))
        .collect();
    JsonWriter::export(&addresses, output, is_appending)?;

    // Pass 2: streets (grouped by adressenavn + kommunenummer)
    let mut street_groups: HashMap<(String, String), StreetAgg> = HashMap::new();
    for addr in all_addresses.iter() {
        if let Some(name) = &addr.adressenavn {
            let key = (name.clone(), addr.kommunenummer.clone().unwrap_or_default());
            let agg = street_groups.entry(key).or_insert_with(|| StreetAgg {
                representative: addr.clone(),
                sum_ost: 0.0, sum_nord: 0.0, count: 0,
            });
            agg.sum_ost += addr.ost;
            agg.sum_nord += addr.nord;
            agg.count += 1;
        }
    }

    let streets: Vec<NominatimPlace> = street_groups.values()
        .map(|agg| {
            let avg_ost = agg.sum_ost / agg.count as f64;
            let avg_nord = agg.sum_nord / agg.count as f64;
            convert_street(&agg.representative, avg_ost, avg_nord, config, &importance_calc, &kommune_mapping)
        })
        .collect();
    JsonWriter::export(&streets, output, true)?;

    Ok(())
}

struct StreetAgg {
    representative: MatrikkelAdresse,
    sum_ost: f64,
    sum_nord: f64,
    count: usize,
}

#[derive(Debug, Clone)]
struct MatrikkelAdresse {
    lokalid: String,
    kommunenummer: Option<String>,
    kommunenavn: Option<String>,
    adressetilleggsnavn: Option<String>,
    adressenavn: Option<String>,
    nummer: Option<String>,
    bokstav: Option<String>,
    nord: f64,
    ost: f64,
    postnummer: Option<String>,
    poststed: String,
    grunnkretsnummer: Option<String>,
    grunnkretsnavn: Option<String>,
}

fn parse_csv(input: &Path) -> Result<Vec<MatrikkelAdresse>, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(input)?;
    let reader = BufReader::new(file);
    let mut addresses = Vec::new();
    let mut first = true;

    for line in reader.lines() {
        let line = line?;
        if first { first = false; continue; } // skip header
        let tokens: Vec<&str> = line.split(';').collect();
        if tokens.len() < 46 || tokens[3] != "vegadresse" { continue; }

        let nord: f64 = tokens[17].parse().unwrap_or(0.0);
        let ost: f64 = tokens[18].parse().unwrap_or(0.0);

        addresses.push(MatrikkelAdresse {
            lokalid: if tokens[0].is_empty() { "-1".to_string() } else { tokens[0].to_string() },
            kommunenummer: non_empty(tokens[1]),
            kommunenavn: non_empty(tokens[2]),
            adressetilleggsnavn: non_empty(tokens[4]),
            adressenavn: non_empty(tokens[7]),
            nummer: non_empty(tokens[8]),
            bokstav: non_empty(tokens[9]),
            nord, ost,
            postnummer: non_empty(tokens[19]),
            poststed: tokens[20].to_string(),
            grunnkretsnummer: non_empty(tokens[21]),
            grunnkretsnavn: non_empty(tokens[22]),
        });
    }
    Ok(addresses)
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() { None } else { Some(s.to_string()) }
}

fn convert_address(
    addr: &MatrikkelAdresse,
    config: &Config,
    importance_calc: &ImportanceCalculator,
    kommune_mapping: &HashMap<String, KommuneInfo>,
) -> NominatimPlace {
    let coord = geo::convert_utm33_to_lat_lon(addr.ost, addr.nord);
    let country = geo::get_country(&coord).unwrap_or_else(Country::no);
    let fylkesnummer = addr.kommunenummer.as_ref().and_then(|k| kommune_mapping.get(k).map(|i| i.fylkesnummer.clone()));
    let county_gid = fylkesnummer.as_ref().map(|f| format!("KVE:TopographicPlace:{f}"));
    let locality_gid = addr.kommunenummer.as_ref().map(|k| format!("KVE:TopographicPlace:{k}"));

    let tags = [OSM_ADDRESS, "legacy.source.openaddresses", "legacy.layer.address", "legacy.category.vegadresse"];
    let id = &addr.lokalid;

    let id_cat = if id.chars().all(|c| c.is_ascii_digit()) {
        format!("openaddresses.address.{id}")
    } else {
        as_category(id)
    };

    let mut indexed_cats: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
    indexed_cats.push(SOURCE_ADRESSE.to_string());
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    indexed_cats.push(id_cat);
    if let Some(gid) = &county_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = &locality_gid { indexed_cats.push(county_ids_category(gid)); }

    let housenumber = match (&addr.nummer, &addr.bokstav) {
        (Some(n), Some(b)) => Some(format!("{n}{b}")),
        (Some(n), None) => Some(n.clone()),
        _ => None,
    };

    let fylkesnavn = addr.kommunenummer.as_ref()
        .and_then(|k| kommune_mapping.get(k).map(|i| titleize(&i.fylkesnavn)));

    let importance = RawNumber::from_f64_6dp(importance_calc.calculate_importance(config.matrikkel.address_popularity));
    let place_id = NominatimId::Address.create(id);

    NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id,
            object_type: "N".to_string(),
            object_id: place_id,
            categories: indexed_cats,
            rank_address: config.matrikkel.rank_address,
            importance,
            parent_place_id: Some(0),
            name: None, // addresses are "nameless"
            housenumber,
            address: Address {
                street: addr.adressenavn.clone(),
                city: Some(titleize(&addr.poststed)),
                county: fylkesnavn,
            },
            postcode: addr.postnummer.clone(),
            country_code: Some(country.name.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: Extra {
                id: Some(id.to_string()),
                source: Some("kartverket-matrikkelenadresse".to_string()),
                accuracy: Some("point".to_string()),
                country_a: Some(country.three_letter_code),
                county_gid,
                locality: addr.kommunenavn.as_ref().map(|n| titleize(n)),
                locality_gid,
                borough: addr.grunnkretsnavn.as_ref().map(|n| titleize(n)),
                borough_gid: addr.grunnkretsnummer.as_ref().map(|n| format!("borough:{n}")),
                tags: join_osm_values(&tags.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                alt_name: addr.adressetilleggsnavn.clone(),
                ..Default::default()
            },
        }],
    }
}

fn convert_street(
    addr: &MatrikkelAdresse,
    avg_ost: f64,
    avg_nord: f64,
    config: &Config,
    importance_calc: &ImportanceCalculator,
    kommune_mapping: &HashMap<String, KommuneInfo>,
) -> NominatimPlace {
    let coord = geo::convert_utm33_to_lat_lon(avg_ost, avg_nord);
    let country = geo::get_country(&coord).unwrap_or_else(Country::no);
    let street_name = addr.adressenavn.as_deref().unwrap_or("");
    let id = format!("KVE:TopographicPlace:{}-{street_name}", addr.kommunenummer.as_deref().unwrap_or(""));

    let fylkesnummer = addr.kommunenummer.as_ref().and_then(|k| kommune_mapping.get(k).map(|i| i.fylkesnummer.clone()));
    let county_gid = fylkesnummer.as_ref().map(|f| format!("KVE:TopographicPlace:{f}"));
    let locality_gid = addr.kommunenummer.as_ref().map(|k| format!("KVE:TopographicPlace:{k}"));

    let tags = [OSM_STREET, "legacy.source.whosonfirst", "legacy.layer.address", "legacy.category.street"];
    let mut indexed_cats: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
    indexed_cats.push(SOURCE_ADRESSE.to_string());
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    indexed_cats.push(as_category(&id));
    if let Some(gid) = &county_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = &locality_gid { indexed_cats.push(county_ids_category(gid)); }

    let mut indexed_alt = Vec::new();
    if let Some(tillegg) = &addr.adressetilleggsnavn { indexed_alt.push(tillegg.clone()); }
    indexed_alt.push(id.clone());

    let fylkesnavn = addr.kommunenummer.as_ref()
        .and_then(|k| kommune_mapping.get(k).map(|i| titleize(&i.fylkesnavn)));

    let importance = RawNumber::from_f64_6dp(importance_calc.calculate_importance(config.matrikkel.street_popularity));
    let place_id = NominatimId::Street.create(&addr.lokalid);

    NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id,
            object_type: "N".to_string(),
            object_id: place_id,
            categories: indexed_cats,
            rank_address: config.matrikkel.rank_address,
            importance,
            parent_place_id: Some(0),
            name: Some(Name {
                name: Some(street_name.to_string()),
                name_en: None,
                alt_name: join_osm_values(&indexed_alt),
            }),
            housenumber: None,
            address: Address {
                street: addr.adressenavn.clone(),
                city: Some(titleize(&addr.poststed)),
                county: fylkesnavn,
            },
            postcode: None,
            country_code: Some(country.name.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: Extra {
                id: Some(id.clone()),
                source: Some("kartverket-matrikkelenadresse".to_string()),
                accuracy: Some("point".to_string()),
                country_a: Some(country.three_letter_code),
                county_gid,
                locality: addr.kommunenavn.as_ref().map(|n| titleize(n)),
                locality_gid,
                borough: addr.grunnkretsnavn.as_ref().map(|n| titleize(n)),
                borough_gid: addr.grunnkretsnummer.as_ref().map(|n| format!("borough:{n}")),
                tags: join_osm_values(&tags.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                alt_name: addr.adressetilleggsnavn.clone(),
                ..Default::default()
            },
        }],
    }
}

// ---- Kommune-Fylke mapping from Stedsnavn GML ----

#[derive(Debug, Clone)]
pub struct KommuneInfo {
    pub fylkesnummer: String,
    pub fylkesnavn: String,
}

fn build_kommune_mapping(gml_path: &Path) -> Result<HashMap<String, KommuneInfo>, Box<dyn std::error::Error>> {
    eprintln!("Building kommune-fylke mapping from {}...", gml_path.display());
    let file = std::fs::File::open(gml_path)?;
    let buf_reader = BufReader::new(file);
    let mut mapping = HashMap::new();
    let mut reader = quick_xml::Reader::from_reader(buf_reader);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut text_buf = Vec::new();
    let mut kommunenummer: Option<String> = None;
    let mut fylkesnummer: Option<String> = None;
    let mut fylkesnavn: Option<String> = None;
    let mut in_feature = false;
    let mut current_field: Option<&'static str> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref()).unwrap_or("");
                match name {
                    "featureMember" | "gml:featureMember" => {
                        in_feature = true;
                        kommunenummer = None;
                        fylkesnummer = None;
                        fylkesnavn = None;
                    }
                    n if in_feature && (n == "kommunenummer" || n == "app:kommunenummer") => {
                        current_field = Some("kommunenummer");
                        text_buf.clear();
                    }
                    n if in_feature && (n == "fylkesnummer" || n == "app:fylkesnummer") => {
                        current_field = Some("fylkesnummer");
                        text_buf.clear();
                    }
                    n if in_feature && (n == "fylkesnavn" || n == "app:fylkesnavn") => {
                        current_field = Some("fylkesnavn");
                        text_buf.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if current_field.is_some() {
                    text_buf.extend_from_slice(e.as_ref());
                }
            }
            Ok(Event::End(ref e)) => {
                if let Some(field) = current_field {
                    let text = String::from_utf8_lossy(&text_buf).trim().to_string();
                    match field {
                        "kommunenummer" => kommunenummer = Some(text),
                        "fylkesnummer" => fylkesnummer = Some(text),
                        "fylkesnavn" => {
                            fylkesnavn = Some(text.split(" - ").next().unwrap_or(&text).to_string());
                        }
                        _ => {}
                    }
                    current_field = None;
                }

                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref()).unwrap_or("");
                if name == "featureMember" || name == "gml:featureMember" {
                    in_feature = false;
                    if let (Some(kn), Some(fn_), Some(fnavn)) = (&kommunenummer, &fylkesnummer, &fylkesnavn) {
                        mapping.entry(kn.clone()).or_insert_with(|| KommuneInfo {
                            fylkesnummer: fn_.clone(),
                            fylkesnavn: fnavn.clone(),
                        });
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(mapping)
}
