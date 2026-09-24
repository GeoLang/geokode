# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- 2026-09-24: named OSM objects from any PBF up to the planet. The build keeps
  nodes, ways and relations that have a `name` and a key from one classification
  table (place, admin boundaries, amenity, tourism, historic, leisure, natural,
  waterway, aeroway, stations, shop, office, man_made, named highways), indexed
  under `name`, `name:en`, `int_name`, `alt_name`, `official_name` and
  `short_name`. Street ways merge into one record per name and municipality.
- 2026-09-24: admin containment. Boundary relations from level 2 to 8 become
  polygons, and every record gets country, `country_code`, state and city from
  the boundaries around its point, falling back to the nearest town for the city.
- 2026-09-24: results carry `name`, `display_name`, `country_code`, `bbox`,
  `osm_type`, `osm_id`, `osm_key`, `osm_value`, `admin_level` and `population`,
  and `kind` gains `street`, `poi` and `boundary`. Every field is always present.
- 2026-09-24: `geokode build --pbf <file> [--addresses <file>]... --out <dir>`
  writes an index directory with a format version, and `geokode serve --index`
  maps it. The PBF is read in three streaming passes with node coordinates in a
  sorted scratch file, so build memory follows the named objects rather than the
  node count. The Swiss extract builds in 7 s and serves from 7 MB of RSS.
- 2026-09-24: request caps. `q` of 1 to 256 characters, `limit` 1 to 50,
  `lat` and `lon` together, batches of 1 to 100 queries with `limit` 1 to 5 in
  at most 64 KiB. A broken cap answers 400 with a JSON error.
- 2026-09-24: `lat`/`lon` bias on `/forward`, a `limit` on `/batch`, and area
  filtering by the text after a comma, or by up to three trailing words when the
  whole text matches nothing.

### Changed
- 2026-09-24: the city, state and country parts of `display_name` and `address`
  use the containing area's `name:en` when tagged, so Swiss records read
  `Switzerland` instead of `Schweiz/Suisse/Svizzera/Svizra`. The record's own
  name stays as tagged, and a qualifier matches the English or the local name.
- 2026-09-24: ranking is one scoring function. An exact name beats a prefix, a
  city beats a same-named street or POI, settlements rank by class then
  population, `wikidata` or `wikipedia` adds a boost, and a bias point lifts
  nearer same-named results.
- 2026-09-24: `/reverse` answers outside address coverage with the nearest
  settlements within 25 km instead of nothing.
- 2026-09-24: matching folds accents, hyphens and apostrophes, drops unit
  designators only after a house number, and completes a half-typed street
  suffix. The typo search runs on the FST instead of scanning every key, and
  joins the prefix hits whenever no key matches exactly.
- 2026-09-24: address records are no longer searchable by street or city alone,
  the street and place records answer those queries.

### Removed
- 2026-09-24: `serve --data`, which built the index in memory at startup, and the
  `-d` flag on `forward` and `reverse`. The Dockerfile, compose file and Helm
  chart serve `/data/index`. The Overpass JSON and CSV parsers, the linear fuzzy
  searcher, Soundex, and the unused `normalize_street`, `normalize_address`,
  `extract_unit` and `detect_format` helpers went with them, as did the
  `osmpbfreader`, `protobuf` and `flate2` dependencies. `osmpbf`, `rayon`,
  `memmap2` and `unicode-normalization` replace them.

### Added
- 2026-09-22: places, so a town is findable by name. The PBF ingest reads
  `place=city/town/village/hamlet/suburb/neighbourhood` nodes and
  `boundary=administrative` relations, a relation taking the centroid of its
  outer ways and skipping a name a place node already covers; the Overpass JSON
  ingest reads the same `place` tag. `GeoResult.kind` says `place` or `address`.
  A query with no house number ranks places above addresses, largest settlement
  first, so "Jasper" no longer answers with the Jasper Avenue in the loaded
  extract. A query whose first comma-separated part matches on its own falls
  back to that part at confidence 0.6, since OSM rarely tags a town with the
  province a caller names it by, and a dropped part that the record contradicts
  discards the hit.

### Fixed
- 2026-09-22: reverse geocoding answers with an address again. Places joined the
  R-tree when they were added, so a Monaco extract answered the middle of its
  bounding box with France, whose boundary is clipped to a sliver there and
  averages to a point at sea 10 km from any address. Places are out of the
  spatial index and out of the coverage bounds, and a boundary relation is
  skipped when fewer than half of its outer ways are in the extract.

### Changed
- 2026-09-22: `MatchType` splits `Exact` into `Exact` and `Prefix`, so a caller
  can tell "Queen Street West", which is a whole indexed key, from "Jasper",
  which only starts one.

### Removed
- 2026-09-02: the `api_keys` module in `geokode-server` and the `offline` and
  `batch` modules in `geokode-core`. Nothing called them, and the `/batch` route
  keeps working through `Geocoder::batch_forward`. The `parallel` feature and the
  rayon, bincode, chrono, sha2, uuid and tempfile dependencies went with them.

### Fixed
- 2026-09-20: an OSM object with a `name` tag was unfindable by its address.
  `build_full_address` puts the name in front of the house number, so the
  prefix search for "100 Queen St W" never reached Toronto City Hall or the
  two other records at that address. Records with a house number and a street
  are now also indexed under "<house number> <street>" and
  "<house number> <street> <city>".

### Changed
- 2026-09-20: a forward query that names a directional ranks records carrying
  that directional first and records carrying a different one last. The match
  key drops directionals, so Queen Street West, East and North all hit
  "100 queen st" and the order used to be whatever the FST yielded.
- 2026-09-20: without a declared header bbox, coverage is the extent of the
  records padded by the largest distance between any record and its nearest
  neighbour, so a point just past the outermost address still resolves. The
  pass costs 0.78 s on the 824,069-record Toronto extract.
- 2026-09-20: reverse geocoding answers only inside the coverage of the loaded
  data, so a query in another country returns nothing instead of `confidence:
  0.0` matches. An OSM PBF declares its coverage in the HeaderBBox of the
  OSMHeader blob and `ingest_osm_pbf` now reads it. A CSV or a PBF without that
  bbox falls back to the extent of the records. Inside the coverage the k nearest
  records come back as before, with the same confidence.
- 2026-09-16: the `geokode-cli` crate doc said the binary does index building and
  batch geocoding. It has `serve`, `forward` and `reverse` and nothing else, and
  the doc now says so. README and docs/index.html were audited against the code
  and needed no change.
- README drops the OpenAddresses house-number “known issue”. CSV ingest already
  joins with spaces, and `123 Main St, Springfield, IL` hits.
- Forward and autocomplete normalize directionals and strip unit/suite tokens
  so "123 North Main Street Apt 4" hits "123 Main St". Autocomplete takes
  optional `lon`/`lat` and ranks nearer hits first.

### Added
- Core geocoding library with FST text index and R-tree spatial index
- Address parsing and normalization (street abbreviations, house number extraction)
- Forward geocoding (text → coordinates) with prefix matching
- Reverse geocoding (coordinates → nearest address) with kNN search
- Autocomplete endpoint for interactive UIs
- Batch forward geocoding API
- OpenAddresses CSV data ingestion
- GeoJSON point feature ingestion
- REST API server (Axum) with CORS support
- CLI tool with `serve`, `forward`, `reverse` subcommands
- GitHub Actions CI (Ubuntu, Windows, macOS)
- AGPL-3.0-or-later license
