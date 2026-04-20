use quick_xml::events::Event;
use quick_xml::Reader;

/// SSR place types to include: cities (by), city districts (bydel), urban settlements
/// (tettsted/tettsteddel), and dense built-up areas (tettbebyggelse).
pub(crate) const TARGET_TYPES: &[&str] = &["by", "bydel", "tettsted", "tettsteddel", "tettbebyggelse"];
/// Only include place names with an approved spelling status. "historisk" and other
/// unapproved statuses are excluded.
pub(crate) const ACCEPTED_STATUS: &[&str] = &["vedtatt", "godkjent", "privat", "samlevedtak"];

/// A parsed SSR (Sentralt Stedsnavnregister) place name entry.
pub(crate) struct StedsnavnEntry {
    pub lokal_id: String,
    pub stedsnavn: String,
    pub navneobjekttype: String,
    pub kommunenummer: String,
    pub kommunenavn: String,
    pub fylkesnummer: String,
    pub fylkesnavn: String,
    /// Coordinates in UTM33 (EPSG:25833): (easting, northing). Multiple points are
    /// averaged to a single centroid during conversion.
    pub coordinates: Vec<(f64, f64)>,
    /// Alternative spellings from `annenSkrivemåte` elements in the GML.
    pub annen_skrivemaate: Vec<String>,
}

#[cfg(test)]
pub(crate) fn parse_gml(xml: &str) -> Result<Vec<StedsnavnEntry>, Box<dyn std::error::Error>> {
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

pub(crate) fn parse_feature_member<R: std::io::BufRead>(reader: &mut Reader<R>) -> Result<Option<StedsnavnEntry>, Box<dyn std::error::Error>> {
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
    let mut current_field: Option<&'static str> = None;
    let mut text_buf = Vec::new();

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref()).unwrap_or("");
                match name {
                    "lokalId" | "app:lokalId" => { current_field = Some("lokalId"); text_buf.clear(); }
                    "navnerom" | "app:navnerom" => { current_field = Some("navnerom"); text_buf.clear(); }
                    "komplettskrivemåte" | "app:komplettskrivemåte" => { current_field = Some("komplettskrivemåte"); text_buf.clear(); }
                    "navneobjekttype" | "app:navneobjekttype" => { current_field = Some("navneobjekttype"); text_buf.clear(); }
                    "skrivemåtestatus" | "app:skrivemåtestatus" => { current_field = Some("skrivemåtestatus"); text_buf.clear(); }
                    "kommunenummer" | "app:kommunenummer" => { current_field = Some("kommunenummer"); text_buf.clear(); }
                    "kommunenavn" | "app:kommunenavn" => { current_field = Some("kommunenavn"); text_buf.clear(); }
                    "fylkesnummer" | "app:fylkesnummer" => { current_field = Some("fylkesnummer"); text_buf.clear(); }
                    "fylkesnavn" | "app:fylkesnavn" => { current_field = Some("fylkesnavn"); text_buf.clear(); }
                    "annenSkrivemåte" | "app:annenSkrivemåte" => inside_annen = true,
                    "posList" | "gml:posList" => { current_field = Some("posList"); text_buf.clear(); }
                    "pos" | "gml:pos" => { current_field = Some("pos"); text_buf.clear(); }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e))
                if current_field.is_some() => {
                    text_buf.extend_from_slice(e.as_ref());
                }
            Ok(Event::End(ref e)) => {
                if let Some(field) = current_field {
                    let text = String::from_utf8_lossy(&text_buf).trim().to_string();
                    match field {
                        "lokalId" => lokal_id = Some(text),
                        "navnerom" => navnerom = Some(text),
                        "komplettskrivemåte" => {
                            if inside_annen {
                                annen_skrivemaate.push(text);
                            } else if stedsnavn.is_none() {
                                stedsnavn = Some(text);
                            }
                        }
                        "navneobjekttype" => navneobjekttype = Some(text),
                        "skrivemåtestatus"
                            if !inside_annen && skrivemaatestatus.is_none() => {
                                skrivemaatestatus = Some(text);
                            }
                        "kommunenummer" => kommunenummer = Some(text),
                        "kommunenavn" => kommunenavn = Some(text),
                        "fylkesnummer" => fylkesnummer = Some(text),
                        "fylkesnavn" => fylkesnavn = Some(text),
                        "posList" => parse_pos_list(&text, &mut coordinates),
                        "pos" => parse_pos(&text, &mut coordinates),
                        _ => {}
                    }
                    current_field = None;
                }

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
