# Geokode

A fast, accurate, self-hosted geocoding service written in Rust.

Forward geocoding, reverse geocoding, autocomplete, and batch processing with FST text indexes and R-tree spatial indexes.

## Features

- **Forward Geocode** — text query → coordinates (fuzzy matching, abbreviation expansion)
- **Reverse Geocode** — coordinates → nearest address (R-tree kNN)
- **Autocomplete** — prefix search with spatial bias for interactive UIs
- **Batch API** — process thousands of addresses in one request
- **Address Parsing** — structured decomposition (house number, street, city, state, zip)
- **Multiple Data Sources** — OpenAddresses CSV, GeoJSON, custom formats
- **OSM Ingest** — Import OpenStreetMap PBF/XML with highway, place, and addr:* tag extraction
- **Enhanced address parsing** — Expanded abbreviation dictionary, directional prefixes/suffixes, unit/suite handling
- **REST API** — JSON endpoints via Axum, CORS-enabled
- **Self-Hosted** — no external API dependencies, your data stays local

## Architecture

```
┌────────────────┐     ┌────────────────┐     ┌────────────────┐
│ geokode-ingest │────▶│  geokode-core  │────▶│ geokode-server │
│  (data import) │     │ (index/search) │     │  (REST API)    │
└────────────────┘     └────────────────┘     └────────────────┘
                              │
                              ▼
                       ┌────────────────┐
                       │  geokode-cli   │
                       │  (CLI tool)    │
                       └────────────────┘
```

### Crates

| Crate | Description |
|-------|-------------|
| `geokode-core` | FST text index, R-tree spatial index, address parsing, geocoding logic |
| `geokode-ingest` | Data source parsers (OpenAddresses, GeoJSON) |
| `geokode-server` | Axum REST API with forward/reverse/autocomplete/batch endpoints |
| `geokode-cli` | CLI for serving, forward/reverse geocoding |

## Quick Start

```bash
# Build
cargo build --all

# Forward geocode
geokode forward -d addresses.csv "123 Main St, Springfield"

# Reverse geocode
geokode reverse -d addresses.csv --lon -89.65 --lat 39.78

# Start REST API server
geokode serve -d addresses.csv --bind 0.0.0.0:3000
```

### REST API

```bash
# Forward geocode
curl "http://localhost:3000/forward?q=123+Main+St"

# Reverse geocode
curl "http://localhost:3000/reverse?lon=-89.65&lat=39.78&limit=5"

# Autocomplete
curl "http://localhost:3000/autocomplete?q=main&limit=10"

# Batch
curl -X POST http://localhost:3000/batch \
  -H "Content-Type: application/json" \
  -d '{"queries": ["123 Main St", "456 Oak Ave"]}'

# Health check
curl http://localhost:3000/health
```

## Data Sources

### OpenAddresses CSV

Standard OpenAddresses format with columns: `LON`, `LAT`, `NUMBER`, `STREET`, `CITY`, `REGION`, `POSTCODE`.

```csv
LON,LAT,NUMBER,STREET,CITY,REGION,POSTCODE
-89.65,39.78,123,Main St,Springfield,IL,62701
```

### GeoJSON

Point features with an `address` or `name` property.

```json
{
  "type": "FeatureCollection",
  "features": [{
    "type": "Feature",
    "geometry": { "type": "Point", "coordinates": [-74.0, 40.7] },
    "properties": { "address": "123 Broadway, New York, NY" }
  }]
}
```

## Integration with GeoLang Ecosystem

- **ETL Pipeline** — geocode address columns during data transformation
- **Stream Processor** — enrich GPS points with address context in real-time
- **Ptolemy** — store geocoded results in versioned geodata

## License

GNU Affero General Public License v3.0 or later. See [LICENSE](LICENSE) for details.
