use quick_xml::de::from_str;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Deserialize;

/// NeTEx TopographicPlace with an optional `ValidBetween` period used to filter
/// out expired or not-yet-active POI entries.
#[derive(Debug, Deserialize)]
pub(crate) struct TopographicPlaceXml {
    #[serde(rename = "@id")]
    pub id: Option<String>,
    #[serde(rename = "ValidBetween")]
    pub valid_between: Option<ValidBetweenXml>,
    #[serde(rename = "Descriptor")]
    pub descriptor: Option<DescriptorXml>,
    #[serde(rename = "Centroid")]
    pub centroid: Option<CentroidXml>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ValidBetweenXml {
    #[serde(rename = "FromDate")]
    pub from_date: Option<String>,
    #[serde(rename = "ToDate")]
    pub to_date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DescriptorXml {
    #[serde(rename = "Name")]
    pub name: Option<String>,
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

pub(crate) fn parse_topographic_places(xml: &str) -> Result<Vec<TopographicPlaceXml>, Box<dyn std::error::Error>> {
    let mut places = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e))
                if e.name().as_ref() == b"TopographicPlace" => {
                    let text = crate::source::stopplace::read_element_as_string(&mut reader, "TopographicPlace", e)?;
                    if let Ok(tp) = from_str::<TopographicPlaceXml>(&text) {
                        places.push(tp);
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
