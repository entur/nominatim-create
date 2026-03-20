/// Norwegian county codes and names as used in Geonorge download URLs.
/// Format: (code, name, geonorge_slug) where slug is "{code}_{name}" as it appears in URLs.
pub const COUNTIES: &[(&str, &str, &str)] = &[
    ("03", "Oslo", "03_Oslo"),
    ("11", "Rogaland", "11_Rogaland"),
    ("15", "Møre og Romsdal", "15_More-og-Romsdal"),
    ("18", "Nordland", "18_Nordland"),
    ("21", "Svalbard", "21_Svalbard"),
    ("31", "Østfold", "31_Ostfold"),
    ("32", "Akershus", "32_Akershus"),
    ("33", "Buskerud", "33_Buskerud"),
    ("34", "Innlandet", "34_Innlandet"),
    ("38", "Vestfold", "38_Vestfold"),
    ("39", "Telemark", "39_Telemark"),
    ("40", "Agder", "40_Agder"),
    ("42", "Vestland", "42_Vestland"),
    ("50", "Trøndelag", "50_Trondelag"),
    ("55", "Troms", "55_Troms"),
    ("56", "Finnmark", "56_Finnmark"),
];

/// Resolve a region argument to a Geonorge URL slug.
/// Accepts: county code ("03"), county name ("Oslo"), or "0000"/"all" for all of Norway.
pub fn resolve_geonorge_region(arg: &str) -> Result<String, String> {
    let lower = arg.to_lowercase();

    if lower == "all" || arg == "0000" {
        return Ok("0000_Norge".to_string());
    }

    // Try exact code match
    if let Some((_, _, slug)) = COUNTIES.iter().find(|(code, _, _)| *code == arg) {
        return Ok(slug.to_string());
    }

    // Try case-insensitive name match
    if let Some((_, _, slug)) = COUNTIES.iter().find(|(_, name, _)| name.to_lowercase() == lower) {
        return Ok(slug.to_string());
    }

    Err(format!("Unknown region '{arg}'. Use a county code (e.g. 03), name (e.g. Oslo), or 'all' for all of Norway."))
}

pub fn list_regions() {
    eprintln!("Available regions for Geonorge download:");
    eprintln!("  all / 0000  All of Norway (large download)");
    for (code, name, _) in COUNTIES {
        eprintln!("  {code:<10}{name}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_all() {
        assert_eq!(resolve_geonorge_region("all").unwrap(), "0000_Norge");
        assert_eq!(resolve_geonorge_region("0000").unwrap(), "0000_Norge");
    }

    #[test]
    fn resolve_by_code() {
        assert_eq!(resolve_geonorge_region("03").unwrap(), "03_Oslo");
        assert_eq!(resolve_geonorge_region("50").unwrap(), "50_Trondelag");
    }

    #[test]
    fn resolve_by_name() {
        assert_eq!(resolve_geonorge_region("Oslo").unwrap(), "03_Oslo");
        assert_eq!(resolve_geonorge_region("oslo").unwrap(), "03_Oslo");
        assert_eq!(resolve_geonorge_region("Trøndelag").unwrap(), "50_Trondelag");
    }

    #[test]
    fn resolve_unknown() {
        assert!(resolve_geonorge_region("99").is_err());
        assert!(resolve_geonorge_region("Narnia").is_err());
    }

    #[test]
    fn has_all_counties() {
        assert_eq!(COUNTIES.len(), 16);
    }
}
