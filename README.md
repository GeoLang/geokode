# Geokode

[![CI](https://github.com/GeoLang/geokode/actions/workflows/ci.yml/badge.svg)](https://github.com/GeoLang/geokode/actions)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)

A fast, accurate, self-hosted geocoding service written in Rust.

Forward geocoding, reverse geocoding, autocomplete, and batch processing with FST text indexes and R-tree spatial indexes.

## Features

- **Forward Geocode** — text query → coordinates (fuzzy matching, abbreviation expansion)
- **Reverse Geocode** — coordinates → nearest address (R-tree kNN)
- **Autocomplete** — prefix search for interactive UIs, with optional `lon`/`lat` bias that reranks only the first `limit * 8` matches, so a distant match outside that window is not pulled in
- **Batch API** — geocode several addresses in one request, processed sequentially with no cap on request size or per-query results, so a batch of queries that miss the index can block for minutes
- **Address Parsing** — splits on commas by part count: a one- or two-part address gets no house number, a four-part address puts the postcode in `country`, `postcode` is only filled from five parts up, and a trailing `"DC 20500"` stays whole in the state field
- **Multiple Data Sources** — OpenAddresses CSV, GeoJSON, OpenStreetMap
- **OSM Ingest** — Import OpenStreetMap PBF, extracting nodes and ways tagged with `addr:housenumber` and `addr:street`
- **Enhanced address parsing** — Expanded abbreviation dictionary, unit/suite handling, and directionals stripped from both index and query (`123 N Main St` and `123 S Main St` both index as `123 main st`, so either query returns both)
- **REST API** — JSON endpoints via Axum, with permissive CORS (any origin) applied outside the auth middleware
- **Self-Hosted** — geokode itself calls no external APIs and your data stays local (the ViewTopia and GeoLang integrations below fall back to public Nominatim)

## Known issue: house-number lookup with OpenAddresses CSV

CSV ingest joins `NUMBER`, `STREET`, `CITY` and `REGION` with `", "`, so a row of `123,Main St,Springfield,IL` is indexed under the key `123, Main St, Springfield, IL`. Normalization never strips commas and search is prefix-based, so a normally written query like `123 Main St, Springfield` matches nothing. Only the comma-after-the-number form works:

```bash
geokode forward -d addresses.csv "123, Main St, Springfield"
```

CSV is the only format the Dockerfile and `docker-compose.yml` load, so this affects the default deployment. Street-level queries without a house number are unaffected.

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
| `geokode-ingest` | Data source parsers (OpenAddresses, GeoJSON, OSM) |
| `geokode-server` | Axum REST API with forward/reverse/autocomplete/batch endpoints |
| `geokode-cli` | CLI for serving, forward/reverse geocoding |

## Quick Start

```bash
# Build
cargo build --all

# Forward geocode (see the known issue above for the comma after the house number)
geokode forward -d addresses.csv "123, Main St, Springfield"

# Reverse geocode — negative values need `--lon=`, clap rejects `--lon -89.65`
geokode reverse -d addresses.csv --lon=-89.65 --lat 39.78

# Start REST API server
geokode serve -d addresses.csv --bind 0.0.0.0:3000
```

### REST API

```bash
# Forward geocode
curl "http://localhost:3000/forward?q=123,+Main+St"

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

Operational endpoints: `/healthz` (liveness), `/readyz` (readiness), `/metrics` (Prometheus).

The OpenAPI spec is [docs/openapi.yml](docs/openapi.yml). A second, divergent copy at `docs/openapi.yaml` was removed.

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

- **ViewTopia** — powers the fly-to search box in the viewer
- **GeoLang agent tools** — backs the `geocode_place` tool

## License

AGPL-3.0-or-later, see [LICENSE](LICENSE).

Copyright (C) 2026 Grok Image Compression Inc.
