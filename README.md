# Geokode

[![CI](https://github.com/GeoLang/geokode/actions/workflows/ci.yml/badge.svg)](https://github.com/GeoLang/geokode/actions)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)

A self-hosted geocoding service written in Rust.

It does forward geocoding, reverse geocoding, autocomplete and batch forward geocoding over an FST text index and an R-tree spatial index, built in memory from an address file at startup.

## Features

- **Forward geocoding**: text query to coordinates. Street suffixes are abbreviated on both sides (`Main Street` and `Main St` match), and a query with no index hit falls back to a fuzzy scan of every indexed key (edit distance up to 2, at most 5 results).
- **Match type**: every result carries `match_type`, `exact` when the query is a whole indexed key, `prefix` when it only starts one, `fuzzy` for the fallback.
- **Places**: OSM `place=*` nodes (city, town, village, hamlet, suburb, neighbourhood) and `boundary=administrative` relations are indexed as settlements with `kind: "place"`. Without a house number in the query, a place ranks above a street that starts with the same name, and a city ranks above a village. Places never come back from reverse geocoding.
- **Partial queries**: when `Jasper, Alberta` matches nothing, the search retries with `Jasper`, drops results whose state or country contradicts `Alberta`, and caps confidence at 0.6.
- **Reverse geocoding**: coordinates to the nearest addresses (R-tree kNN). A point outside the data coverage returns no results. Coverage is the PBF header bbox when there is one, otherwise the address extent padded by the widest gap between neighbouring addresses.
- **Autocomplete**: prefix search with an optional `lon`/`lat` bias. The bias reranks only the first `limit * 8` matches, so a nearer match outside that window is not pulled in.
- **Batch**: `POST /batch` runs `/forward` for each query in turn. There is no cap on request size or per-query results, and each query that misses the index runs the full fuzzy scan, so a large batch of misses is slow.
- **Address parsing**: splits on commas by part count. A one- or two-part address gets no house number, a four-part address puts the fourth part in `country`, `postcode` is only filled from five parts up, and a trailing `"DC 20500"` stays whole in the state field.
- **Directionals and units**: `N`, `North` and the other directionals, and units such as `Apt 4`, are stripped from both index and query. `123 N Main St` and `123 S Main St` both index as `123 main st`, so either query returns both, with the one that matches the query's directional first.
- **Data sources**: OpenAddresses CSV, GeoJSON and OpenStreetMap PBF.
- **REST API**: JSON endpoints on Axum, with permissive CORS (any origin) applied outside the auth middleware.
- **Self-hosted**: geokode calls no external APIs. The ViewTopia and GeoLang integrations below fall back to public Nominatim on their own side.

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
| `geokode-core` | FST text index, R-tree spatial index, fuzzy matching, address parsing, geocoding |
| `geokode-ingest` | Parsers for OpenAddresses CSV, GeoJSON, OSM PBF and Overpass exports |
| `geokode-server` | Axum REST API, JWT middleware, Prometheus metrics |
| `geokode-cli` | The `geokode` binary: `serve`, `forward`, `reverse` |

## Quick Start

```bash
cargo install --path crates/geokode-cli

geokode forward -d addresses.csv "123 Main St, Springfield"

# a negative value needs `--lon=`, clap rejects `--lon -89.65`
geokode reverse -d addresses.csv --lon=-89.65 --lat 39.78

geokode serve -d addresses.csv --bind 0.0.0.0:3000
```

`--bind` defaults to `0.0.0.0:3000`. Every command builds the index from `-d` on startup.

### Docker

Tagged releases publish `ghcr.io/geolang/geokode` and prebuilt binaries for Linux and macOS on x86_64 and aarch64. The image runs `geokode serve --data /data/addresses.csv`, so mount a directory holding that file:

```bash
docker run -p 3000:3000 -v "$PWD/data:/data:ro" ghcr.io/geolang/geokode:latest
```

`docker compose up -d` builds the image locally, mounts `./data`, and starts Prometheus on port 9090. No sample data ships, and the container exits without `./data/addresses.csv`.

### REST API

```bash
curl "http://localhost:3000/forward?q=123,+Main+St"
curl "http://localhost:3000/reverse?lon=-89.65&lat=39.78&limit=5"
curl "http://localhost:3000/autocomplete?q=main&limit=10&lon=-89.65&lat=39.78"
curl -X POST http://localhost:3000/batch \
  -H "Content-Type: application/json" \
  -d '{"queries": ["123 Main St", "456 Oak Ave"]}'
curl http://localhost:3000/health
```

`/health` returns the record count. `/healthz` is liveness, `/readyz` returns 503 while the index is empty, and `/metrics` serves Prometheus request counters. `limit` defaults to 5 on `/reverse` and `/autocomplete`.

The OpenAPI spec is [docs/openapi.yml](docs/openapi.yml).

### Authentication

Set `GEOKODE_JWT_SECRET` to require an `Authorization: Bearer <token>` JWT on the geocoding endpoints. The token is HS256 and must carry `sub`, `exp` and `role` claims. With the variable unset every request is allowed. `/health`, `/healthz`, `/readyz` and `/metrics` stay public either way. `docker-compose.yml` sets the secret to `change-me-in-production`, so change it before exposing the service.

A JWT is the only credential the server checks. There are no API keys or rate limits.

## Data Sources

The CLI picks the parser from the file extension: `.geojson` and `.json` as GeoJSON, `.pbf` as OSM, anything else as OpenAddresses CSV.

### OpenAddresses CSV

Columns `LON`, `LAT`, `NUMBER`, `STREET`, `CITY`, `REGION`, `POSTCODE`. Lowercase names and `longitude`/`latitude`, `x`/`y`, `house_number`, `state` and `zip` are also accepted. Only the two coordinate columns are required.

```csv
LON,LAT,NUMBER,STREET,CITY,REGION,POSTCODE
-89.65,39.78,123,Main St,Springfield,IL,62701
```

### GeoJSON

A FeatureCollection of Point features with an `address` or `name` property. The string is split with the address parser above.

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

### OpenStreetMap PBF

```bash
geokode serve -d region.osm.pbf --bind 0.0.0.0:3000
```

Addresses are nodes and ways tagged with both `addr:housenumber` and `addr:street`. A way is placed at the centroid of its member nodes. Places are `place=*` nodes with a `name`, plus named `boundary=administrative` relations placed at the centroid of their outer ways. A relation is skipped when a place node already has its name, or when fewer than half its outer ways are inside the extract. The header bbox, when present, sets the reverse geocoding coverage.

`geokode-ingest` also parses Overpass API JSON (addresses and places) and tab-separated Overpass CSV through `ingest_osm_overpass` and `ingest_osm_csv`. The CLI does not call either.

## GeoLang integration

- **ViewTopia** uses `/forward` for the fly-to search box.
- **GeoLang agent** uses `/forward` in the `geocode_place` tool when `GEOKODE_URL` is set.

## License

AGPL-3.0-or-later, see [LICENSE](LICENSE).

Copyright (C) 2026 Grok Image Compression Inc.
