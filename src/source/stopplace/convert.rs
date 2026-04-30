use crate::common::category::*;
use crate::common::coordinate::Coordinate;
use crate::common::country::Country;
use crate::common::extra::Extra;
use crate::common::geo;
use crate::common::importance::ImportanceCalculator;
use crate::common::text::{join_osm_values, OSM_TAG_SEPARATOR};
use crate::common::translator;
use crate::common::usage::UsageBoost;
use crate::config::Config;
use crate::target::json_writer::JsonWriter;
use crate::target::nominatim_id::as_place_id;
use crate::target::nominatim_place::*;
use std::collections::HashMap;
use std::path::Path;

use super::popularity::calculate_stop_popularity;
use super::xml::*;

pub fn convert_all(
    config: &Config,
    input: &Path,
    output: &Path,
    is_appending: bool,
    usage: &UsageBoost,
) -> Result<(), Box<dyn std::error::Error>> {
    let xml = std::fs::read_to_string(input)?;
    let result = parse_netex(&xml)?;
    let importance_calc = ImportanceCalculator::new(&config.importance, usage);

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

    // Calculate popularities. The optional usage boost nudges popular stops upward;
    // GoSPs inherit the signal automatically through the member-product propagation
    // in `calculate_gosp_popularity` so they don't need a separate lookup.
    let stop_popularities: HashMap<String, i64> = result.stop_places.iter().map(|sp| {
        let child_types = stop_place_types.get(&sp.id).cloned().unwrap_or_default();
        let pop = calculate_stop_popularity(&config.stop_place, sp, &child_types, usage.factor(&sp.id));
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

/// A stop place's role in the parent-child hierarchy. Affects which source category
/// and multimodal marker are assigned in the output.
#[derive(Debug, PartialEq)]
enum StopPlaceRole {
    Child,
    Parent,
    Standalone,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn convert_stop_place(
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
    let role = classify_role(&child_types, sp.parent_site_ref.is_some());

    let inferred_types: Vec<String> = child_types
        .iter()
        .cloned()
        .chain(sp.stop_place_type.iter().cloned())
        .collect();

    let (visible_cats, indexed_cats) = build_stop_categories(
        sp, &role, &inferred_types, &country, &county_gid, &locality_gid, fare_zones,
    );

    let indexed_alt = build_stop_alt_names(sp, sp_name, child_stop_names);
    let visible_alt: Vec<String> = alt_stop_names(sp, sp_name, Some("label"));

    let entry = NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: as_place_id(&sp.id),
            object_type: "N".to_string(),
            object_id: 0,
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
            extra: build_stop_extra(
                sp, &country, &county_gid, &locality, &locality_gid,
                &visible_alt, &visible_cats, &inferred_types, child_stops,
            ),
        }],
    };
    Some(entry)
}

/// Determine role: if this stop has children → Parent, if it references a parent → Child,
/// otherwise → Standalone.
fn classify_role(child_types: &[String], has_parent: bool) -> StopPlaceRole {
    if !child_types.is_empty() {
        StopPlaceRole::Parent
    } else if has_parent {
        StopPlaceRole::Child
    } else {
        StopPlaceRole::Standalone
    }
}

fn build_stop_categories(
    sp: &StopPlaceXml,
    role: &StopPlaceRole,
    inferred_types: &[String],
    country: &Country,
    county_gid: &Option<String>,
    locality_gid: &Option<String>,
    fare_zones: &HashMap<String, FareZoneXml>,
) -> (Vec<String>, Vec<String>) {
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
    if sp.transport_mode.as_deref() == Some("funicular") {
        visible_cats.push(format!("{LEGACY_CATEGORY_PREFIX}funicular"));
    }
    visible_cats.push(source_cat.to_string());

    let mut indexed_cats = visible_cats.clone();
    for t in inferred_types {
        indexed_cats.push(format!("{LEGACY_CATEGORY_PREFIX}{t}"));
    }
    indexed_cats.push(format!("{SOURCE_NSR}.{}", match role {
        StopPlaceRole::Child => "child",
        StopPlaceRole::Parent => "parent",
        StopPlaceRole::Standalone => "standalone",
    }));
    indexed_cats.push(SOURCE_NSR.to_string());
    indexed_cats.push(LAYER_STOP_PLACE.to_string());
    append_tariff_zone_categories(&mut indexed_cats, sp, fare_zones);
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    if let Some(gid) = county_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = locality_gid { indexed_cats.push(locality_ids_category(gid)); }
    if let Some(mc) = multimodal_cat { indexed_cats.push(mc); }
    indexed_cats.push(as_category(&sp.id));

    (visible_cats, indexed_cats)
}

/// Append tariff zone categories in 3 passes to match the original converter's ordering:
/// 1. Zone IDs (e.g. `tariff_zone_id.RUT.TariffZone.1`)
/// 2. Zone authorities extracted from the zone ref prefix (e.g. `tariff_zone_authority.RUT`)
/// 3. Fare zone authorities from the FareZone → AuthorityRef lookup
fn append_tariff_zone_categories(
    indexed_cats: &mut Vec<String>,
    sp: &StopPlaceXml,
    fare_zones: &HashMap<String, FareZoneXml>,
) {
    let Some(tz) = &sp.tariff_zones else { return };
    // Pass 1: tariff zone IDs
    for tz_ref in &tz.refs {
        if let Some(ref_) = &tz_ref.ref_ {
            indexed_cats.push(tariff_zone_id_category(ref_));
        }
    }
    // Pass 2: tariff zone authorities (deduplicated).
    // Zone refs follow the pattern "AUTHORITY:TariffZone:NUMBER", so the authority
    // is the prefix before the first colon.
    let mut seen_tz_auth = std::collections::HashSet::new();
    for tz_ref in &tz.refs {
        if let Some(ref_) = &tz_ref.ref_
            && ref_.contains(":TariffZone:")
            && let Some(auth) = ref_.split(':').next()
        {
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
            && let Some(auth_ref) = fz.authority_ref.as_ref().map(|a| a.ref_.as_str())
        {
            let cat = fare_zone_authority_category(auth_ref);
            if seen_fz_auth.insert(cat.clone()) {
                indexed_cats.push(cat);
            }
        }
    }
}

fn build_stop_alt_names(sp: &StopPlaceXml, sp_name: &str, child_stop_names: &[String]) -> Vec<String> {
    let mut indexed_alt: Vec<String> = alt_stop_names(sp, sp_name, None);
    indexed_alt.extend_from_slice(child_stop_names);
    indexed_alt.push(sp.id.clone());
    dedup_preserve_order(&mut indexed_alt);
    indexed_alt
}

#[allow(clippy::too_many_arguments)]
fn build_stop_extra(
    sp: &StopPlaceXml,
    country: &Country,
    county_gid: &Option<String>,
    locality: &Option<String>,
    locality_gid: &Option<String>,
    visible_alt: &[String],
    visible_cats: &[String],
    inferred_types: &[String],
    child_stops: &[&StopPlaceXml],
) -> Extra {
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

    let stop_place_type_str = if inferred_types.is_empty() {
        None
    } else {
        Some(inferred_types.join(OSM_TAG_SEPARATOR))
    };

    Extra {
        id: Some(sp.id.clone()),
        source: Some("nsr".to_string()),
        accuracy: Some("point".to_string()),
        country_a: Some(country.three_letter_code.clone()),
        county_gid: county_gid.clone(),
        locality: locality.clone(),
        locality_gid: locality_gid.clone(),
        tariff_zones: tariff_zone_list,
        alt_name: join_osm_values(visible_alt),
        description,
        tags: join_osm_values(visible_cats),
        transport_mode,
        stop_place_type: stop_place_type_str,
        ..Default::default()
    }
}

pub(crate) fn convert_gosp(
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

    let (locality, locality_gid, county, county_gid) =
        resolve_gosp_geography(gosp, topo_places, stop_places);

    let gos_pop = calculate_gosp_popularity(gosp, stop_popularities);
    let country = geo::get_country(&coord).unwrap_or_else(Country::no);
    // GoSP popularity grows multiplicatively with member count and easily exceeds
    // `importance.maxPopularity`. For home-country GoSPs we use the unclamped variant and
    // apply the configured multiplier so major Norwegian cities (Bergen, Trondheim) outrank
    // near-focus streets that share the same name prefix. Foreign GoSPs (e.g. NSR's Berlin ZOB
    // entry for international bus routes) keep the clamped 0-1 importance so they don't
    // outrank Norwegian cities for users searching in Norway.
    let raw_importance = if country.name == config.group_of_stop_places.home_country {
        importance_calc.calculate_importance_unclamped(gos_pop)
            * config.group_of_stop_places.importance_multiplier
    } else {
        importance_calc.calculate_importance(gos_pop)
    };
    let importance = RawNumber::from_f64_6dp(raw_importance);

    let visible_cats = vec![
        OSM_GOSP.to_string(),
        "legacy.layer.address".to_string(),
        "legacy.source.whosonfirst".to_string(),
        format!("{LEGACY_CATEGORY_PREFIX}{GOSP}"),
    ];
    let mut indexed_cats = visible_cats.clone();
    indexed_cats.push(SOURCE_NSR.to_string());
    indexed_cats.push(LAYER_GOSP.to_string());
    indexed_cats.push(format!("{COUNTRY_PREFIX}{}", country.name));
    if let Some(gid) = &county_gid { indexed_cats.push(county_ids_category(gid)); }
    if let Some(gid) = &locality_gid { indexed_cats.push(locality_ids_category(gid)); }
    indexed_cats.push(as_category(&gosp.id));

    Some(NominatimPlace {
        type_: "Place".to_string(),
        content: vec![PlaceContent {
            place_id: as_place_id(&gosp.id),
            object_type: "N".to_string(),
            object_id: 0,
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

fn resolve_gosp_geography(
    gosp: &GroupOfStopPlacesXml,
    topo_places: &HashMap<String, TopographicPlaceXml>,
    stop_places: &[StopPlaceXml],
) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    let group_name = gosp.name.as_deref().unwrap_or_default();
    let mut locality = Some(group_name.to_string());
    let mut locality_gid: Option<String> = None;
    let mut county: Option<String> = None;
    let mut county_gid: Option<String> = None;

    if let Some(members) = &gosp.members {
        for sp_ref in &members.refs {
            if let Some(sp) = stop_places.iter().find(|s| s.id == sp_ref.ref_)
                && let Some(topo_ref) = sp.topographic_place_ref.as_ref()
                && let Some(tp) = topo_places.get(&topo_ref.ref_)
                && tp.topographic_place_type.as_deref() == Some("municipality")
            {
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

    (locality, locality_gid, county, county_gid)
}

/// GoSP popularity is the product of its members' popularities. Empty product is 1.0,
/// which lands on the importance floor for GoSPs whose members couldn't be resolved.
fn calculate_gosp_popularity(
    gosp: &GroupOfStopPlacesXml,
    stop_popularities: &HashMap<String, i64>,
) -> f64 {
    let Some(members) = gosp.members.as_ref() else { return 1.0 };
    members.refs.iter()
        .filter_map(|r| stop_popularities.get(&r.ref_).copied())
        .fold(1.0, |acc, p| acc * p as f64)
}

fn determine_country(
    topo_places: &HashMap<String, TopographicPlaceXml>,
    sp: &StopPlaceXml,
    coord: &Coordinate,
) -> Country {
    if let Some(topo_ref) = sp.topographic_place_ref.as_ref()
        && let Some(tp) = topo_places.get(&topo_ref.ref_)
        && let Some(cr) = &tp.country_ref
        && let Some(c) = Country::parse(Some(&cr.ref_))
    {
        return c;
    }
    geo::get_country(coord).unwrap_or_else(Country::no)
}

pub(crate) fn dedup_preserve_order(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|s| seen.insert(s.clone()));
}

pub(crate) fn alt_stop_names(
    sp: &StopPlaceXml,
    primary_name: &str,
    name_type_filter: Option<&str>,
) -> Vec<String> {
    let Some(alt_names) = &sp.alternative_names else { return Vec::new() };
    alt_names.names.iter()
        .filter(|an| name_type_filter.is_none() || an.name_type.as_deref() == name_type_filter)
        .filter_map(|an| an.name.as_ref())
        .filter(|n| n.as_str() != primary_name && !n.is_empty())
        .cloned()
        .collect()
}

pub(crate) fn format_transport_mode(sp: &StopPlaceXml) -> Option<String> {
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

pub(crate) fn collect_transport_modes(sp: &StopPlaceXml, child_stops: &[&StopPlaceXml]) -> Option<String> {
    let own = format_transport_mode(sp);
    let child_modes: Vec<String> = child_stops.iter().filter_map(|cs| format_transport_mode(cs)).collect();
    let mut all: Vec<String> = own.into_iter().chain(child_modes).collect();
    dedup_preserve_order(&mut all);
    if all.is_empty() { None } else { Some(all.join(OSM_TAG_SEPARATOR)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::helpers::*;

    static EMPTY_USAGE: std::sync::LazyLock<UsageBoost> =
        std::sync::LazyLock::new(UsageBoost::empty);

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
        let importance_calc = ImportanceCalculator::new(&config.importance, &EMPTY_USAGE);
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
        let importance_calc = ImportanceCalculator::new(&config.importance, &EMPTY_USAGE);
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
        let importance_calc = ImportanceCalculator::new(&config.importance, &EMPTY_USAGE);
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
        let output = std::env::temp_dir().join("test_stopplace_convert_output.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("NominatimDumpFile"));
        assert!(content.contains("NSR:StopPlace:56697"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn convert_produces_group_of_stop_places() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_gosp_convert_output.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
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
        let output = std::env::temp_dir().join("test_convert_valid_json.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
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
        let output = std::env::temp_dir().join("test_convert_coords.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
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
        let output = std::env::temp_dir().join("test_convert_authority.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("fare_zone_authority.FIN.Authority.FIN_ID"));
        assert!(content.contains("fare_zone_authority.RUT.Authority.RUT_ID"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn stop_place_with_bus_submode_has_transport_mode_in_output() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_transport_mode.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("\"transport_mode\":\"bus:localBus\""));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn stop_places_have_county_gid_and_locality_gid() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");
        let output = std::env::temp_dir().join("test_convert_gid.ndjson");
        convert_all(&config, &input, &output, false, &UsageBoost::empty()).unwrap();
        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("county_gid.KVE"));
        assert!(content.contains("locality_gid.KVE"));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn usage_csv_lifts_named_stop_importance() {
        let config = test_config();
        let input = test_data_path("stopPlaces.xml");

        let baseline_out = std::env::temp_dir().join("test_usage_baseline.ndjson");
        convert_all(&config, &input, &baseline_out, false, &UsageBoost::empty()).unwrap();
        let baseline = std::fs::read_to_string(&baseline_out).unwrap();
        let _ = std::fs::remove_file(&baseline_out);

        let csv = std::env::temp_dir().join("test_usage_boost_input.csv");
        std::fs::write(&csv, "id;name;usage\nNSR:StopPlace:56697;Oslo S;5000000\n").unwrap();
        let usage = UsageBoost::load(Some(&csv), &crate::config::UsageConfig::default()).unwrap();
        let boosted_out = std::env::temp_dir().join("test_usage_boosted.ndjson");
        convert_all(&config, &input, &boosted_out, false, &usage).unwrap();
        let boosted = std::fs::read_to_string(&boosted_out).unwrap();
        let _ = std::fs::remove_file(&boosted_out);
        let _ = std::fs::remove_file(&csv);

        let pick = |s: &str| -> f64 {
            let line = s.lines()
                .find(|l| l.contains("\"place_id\"") && l.contains("NSR:StopPlace:56697"))
                .expect("stop in output");
            let key = "\"importance\":";
            let i = line.find(key).expect("importance field") + key.len();
            let rest = &line[i..];
            let end = rest.find(|c: char| c != '.' && !c.is_ascii_digit()).unwrap_or(rest.len());
            rest[..end].parse().unwrap()
        };
        assert!(pick(&boosted) > pick(&baseline),
            "boosted importance {} should exceed baseline {}", pick(&boosted), pick(&baseline));
    }
}
