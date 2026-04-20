# nominatim-converter

A Rust CLI tool that converts geographic data sources into Nominatim-compatible NDJSON.

## Data sources

| Source | Input format | Description |
|--------|-------------|-------------|
| **stopplace** | NeTEx XML | NSR/SAM stop places and groups of stop places |
| **matrikkel** | CSV + GML | Kartverket address registry (vegadresser) with street aggregation |
| **stedsnavn** | GML | Kartverket place names (SSR) |
| **poi** | NeTEx XML | Points of interest from NeTEx |
| **osm** | PBF | OpenStreetMap entities (nodes, ways, relations) |
| **belagenhet** | GeoPackage | Lantmäteriet belägenhetsadresser (Swedish cadastral addresses) |

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

# Belägenhetsadress (from local GeoPackage file)
nominatim-converter belagenhet -i belagenhetsadresser_kn0180.gpkg -o output.ndjson -c converter.json

# Belägenhetsadress (download from Lantmäteriet by municipality code)
nominatim-converter belagenhet -m 0180 -o output.ndjson -c converter.json

# Belägenhetsadress (multiple municipalities)
nominatim-converter belagenhet -m 0180 0114 1480 -o output.ndjson -c converter.json
```

The `belagenhet -m` download mode requires Lantmäteriet Geotorget credentials via environment variables or a `.env` file:

```bash
export LANTMATERIET_USER=your_username
export LANTMATERIET_PASS=your_password
```

### Common flags

| Flag | Description |
|------|-------------|
| `-i` | Input file (required) |
| `-o` | Output file (required) |
| `-c` | Config file (defaults to `converter.json` in CWD) |
| `-f` | Force overwrite existing output |
| `-a` | Append to existing output |
| `-d` | Cache directory for downloads (see below); also via `NOMINATIM_CACHE_DIR` |
| `--refresh-cache` | Ignore cache hits and re-download |

### Caching downloads

`-d <DIR>` (or `NOMINATIM_CACHE_DIR`) persists downloaded source files and
reuses them on subsequent runs. For ZIP sources, the extracted entry is
cached too. With a warm cache, `belagenhet` runs without `LANTMATERIET_*`.

```bash
nominatim-converter -d ~/.cache/nominatim-converter osm \
    -i https://example.com/norway-latest.osm.pbf -o out.ndjson -c converter.json
```

Rolling URLs like `Current_latest.zip` or `norway-latest.osm.pbf` silently
serve stale data from the cache. Pass `--refresh-cache` to force a
re-download. Or just `rm` the cache directory.

The cache directory is created with default umask permissions; use a
user-owned location, not a shared one.

## Output format

NDJSON (newline-delimited JSON). First line is a header:

```json
{"type":"NominatimDumpFile","content":{"version":"0.1.0","generator":"geocoder",...}}
```

Subsequent lines are place entries:

```json
{"type":"Place","content":[{"place_id":"KVE-PostalAddress-225678815","object_type":"N","categories":[...],...}]}
```

All floating-point values are serialized with exactly 6 decimal places.

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
│   ├── geo.rs               # Coordinate projection (UTM33, SWEREF99 TM → WGS84), country detection
│   ├── importance.rs        # Log-normalized importance scoring
│   ├── text.rs              # OSM tag formatting
│   ├── translator.rs        # Name/type translations
│   └── util.rs              # titleize, round6, etc.
├── source/
│   ├── stopplace/           # NeTEx StopPlace (xml, convert, popularity)
│   ├── matrikkel/           # Kartverket CSV addresses (parse, convert)
│   ├── stedsnavn/           # SSR GML place names (gml, convert)
│   ├── poi/                 # NeTEx POI (xml, convert)
│   ├── belagenhet/          # Lantmäteriet GeoPackage addresses (parse, convert, download)
│   └── osm/                 # OSM PBF 4-pass (passes, entity, admin, street, ...)
└── target/
    ├── json_writer.rs       # NDJSON output with header
    ├── nominatim_id.rs      # Structured ID → Photon place_id sanitization
    └── nominatim_place.rs   # Nominatim NDJSON schema (serde)
```

## Embedded data

- `data/boundaries60x30.ser` — Country boundary raster data, embedded in the binary via `include_bytes!`. Originally from [JOSM's boundaries.osm](https://josm.openstreetmap.de/browser/josm/trunk/resources/data/boundaries.osm), converted to `.ser` format using the [countryboundaries](https://github.com/westnordost/countryboundaries) generator.

## License

EUPL-1.2
