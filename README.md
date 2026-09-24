# Geokode

[![CI](https://github.com/GeoLang/geokode/actions/workflows/ci.yml/badge.svg)](https://github.com/GeoLang/geokode/actions)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)

A self-hosted geocoder written in Rust.

`geokode build` reads an OpenStreetMap PBF, up to the whole planet, plus optional address files, and writes an index directory. `geokode serve` memory-maps that directory and answers forward, autocomplete, reverse and batch queries over HTTP. geokode calls no external APIs.

## What gets indexed

Named objects only. The first matching key in this table classifies an object, and objects without a `name` are skipped.

| Key | Values | Objects | `kind` |
|-----|--------|---------|--------|
| `boundary` | `administrative`, with an `admin_level` | relations | `boundary` |
| `place` | any | all | `place` |
| `amenity`, `tourism`, `historic`, `leisure`, `natural`, `waterway`, `shop`, `office`, `man_made` | any | all | `poi` |
| `aeroway` | any except runway, taxiway, taxilane, apron, stopway, holding and parking positions | all | `poi` |
| `railway` | `station`, `halt`, `tram_stop` | all | `poi` |
| `public_transport` | `station` | all | `poi` |
| `highway` | street values from `motorway` to `steps` | ways | `street` |

Relations count when their `type` is `multipolygon`, `boundary` or `waterway`. The search keys are `name`, `name:en`, `int_name`, `alt_name` (split on `;`), `official_name` and `short_name`. `display_name` leads with the object's `name:en` when tagged, else `name`. The `name` field is always the tagged `name`, so Tokyo has `name` 東京都 and a `display_name` starting with Tokyo.

The ways of one street merge into one record per name and most local containing boundary. The record takes the id, point and `highway` value of the longest way and the bbox of all of them.

A boundary's `label` node, and its `admin_centre` node at `admin_level` 7 and above, merge into the boundary when they carry the same name. The boundary inherits their `place` class and population, so `Monaco` answers with the country and the commune relations instead of duplicate nodes.

An address object from a PBF input with the same OSM type and id as a named record is not indexed on its own. The named record takes its house number, street and postcode where its own are empty, and becomes searchable by house number. House numbers come only from `--addresses` inputs: an OSM PBF (objects with `addr:housenumber` and `addr:street`), an OpenAddresses CSV or a GeoJSON FeatureCollection of points.

## Containment

`boundary=administrative` relations from level 2 to 8 are stitched into rings. Rings are tested under the even-odd rule, so outer and inner roles need not be right. A boundary whose rings never close, usually one cut by an extract edge, gives no containment and no record.

- `country` and `country_code` come from level 2 (`ISO3166-1:alpha2` or `ISO3166-1`, lowercased).
- `state` comes from level 4.
- `city` is the most local of levels 8, 7 and 6. With none of those, it is the nearest `place=city` within 10 km, `town` within 5 km or `village` within 2 km.
- A boundary, or a country, state or county place, gets no context more local than its own level.
- These parts use the area's `name:en` when tagged, else `name`, so the Matterhorn reads `Matterhorn, Zermatt, Valais/Wallis, Switzerland`. A record named like its own area keeps its own spelling there, so Zürich reads `Zürich, Switzerland`. The record's own `name` stays as tagged.
- CSV and GeoJSON addresses keep their own city and state, containment fills only what is missing.

Every result's point lies on or inside the object: the node itself, the middle vertex of a line, or the middle of the widest inside span of an area.

## Ranking

One scoring function in `crates/geokode-core/src/rank.rs` adds these terms:

- Match: an exact key beats a prefix, which beats a typo.
- Class tier: country, then city, state, town, famous objects, village, county, municipality or island, suburb, hamlet, neighbourhood, then street, POI and other places, then addresses. Settlement tiers sit at least 10 apart.
- An object with 10 or more `name:xx` language tags counts as famous and ranks between towns and villages, so Central Park beats the village of Central Park but never Paris.
- Population, as log10 and capped at 6.9, and a boost of 3 for a `wikidata` or `wikipedia` tag. Records that are not settlements also get 0.1 per language tag, capped at 4, so the Eiffel Tower beats its replica in Texas. None of these cross a settlement tier.
- A query starting with a digit puts addresses first. A directional in the query (`Queen St W`) ranks records with the same directional first.
- With `lat`/`lon`, a bias of up to 35 that halves at 20 km. It lifts a nearby town over a far city, never a POI over a settlement.
- A query led by `mount`, `mt`, `mountain`, `lake` or `river` with no exact hit is also searched without that word, keeping only peaks, volcanoes, massifs, ranges and hills, water, or rivers, and those rank first. `Mount Kilimanjaro` finds the massif tagged `name=Kilimanjaro`.
- Parts after a comma filter by containing area, matched against any of the area's names, English or local. `Springfield, Illinois` keeps records inside an area named Illinois, preferring more local areas, so `Bahnhofstrasse 1, Zürich` puts the city before the canton. A record that knows its state or country and is not inside the named area is dropped. A record that knows neither is kept at confidence 0.6 or less. Without a comma, when the whole text matches nothing, up to three trailing words are tried as the area.

Names fold accents (`Zurich` finds `Zürich`), hyphens and apostrophes, abbreviate street suffixes (`Main Street` and `Main St` match), and drop directionals. Unit designators such as `Apt 4` are dropped after a house number. A half-typed suffix (`Avenu`) also searches its abbreviation. With no exact hit, `/forward` also runs a typo search over the name index, one edit up to 5 characters and two above.

## Quick Start

```bash
cargo install --path crates/geokode-cli

geokode build --pbf switzerland-latest.osm.pbf --out ch-index
geokode build --pbf planet.osm.pbf --addresses ch-addresses.osm.pbf --addresses openaddresses-us.csv --out planet-index

geokode serve --index ch-index --bind 0.0.0.0:3000

geokode forward --index ch-index "Zurich"
# a negative value needs `--lon=`, clap rejects `--lon -89.65`
geokode reverse --index ch-index --lon=8.54 --lat 47.37
```

`--bind` defaults to `0.0.0.0:3000`. The index directory holds `meta.json` with a format version, and `serve` refuses an index built by a geokode with another version. `build` writes `meta.json` last, so an interrupted build never loads. It needs scratch space in `<out>/build.tmp` for the sorted node ids and coordinates it looks way geometry up in.

### Docker

Tagged releases publish `ghcr.io/geolang/geokode` and prebuilt binaries for Linux and macOS on x86_64 and aarch64. The image runs `geokode serve --index /data/index`, so build an index into a mounted directory first:

```bash
docker run -v "$PWD/data:/data" ghcr.io/geolang/geokode:latest build --pbf /data/region.osm.pbf --out /data/index
docker run -p 3000:3000 -v "$PWD/data:/data:ro" ghcr.io/geolang/geokode:latest
```

With `docker compose`, put the PBF at `./data/region.osm.pbf`, run `docker compose --profile index run --rm geokode-index` once, then `docker compose up -d`. Compose also starts Prometheus on port 9090. The Helm chart serves `indexPath` (default `/data/index`) from its volume.

## REST API

```bash
curl "http://localhost:3000/forward?q=Paris,+France&limit=5"
curl "http://localhost:3000/forward?q=Bahnhofstrasse&lat=47.37&lon=8.54"
curl "http://localhost:3000/autocomplete?q=zur&limit=10"
curl "http://localhost:3000/reverse?lat=47.37&lon=8.54&limit=5"
curl -X POST http://localhost:3000/batch \
  -H "Content-Type: application/json" \
  -d '{"queries": ["Zurich", "Bern"], "limit": 1}'
curl http://localhost:3000/health
```

- `/forward` and `/autocomplete`: `q` of 1 to 256 characters after trimming, `limit` 1 to 50 (default 5), and optionally both `lat` and `lon` as a bias. `/autocomplete` skips the typo search.
- `/reverse`: `lat` and `lon` required, `limit` 1 to 50 (default 5). Inside the coverage of an address input (its PBF header bbox, or its extent padded by the widest gap between neighbouring addresses) it answers with the nearest addresses. Elsewhere it answers with the nearest settlements within 25 km, or nothing.
- `/batch`: 1 to 100 queries of 1 to 256 characters, `limit` 1 to 5 (default 1), body at most 64 KiB. Results come back in query order.
- Any broken cap answers 400 with `{"error": "<sentence>"}`.

Each result has every one of these fields, null where unknown: `name`, `display_name`, `address` (`house_number`, `street`, `city`, `state`, `postcode`, `country`, `full`), `country_code`, `lat`, `lon`, `bbox` (`[min_lon, min_lat, max_lon, max_lat]`, null for nodes), `kind` (`address`, `place`, `street`, `poi`, `boundary`), `osm_type`, `osm_id`, `osm_key`, `osm_value`, `admin_level`, `population`, `confidence` and `match_type` (`exact`, `prefix`, `fuzzy`). Callers fetch outlines from Overpass by `osm_type` and `osm_id`.

`/health` returns the record count. `/healthz` is liveness, `/readyz` returns 503 while the index is empty, and `/metrics` serves Prometheus request counters. The OpenAPI spec is [docs/openapi.yml](docs/openapi.yml).

### Authentication

Set `GEOKODE_JWT_SECRET` to require an `Authorization: Bearer <token>` JWT on the geocoding endpoints. The token is HS256 and must carry `sub`, `exp` and `role` claims. With the variable unset every request is allowed. `/health`, `/healthz`, `/readyz` and `/metrics` stay public either way. `docker-compose.yml` sets the secret to `change-me-in-production`, so change it before exposing the service.

## Address inputs

The file extension picks the parser: `.pbf` as OSM, `.geojson` and `.json` as GeoJSON, anything else as OpenAddresses CSV.

The CSV needs `LON` and `LAT` columns and may have `NUMBER`, `STREET`, `CITY`, `REGION` and `POSTCODE`. Lowercase names and `longitude`/`latitude`, `x`/`y`, `house_number`, `state` and `zip` are also accepted.

```csv
LON,LAT,NUMBER,STREET,CITY,REGION,POSTCODE
-89.65,39.78,123,Main St,Springfield,IL,62701
```

GeoJSON features are points with an `address` or `name` property, which is split on commas: two parts are street and city, three add a house number and state, four a country, five a postcode.

## Architecture

```
┌────────────────┐     ┌────────────────┐     ┌────────────────┐
│ geokode-ingest │────▶│  geokode-core  │────▶│ geokode-server │
│  (PBF passes)  │     │ (index/search) │     │  (REST API)    │
└────────────────┘     └────────────────┘     └────────────────┘
                              │
                              ▼
                       ┌────────────────┐
                       │  geokode-cli   │
                       │ build, serve   │
                       └────────────────┘
```

| Crate | Description |
|-------|-------------|
| `geokode-core` | Index format and writer, FST name index, kd-tree reverse index, ranking, external sort |
| `geokode-ingest` | PBF passes, classification, ring assembly, containment, address inputs |
| `geokode-server` | Axum REST API with request caps, JWT middleware, Prometheus metrics |
| `geokode-cli` | The `geokode` binary: `build`, `serve`, `forward`, `reverse` |

`build` streams the PBF three times. The first pass keeps classified nodes and ways in scratch files and relations in memory. The second reads the member ways of those relations. Both push the node ids they need into an external sort. The third pass fills a coordinate file aligned with the sorted ids. Memory follows the number of named objects and admin boundary vertices, not the planet's node count.

The index is a fixed-width record file, a details file with a shared label table, an FST from normalized name to a posting list, and two kd-trees (addresses, settlements) stored as sorted point arrays. `serve` maps all of them and loads only the area and label tables.

## License

AGPL-3.0-or-later, see [LICENSE](LICENSE).

The Monaco test extract in `crates/geokode-ingest/tests/data` is © OpenStreetMap contributors under the ODbL.

Copyright (C) 2026 Grok Image Compression Inc.
