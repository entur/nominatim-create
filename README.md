# nominatim-converter

A Rust CLI tool that converts Norwegian geographic data sources into Nominatim-compatible NDJSON. This is a port of the [Kotlin converter](https://github.com/entur/geocoder/tree/last-with-kotlin-converter/converter), producing identical output.

## Data sources

| Source | Input format | Description |
|--------|-------------|-------------|
| **stopplace** | NeTEx XML | NSR/SAM stop places and groups of stop places |
| **matrikkel** | CSV + GML | Kartverket address registry (vegadresser) with street aggregation |
| **stedsnavn** | GML | Kartverket place names (SSR) |
| **poi** | NeTEx XML | Points of interest from NeTEx |
| **osm** | PBF | OpenStreetMap entities (nodes, ways, relations) |

## Building

Requires Rust 2024 edition (1.85+). PROJ is statically linked via `bundled_proj`.

```bash
cargo build --release
```

The binary is at `target/release/nominatim-converter`.

## Usage

All subcommands require a `converter.json` configuration file (see [`converter.example.json`](converter.example.json) for the schema).

```bash
# StopPlace
nominatim-converter stopplace -i stop_places.xml -o output.ndjson -c converter.json

# Matrikkel (addresses + streets, needs stedsnavn GML for county lookup)
nominatim-converter matrikkel -i adresse.csv -o output.ndjson -c converter.json -g stedsnavn.gml

# Matrikkel (without county data)
nominatim-converter matrikkel -i adresse.csv -o output.ndjson -c converter.json --no-county

# Stedsnavn
nominatim-converter stedsnavn -i stedsnavn.gml -o output.ndjson -c converter.json

# POI
nominatim-converter poi -i poi.xml -o output.ndjson -c converter.json

# OSM
nominatim-converter osm -i planet.osm.pbf -o output.ndjson -c converter.json
```

### Common flags

| Flag | Description |
|------|-------------|
| `-i` | Input file (required) |
| `-o` | Output file (required) |
| `-c` | Config file (defaults to `converter.json` in CWD) |
| `-f` | Force overwrite existing output |
| `-a` | Append to existing output |

## Output format

NDJSON (newline-delimited JSON). First line is a header:

```json
{"type":"NominatimDumpFile","content":{"version":"0.1.0","generator":"geocoder",...}}
```

Subsequent lines are place entries:

```json
{"type":"Place","content":[{"place_id":"KVE-PostalAddress-225678815","object_type":"N","categories":[...],...}]}
```

All floating-point values are serialized with exactly 6 decimal places to match the Kotlin output.

## Architecture

```
src/
├── main.rs                  # CLI entry point (clap)
├── config.rs                # converter.json schema
├── common/
│   ├── category.rs          # Category string constants
│   ├── coordinate.rs        # Lat/lon coordinate type
│   ├── country.rs           # ISO 3166-1 alpha-2/alpha-3 mapping (full set)
│   ├── extra.rs             # Extra metadata fields
│   ├── geo.rs               # UTM33→WGS84 projection, country detection
│   ├── importance.rs        # Log-normalized importance scoring
│   ├── text.rs              # OSM tag formatting
│   ├── translator.rs        # Name/type translations
│   └── util.rs              # titleize, round6, etc.
├── source/
│   ├── stopplace/           # NeTEx StopPlace (xml, convert, popularity)
│   ├── matrikkel/           # Kartverket CSV addresses (parse, convert)
│   ├── stedsnavn/           # SSR GML place names (gml, convert)
│   ├── poi/                 # NeTEx POI (xml, convert)
│   └── osm/                 # OSM PBF 4-pass (passes, entity, admin, street, ...)
└── target/
    ├── json_writer.rs       # NDJSON output with header
    ├── nominatim_id.rs      # Structured ID → Photon place_id sanitization
    └── nominatim_place.rs   # Nominatim NDJSON schema (serde)
```

## Embedded data

- `data/boundaries60x30.ser` — Country boundary raster data, embedded in the binary via `include_bytes!`. Originally from [JOSM's boundaries.osm](https://josm.openstreetmap.de/browser/josm/trunk/resources/data/boundaries.osm), manually edited to improve border accuracy, and stored in [entur/geocoder-data](https://github.com/entur/geocoder-data). Converted to `.ser` format using the [countryboundaries](https://github.com/westnordost/countryboundaries) generator. Uses the same file as the Kotlin converter for identical country detection results.

## Compatibility with the Kotlin converter

This converter originally produced identical output to the Kotlin version. It has since diverged with the adoption of structured IDs.

Key differences from the Kotlin converter:
- **place_id**: Now a string using Transmodel-style structured IDs (e.g. `KVE-PostalAddress-225678815`, `NSR-StopPlace-59977`, `OSM-PointOfInterest-12345`), replacing the old numeric Java `hashCode`-based IDs
- **object_id**: Set to 0 for all entries (vestigial field)
- **Coordinate rounding**: Last-digit differences (0.000001° ≈ 0.1m) from different projection libraries (GeoTools/JTS in Kotlin vs proj4 in Rust)

Implementation details carried over from the Kotlin converter:
- **Country detection**: Same `boundaries60x30.ser` file and `country-boundaries` crate (by the same author as the Java library)
- **Float formatting**: 6 decimal places for importance, coordinates, and bounding boxes
- **Category ordering**: Matches Kotlin's 3-pass tariff zone construction (StopPlace), BTreeMap for sorted tag keys (OSM)
- **Alt name deduplication**: Preserves insertion order (like Java's LinkedHashSet)
- **PBF file order**: OSM entities are processed in PBF file order via ordered ID vectors, matching Kotlin's Osmosis sequential reader
- **CoordinateStore**: Open-addressing hash map with delta-encoded int coordinates at 1e5 scale (~1.1m precision), matching Kotlin's implementation

### Production verification

Verified with full Norway production datasets:

| Converter | Entries | Identical | Remaining Diffs |
|-----------|---------|-----------|-----------------|
| StopPlace (403MB XML) | 58,085 | 100% | 0 |
| Stedsnavn (2.4GB GML) | 2,215 | 87% | 288 coord precision |
| OSM (1.3GB PBF) | 37,001 | 98.9% | 346 coord + 63 street edge cases |
| Matrikkel (775MB CSV + 2.4GB GML) | 2,659,069 | 89.3% | 285,481 coord precision |

All diffs are last-digit coordinate rounding from different UTM33→WGS84 projection libraries.

## Performance

Benchmarks on Apple Silicon (M-series), release build with LTO. Compared to the [Kotlin converter](https://github.com/entur/geocoder/tree/last-with-kotlin-converter/converter) (JVM 21):

| Source | Entries | Rust | Kotlin | Speedup |
|--------|---------|------|--------|---------|
| StopPlace (403MB XML) | 58,062 | 1.1s | 6.5s | **5.9x** |
| Stedsnavn (2.4GB GML) | 2,216 | 4.4s | 8.4s | **1.9x** |
| OSM (1.3GB PBF) | 37,001 | 79s | 137s | **1.7x** |
| Matrikkel (775MB CSV + 2.4GB GML) | 2,659,069 | 14s | 25s | **1.8x** |

## Comparison tool

`compare-ndjson.py` is a reusable tool for comparing Nominatim NDJSON files:

```bash
# Basic 2-file comparison
./compare-ndjson.py kotlin.ndjson rust.ndjson

# Inspect a specific entry
./compare-ndjson.py kotlin.ndjson rust.ndjson --inspect KVE-PostalAddress-225678815

# Compare ordering patterns
./compare-ndjson.py kotlin.ndjson rust.ndjson --order

# Value distribution for differing entries
./compare-ndjson.py kotlin.ndjson rust.ndjson --histogram extra.source

# Focus on specific fields
./compare-ndjson.py kotlin.ndjson rust.ndjson --field categories --subfield extra
```

## License

Same as the upstream Kotlin converter.
