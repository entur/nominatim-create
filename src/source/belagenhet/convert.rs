use crate::common::category::*;
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
use std::collections::HashMap;
use std::path::Path;

use super::parse::*;

pub fn convert_all(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let importance_calc = ImportanceCalculator::new(&config.importance);

    let all_addresses = parse_gpkg(input)?;

    // Pass 1: stream addresses to output
    let mut writer = JsonWriter::open(output, is_appending)?;
    let mut street_groups: HashMap<String, StreetAgg> = HashMap::new();

    for addr in &all_addresses {
        let place = convert_address(addr, config, &importance_calc);
        writer.write_entry(&place)?;

        // Build street aggregation for Pass 2, keyed by (street_name, kommunkod)
        if let Some(name) = addr.street_or_place_name() {
            let kommun = addr.kommunkod.as_deref().unwrap_or("");
            let key = format!("{name}|{kommun}");
            let agg = street_groups.entry(key).or_insert_with(|| StreetAgg {
                representative: addr.clone(),
                sum_east: 0.0,
                sum_north: 0.0,
                count: 0,
            });
            agg.sum_east += addr.easting;
            agg.sum_north += addr.northing;
            agg.count += 1;
        }
    }

    // Pass 2: stream streets to output
    let mut street_count = 0;
    for agg in street_groups.values() {
        let avg_east = agg.sum_east / agg.count as f64;
        let avg_north = agg.sum_north / agg.count as f64;
        let place = convert_street(&agg.representative, avg_east, avg_north, config, &importance_calc);
        writer.write_entry(&place)?;
        street_count += 1;
    }

    eprintln!("Converted {} addresses and {street_count} streets", all_addresses.len());
    Ok(())
}

/// Format a Swedish county (län) code as a Nominatim GID.
/// "LAN" is the codespace for Lantmäteriet identifiers.
fn county_gid(lanskod: Option<&str>) -> Option<String> {
    lanskod.map(|code| format!("LAN:TopographicPlace:{code}"))
}

/// Format a Swedish municipality (kommun) code as a Nominatim GID.
fn locality_gid(kommunkod: Option<&str>) -> Option<String> {
    kommunkod.map(|code| format!("LAN:TopographicPlace:{code}"))
}

fn convert_address(
    addr: &BelagenhetAdress,
    config: &Config,
    importance_calc: &ImportanceCalculator,
) -> NominatimPlace {
    let coord = geo::convert_sweref99tm_to_lat_lon(addr.easting, addr.northing);
    let country = geo::get_country(&coord).unwrap_or_else(Country::se);

    let street_name = addr.street_or_place_name();
    let housenumber = addr.housenumber();

    // Use Lantmäteriet's unique object identifier for a collision-free stable ID
    let id = format!("LAN:PostalAddress:{}", addr.objektidentitet);
    let id_cat = as_category(&id);

    let c_gid = county_gid(addr.lanskod.as_deref());
    let l_gid = locality_gid(addr.kommunkod.as_deref());

    let tags = [OSM_ADDRESS, "legacy.source.openaddresses", "legacy.layer.address"];
    let mut indexed_cats: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
    indexed_cats.push(SOURCE_BELAGENHET.to_string());
    indexed_cats.push(LAYER_ADDRESS.to_string());
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    indexed_cats.push(id_cat);
    if let Some(gid) = &c_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = &l_gid { indexed_cats.push(locality_ids_category(gid)); }

    let importance = RawNumber::from_f64_6dp(importance_calc.calculate_importance(config.belagenhet.address_popularity));

    let postort = addr.postort.as_deref().unwrap_or("");

    NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: as_place_id(&id),
            object_type: "N".to_string(),
            object_id: 0,
            categories: indexed_cats,
            rank_address: config.belagenhet.rank_address,
            importance,
            parent_place_id: Some(0),
            name: None,
            housenumber,
            address: Address {
                street: street_name,
                city: Some(titleize(postort)),
                county: addr.lanskod.as_deref().map(|_| {
                    // Use kommunnamn as a locality stand-in; county (län) is not in the data
                    // by name, but kommunnamn provides useful context
                    addr.kommunnamn.as_deref().map(titleize).unwrap_or_default()
                }).filter(|s| !s.is_empty()),
            },
            postcode: addr.postnummer.clone(),
            country_code: Some(country.name.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: Extra {
                id: Some(id),
                source: Some("lantmateriet-belagenhetsadress".to_string()),
                accuracy: Some("point".to_string()),
                country_a: Some(country.three_letter_code),
                locality: addr.kommunnamn.as_deref().map(titleize),
                locality_gid: l_gid,
                county_gid: c_gid,
                borough: addr.kommundel_namn.clone(),
                alt_name: addr.popularnamn.clone(),
                tags: join_osm_values(&tags.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                ..Default::default()
            },
        }],
    }
}

fn convert_street(
    addr: &BelagenhetAdress,
    avg_east: f64,
    avg_north: f64,
    config: &Config,
    importance_calc: &ImportanceCalculator,
) -> NominatimPlace {
    let coord = geo::convert_sweref99tm_to_lat_lon(avg_east, avg_north);
    let country = geo::get_country(&coord).unwrap_or_else(Country::se);
    let street_name = addr.street_or_place_name().unwrap_or_default();
    let kommun = addr.kommunkod.as_deref().unwrap_or("");

    let id = format!("LAN:TopographicPlace:{kommun}-{street_name}");

    let c_gid = county_gid(addr.lanskod.as_deref());
    let l_gid = locality_gid(addr.kommunkod.as_deref());

    let tags = [OSM_STREET, "legacy.layer.address", "legacy.category.street"];
    let mut indexed_cats: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
    indexed_cats.push(SOURCE_BELAGENHET.to_string());
    indexed_cats.push(LAYER_STREET.to_string());
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    indexed_cats.push(as_category(&id));
    if let Some(gid) = &c_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = &l_gid { indexed_cats.push(locality_ids_category(gid)); }

    let importance = RawNumber::from_f64_6dp(importance_calc.calculate_importance(config.belagenhet.street_popularity));

    let postort = addr.postort.as_deref().unwrap_or("");

    let indexed_alt = vec![id.clone()];

    NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: as_place_id(&id),
            object_type: "N".to_string(),
            object_id: 0,
            categories: indexed_cats,
            rank_address: config.belagenhet.rank_address,
            importance,
            parent_place_id: Some(0),
            name: Some(Name {
                name: Some(street_name.clone()),
                name_en: None,
                alt_name: join_osm_values(&indexed_alt),
            }),
            housenumber: None,
            address: Address {
                street: Some(street_name),
                city: Some(titleize(postort)),
                county: addr.kommunnamn.as_deref().map(titleize),
            },
            postcode: None,
            country_code: Some(country.name.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: Extra {
                id: Some(id),
                source: Some("lantmateriet-belagenhetsadress".to_string()),
                accuracy: Some("point".to_string()),
                country_a: Some(country.three_letter_code),
                locality: addr.kommunnamn.as_deref().map(titleize),
                locality_gid: l_gid,
                county_gid: c_gid,
                borough: addr.kommundel_namn.clone(),
                tags: join_osm_values(&tags.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                ..Default::default()
            },
        }],
    }
}
