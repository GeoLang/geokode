# Test data

`monaco.osm.pbf` is the Geofabrik Monaco extract (https://download.geofabrik.de/europe/monaco-latest.osm.pbf), replication timestamp 2026-09-23T20:22:04Z.

It contains OpenStreetMap data, © OpenStreetMap contributors, available under the Open Database License 1.0 (ODbL). See https://www.openstreetmap.org/copyright.

`monaco-bnp-address.osm.pbf` is node 12323382259 from that extract with invented `addr:housenumber`, `addr:street` and `addr:postcode` tags added, so a named POI can take in an address object whose tags differ from its own.

`famous.osm.pbf` holds objects copied with their real tags from the Geofabrik ile-de-france, new-york and washington extracts and the OpenStreetMap API on 2026-09-24: the Eiffel Tower and its Texas replica, Central Park and the Central Park village in Washington, the Kilimanjaro massif and one Mount Kilimanjaro Street, the Tokyo and Kyiv place nodes, and the Paris, Springfield and Portland settlements. © OpenStreetMap contributors, ODbL.
