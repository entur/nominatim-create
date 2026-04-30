# AGENTS.md

Instructions for AI coding agents working on this codebase.

## Project overview

This is a Rust CLI that converts geographic data into Nominatim NDJSON. It is a rewrite of an earlier converter and must produce **identical output**. Any behavioral change should be validated against the original version.

## Build and test

```bash
cargo build --release    # PROJ is statically linked via bundled_proj
cargo test --release     # run all tests
cargo clippy --release   # should produce zero warnings
```

The release build uses LTO (`[profile.release] lto = true`).

## Key design decisions

### Output must match the original converter exactly (unless `--usage` is in play)

This is the most important constraint when running without `--usage`. Specifically:

- **place_id values** use the `String.hashCode()` algorithm from the original converter (`src/target/nominatim_id.rs`). Do not replace with Rust's `DefaultHasher` — the hash values must match the original output.
- **Floating-point formatting** uses exactly 6 decimal places for `importance`, `centroid`, and `bbox` fields (`src/target/nominatim_place.rs`). This is enforced via custom serde serializers using `serde_json::value::RawValue`.
- **Country detection** uses `boundaries60x30.ser`, embedded via `include_bytes!` (`src/common/geo.rs`). This file originates from [JOSM's boundaries.osm](https://josm.openstreetmap.de/browser/josm/trunk/resources/data/boundaries.osm), manually edited for border accuracy and stored in [entur/geocoder-data](https://github.com/entur/geocoder-data), then converted to `.ser` using the [countryboundaries](https://github.com/westnordost/countryboundaries) generator. Do not switch to the Rust crate's built-in data — it produces different results for border cases.
- **Country code mapping** covers all ISO 3166-1 countries (`src/common/country.rs`). Do not reduce to a subset.

### Optional usage-driven popularity boosts (`--usage`)

The global `--usage <FILE>` CLI flag points at a semicolon-separated CSV (`id;name;usage`, or just `id;usage` - the middle column is purely human-friendly and ignored) that nudges popular entities upward in the ranking (`src/common/usage.rs`). The boost is `1 + alpha * log10(usage / usageFloor)` (defaults: alpha=0.5, floor=100), applied as a multiplicative factor on each source's raw popularity *before* `ImportanceCalculator` runs. Missing IDs and IDs at or below the floor receive factor 1.0.

The CSV is shared across every subcommand. Each source converter looks up by its own ID format (`NSR:StopPlace:N`, `KVE:PostalAddress:N`, `OSM:PointOfInterest:N`, etc.) so a single file can carry signals for multiple sources.

The canonical CSV is generated from PostHog by the `posthog-popular-stops` job in [`geocoder/.github/workflows/cache-data-sources.yml`](../geocoder/.github/workflows/cache-data-sources.yml) and uploaded to:

- `gs://ent-geocoder-prd/data-sources/popular-stops-fra.csv` (boardings - use this for general search ranking)
- `gs://ent-geocoder-prd/data-sources/popular-stops-til.csv` (alightings)

`--usage` only accepts local paths, not GCS URIs, so download first:

```bash
gcloud storage cp gs://ent-geocoder-prd/data-sources/popular-stops-fra.csv .
nominatim-converter --usage popular-stops-fra.csv stopplace -i stops.xml -o stops.ndjson -c converter.json
```

When `--usage` is set, output **deliberately diverges** from the original Java converter for any boosted entity. Do not use `compare-ndjson.py` against the Java baseline as a regression check in that mode - importance values will differ. Without `--usage`, output remains bit-identical and the comparison still applies.

GoSP popularity grows multiplicatively from member popularities, so member-level boosts compound. The `gosBoostFactor` in `converter.example.json` is now `3.0` (down from `10.0`) to leave headroom for usage-driven differentiation; retune downward further if real-world output shows GoSPs over-dominating.

### Coordinate conversions have inherent precision differences

UTM33 (EPSG:25833) → WGS84 conversions use the `proj` crate, which produces results that differ from the original converter at the 6th decimal place (~0.1m). This is accepted as unavoidable — the difference is sub-meter.

### OSM converter specifics

The OSM converter (`src/source/osm/`) has several critical patterns for output compatibility:

- **PBF file order**: Entities must be processed in PBF file order, not HashMap iteration order. The pass 4 data structs use `ids: Vec<i64>` to preserve insertion order alongside `HashMap` lookups. Do not iterate over the HashMaps directly.
- **BTreeMap for filtered tags**: `filter_tags()` returns `BTreeMap<&str, &str>` (sorted by key) to match the original converter's alphabetical ordering. Using `HashMap` causes non-deterministic category ordering.
- **Alt names from filtered tags**: `alt_name`, `old_name`, etc. are looked up from the filtered tags (BTreeMap), not all_tags (HashMap). This matches the original converter's `filterTags()` behavior.
- **RefCell for StreetIndex cache**: `lookup_cache` uses `RefCell<HashMap>` for interior mutability so `find_nearest_street` can take `&self` instead of `&mut self`.
- **CoordinateStore at 1e5 scale**: The custom hash-based coordinate store uses 1e5 precision (~1.1m). Do not increase — it causes more diffs, not fewer, because it affects all coordinates including polygon centroid averaging.
- **4-pass PBF processing**: Relations → Ways → Nodes → Convert. This is critical for collecting the dependency graph (relations need way IDs, ways need node IDs).

### Performance-sensitive code

- `geo::convert_utm33_to_lat_lon` caches the `Proj` instance in `thread_local!` storage. Creating a `Proj` per call is ~1000x slower. The `Proj` type is not `Send+Sync`, so `LazyLock` cannot be used.
- Matrikkel's `build_kommune_mapping` streams the GML via `BufReader` — do not use `read_to_string` on the 2.6GB file.
- Matrikkel parses the CSV once and reuses the vec for both address and street passes.
- OSM's StreetIndex uses grid-based spatial indexing (0.005° cells) with expanding ring search, plus a 0.001° lookup cache for repeated queries at similar coordinates.

## Project structure

- `src/common/` — Shared types and utilities (coordinates, countries, categories, text formatting)
- `src/source/` — One module per data source, each a thin facade (`name.rs`) with submodules (`name/`)
  - `stopplace/` — NeTEx StopPlace (xml, convert, popularity)
  - `matrikkel/` — Kartverket CSV addresses (parse, convert)
  - `stedsnavn/` — SSR GML place names (gml, convert)
  - `poi/` — NeTEx POI (xml, convert)
  - `osm/` — OSM PBF 4-pass (passes, pass4, entity, admin, street, popularity, coordinate, geometry, indexing)
- `src/source.rs` — Module declarations + shared test helpers (`test_config`, `test_data_path`)
- `src/target/` — Output format (NDJSON schema, ID generation, JSON writer)
- `src/config.rs` — `converter.json` deserialization
- `data/` — Embedded binary data (country boundaries)

## Testing against the original converter output

Use the comparison tool for validation:

```bash
# Run both converters
java -jar converter-all.jar stopplace -i input.xml -o /tmp/original.ndjson -f
./target/release/nominatim-converter stopplace -i input.xml -o /tmp/rust.ndjson -f -c converter.json

# Compare with the reusable tool
python3 compare-ndjson.py /tmp/original.ndjson /tmp/rust.ndjson

# Inspect a specific entry
python3 compare-ndjson.py /tmp/original.ndjson /tmp/rust.ndjson --inspect 400123

# Compare ordering patterns
python3 compare-ndjson.py /tmp/original.ndjson /tmp/rust.ndjson --order
```

For matrikkel/stedsnavn/osm, coordinate diffs at the 6th decimal are expected (different projection libraries).

## Downstream pipeline context

This converter produces `nominatim.ndjson` which is imported into the **Photon geocoder**, proxied by `../geocoder/proxy`, and validated by `../geocoder-acceptance-tests/`. Understanding what the acceptance tests actually check helps prioritize what matters most in the converter output.

### Fields that acceptance tests validate

- **name / alt_name** — Fuzzy search, popular name matching (e.g. "gardermoen" → "Oslo lufthavn"). Norwegian diacritics (ø, å, æ, ü) must be preserved.
- **categories** — Layer/category filtering (`onstreetBus`, `railStation`, `airport`, `busStation`). Multi-modal stops must include all transport modes.
- **housenumber** — Address searches like "karl johans gate 2" depend on correct housenumber extraction.
- **source (extra field)** — Acceptance tests filter by data source (`openaddresses`, `openstreetmap`). Source tags must match expected values.
- **importance** — Directly affects result ranking. Acceptance tests use `priorityThresh` to verify top-N placement.
- **county_gid / locality_gid (extra fields)** — Used for `boundary.county_ids` filtering. Must support both full (`KVE:TopographicPlace:18`) and numeric (`18`) formats.
- **tariff_zones (extra field)** — Used for tariff-based filtering downstream.
- **centroid coordinates** — Reverse geocoding, focus-point disambiguation, and distance calculations all depend on coordinate accuracy.

### Acceptance test patterns worth knowing

- **Geographic disambiguation**: Same place name in multiple locations (e.g. "Haugen") — focus points select the closest. Correct coordinates are critical.
- **Data source priority**: NSR takes priority over WhosOnFirst for stop places. GroupOfStopPlaces rank above individual StopPlaces for major cities.
- **Popular vs official names**: "Gardermoen" (popular) should find "Oslo lufthavn" (official). Alt name deduplication and ordering matter.
- **House number edge cases**: Numbers can appear before street name ("10 schw"), with suffixes ("10B"), or after ("strandkaien 22").
- **Multi-modal categories**: Stavanger stasjon = railStation + onstreetBus. Oslo lufthavn = railStation + onstreetBus + busStation + airport. Category arrays must be complete.
- **Reverse geocoding should NOT return bare house numbers** — layer filtering depends on correct `object_type` and category assignment.

### Test coverage

All source converters have unit tests (`cargo test --release` runs ~240 tests). Coverage by module:

1. **stopplace** (38 tests): NeTEx parsing, popularity calculation (base × type factors × interchange), GroupOfStopPlaces boost (gosBoostFactor × product of member popularities), transport mode formatting (mode:submode, parent collecting children with dedup), alt name handling (label → visible, translation → indexed only), category generation (funicular included, bus excluded, multimodal.parent marker), tariff zone ordering, full conversion integration tests (coordinates, authority categories, county_gid/locality_gid)
2. **stedsnavn** (22 tests): Target type recognition (by/bydel/tettsted/tettsteddel/tettbebyggelse), spelling status filtering (vedtatt/godkjent/privat/samlevedtak accepted), GML parsing with historisk alt spelling, diacritics preservation, field validation (source, accuracy, country_code, importance, rank_address), locality/county GID format, coordinate ranges, titleized names
3. **matrikkel** (12 tests): CSV→NDJSON conversion, field validation (id, source, accuracy, country_a, locality, borough, housenumber with letter suffix), county population via stedsnavn GML, address + street entry generation, category correctness, coordinate validity, importance range, county GID in categories
4. **poi** (7 tests): ValidBetween date filtering (valid/expired/future/always-valid/open-ended), coordinate and category correctness
5. **integration** (17 tests, `tests/integration.rs`): Black-box binary tests via `std::process::Command`. CLI behavior (no args, missing input, output-exists-without-force), all subcommands produce valid NDJSON with correct headers/sources/fields, append mode doesn't duplicate headers, force flag overwrites, coordinate validity, matrikkel --no-county flag, matrikkel missing GML error, expired POI filtering, Norwegian diacritics
6. **osm** (47 tests): Popularity formula (base × max priority, highest priority wins, unmatched/empty → zero), filter_tags (keeps only configured filters, sorted BTreeMap keys, empty for no matches), rank_address determination (boundary > place > road > building > poi priority), convert_node integration (object_type, accuracy, source, categories from filtered tags, alt name extraction from filtered tags only, en:name, OSM ID in extra and indexed alt_names, coordinates, importance reflects priority), admin boundary integration (county_gid, locality_gid, titleized municipality name, county_gid in categories), extract_country_code (ISO3166-2, country_code tag, numeric ref → Norway), as_category colon replacement, plus low-level tests (CoordinateStore, BoundingBox, ray casting, street segment distance, centroid calculation, titleize)

### Test data fixtures

- `test-data/stopPlaces.xml` — NeTEx with TopographicPlaces (counties/municipalities for topo lookups), 2 GroupOfStopPlaces, 6 StopPlaces (bus, rail, parent, child, alt names, submodes), 3 FareZones
- `test-data/poi-test.xml` — 5 TopographicPlaces with varying validity periods
- `test-data/bydel.gml` — 2 Oslo bydeler (Grünerløkka, Frogner) in UTM33
- `test-data/Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv` — Real Elverum address data (10,871 lines)
- `test-data/Basisdata_3420_Elverum_25833_Stedsnavn_GML.gml` — Kommune-fylke mapping for matrikkel tests

### Test patterns

- Shared test helpers live in `src/source.rs` (`test_config()`, `test_config_with_osm_filters()`, `test_data_path()`) — all modules import from there
- Stopplace-specific helpers (`make_stop_place`, etc.) live in `src/source/stopplace.rs` `tests::helpers`
- Temp output files use unique suffixes per test to avoid parallel test conflicts
- Integration tests (`tests/integration.rs`) run the binary as a black box via `std::process::Command`, testing all subcommands end-to-end
- Module-level integration tests call the module's `convert()` function end-to-end, then parse the NDJSON output

## Common pitfalls

- **XML tag names are case-sensitive**: `alternativeNames` not `AlternativeNames`, `parentSiteRef` has an `@ref` attribute (use `RefAttr` struct).
- **quick-xml `read_text` doesn't work with `Reader<BufReader<File>>`**: Use manual text collection with `Event::Text` instead.
- **Serde rename for XML attributes**: Use `#[serde(rename = "@ref")]` for XML attributes parsed by quick-xml.
- **Alt name deduplication must preserve order**: Use a `HashSet` seen-tracker with `Vec` output, not `BTreeSet` or `sort + dedup`.
- **Tariff zone categories have specific ordering**: Built in 3 separate passes (IDs, authorities, fare zone authorities) matching the original converter.
- **HashMap iteration is non-deterministic**: Never rely on HashMap iteration order for output. Use Vec for ordered processing, BTreeMap for sorted keys.
- **Street matching has edge cases**: The 100m threshold + 0.001° cache precision means ~0.1% of street lookups differ from the original converter due to coordinate quantization.
