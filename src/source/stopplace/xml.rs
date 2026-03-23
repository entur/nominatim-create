use quick_xml::de::from_str;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Deserialize;
use std::collections::HashMap;

// ---- XML types ----

#[derive(Debug, Deserialize)]
pub(crate) struct StopPlaceXml {
    #[serde(rename = "@id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: Option<String>,
    #[serde(rename = "Description")]
    pub description: Option<String>,
    #[serde(rename = "Centroid")]
    pub centroid: Option<CentroidXml>,
    #[serde(rename = "TransportMode")]
    pub transport_mode: Option<String>,
    #[serde(rename = "BusSubmode")]
    pub bus_submode: Option<String>,
    #[serde(rename = "TramSubmode")]
    pub tram_submode: Option<String>,
    #[serde(rename = "RailSubmode")]
    pub rail_submode: Option<String>,
    #[serde(rename = "MetroSubmode")]
    pub metro_submode: Option<String>,
    #[serde(rename = "AirSubmode")]
    pub air_submode: Option<String>,
    #[serde(rename = "WaterSubmode")]
    pub water_submode: Option<String>,
    #[serde(rename = "TelecabinSubmode")]
    pub telecabin_submode: Option<String>,
    #[serde(rename = "StopPlaceType")]
    pub stop_place_type: Option<String>,
    #[serde(rename = "Weighting")]
    pub weighting: Option<String>,
    #[serde(rename = "TopographicPlaceRef")]
    pub topographic_place_ref: Option<RefAttr>,
    #[serde(rename = "ParentSiteRef")]
    pub parent_site_ref: Option<RefAttr>,
    #[serde(rename = "alternativeNames")]
    pub alternative_names: Option<AlternativeNamesXml>,
    #[serde(rename = "tariffZones")]
    pub tariff_zones: Option<TariffZonesXml>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CentroidXml {
    #[serde(rename = "Location")]
    pub location: LocationXml,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LocationXml {
    #[serde(rename = "Longitude")]
    pub longitude: f64,
    #[serde(rename = "Latitude")]
    pub latitude: f64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RefAttr {
    #[serde(rename = "@ref")]
    pub ref_: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AlternativeNamesXml {
    #[serde(rename = "AlternativeName", default)]
    pub names: Vec<AlternativeNameXml>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AlternativeNameXml {
    #[serde(rename = "NameType")]
    pub name_type: Option<String>,
    #[serde(rename = "Name")]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TariffZonesXml {
    #[serde(rename = "TariffZoneRef", default)]
    pub refs: Vec<TariffZoneRefXml>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TariffZoneRefXml {
    #[serde(rename = "@ref")]
    pub ref_: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GroupOfStopPlacesXml {
    #[serde(rename = "@id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: Option<String>,
    #[serde(rename = "Centroid")]
    pub centroid: Option<CentroidXml>,
    #[serde(rename = "members")]
    pub members: Option<MembersXml>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MembersXml {
    #[serde(rename = "StopPlaceRef", default)]
    pub refs: Vec<RefAttr>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TopographicPlaceXml {
    #[serde(rename = "@id")]
    pub id: Option<String>,
    #[serde(rename = "Descriptor")]
    pub descriptor: Option<DescriptorXml>,
    #[serde(rename = "TopographicPlaceType")]
    pub topographic_place_type: Option<String>,
    #[serde(rename = "CountryRef")]
    pub country_ref: Option<RefAttr>,
    #[serde(rename = "ParentTopographicPlaceRef")]
    pub parent_ref: Option<RefAttr>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DescriptorXml {
    #[serde(rename = "Name")]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FareZoneXml {
    #[serde(rename = "@id")]
    pub id: Option<String>,
    #[serde(rename = "AuthorityRef")]
    pub authority_ref: Option<RefAttr>,
}

/// All NeTEx entities parsed from the XML, grouped by type. The HashMaps are keyed
/// by entity ID for O(1) lookup during conversion (e.g. resolving topographic place
/// references to county/municipality names).
pub(crate) struct ParseResult {
    pub stop_places: Vec<StopPlaceXml>,
    pub groups: Vec<GroupOfStopPlacesXml>,
    pub topo_places: HashMap<String, TopographicPlaceXml>,
    pub fare_zones: HashMap<String, FareZoneXml>,
}

// ---- Parsing ----

pub(crate) fn parse_netex(xml: &str) -> Result<ParseResult, Box<dyn std::error::Error>> {
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

pub fn read_element_as_string(
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
                append_start_tag(&mut inner, e)?;
                depth += 1;
            }
            Ok(Event::End(ref e)) => {
                depth -= 1;
                inner.extend_from_slice(b"</");
                inner.extend_from_slice(e.name().as_ref());
                inner.push(b'>');
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Empty(ref e)) => {
                append_empty_tag(&mut inner, e)?;
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

fn append_start_tag(
    inner: &mut Vec<u8>,
    e: &quick_xml::events::BytesStart,
) -> Result<(), Box<dyn std::error::Error>> {
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
    Ok(())
}

fn append_empty_tag(
    inner: &mut Vec<u8>,
    e: &quick_xml::events::BytesStart,
) -> Result<(), Box<dyn std::error::Error>> {
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::test_helpers::test_data_path;

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
}
