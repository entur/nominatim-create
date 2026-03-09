use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_nominatim-converter"))
}

fn test_data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-data")
        .join(name)
}

fn config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("converter.example.json")
}

fn temp_output(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "nominatim-integration-{}-{name}.ndjson",
        std::process::id()
    ))
}

fn run_converter(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(binary())
        .args(args)
        .output()
        .expect("failed to execute binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status.success(), stdout, stderr)
}

fn read_ndjson(path: &Path) -> Vec<serde_json::Value> {
    let content = std::fs::read_to_string(path).expect("failed to read output");
    content
        .lines()
        .map(|line| serde_json::from_str(line).expect("invalid JSON line"))
        .collect()
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
}

// ===== CLI behavior =====

#[test]
fn no_args_shows_help() {
    let (success, _, stderr) = run_converter(&[]);
    assert!(!success);
    assert!(
        stderr.contains("Usage") || stderr.contains("usage"),
        "expected usage info in stderr: {stderr}"
    );
}

#[test]
fn missing_input_file_fails() {
    let output = temp_output("missing-input");
    let (success, _, stderr) = run_converter(&[
        "stopplace",
        "-i",
        "/nonexistent/file.xml",
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(!success);
    assert!(
        stderr.contains("Error") || stderr.contains("error"),
        "expected error in stderr: {stderr}"
    );
    cleanup(&output);
}

#[test]
fn output_file_exists_without_force_fails() {
    let output = temp_output("exists-no-force");
    std::fs::write(&output, "existing content").unwrap();

    let (success, _, stderr) = run_converter(&[
        "stopplace",
        "-i",
        test_data("stopPlaces.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
    ]);
    assert!(!success);
    assert!(stderr.contains("already exists"));
    cleanup(&output);
}

// ===== StopPlace conversion =====

#[test]
fn stopplace_produces_valid_ndjson() {
    let output = temp_output("stopplace-valid");
    let (success, _, stderr) = run_converter(&[
        "stopplace",
        "-i",
        test_data("stopPlaces.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(success, "stopplace failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() >= 2, "expected header + at least 1 entry, got {}", lines.len());

    // First line is the header
    assert_eq!(lines[0]["type"], "NominatimDumpFile");

    // All data lines are Place type
    for line in &lines[1..] {
        assert_eq!(line["type"], "Place");
    }

    cleanup(&output);
}

#[test]
fn stopplace_entries_have_required_fields() {
    let output = temp_output("stopplace-fields");
    let (success, _, stderr) = run_converter(&[
        "stopplace",
        "-i",
        test_data("stopPlaces.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(success, "stopplace failed: {stderr}");

    let lines = read_ndjson(&output);
    for entry in &lines[1..] {
        let content = &entry["content"][0];
        assert!(content["place_id"].is_i64(), "missing place_id");
        assert!(content["object_type"].is_string(), "missing object_type");
        assert!(content["categories"].is_array(), "missing categories");
        assert!(content["rank_address"].is_i64(), "missing rank_address");
        assert!(content["importance"].is_f64(), "missing importance");
        assert!(content["centroid"].is_array(), "missing centroid");
        assert_eq!(content["centroid"].as_array().unwrap().len(), 2, "centroid should have 2 elements");

        let name = &content["name"];
        if !name.is_null() {
            assert!(name["name"].is_string(), "name.name should be a string");
        }

        let extra = &content["extra"];
        assert!(extra["source"].is_string(), "missing extra.source");
        assert!(extra["id"].is_string(), "missing extra.id");
    }

    cleanup(&output);
}

#[test]
fn stopplace_has_groups_and_stops() {
    let output = temp_output("stopplace-groups");
    let (success, _, _) = run_converter(&[
        "stopplace",
        "-i",
        test_data("stopPlaces.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(success);

    let lines = read_ndjson(&output);
    let data: Vec<&serde_json::Value> = lines[1..].iter().collect();

    let has_stop = data.iter().any(|e| {
        e["content"][0]["extra"]["source"]
            .as_str()
            .is_some_and(|s| s == "nsr")
    });
    assert!(has_stop, "expected at least one StopPlace entry");
    assert!(data.len() >= 2, "expected multiple stop place entries");

    cleanup(&output);
}

// ===== POI conversion =====

#[test]
fn poi_produces_valid_ndjson() {
    let output = temp_output("poi-valid");
    let (success, _, stderr) = run_converter(&[
        "poi",
        "-i",
        test_data("poi-test.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(success, "poi failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() >= 2, "expected header + entries");
    assert_eq!(lines[0]["type"], "NominatimDumpFile");

    for entry in &lines[1..] {
        let content = &entry["content"][0];
        assert_eq!(
            content["extra"]["source"].as_str(),
            Some("custom-poi"),
            "poi entries should have source=custom-poi"
        );
    }

    cleanup(&output);
}

#[test]
fn poi_filters_expired_entries() {
    let output = temp_output("poi-expired");
    let (success, _, _) = run_converter(&[
        "poi",
        "-i",
        test_data("poi-test.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(success);

    let content = std::fs::read_to_string(&output).unwrap();
    // expired entry (id 2) and future entry (id 3) should be filtered
    assert!(
        !content.contains("TEST:TopographicPlace:2"),
        "expired entry should be filtered"
    );
    assert!(
        !content.contains("TEST:TopographicPlace:3"),
        "future entry should be filtered"
    );
    // valid entries should be present
    assert!(content.contains("TEST:TopographicPlace:1"));
    assert!(content.contains("TEST:TopographicPlace:4"));

    cleanup(&output);
}

// ===== Stedsnavn conversion =====

#[test]
fn stedsnavn_produces_valid_ndjson() {
    let output = temp_output("stedsnavn-valid");
    let (success, _, stderr) = run_converter(&[
        "stedsnavn",
        "-i",
        test_data("bydel.gml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(success, "stedsnavn failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() >= 2, "expected header + entries");
    assert_eq!(lines[0]["type"], "NominatimDumpFile");

    for entry in &lines[1..] {
        let content = &entry["content"][0];
        assert_eq!(content["extra"]["source"].as_str(), Some("kartverket-stedsnavn"));
        assert_eq!(content["centroid"].as_array().unwrap().len(), 2);
    }

    cleanup(&output);
}

#[test]
fn stedsnavn_preserves_norwegian_diacritics() {
    let output = temp_output("stedsnavn-diacritics");
    let (success, _, _) = run_converter(&[
        "stedsnavn",
        "-i",
        test_data("bydel.gml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(success);

    let content = std::fs::read_to_string(&output).unwrap();
    assert!(
        content.contains("Grünerløkka"),
        "should preserve diacritics in place names"
    );

    cleanup(&output);
}

// ===== Matrikkel conversion =====

#[test]
fn matrikkel_produces_valid_ndjson() {
    let output = temp_output("matrikkel-valid");
    let (success, _, stderr) = run_converter(&[
        "matrikkel",
        "-i",
        test_data("Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-g",
        test_data("Basisdata_3420_Elverum_25833_Stedsnavn_GML.gml").to_str().unwrap(),
        "-f",
    ]);
    assert!(success, "matrikkel failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() > 100, "expected many entries for Elverum, got {}", lines.len());
    assert_eq!(lines[0]["type"], "NominatimDumpFile");

    cleanup(&output);
}

#[test]
fn matrikkel_has_addresses_and_streets() {
    let output = temp_output("matrikkel-types");
    let (success, _, _) = run_converter(&[
        "matrikkel",
        "-i",
        test_data("Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-g",
        test_data("Basisdata_3420_Elverum_25833_Stedsnavn_GML.gml").to_str().unwrap(),
        "-f",
    ]);
    assert!(success);

    let lines = read_ndjson(&output);
    let data: Vec<&serde_json::Value> = lines[1..].iter().collect();

    let has_address = data.iter().any(|e| {
        e["content"][0]["extra"]["source"]
            .as_str()
            .is_some_and(|s| s == "kartverket-matrikkelenadresse")
    });
    assert!(has_address, "expected matrikkel address entries");
    assert!(data.len() >= 100, "expected many address entries");

    cleanup(&output);
}

#[test]
fn matrikkel_no_county_flag_works() {
    let output = temp_output("matrikkel-no-county");
    let (success, _, stderr) = run_converter(&[
        "matrikkel",
        "-i",
        test_data("Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "--no-county",
        "-f",
    ]);
    assert!(success, "matrikkel --no-county failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() > 1, "expected output even without county data");

    cleanup(&output);
}

#[test]
fn matrikkel_without_gml_or_flag_fails() {
    let output = temp_output("matrikkel-no-gml");
    let (success, _, stderr) = run_converter(&[
        "matrikkel",
        "-i",
        test_data("Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(!success);
    assert!(
        stderr.contains("requires -g") || stderr.contains("no-county"),
        "expected helpful error about missing GML: {stderr}"
    );
    cleanup(&output);
}

// ===== Append mode =====

#[test]
fn append_mode_does_not_duplicate_header() {
    let output = temp_output("append-header");
    // First write
    let (s1, _, _) = run_converter(&[
        "poi",
        "-i",
        test_data("poi-test.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(s1);

    // Append
    let (s2, _, _) = run_converter(&[
        "poi",
        "-i",
        test_data("poi-test.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-a",
    ]);
    assert!(s2);

    let content = std::fs::read_to_string(&output).unwrap();
    let header_count = content
        .lines()
        .filter(|l| l.contains("NominatimDumpFile"))
        .count();
    assert_eq!(header_count, 1, "header should appear exactly once after append");

    cleanup(&output);
}

// ===== Force overwrite =====

#[test]
fn force_flag_overwrites_existing_output() {
    let output = temp_output("force-overwrite");
    std::fs::write(&output, "garbage content").unwrap();

    let (success, _, _) = run_converter(&[
        "poi",
        "-i",
        test_data("poi-test.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(success);

    let content = std::fs::read_to_string(&output).unwrap();
    assert!(!content.contains("garbage"));
    assert!(content.contains("NominatimDumpFile"));

    cleanup(&output);
}

// ===== Output coordinates are valid =====

#[test]
fn all_centroids_are_valid_coordinates() {
    let output = temp_output("centroids-valid");
    let (success, _, _) = run_converter(&[
        "stopplace",
        "-i",
        test_data("stopPlaces.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(success);

    let lines = read_ndjson(&output);
    for entry in &lines[1..] {
        let centroid = entry["content"][0]["centroid"].as_array().unwrap();
        let lon = centroid[0].as_f64().unwrap();
        let lat = centroid[1].as_f64().unwrap();
        assert!(
            (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat),
            "invalid centroid: [{lon}, {lat}]"
        );
    }

    cleanup(&output);
}
