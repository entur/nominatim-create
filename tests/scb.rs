//! Integration tests that verify the Swedish municipality and county lists
//! against the SCB (Statistics Sweden) API.
//!
//! Gated behind the `external-tests` feature so they only run on demand:
//!   cargo test --features external-tests

#![cfg(feature = "external-tests")]

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Parse the MUNICIPALITIES constant from the source file.
///
/// Returns (code, name) pairs extracted from the Rust source, so the test
/// stays in sync without needing to import from the binary crate.
fn read_municipalities_from_source() -> Vec<(String, String)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/source/belagenhet/municipalities.rs");
    let source = std::fs::read_to_string(&path).expect("failed to read municipalities.rs");

    let mut entries = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        // Match lines like: ("0114", "Upplands Väsby"),
        if let Some(rest) = trimmed.strip_prefix("(\"") {
            if let Some((code, after)) = rest.split_once("\", \"") {
                if let Some(name) = after.strip_suffix("\"),") {
                    entries.push((code.to_string(), name.to_string()));
                }
            }
        }
    }
    entries
}

/// Fetch the SCB metadata for a statistics table and extract the "Region"
/// variable's codes and texts.
fn fetch_scb_region(url: &str) -> Vec<(String, String)> {
    let body = ureq::get(url)
        .call()
        .unwrap()
        .into_body()
        .read_to_string()
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&body).unwrap();

    let variables = response["variables"].as_array().expect("missing variables array");
    let region_var = variables
        .iter()
        .find(|v| v["code"].as_str() == Some("Region"))
        .expect("no Region variable in SCB response");

    let codes: Vec<String> = region_var["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let texts: Vec<String> = region_var["valueTexts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    assert_eq!(codes.len(), texts.len(), "SCB codes/texts length mismatch");
    codes.into_iter().zip(texts).collect()
}

/// Verify that our municipality list matches the official SCB register.
///
/// Queries a robust SCB table (municipal greenhouse gas emissions) whose
/// Region variable lists all 290 municipalities by 4-digit code and name.
#[test]
fn municipalities_match_scb() {
    let our_municipalities = read_municipalities_from_source();
    assert_eq!(
        our_municipalities.len(),
        290,
        "expected 290 municipalities in source, got {}",
        our_municipalities.len()
    );

    let scb_regions = fetch_scb_region(
        "https://api.scb.se/OV0104/v1/doris/en/ssd/MI/MI1301/MI1301B/UtslappKommun",
    );

    // SCB may include aggregates like "0000" — keep only 4-digit codes
    // that don't start with "00".
    let scb_municipalities: Vec<(&str, &str)> = scb_regions
        .iter()
        .filter(|(code, _)| code.len() == 4 && !code.starts_with("00"))
        .map(|(c, t)| (c.as_str(), t.as_str()))
        .collect();

    let our_set: BTreeSet<(&str, &str)> = our_municipalities
        .iter()
        .map(|(c, t)| (c.as_str(), t.as_str()))
        .collect();
    let scb_set: BTreeSet<(&str, &str)> = scb_municipalities.iter().copied().collect();

    let missing: Vec<_> = scb_set.difference(&our_set).collect();
    let extra: Vec<_> = our_set.difference(&scb_set).collect();

    assert!(
        missing.is_empty() && extra.is_empty(),
        "Municipality list is out of date!\n  \
         Missing (in SCB but not in code): {missing:?}\n  \
         Extra (in code but not in SCB): {extra:?}"
    );

    assert_eq!(
        scb_municipalities.len(),
        290,
        "expected 290 municipalities from SCB, got {}",
        scb_municipalities.len()
    );
}

/// Verify that Swedish county (län) codes 01–25 are present in SCB.
///
/// Queries a table whose Region variable includes all 21 counties by
/// 2-digit code, filtering out "00" (national aggregate). Also checks that
/// every municipality's 2-digit prefix maps to a valid county.
#[test]
fn county_codes_present_in_scb() {
    let our_municipalities = read_municipalities_from_source();

    let scb_regions = fetch_scb_region(
        "https://api.scb.se/OV0104/v1/doris/en/ssd/BO/BO0601/BO0601D/AntTaxvAreal",
    );

    let counties: Vec<(&str, &str)> = scb_regions
        .iter()
        .filter(|(code, _)| code.len() == 2 && code.as_str() != "00")
        .map(|(c, t)| (c.as_str(), t.as_str()))
        .collect();

    assert_eq!(
        counties.len(),
        21,
        "expected 21 counties from SCB, got {}: {counties:?}",
        counties.len()
    );

    // Every municipality's first 2 digits should map to a valid county.
    let county_codes: BTreeSet<&str> = counties.iter().map(|(c, _)| *c).collect();
    for (code, name) in &our_municipalities {
        assert!(
            county_codes.contains(&code[..2]),
            "municipality {code} ({name}) has county prefix {} not in SCB county list",
            &code[..2]
        );
    }
}
