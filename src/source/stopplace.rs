use crate::common::category::*;
use crate::common::coordinate::Coordinate;
use crate::common::country::Country;
use crate::common::extra::Extra;
use crate::common::geo;
use crate::common::importance::ImportanceCalculator;
use crate::common::text::{join_osm_values, OSM_TAG_SEPARATOR};
use crate::common::translator;
use crate::config::Config;
use crate::target::json_writer::JsonWriter;
use crate::target::nominatim_id::NominatimId;
use crate::target::nominatim_place::*;
use quick_xml::de::from_str;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

pub fn convert(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let xml = std::fs::read_to_string(input)?;
    let result = parse_netex(&xml)?;
    let importance_calc = ImportanceCalculator::new(&config.importance);

    // Build child stop types map (parentRef -> list of child stopPlaceTypes)
    let mut stop_place_types: HashMap<String, Vec<String>> = HashMap::new();
    for sp in &result.stop_places {
        if let (Some(parent_ref), Some(sp_type)) = (&sp.parent_site_ref, &sp.stop_place_type) {
            stop_place_types
                .entry(parent_ref.ref_.clone())
                .or_default()
                .push(sp_type.clone());
        }
    }

    // Calculate popularities
    let stop_popularities: HashMap<String, i64> = result.stop_places.iter().map(|sp| {
        let child_types = stop_place_types.get(&sp.id).cloned().unwrap_or_default();
        let pop = calculate_stop_popularity(&config.stop_place, sp, &child_types);
        (sp.id.clone(), pop)
    }).collect();

    // Build child stop names and child stops maps
    let mut child_names: HashMap<String, Vec<String>> = HashMap::new();
    let mut child_stops: HashMap<String, Vec<&StopPlaceXml>> = HashMap::new();
    for sp in &result.stop_places {
        if let Some(parent_ref) = &sp.parent_site_ref {
            if let Some(name) = &sp.name {
                child_names.entry(parent_ref.ref_.clone()).or_default().push(name.clone());
            }
            child_stops.entry(parent_ref.ref_.clone()).or_default().push(sp);
        }
    }

    let mut entries = Vec::new();

    // Convert stop places
    for sp in &result.stop_places {
        let pop = stop_popularities.get(&sp.id).copied().unwrap_or(0);
        let child_stop_names = child_names.get(&sp.id).cloned().unwrap_or_default();
        let my_child_stops = child_stops.get(&sp.id).cloned().unwrap_or_default();

        if let Some(entry) = convert_stop_place(
            config, &importance_calc, sp, &result.topo_places,
            &stop_place_types, &result.fare_zones, pop,
            &child_stop_names, &my_child_stops,
        ) {
            entries.push(entry);
        }
    }

    // Convert groups of stop places
    for gosp in &result.groups {
        if let Some(entry) = convert_gosp(
            config, &importance_calc, gosp, &result.topo_places,
            &stop_popularities, &result.stop_places,
        ) {
            entries.push(entry);
        }
    }

    JsonWriter::export(&entries, output, is_appending)?;
    Ok(())
}

// ---- XML types ----

#[derive(Debug, Deserialize)]
struct StopPlaceXml {
    #[serde(rename = "@id")]
    id: String,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Description")]
    description: Option<String>,
    #[serde(rename = "Centroid")]
    centroid: Option<CentroidXml>,
    #[serde(rename = "TransportMode")]
    transport_mode: Option<String>,
    #[serde(rename = "BusSubmode")]
    bus_submode: Option<String>,
    #[serde(rename = "TramSubmode")]
    tram_submode: Option<String>,
    #[serde(rename = "RailSubmode")]
    rail_submode: Option<String>,
    #[serde(rename = "MetroSubmode")]
    metro_submode: Option<String>,
    #[serde(rename = "AirSubmode")]
    air_submode: Option<String>,
    #[serde(rename = "WaterSubmode")]
    water_submode: Option<String>,
    #[serde(rename = "TelecabinSubmode")]
    telecabin_submode: Option<String>,
    #[serde(rename = "StopPlaceType")]
    stop_place_type: Option<String>,
    #[serde(rename = "Weighting")]
    weighting: Option<String>,
    #[serde(rename = "TopographicPlaceRef")]
    topographic_place_ref: Option<RefAttr>,
    #[serde(rename = "ParentSiteRef")]
    parent_site_ref: Option<RefAttr>,
    #[serde(rename = "alternativeNames")]
    alternative_names: Option<AlternativeNamesXml>,
    #[serde(rename = "tariffZones")]
    tariff_zones: Option<TariffZonesXml>,
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

#[derive(Debug, Deserialize)]
struct RefAttr {
    #[serde(rename = "@ref")]
    ref_: String,
}

#[derive(Debug, Deserialize)]
struct AlternativeNamesXml {
    #[serde(rename = "AlternativeName", default)]
    names: Vec<AlternativeNameXml>,
}

#[derive(Debug, Deserialize)]
struct AlternativeNameXml {
    #[serde(rename = "NameType")]
    name_type: Option<String>,
    #[serde(rename = "Name")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TariffZonesXml {
    #[serde(rename = "TariffZoneRef", default)]
    refs: Vec<TariffZoneRefXml>,
}

#[derive(Debug, Deserialize)]
struct TariffZoneRefXml {
    #[serde(rename = "@ref")]
    ref_: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GroupOfStopPlacesXml {
    #[serde(rename = "@id")]
    id: String,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Centroid")]
    centroid: Option<CentroidXml>,
    #[serde(rename = "members")]
    members: Option<MembersXml>,
}

#[derive(Debug, Deserialize)]
struct MembersXml {
    #[serde(rename = "StopPlaceRef", default)]
    refs: Vec<RefAttr>,
}

#[derive(Debug, Deserialize)]
struct TopographicPlaceXml {
    #[serde(rename = "@id")]
    id: Option<String>,
    #[serde(rename = "Descriptor")]
    descriptor: Option<DescriptorXml>,
    #[serde(rename = "TopographicPlaceType")]
    topographic_place_type: Option<String>,
    #[serde(rename = "CountryRef")]
    country_ref: Option<RefAttr>,
    #[serde(rename = "ParentTopographicPlaceRef")]
    parent_ref: Option<RefAttr>,
}

#[derive(Debug, Deserialize)]
struct DescriptorXml {
    #[serde(rename = "Name")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FareZoneXml {
    #[serde(rename = "@id")]
    id: Option<String>,
    #[serde(rename = "AuthorityRef")]
    authority_ref: Option<RefAttr>,
}

struct ParseResult {
    stop_places: Vec<StopPlaceXml>,
    groups: Vec<GroupOfStopPlacesXml>,
    topo_places: HashMap<String, TopographicPlaceXml>,
    fare_zones: HashMap<String, FareZoneXml>,
}

// ---- Parsing ----

fn parse_netex(xml: &str) -> Result<ParseResult, Box<dyn std::error::Error>> {
    let mut stop_places = Vec::new();
    let mut groups = Vec::new();
    let mut topo_places = HashMap::new();
    let mut fare_zones = HashMap::new();

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref())?;
                match name {
                    "StopPlace" => {
                        let text = read_element_as_string(&mut reader, "StopPlace", e)?;
                        if let Ok(sp) = from_str::<StopPlaceXml>(&text) {
                            stop_places.push(sp);
                        }
                    }
                    "GroupOfStopPlaces" => {
                        let text = read_element_as_string(&mut reader, "GroupOfStopPlaces", e)?;
                        if let Ok(g) = from_str::<GroupOfStopPlacesXml>(&text) {
                            groups.push(g);
                        }
                    }
                    "TopographicPlace" => {
                        let text = read_element_as_string(&mut reader, "TopographicPlace", e)?;
                        if let Ok(tp) = from_str::<TopographicPlaceXml>(&text)
                            && let Some(id) = &tp.id {
                                topo_places.insert(id.clone(), tp);
                            }
                    }
                    "FareZone" => {
                        let text = read_element_as_string(&mut reader, "FareZone", e)?;
                        if let Ok(fz) = from_str::<FareZoneXml>(&text)
                            && let Some(id) = &fz.id {
                                fare_zones.insert(id.clone(), fz);
                            }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(ParseResult { stop_places, groups, topo_places, fare_zones })
}

pub fn read_element_as_string_pub(
    reader: &mut Reader<&[u8]>,
    tag_name: &str,
    start: &quick_xml::events::BytesStart,
) -> Result<String, Box<dyn std::error::Error>> {
    read_element_as_string(reader, tag_name, start)
}

fn read_element_as_string(
    reader: &mut Reader<&[u8]>,
    tag_name: &str,
    start: &quick_xml::events::BytesStart,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut inner = Vec::new();
    // Reconstruct the start tag
    inner.extend_from_slice(b"<");
    inner.extend_from_slice(tag_name.as_bytes());
    for attr in start.attributes() {
        let attr = attr?;
        inner.push(b' ');
        inner.extend_from_slice(attr.key.as_ref());
        inner.extend_from_slice(b"=\"");
        inner.extend_from_slice(&attr.value);
        inner.push(b'"');
    }
    inner.push(b'>');
    let mut depth = 1u32;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                inner.push(b'<');
                inner.extend_from_slice(e.name().as_ref());
                for attr in e.attributes() {
                    let attr = attr?;
                    inner.push(b' ');
                    inner.extend_from_slice(attr.key.as_ref());
                    inner.extend_from_slice(b"=\"");
                    inner.extend_from_slice(&attr.value);
                    inner.push(b'"');
                }
                inner.push(b'>');
                depth += 1;
            }
            Ok(Event::End(ref e)) => {
                depth -= 1;
                if depth == 0 {
                    inner.extend_from_slice(b"</");
                    inner.extend_from_slice(e.name().as_ref());
                    inner.push(b'>');
                    break;
                }
                inner.extend_from_slice(b"</");
                inner.extend_from_slice(e.name().as_ref());
                inner.push(b'>');
            }
            Ok(Event::Empty(ref e)) => {
                inner.push(b'<');
                inner.extend_from_slice(e.name().as_ref());
                for attr in e.attributes() {
                    let attr = attr?;
                    inner.push(b' ');
                    inner.extend_from_slice(attr.key.as_ref());
                    inner.extend_from_slice(b"=\"");
                    inner.extend_from_slice(&attr.value);
                    inner.push(b'"');
                }
                inner.extend_from_slice(b"/>");
            }
            Ok(Event::Text(ref e)) => {
                inner.extend_from_slice(e.as_ref());
            }
            Ok(Event::CData(ref e)) => {
                inner.extend_from_slice(b"<![CDATA[");
                inner.extend_from_slice(e.as_ref());
                inner.extend_from_slice(b"]]>");
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Box::new(e)),
            _ => {}
        }
        buf.clear();
    }
    Ok(String::from_utf8(inner)?)
}

// ---- Popularity ----

fn calculate_stop_popularity(
    config: &crate::config::StopPlaceConfig,
    sp: &StopPlaceXml,
    child_types: &[String],
) -> i64 {
    let mut pop = config.default_value;
    let mut all_types: Vec<&str> = child_types.iter().map(|s| s.as_str()).collect();
    if let Some(t) = &sp.stop_place_type {
        all_types.push(t);
    }
    let sum: f64 = all_types.iter().map(|t| config.stop_type_factors.get(*t).copied().unwrap_or(1.0)).sum();
    if sum > 0.0 {
        pop = (pop as f64 * sum) as i64;
    }
    if let Some(w) = &sp.weighting
        && let Some(factor) = config.interchange_factors.get(w) {
            pop = (pop as f64 * factor) as i64;
        }
    pop
}

// ---- Conversion ----

#[derive(Debug, PartialEq)]
enum StopPlaceRole { Child, Parent, Standalone }

#[allow(clippy::too_many_arguments)]
fn convert_stop_place(
    config: &Config,
    importance_calc: &ImportanceCalculator,
    sp: &StopPlaceXml,
    topo_places: &HashMap<String, TopographicPlaceXml>,
    stop_place_types: &HashMap<String, Vec<String>>,
    fare_zones: &HashMap<String, FareZoneXml>,
    popularity: i64,
    child_stop_names: &[String],
    child_stops: &[&StopPlaceXml],
) -> Option<NominatimPlace> {
    let centroid_xml = sp.centroid.as_ref()?;
    let coord = Coordinate::new(centroid_xml.location.latitude, centroid_xml.location.longitude);
    let sp_name = sp.name.as_deref()?;

    let locality_gid = sp.topographic_place_ref.as_ref().map(|r| r.ref_.clone());
    let locality = locality_gid.as_ref().and_then(|gid| {
        topo_places.get(gid).and_then(|tp| tp.descriptor.as_ref()?.name.clone())
    });
    let county_gid = locality_gid.as_ref().and_then(|gid| {
        topo_places.get(gid).and_then(|tp| tp.parent_ref.as_ref().map(|r| r.ref_.clone()))
    });
    let county = county_gid.as_ref().and_then(|gid| {
        topo_places.get(gid).and_then(|tp| tp.descriptor.as_ref()?.name.clone())
    });
    let country = determine_country(topo_places, sp, &coord);
    let child_types = stop_place_types.get(&sp.id).cloned().unwrap_or_default();
    let importance = RawNumber::from_f64_6dp(importance_calc.calculate_importance(popularity as f64));

    let role = if !child_types.is_empty() {
        StopPlaceRole::Parent
    } else if sp.parent_site_ref.is_some() {
        StopPlaceRole::Child
    } else {
        StopPlaceRole::Standalone
    };

    let source_cat = match role {
        StopPlaceRole::Parent => "legacy.source.openstreetmap",
        StopPlaceRole::Child => "legacy.source.geonames",
        StopPlaceRole::Standalone => "legacy.source.whosonfirst",
    };
    let multimodal_cat = match role {
        StopPlaceRole::Parent => Some("multimodal.parent".to_string()),
        StopPlaceRole::Child => Some("multimodal.child".to_string()),
        StopPlaceRole::Standalone => None,
    };

    let mut visible_cats = vec![
        OSM_STOP_PLACE.to_string(),
        "legacy.layer.venue".to_string(),
    ];
    // Legacy transport mode categories
    if sp.transport_mode.as_deref() == Some("funicular") {
        visible_cats.push(format!("{LEGACY_CATEGORY_PREFIX}funicular"));
    }
    visible_cats.push(source_cat.to_string());

    let mut indexed_cats = visible_cats.clone();
    let inferred_types: Vec<String> = child_types.iter().cloned()
        .chain(sp.stop_place_type.iter().cloned())
        .collect();
    for t in &inferred_types {
        indexed_cats.push(format!("{LEGACY_CATEGORY_PREFIX}{t}"));
    }
    indexed_cats.push(format!("{SOURCE_NSR}.{}", match role {
        StopPlaceRole::Child => "child",
        StopPlaceRole::Parent => "parent",
        StopPlaceRole::Standalone => "standalone",
    }));
    // Tariff zone categories (3 separate passes to match Kotlin ordering)
    if let Some(tz) = &sp.tariff_zones {
        // Pass 1: tariff zone IDs
        for tz_ref in &tz.refs {
            if let Some(ref_) = &tz_ref.ref_ {
                indexed_cats.push(tariff_zone_id_category(ref_));
            }
        }
        // Pass 2: tariff zone authorities (deduplicated)
        let mut seen_tz_auth = std::collections::HashSet::new();
        for tz_ref in &tz.refs {
            if let Some(ref_) = &tz_ref.ref_
                && ref_.contains(":TariffZone:")
                    && let Some(auth) = ref_.split(':').next() {
                        let cat = format!("{TARIFF_ZONE_AUTH_PREFIX}{auth}");
                        if seen_tz_auth.insert(cat.clone()) {
                            indexed_cats.push(cat);
                        }
                    }
        }
        // Pass 3: fare zone authorities (deduplicated)
        let mut seen_fz_auth = std::collections::HashSet::new();
        for tz_ref in &tz.refs {
            if let Some(ref_) = &tz_ref.ref_
                && let Some(fz) = fare_zones.get(ref_.as_str())
                    && let Some(auth_ref) = fz.authority_ref.as_ref().map(|a| a.ref_.as_str()) {
                        let cat = fare_zone_authority_category(auth_ref);
                        if seen_fz_auth.insert(cat.clone()) {
                            indexed_cats.push(cat);
                        }
                    }
        }
    }
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    if let Some(gid) = &county_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = &locality_gid { indexed_cats.push(locality_ids_category(gid)); }
    if let Some(mc) = multimodal_cat { indexed_cats.push(mc); }
    indexed_cats.push(as_category(&sp.id));

    // Alt names (deduplicated, preserving insertion order like Kotlin's Set)
    let visible_alt: Vec<String> = alt_stop_names(sp, sp_name, Some("label"));
    let mut indexed_alt: Vec<String> = alt_stop_names(sp, sp_name, None);
    indexed_alt.extend_from_slice(child_stop_names);
    indexed_alt.push(sp.id.clone());
    dedup_preserve_order(&mut indexed_alt);

    let tariff_zone_list = sp.tariff_zones.as_ref().and_then(|tz| {
        let refs: Vec<String> = tz.refs.iter().filter_map(|r| r.ref_.clone()).collect();
        join_osm_values(&refs)
    });

    let description = sp.description.as_ref()
        .filter(|d| !d.is_empty())
        .map(|d| {
            let eng = translator::translate(d);
            format!("nor:{d};eng:{eng}")
        });

    let transport_mode = collect_transport_modes(sp, child_stops);

    let stop_place_type_str = if inferred_types.is_empty() { None } else {
        Some(inferred_types.join(OSM_TAG_SEPARATOR))
    };

    let nominatim_id = NominatimId::StopPlace.create(&sp.id);
    let entry = NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: nominatim_id,
            object_type: "N".to_string(),
            object_id: nominatim_id,
            categories: indexed_cats,
            rank_address: config.stop_place.rank_address,
            importance,
            parent_place_id: Some(0),
            name: Some(Name {
                name: Some(sp_name.to_string()),
                name_en: None,
                alt_name: join_osm_values(&indexed_alt),
            }),
            address: Address { city: locality.clone(), county: county.clone(), ..Default::default() },
            housenumber: None,
            postcode: None,
            country_code: Some(country.name.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: Extra {
                id: Some(sp.id.clone()),
                source: Some("nsr".to_string()),
                accuracy: Some("point".to_string()),
                country_a: Some(country.three_letter_code.clone()),
                county_gid: county_gid.clone(),
                locality: locality.clone(),
                locality_gid: locality_gid.clone(),
                tariff_zones: tariff_zone_list,
                alt_name: join_osm_values(&visible_alt),
                description,
                tags: join_osm_values(&visible_cats),
                transport_mode,
                stop_place_type: stop_place_type_str,
                ..Default::default()
            },
        }],
    };
    Some(entry)
}

fn convert_gosp(
    config: &Config,
    importance_calc: &ImportanceCalculator,
    gosp: &GroupOfStopPlacesXml,
    topo_places: &HashMap<String, TopographicPlaceXml>,
    stop_popularities: &HashMap<String, i64>,
    stop_places: &[StopPlaceXml],
) -> Option<NominatimPlace> {
    let centroid_xml = gosp.centroid.as_ref()?;
    let coord = Coordinate::new(centroid_xml.location.latitude, centroid_xml.location.longitude);
    let group_name = gosp.name.as_deref()?;

    let mut locality = Some(group_name.to_string());
    let mut locality_gid: Option<String> = None;
    let mut county: Option<String> = None;
    let mut county_gid: Option<String> = None;

    if let Some(members) = &gosp.members {
        for sp_ref in &members.refs {
            if let Some(sp) = stop_places.iter().find(|s| s.id == sp_ref.ref_)
                && let Some(topo_ref) = sp.topographic_place_ref.as_ref()
                    && let Some(tp) = topo_places.get(&topo_ref.ref_)
                        && tp.topographic_place_type.as_deref() == Some("municipality") {
                            locality_gid = Some(topo_ref.ref_.clone());
                            locality = tp.descriptor.as_ref().and_then(|d| d.name.clone());
                            county_gid = tp.parent_ref.as_ref().map(|r| r.ref_.clone());
                            county = county_gid.as_ref().and_then(|gid| {
                                topo_places.get(gid).and_then(|tp2| tp2.descriptor.as_ref()?.name.clone())
                            });
                            break;
                        }
        }
    }

    let member_pops: Vec<i64> = gosp.members.as_ref().map(|m| {
        m.refs.iter().filter_map(|r| stop_popularities.get(&r.ref_).copied()).collect()
    }).unwrap_or_default();
    let gos_pop = if member_pops.is_empty() {
        config.group_of_stop_places.gos_boost_factor
    } else {
        config.group_of_stop_places.gos_boost_factor * member_pops.iter().fold(1.0, |acc, &p| acc * p as f64)
    };
    let importance = RawNumber::from_f64_6dp(importance_calc.calculate_importance(gos_pop));
    let country = geo::get_country(&coord).unwrap_or_else(Country::no);

    let visible_cats = vec![
        OSM_GOSP.to_string(),
        "legacy.layer.address".to_string(),
        "legacy.source.whosonfirst".to_string(),
        format!("{LEGACY_CATEGORY_PREFIX}{GOSP}"),
    ];
    let mut indexed_cats = visible_cats.clone();
    indexed_cats.push(SOURCE_NSR.to_string());
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    if let Some(gid) = &county_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = &locality_gid { indexed_cats.push(locality_ids_category(gid)); }
    indexed_cats.push(as_category(&gosp.id));

    let nominatim_id = NominatimId::Gosp.create(&gosp.id);
    Some(NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: nominatim_id,
            object_type: "N".to_string(),
            object_id: nominatim_id,
            categories: indexed_cats,
            rank_address: config.group_of_stop_places.rank_address,
            importance,
            parent_place_id: Some(0),
            name: Some(Name {
                name: Some(group_name.to_string()),
                name_en: None,
                alt_name: Some(gosp.id.clone()),
            }),
            address: Address { city: locality.clone(), county: county.clone(), ..Default::default() },
            housenumber: None,
            postcode: None,
            country_code: Some(country.name.clone()),
            centroid: coord.centroid(),
            bbox: coord.bbox(),
            extra: Extra {
                id: Some(gosp.id.clone()),
                source: Some("nsr".to_string()),
                accuracy: Some("point".to_string()),
                country_a: Some(country.three_letter_code),
                county_gid,
                locality,
                locality_gid,
                tags: join_osm_values(&visible_cats),
                ..Default::default()
            },
        }],
    })
}

fn determine_country(
    topo_places: &HashMap<String, TopographicPlaceXml>,
    sp: &StopPlaceXml,
    coord: &Coordinate,
) -> Country {
    if let Some(topo_ref) = sp.topographic_place_ref.as_ref()
        && let Some(tp) = topo_places.get(&topo_ref.ref_)
            && let Some(cr) = &tp.country_ref
                && let Some(c) = Country::parse(Some(&cr.ref_)) {
                    return c;
                }
    geo::get_country(coord).unwrap_or_else(Country::no)
}

fn dedup_preserve_order(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|s| seen.insert(s.clone()));
}

fn alt_stop_names(sp: &StopPlaceXml, primary_name: &str, name_type_filter: Option<&str>) -> Vec<String> {
    let Some(alt_names) = &sp.alternative_names else { return Vec::new() };
    alt_names.names.iter()
        .filter(|an| name_type_filter.is_none() || an.name_type.as_deref() == name_type_filter)
        .filter_map(|an| an.name.as_ref())
        .filter(|n| n.as_str() != primary_name && !n.is_empty())
        .cloned()
        .collect()
}

fn format_transport_mode(sp: &StopPlaceXml) -> Option<String> {
    let mode = sp.transport_mode.as_ref()?;
    let submode = sp.bus_submode.as_ref()
        .or(sp.tram_submode.as_ref())
        .or(sp.rail_submode.as_ref())
        .or(sp.metro_submode.as_ref())
        .or(sp.air_submode.as_ref())
        .or(sp.water_submode.as_ref())
        .or(sp.telecabin_submode.as_ref());
    Some(match submode {
        Some(sub) => format!("{mode}:{sub}"),
        None => mode.clone(),
    })
}

fn collect_transport_modes(sp: &StopPlaceXml, child_stops: &[&StopPlaceXml]) -> Option<String> {
    let own = format_transport_mode(sp);
    let child_modes: Vec<String> = child_stops.iter().filter_map(|cs| format_transport_mode(cs)).collect();
    let mut all: Vec<String> = own.into_iter().chain(child_modes).collect();
    dedup_preserve_order(&mut all);
    if all.is_empty() { None } else { Some(all.join(OSM_TAG_SEPARATOR)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::PathBuf;

    fn test_config() -> Config {
        serde_json::from_str(r#"{
            "osm": {
                "defaultValue": 1.0,
                "rankAddress": { "boundary": 10, "place": 20, "road": 26, "building": 28, "poi": 30 },
                "filters": [{"key": "amenity", "value": "hospital", "priority": 9}]
            },
            "stedsnavn": { "defaultValue": 40.0, "rankAddress": 16 },
            "matrikkel": { "addressPopularity": 20.0, "streetPopularity": 20.0, "rankAddress": 26 },
            "poi": { "importance": 0.5, "rankAddress": 30 },
            "stopPlace": {
                "defaultValue": 50,
                "rankAddress": 30,
                "stopTypeFactors": { "busStation": 2.0, "metroStation": 2.0, "railStation": 2.0 },
                "interchangeFactors": { "recommendedInterchange": 3.0, "preferredInterchange": 10.0 }
            },
            "groupOfStopPlaces": { "gosBoostFactor": 10.0, "rankAddress": 30 },
            "importance": { "minPopularity": 1.0, "maxPopularity": 1000000000.0, "floor": 0.1 }
        }"#).unwrap()
    }

    fn test_data_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-data").join(name)
    }

    fn make_stop_place(id: &str, name: &str, transport_mode: Option<&str>, stop_place_type: Option<&str>) -> StopPlaceXml {
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

    fn make_stop_place_with_submode(id: &str, transport_mode: &str, bus_sub: Option<&str>, rail_sub: Option<&str>, tram_sub: Option<&str>) -> StopPlaceXml {
        let mut sp = make_stop_place(id, "Test Stop", Some(transport_mode), None);
        sp.bus_submode = bus_sub.map(|s| s.to_string());
        sp.rail_submode = rail_sub.map(|s| s.to_string());
        sp.tram_submode = tram_sub.map(|s| s.to_string());
        sp
    }

    fn make_stop_place_with_alt_names(id: &str, name: &str, alt_names: Vec<(&str, Option<&str>)>) -> StopPlaceXml {
        let mut sp = make_stop_place(id, name, None, None);
        sp.alternative_names = Some(AlternativeNamesXml {
            names: alt_names.into_iter().map(|(n, nt)| AlternativeNameXml {
                name_type: nt.map(|s| s.to_string()),
                name: Some(n.to_string()),
            }).collect(),
        });
        sp
    }

    // ===== NetEx parsing tests =====

    #[test]
    fn parse_transport_mode_from_stop_place() {
        let xml = std::fs::read_to_string(test_data_path("stopPlaces.xml")).unwrap();
        let result = parse_netex(&xml).unwrap();
        let bus_stop = result.stop_places.iter().find(|sp| sp.id == "NSR:StopPlace:56697").unwrap();
        assert_eq!(bus_stop.transport_mode.as_deref(), Some("bus"));
        assert_eq!(bus_stop.stop_place_type.as_deref(), Some("onstreetBus"));

        let rail_station = result.stop_places.iter().find(|sp| sp.id == "NSR:StopPlace:305").unwrap();
        assert_eq!(rail_station.transport_mode.as_deref(), Some("rail"));
        assert_eq!(rail_station.stop_place_type.as_deref(), Some("railStation"));
    }

    #[test]
    fn parse_group_of_stop_places() {
        let xml = std::fs::read_to_string(test_data_path("stopPlaces.xml")).unwrap();
        let result = parse_netex(&xml).unwrap();
        assert_eq!(result.groups.len(), 2);
        assert_eq!(result.groups[0].id, "NSR:GroupOfStopPlaces:72");
        assert_eq!(result.groups[0].name.as_deref(), Some("Hammerfest"));
        assert_eq!(result.groups[1].id, "NSR:GroupOfStopPlaces:1");
        assert_eq!(result.groups[1].name.as_deref(), Some("Oslo"));
    }

    #[test]
    fn parse_fare_zones_with_authority_ref() {
        let xml = std::fs::read_to_string(test_data_path("stopPlaces.xml")).unwrap();
        let result = parse_netex(&xml).unwrap();
        assert_eq!(result.fare_zones.len(), 3);
        let fin31 = result.fare_zones.get("FIN:FareZone:31").unwrap();
        assert_eq!(fin31.authority_ref.as_ref().unwrap().ref_, "FIN:Authority:FIN_ID");
        let rut4 = result.fare_zones.get("RUT:FareZone:4").unwrap();
        assert_eq!(rut4.authority_ref.as_ref().unwrap().ref_, "RUT:Authority:RUT_ID");
    }

    // ===== Stop place popularity tests =====

    #[test]
    fn basic_stop_returns_default_popularity() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &[]);
        assert_eq!(pop, config.stop_place.default_value);
    }

    #[test]
    fn bus_station_has_higher_popularity_than_basic() {
        let config = test_config();
        let basic = make_stop_place("NSR:StopPlace:1", "Test", None, Some("onstreetBus"));
        let bus_station = make_stop_place("NSR:StopPlace:2", "Test", None, Some("busStation"));
        let basic_pop = calculate_stop_popularity(&config.stop_place, &basic, &[]);
        let bus_pop = calculate_stop_popularity(&config.stop_place, &bus_station, &[]);
        assert!(bus_pop > basic_pop);
    }

    #[test]
    fn metro_station_has_boosted_popularity() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("metroStation"));
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &[]);
        assert_eq!(pop, (config.stop_place.default_value as f64 * 2.0) as i64);
    }

    #[test]
    fn rail_station_has_boosted_popularity() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("railStation"));
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &[]);
        assert_eq!(pop, (config.stop_place.default_value as f64 * 2.0) as i64);
    }

    #[test]
    fn recommended_interchange_multiplies_popularity() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("railStation"));
        sp.weighting = Some("recommendedInterchange".to_string());
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &[]);
        // 50 * 2 (rail) * 3 (interchange) = 300
        assert_eq!(pop, (config.stop_place.default_value as f64 * 2.0 * 3.0) as i64);
    }

    #[test]
    fn preferred_interchange_gives_high_popularity() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("railStation"));
        sp.weighting = Some("preferredInterchange".to_string());
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &[]);
        // 50 * 2 * 10 = 1000
        assert_eq!(pop, (config.stop_place.default_value as f64 * 2.0 * 10.0) as i64);
    }

    #[test]
    fn popularity_values_strictly_ordered() {
        let config = test_config();
        let pops: Vec<i64> = vec![
            calculate_stop_popularity(&config.stop_place, &make_stop_place("1", "T", None, None), &[]),
            calculate_stop_popularity(&config.stop_place, &make_stop_place("2", "T", None, Some("busStation")), &[]),
            {
                let mut sp = make_stop_place("3", "T", None, Some("railStation"));
                sp.weighting = Some("recommendedInterchange".to_string());
                calculate_stop_popularity(&config.stop_place, &sp, &[])
            },
            {
                let mut sp = make_stop_place("4", "T", None, Some("railStation"));
                sp.weighting = Some("preferredInterchange".to_string());
                calculate_stop_popularity(&config.stop_place, &sp, &[])
            },
        ];
        for i in 0..pops.len() - 1 {
            assert!(pops[i] < pops[i + 1], "Expected {} < {}", pops[i], pops[i + 1]);
        }
    }

    // ===== Multimodal parent tests =====

    #[test]
    fn multimodal_parent_uses_sum_of_child_types() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let child_types = vec!["railStation".to_string(), "metroStation".to_string()];
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &child_types);
        // 50 * (2 + 2) = 200
        assert_eq!(pop, (config.stop_place.default_value as f64 * 4.0) as i64);
    }

    #[test]
    fn multimodal_parent_sums_factors_not_multiplies() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let child_types = vec!["railStation".to_string(), "metroStation".to_string(), "busStation".to_string()];
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &child_types);
        // 50 * (2+2+2) = 300, NOT 50 * 2*2*2 = 400
        assert_eq!(pop, (config.stop_place.default_value as f64 * 6.0) as i64);
    }

    #[test]
    fn multimodal_parent_unconfigured_child_defaults_to_factor_1() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let child_types = vec!["ferryStop".to_string(), "tramStation".to_string()];
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &child_types);
        // 50 * (1+1) = 100
        assert_eq!(pop, (config.stop_place.default_value as f64 * 2.0) as i64);
    }

    #[test]
    fn multimodal_parent_with_interchange() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        sp.weighting = Some("preferredInterchange".to_string());
        let child_types = vec!["railStation".to_string(), "metroStation".to_string()];
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &child_types);
        // 50 * (2+2) * 10 = 2000
        assert_eq!(pop, (config.stop_place.default_value as f64 * 4.0 * 10.0) as i64);
    }

    #[test]
    fn duplicate_child_types_are_summed() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let child_types = vec!["railStation".to_string(), "railStation".to_string(), "railStation".to_string()];
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &child_types);
        // 50 * (2+2+2) = 300
        assert_eq!(pop, (config.stop_place.default_value as f64 * 6.0) as i64);
    }

    // ===== GroupOfStopPlaces popularity tests =====

    #[test]
    fn gosp_single_member_boosted() {
        let config = test_config();
        let pop = config.group_of_stop_places.gos_boost_factor * 60.0;
        assert_eq!(pop, 600.0);
    }

    #[test]
    fn gosp_two_members_multiplies() {
        let config = test_config();
        let pop = config.group_of_stop_places.gos_boost_factor * (60.0 * 60.0);
        assert_eq!(pop, 36000.0);
    }

    #[test]
    fn gosp_empty_returns_boost_factor() {
        let config = test_config();
        // Empty fold: 1.0 * boost
        let pops: Vec<i64> = vec![];
        let result = if pops.is_empty() {
            config.group_of_stop_places.gos_boost_factor
        } else {
            config.group_of_stop_places.gos_boost_factor * pops.iter().fold(1.0, |acc, &p| acc * p as f64)
        };
        assert_eq!(result, 10.0);
    }

    #[test]
    fn gosp_realistic_oslo_scenario() {
        let config = test_config();
        let member_pops: Vec<i64> = vec![600, 60, 60];
        let pop = config.group_of_stop_places.gos_boost_factor
            * member_pops.iter().fold(1.0, |acc, &p| acc * p as f64);
        assert_eq!(pop, 21_600_000.0);
    }

    // ===== Transport mode formatting tests =====

    #[test]
    fn transport_mode_with_bus_submode() {
        let sp = make_stop_place_with_submode("1", "bus", Some("localBus"), None, None);
        assert_eq!(format_transport_mode(&sp), Some("bus:localBus".to_string()));
    }

    #[test]
    fn transport_mode_with_rail_submode() {
        let sp = make_stop_place_with_submode("1", "rail", None, Some("highSpeedRail"), None);
        assert_eq!(format_transport_mode(&sp), Some("rail:highSpeedRail".to_string()));
    }

    #[test]
    fn transport_mode_without_submode() {
        let sp = make_stop_place("1", "Test", Some("bus"), Some("onstreetBus"));
        assert_eq!(format_transport_mode(&sp), Some("bus".to_string()));
    }

    #[test]
    fn parent_collects_child_transport_modes() {
        let parent = make_stop_place("1", "Parent", Some("rail"), Some("railStation"));
        let child_bus = make_stop_place_with_submode("2", "bus", Some("localBus"), None, None);
        let child_tram = make_stop_place("3", "Tram", Some("tram"), None);
        let child_refs: Vec<&StopPlaceXml> = vec![&child_bus, &child_tram];
        let result = collect_transport_modes(&parent, &child_refs);
        assert_eq!(result, Some("rail;bus:localBus;tram".to_string()));
    }

    #[test]
    fn parent_preserves_duplicate_mode_keys_with_different_submodes() {
        let parent = make_stop_place_with_submode("1", "tram", None, None, Some("cityTram"));
        let child = make_stop_place("2", "Tram", Some("tram"), None);
        let child_refs: Vec<&StopPlaceXml> = vec![&child];
        let result = collect_transport_modes(&parent, &child_refs);
        assert_eq!(result, Some("tram:cityTram;tram".to_string()));
    }

    #[test]
    fn standalone_has_only_own_transport_mode() {
        let sp = make_stop_place_with_submode("1", "bus", Some("localBus"), None, None);
        let result = collect_transport_modes(&sp, &[]);
        assert_eq!(result, Some("bus:localBus".to_string()));
    }

    // ===== Alternative names tests =====

    #[test]
    fn only_label_visible_in_extra_alt_name() {
        let sp = make_stop_place_with_alt_names("1", "Oslo S", vec![
            ("Oslo Sentralstasjon", Some("label")),
            ("Oslo Central Station", Some("translation")),
            ("Jernbanetorget", None),
        ]);
        let visible = alt_stop_names(&sp, "Oslo S", Some("label"));
        let indexed = alt_stop_names(&sp, "Oslo S", None);
        assert!(visible.contains(&"Oslo Sentralstasjon".to_string()));
        assert!(!visible.contains(&"Oslo Central Station".to_string()));
        assert!(!visible.contains(&"Jernbanetorget".to_string()));
        assert!(indexed.contains(&"Oslo Sentralstasjon".to_string()));
        assert!(indexed.contains(&"Oslo Central Station".to_string()));
        assert!(indexed.contains(&"Jernbanetorget".to_string()));
    }

    #[test]
    fn alt_names_empty_when_none() {
        let sp = make_stop_place("1", "Simple Stop", None, None);
        let result = alt_stop_names(&sp, "Simple Stop", None);
        assert!(result.is_empty());
    }

    #[test]
    fn alt_names_exclude_primary_name() {
        let sp = make_stop_place_with_alt_names("1", "Oslo S", vec![
            ("Oslo S", Some("label")),
            ("Oslo Central", Some("translation")),
        ]);
        let result = alt_stop_names(&sp, "Oslo S", None);
        assert!(!result.contains(&"Oslo S".to_string()));
        assert!(result.contains(&"Oslo Central".to_string()));
    }

    // ===== Category tests =====

    #[test]
    fn funicular_transport_mode_included_in_categories() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);
        let sp = make_stop_place("NSR:StopPlace:1", "Test", Some("funicular"), Some("other"));
        let result = convert_stop_place(
            &config, &importance_calc, &sp, &HashMap::new(), &HashMap::new(),
            &HashMap::new(), 50, &[], &[],
        ).unwrap();
        let cats = &result.content[0].categories;
        assert!(cats.iter().any(|c| c == "legacy.category.funicular"));
        assert!(cats.iter().any(|c| c == "legacy.category.other"));
    }

    #[test]
    fn bus_transport_mode_not_in_categories() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);
        let sp = make_stop_place("NSR:StopPlace:1", "Test", Some("bus"), Some("onstreetBus"));
        let result = convert_stop_place(
            &config, &importance_calc, &sp, &HashMap::new(), &HashMap::new(),
            &HashMap::new(), 50, &[], &[],
        ).unwrap();
        let cats = &result.content[0].categories;
        assert!(!cats.iter().any(|c| c == "legacy.category.bus"));
        assert!(cats.iter().any(|c| c == "legacy.category.onstreetBus"));
    }

    #[test]
    fn parent_stop_includes_child_types_and_multimodal_category() {
        let config = test_config();
        let importance_calc = ImportanceCalculator::new(&config.importance);
        let sp = make_stop_place("NSR:StopPlace:Parent", "Hub", Some("funicular"), Some("other"));
        let mut child_types_map: HashMap<String, Vec<String>> = HashMap::new();
        child_types_map.insert("NSR:StopPlace:Parent".to_string(),
            vec!["onstreetBus".to_string(), "railStation".to_string(), "metroStation".to_string()]);
        let result = convert_stop_place(
            &config, &importance_calc, &sp, &HashMap::new(), &child_types_map,
            &HashMap::new(), 50, &[], &[],
        ).unwrap();
        let cats = &result.content[0].categories;
        assert!(cats.iter().any(|c| c == "legacy.category.funicular"));
        assert!(cats.iter().any(|c| c == "legacy.category.onstreetBus"));
        assert!(cats.iter().any(|c| c == "legacy.category.railStation"));
        assert!(cats.iter().any(|c| c == "legacy.category.metroStation"));
        assert!(cats.iter().any(|c| c == "multimodal.parent"));
    }

    // ===== Full conversion tests =====

    #[test]
    fn convert_stop_places_xml_produces_output() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_stopplace_output.ndjson");
        convert(&config, &input, &output, false).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("NominatimDumpFile"));
        assert!(content.contains("NSR:StopPlace:56697"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn convert_produces_group_of_stop_places() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_gosp_output.ndjson");
        convert(&config, &input, &output, false).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("NSR:GroupOfStopPlaces:1"));
        assert!(content.contains("NSR:GroupOfStopPlaces:72"));
        assert!(content.contains("\"name\":\"Oslo\""));
        assert!(content.contains("osm.public_transport.group_of_stop_places"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn output_has_valid_json_on_each_line() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_valid_json.ndjson");
        convert(&config, &input, &output, false).unwrap();
        let lines: Vec<String> = std::fs::read_to_string(&output).unwrap().lines().map(String::from).collect();
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(line.starts_with('{'));
            assert!(line.ends_with('}'));
        }
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn all_stop_places_have_coordinates() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_coords.ndjson");
        convert(&config, &input, &output, false).unwrap();
        let lines: Vec<String> = std::fs::read_to_string(&output).unwrap().lines()
            .filter(|l| l.contains("NSR:StopPlace:"))
            .map(String::from).collect();
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(line.contains("\"centroid\":["));
        }
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn stop_places_have_fare_zone_authority_categories() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_authority.ndjson");
        convert(&config, &input, &output, false).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("fare_zone_authority.FIN.Authority.FIN_ID"));
        assert!(content.contains("fare_zone_authority.RUT.Authority.RUT_ID"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn stop_place_with_bus_submode_has_transport_mode_in_output() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_transport_mode.ndjson");
        convert(&config, &input, &output, false).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("\"transport_mode\":\"bus:localBus\""));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn stop_places_have_county_gid_and_locality_gid() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_gid.ndjson");
        convert(&config, &input, &output, false).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("county_gid.KVE"));
        assert!(content.contains("locality_gid.KVE"));
        let _ = std::fs::remove_file(&output);
    }
}
