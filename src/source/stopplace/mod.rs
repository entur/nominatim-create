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
