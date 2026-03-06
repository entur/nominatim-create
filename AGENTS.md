# AGENTS.md

Instructions for AI coding agents working on this codebase.

## Project overview

This is a Rust CLI that converts Norwegian geographic data into Nominatim NDJSON. It is a port of a Kotlin converter and must produce **identical output**. Any behavioral change should be validated against the Kotlin version.

## Build and test

```bash
cargo build --release    # requires PROJ C library (brew install proj on macOS)
cargo test --release     # run all tests
cargo clippy --release   # should produce zero warnings
```

The release build uses LTO (`[profile.release] lto = true`).

## Key design decisions

### Output must match the Kotlin converter exactly

This is the most important constraint. Specifically:

- **place_id values** use Java's `String.hashCode()` algorithm (`src/target/nominatim_id.rs`). Do not replace with Rust's `DefaultHasher` — the hash values must match Java/Kotlin.
- **Floating-point formatting** uses exactly 6 decimal places for `importance`, `centroid`, and `bbox` fields (`src/target/nominatim_place.rs`). This is enforced via custom serde serializers using `serde_json::value::RawValue`.
- **Country detection** uses the exact same `boundaries60x30.ser` file as the Kotlin project, embedded via `include_bytes!` (`src/common/geo.rs`). Do not switch to the Rust crate's built-in data — it produces different results for border cases.
- **Country code mapping** covers all ISO 3166-1 countries (`src/common/country.rs`), matching Java's `Locale.getISOCountries()`. Do not reduce to a subset.

### Coordinate conversions have inherent precision differences

UTM33 (EPSG:25833) → WGS84 conversions use the Rust `proj` crate, which produces results that differ from Java GeoTools at the 6th decimal place (~0.1m). This is accepted as unavoidable — the difference is sub-meter.

### OSM converter specifics

The OSM converter (`src/source/osm/mod.rs`) has several critical patterns for Kotlin compatibility:

- **PBF file order**: Entities must be processed in PBF file order, not HashMap iteration order. The pass 4 data structs use `ids: Vec<i64>` to preserve insertion order alongside `HashMap` lookups. Do not iterate over the HashMaps directly.
- **BTreeMap for filtered tags**: `filter_tags()` returns `BTreeMap<&str, &str>` (sorted by key) to match Kotlin's `LinkedHashMap` ordering (which happens to be alphabetical for OSM tags). Using `HashMap` causes non-deterministic category ordering.
- **Alt names from filtered tags**: `alt_name`, `old_name`, etc. are looked up from the filtered tags (BTreeMap), not all_tags (HashMap). This matches Kotlin's `filterTags()` behavior.
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
- `src/source/` — One module per data source (stopplace, matrikkel, stedsnavn, poi, osm)
- `src/target/` — Output format (NDJSON schema, ID generation, JSON writer)
- `src/config.rs` — `converter.json` deserialization
- `data/` — Embedded binary data (country boundaries)

## Testing against Kotlin output

Use the comparison tool for validation:

```bash
# Run both converters
java -jar converter-all.jar stopplace -i input.xml -o /tmp/kotlin.ndjson -f
./target/release/nominatim-convert stopplace -i input.xml -o /tmp/rust.ndjson -f -c converter.json

# Compare with the reusable tool
python3 compare-ndjson.py /tmp/kotlin.ndjson /tmp/rust.ndjson

# Inspect a specific entry
python3 compare-ndjson.py /tmp/kotlin.ndjson /tmp/rust.ndjson --inspect 400123

# Compare ordering patterns
python3 compare-ndjson.py /tmp/kotlin.ndjson /tmp/rust.ndjson --order
```

For matrikkel/stedsnavn/osm, coordinate diffs at the 6th decimal are expected (different projection libraries).

## Common pitfalls

- **XML tag names are case-sensitive**: `alternativeNames` not `AlternativeNames`, `parentSiteRef` has an `@ref` attribute (use `RefAttr` struct).
- **quick-xml `read_text` doesn't work with `Reader<BufReader<File>>`**: Use manual text collection with `Event::Text` instead.
- **Serde rename for XML attributes**: Use `#[serde(rename = "@ref")]` for XML attributes parsed by quick-xml.
- **Alt name deduplication must preserve order**: Use a `HashSet` seen-tracker with `Vec` output, not `BTreeSet` or `sort + dedup`.
- **Tariff zone categories have specific ordering**: Built in 3 separate passes (IDs, authorities, fare zone authorities) matching Kotlin.
- **HashMap iteration is non-deterministic**: Never rely on HashMap iteration order for output. Use Vec for ordered processing, BTreeMap for sorted keys.
- **Street matching has edge cases**: The 100m threshold + 0.001° cache precision means ~0.1% of street lookups differ from Kotlin due to coordinate quantization.
