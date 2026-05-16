# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
