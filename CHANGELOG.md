# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
