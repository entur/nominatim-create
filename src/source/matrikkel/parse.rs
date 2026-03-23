use quick_xml::events::Event;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone)]
pub(crate) struct MatrikkelAdresse {
    pub lokalid: String,
    pub kommunenummer: Option<String>,
    pub kommunenavn: Option<String>,
    pub adressetilleggsnavn: Option<String>,
    pub adressenavn: Option<String>,
    pub nummer: Option<String>,
    pub bokstav: Option<String>,
    pub nord: f64,
    pub ost: f64,
    pub postnummer: Option<String>,
    pub poststed: String,
    pub grunnkretsnummer: Option<String>,
    pub grunnkretsnavn: Option<String>,
}

/// Running aggregation for a street: accumulates UTM33 coordinates across all addresses
/// on the same street so we can compute an average centroid for the street entry.
pub(crate) struct StreetAgg {
    pub representative: MatrikkelAdresse,
    pub sum_ost: f64,
    pub sum_nord: f64,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct KommuneInfo {
    pub fylkesnummer: String,
    pub fylkesnavn: String,
}

pub(crate) fn parse_csv(input: &Path) -> Result<Vec<MatrikkelAdresse>, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(input)?;
    let reader = BufReader::new(file);
    let mut addresses = Vec::new();

    for line in reader.lines().skip(1) {
        let line = line?;
        let tokens: Vec<&str> = line.split(';').collect();
        // Kartverket CSV has 46+ columns; column 3 is address type ("vegadresse" = street address).
        // Other types (e.g. "matrikkeladresse") are farm-based and excluded.
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

pub(crate) fn build_kommune_mapping(gml_path: &Path) -> Result<HashMap<String, KommuneInfo>, Box<dyn std::error::Error>> {
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
