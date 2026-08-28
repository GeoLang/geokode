# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
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

### Changed
- sha2 on 0.11. API key digests are hex encoded by a local module instead of
  `{:x}`, which digest 0.11 no longer implements, and a golden test pins the
  string so a stored hash still matches.
