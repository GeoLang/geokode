//! OpenStreetMap data ingest for geocoding indexes.
//!
//! Parses OSM data and extracts addressable features (nodes, ways via centroid)
//! tagged with `addr:*`. Supports the Overpass API JSON/CSV exports and binary
//! OSM PBF extracts (e.g. Geofabrik downloads).

use flate2::read::ZlibDecoder;
use geokode_core::address::{Address, Place, PlaceClass};
use geokode_core::geocode::{Coverage, GeocoderBuilder};
use osmpbfreader::fileformat::{Blob, BlobHeader};
use osmpbfreader::osmformat::HeaderBlock;
use protobuf::Message;
use serde::Deserialize;
use std::io::{Read, Seek, SeekFrom};
use thiserror::Error;

// caps from the PBF format: 64 KiB blob header, 32 MiB blob
const MAX_BLOB_HEADER_BYTES: u64 = 64 * 1024;
const MAX_BLOB_BYTES: u64 = 32 * 1024 * 1024;

const NANODEGREES_PER_DEGREE: f64 = 1e9;

#[derive(Debug, Error)]
pub enum OsmError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no elements found")]
    NoElements,
}

#[derive(Debug, Error)]
pub enum OsmPbfError {
    #[error("PBF error: {0}")]
    Pbf(#[from] osmpbfreader::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Ingest addresses and places from a binary OSM PBF extract.
///
/// Extracts nodes and ways tagged with both `addr:housenumber` and
/// `addr:street`; way addresses use the centroid of the way's member nodes.
/// Also extracts `place=*` nodes and `boundary=administrative` relations, the
/// relation taking the centroid of its outer ways, and skipping a relation
/// whose name a place node already covers.
/// Requires `Read + Seek` because the PBF is scanned twice — once to find the
/// matching objects, once to pull in the nodes they depend on for centroids.
pub fn ingest_osm_pbf<R: Read + Seek>(
    mut reader: R,
    builder: &mut GeocoderBuilder,
) -> Result<usize, OsmPbfError> {
    use osmpbfreader::{OsmId, OsmObj, OsmPbfReader};

    if let Some(coverage) = read_header_bbox(&mut reader) {
        builder.set_coverage(coverage);
    }
    reader.seek(SeekFrom::Start(0))?;

    let is_address = |obj: &OsmObj| {
        obj.tags().contains_key("addr:housenumber") && obj.tags().contains_key("addr:street")
    };
    let is_place_node = |obj: &OsmObj| {
        matches!(obj, OsmObj::Node(_))
            && obj.tags().contains_key("name")
            && obj
                .tags()
                .get("place")
                .and_then(|tag| PlaceClass::from_tag(tag))
                .is_some()
    };
    let is_admin_relation = |obj: &OsmObj| {
        matches!(obj, OsmObj::Relation(_))
            && obj
                .tags()
                .get("boundary")
                .is_some_and(|b| b == "administrative")
            && obj.tags().contains_key("name")
    };

    let mut pbf = OsmPbfReader::new(reader);
    let objs = pbf
        .get_objs_and_deps(|obj| is_address(obj) || is_place_node(obj) || is_admin_relation(obj))?;

    let mut count = 0;
    let mut place_names = std::collections::HashSet::new();
    for obj in objs.values() {
        if !is_place_node(obj) {
            continue;
        }
        let OsmObj::Node(node) = obj else { continue };
        let Some(place) = read_place(obj.tags(), PlaceClass::Other) else {
            continue;
        };
        place_names.insert(place.name.to_lowercase());
        builder.add_place(place, node.lat(), node.lon());
        count += 1;
    }

    for obj in objs.values() {
        if !is_admin_relation(obj) {
            continue;
        }
        let Some(place) = read_place(obj.tags(), PlaceClass::Other) else {
            continue;
        };
        if place_names.contains(&place.name.to_lowercase()) {
            continue;
        }
        let OsmObj::Relation(relation) = obj else {
            continue;
        };
        let Some((lat, lon)) = relation_centroid(relation, &objs) else {
            continue;
        };
        builder.add_place(place, lat, lon);
        count += 1;
    }

    for obj in objs.values() {
        // Only emit matched address objects; dependency nodes pulled in for way
        // centroids are skipped here.
        if !is_address(obj) {
            continue;
        }

        let (lat, lon) = match obj {
            OsmObj::Node(n) => (n.lat(), n.lon()),
            OsmObj::Way(w) => {
                let (mut slat, mut slon, mut n) = (0.0, 0.0, 0u32);
                for node_id in &w.nodes {
                    if let Some(OsmObj::Node(nd)) = objs.get(&OsmId::Node(*node_id)) {
                        slat += nd.lat();
                        slon += nd.lon();
                        n += 1;
                    }
                }
                if n == 0 {
                    continue;
                }
                (slat / f64::from(n), slon / f64::from(n))
            }
            OsmObj::Relation(_) => continue,
        };

        let tags = obj.tags();
        let osm_tags = OsmTags {
            house_number: tags.get("addr:housenumber").map(ToString::to_string),
            street: tags.get("addr:street").map(ToString::to_string),
            city: tags.get("addr:city").map(ToString::to_string),
            state: tags.get("addr:state").map(ToString::to_string),
            postcode: tags.get("addr:postcode").map(ToString::to_string),
            country: tags.get("addr:country").map(ToString::to_string),
            name: tags.get("name").map(ToString::to_string),
            place: tags.get("place").map(ToString::to_string),
            population: tags.get("population").map(ToString::to_string),
        };
        let full = build_full_address(&osm_tags);
        if full.is_empty() {
            continue;
        }

        builder.add(
            Address {
                house_number: osm_tags.house_number,
                street: osm_tags.street,
                city: osm_tags.city,
                state: osm_tags.state,
                postcode: osm_tags.postcode,
                country: osm_tags.country,
                full,
            },
            lat,
            lon,
        );
        count += 1;
    }

    Ok(count)
}

// an administrative boundary usually carries no place tag
fn read_place(tags: &osmpbfreader::Tags, default_class: PlaceClass) -> Option<Place> {
    let name = tags.get("name")?.to_string();
    let class = tags
        .get("place")
        .and_then(|tag| PlaceClass::from_tag(tag))
        .unwrap_or(default_class);
    Some(Place {
        name,
        class,
        population: tags.get("population").and_then(|p| p.parse().ok()),
        state: tags
            .get("is_in:state")
            .or_else(|| tags.get("addr:state"))
            .map(ToString::to_string),
        country: tags
            .get("is_in:country")
            .or_else(|| tags.get("addr:country"))
            .map(ToString::to_string),
    })
}

// a relation whose ways are outside the extract has no centroid
fn relation_centroid(
    relation: &osmpbfreader::Relation,
    objs: &std::collections::BTreeMap<osmpbfreader::OsmId, osmpbfreader::OsmObj>,
) -> Option<(f64, f64)> {
    use osmpbfreader::{OsmId, OsmObj};

    let (mut sum_lat, mut sum_lon, mut seen) = (0.0, 0.0, 0u32);
    for reference in &relation.refs {
        if !reference.role.is_empty() && reference.role != "outer" {
            continue;
        }
        let OsmId::Way(way_id) = reference.member else {
            continue;
        };
        let Some(OsmObj::Way(way)) = objs.get(&OsmId::Way(way_id)) else {
            continue;
        };
        for node_id in &way.nodes {
            if let Some(OsmObj::Node(node)) = objs.get(&OsmId::Node(*node_id)) {
                sum_lat += node.lat();
                sum_lon += node.lon();
                seen += 1;
            }
        }
    }
    if seen == 0 {
        return None;
    }
    Some((sum_lat / f64::from(seen), sum_lon / f64::from(seen)))
}

// a file with no OSMHeader bbox returns None and falls back to the record extent
fn read_header_bbox<R: Read>(reader: &mut R) -> Option<Coverage> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length).ok()?;
    let header_length = u64::from(u32::from_be_bytes(length));
    if header_length > MAX_BLOB_HEADER_BYTES {
        return None;
    }
    let blob_header: BlobHeader = read_message(reader, header_length)?;
    if blob_header.get_field_type() != "OSMHeader" {
        return None;
    }
    let blob: Blob = read_message(reader, u64::try_from(blob_header.get_datasize()).ok()?)?;
    let header: HeaderBlock = Message::parse_from_bytes(&blob_bytes(&blob)?).ok()?;
    if !header.has_bbox() {
        return None;
    }
    let bbox = header.get_bbox();
    Some(Coverage {
        min_lon: bbox.get_left() as f64 / NANODEGREES_PER_DEGREE,
        min_lat: bbox.get_bottom() as f64 / NANODEGREES_PER_DEGREE,
        max_lon: bbox.get_right() as f64 / NANODEGREES_PER_DEGREE,
        max_lat: bbox.get_top() as f64 / NANODEGREES_PER_DEGREE,
    })
}

fn read_message<M: Message, R: Read>(reader: &mut R, length: u64) -> Option<M> {
    if length > MAX_BLOB_BYTES {
        return None;
    }
    let mut buf = Vec::new();
    reader.take(length).read_to_end(&mut buf).ok()?;
    M::parse_from_bytes(&buf).ok()
}

fn blob_bytes(blob: &Blob) -> Option<Vec<u8>> {
    if blob.has_raw() {
        return Some(blob.get_raw().to_vec());
    }
    if !blob.has_zlib_data() {
        return None;
    }
    let mut out = Vec::new();
    ZlibDecoder::new(blob.get_zlib_data())
        .take(MAX_BLOB_BYTES)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

#[derive(Debug, Deserialize)]
struct OsmResponse {
    elements: Vec<OsmElement>,
}

#[derive(Debug, Deserialize)]
struct OsmElement {
    #[serde(rename = "type")]
    elem_type: String,
    lat: Option<f64>,
    lon: Option<f64>,
    center: Option<OsmCenter>,
    tags: Option<OsmTags>,
}

#[derive(Debug, Deserialize)]
struct OsmCenter {
    lat: f64,
    lon: f64,
}

#[derive(Debug, Deserialize)]
struct OsmTags {
    #[serde(rename = "addr:housenumber")]
    house_number: Option<String>,
    #[serde(rename = "addr:street")]
    street: Option<String>,
    #[serde(rename = "addr:city")]
    city: Option<String>,
    #[serde(rename = "addr:state")]
    state: Option<String>,
    #[serde(rename = "addr:postcode")]
    postcode: Option<String>,
    #[serde(rename = "addr:country")]
    country: Option<String>,
    name: Option<String>,
    place: Option<String>,
    population: Option<String>,
}

/// Ingest OSM Overpass API JSON response into a geocoder builder.
pub fn ingest_osm_overpass(data: &str, builder: &mut GeocoderBuilder) -> Result<usize, OsmError> {
    let response: OsmResponse = serde_json::from_str(data)?;

    if response.elements.is_empty() {
        return Err(OsmError::NoElements);
    }

    let mut count = 0;

    for elem in &response.elements {
        let (lat, lon) = match (&elem.elem_type, elem.lat, elem.lon, &elem.center) {
            (_, Some(lat), Some(lon), _) => (lat, lon),
            (_, _, _, Some(c)) => (c.lat, c.lon),
            _ => continue,
        };

        let tags = match &elem.tags {
            Some(t) => t,
            None => continue,
        };

        if let Some(class) = tags.place.as_deref().and_then(PlaceClass::from_tag)
            && let Some(name) = &tags.name
        {
            builder.add_place(
                Place {
                    name: name.clone(),
                    class,
                    population: tags.population.as_ref().and_then(|p| p.parse().ok()),
                    state: tags.state.clone(),
                    country: tags.country.clone(),
                },
                lat,
                lon,
            );
            count += 1;
            continue;
        }

        // Skip elements without address information
        if tags.street.is_none() && tags.name.is_none() {
            continue;
        }

        let full = build_full_address(tags);
        if full.is_empty() {
            continue;
        }

        let address = Address {
            house_number: tags.house_number.clone(),
            street: tags.street.clone(),
            city: tags.city.clone(),
            state: tags.state.clone(),
            postcode: tags.postcode.clone(),
            country: tags.country.clone(),
            full,
        };

        builder.add(address, lat, lon);
        count += 1;
    }

    Ok(count)
}

fn build_full_address(tags: &OsmTags) -> String {
    let parts: Vec<&str> = [
        tags.house_number.as_deref(),
        tags.street.as_deref(),
        tags.city.as_deref(),
        tags.state.as_deref(),
        tags.postcode.as_deref(),
        tags.country.as_deref(),
    ]
    .iter()
    .filter_map(|p| *p)
    .filter(|s| !s.is_empty())
    .collect();

    if parts.is_empty() {
        tags.name.as_deref().unwrap_or("").to_string()
    } else {
        // Prefix with name if available and different from street
        if let Some(name) = &tags.name
            && tags.street.as_deref() != Some(name.as_str())
        {
            return format!("{}, {}", name, parts.join(", "));
        }
        parts.join(", ")
    }
}

/// Parse an Overpass CSV export (tab-separated with @lat, @lon columns).
pub fn ingest_osm_csv(
    reader: impl std::io::Read,
    builder: &mut GeocoderBuilder,
) -> Result<usize, csv::Error> {
    let mut csv_reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(true)
        .from_reader(reader);

    let headers = csv_reader.headers()?.clone();
    let lat_idx = headers.iter().position(|h| h == "@lat");
    let lon_idx = headers.iter().position(|h| h == "@lon");
    let name_idx = headers.iter().position(|h| h == "name");

    let (lat_idx, lon_idx) = match (lat_idx, lon_idx) {
        (Some(a), Some(b)) => (a, b),
        _ => return Ok(0),
    };

    let mut count = 0;

    for result in csv_reader.records() {
        let record = result?;

        let lat: f64 = match record.get(lat_idx).and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let lon: f64 = match record.get(lon_idx).and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };

        let name = name_idx
            .and_then(|i| record.get(i))
            .unwrap_or("")
            .to_string();

        if name.is_empty() {
            continue;
        }

        let address = Address {
            house_number: None,
            street: None,
            city: None,
            state: None,
            postcode: None,
            country: None,
            full: name,
        };

        builder.add(address, lat, lon);
        count += 1;
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use geokode_core::address::parse_address;
    use osmpbfreader::osmformat::HeaderBBox;
    use std::io::{Cursor, Write};

    // left, right, top, bottom in nanodegrees
    const MONACO_BBOX: [i64; 4] = [7_360_000_000, 7_470_000_000, 43_780_000_000, 43_710_000_000];

    fn header_only_pbf(bbox: Option<[i64; 4]>, compress: bool) -> Vec<u8> {
        let mut header = HeaderBlock::new();
        if let Some([left, right, top, bottom]) = bbox {
            let mut header_bbox = HeaderBBox::new();
            header_bbox.set_left(left);
            header_bbox.set_right(right);
            header_bbox.set_top(top);
            header_bbox.set_bottom(bottom);
            header.set_bbox(header_bbox);
        }
        let payload = header.write_to_bytes().unwrap();

        let mut blob = Blob::new();
        blob.set_raw_size(payload.len() as i32);
        if compress {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&payload).unwrap();
            blob.set_zlib_data(encoder.finish().unwrap());
        } else {
            blob.set_raw(payload);
        }
        let blob_bytes = blob.write_to_bytes().unwrap();

        let mut blob_header = BlobHeader::new();
        blob_header.set_field_type("OSMHeader".to_string());
        blob_header.set_datasize(blob_bytes.len() as i32);
        let blob_header_bytes = blob_header.write_to_bytes().unwrap();

        let mut out = Vec::new();
        out.extend_from_slice(&(blob_header_bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&blob_header_bytes);
        out.extend_from_slice(&blob_bytes);
        out
    }

    fn monaco_geocoder_from(pbf: Vec<u8>) -> geokode_core::geocode::Geocoder {
        let mut builder = GeocoderBuilder::new();
        let count = ingest_osm_pbf(Cursor::new(pbf), &mut builder).unwrap();
        assert_eq!(count, 0, "header-only PBF has no address objects");
        builder.add(
            parse_address("1 Avenue Grimaldi, Monaco, MC"),
            43.7355,
            7.4197,
        );
        builder.build().unwrap()
    }

    #[test]
    fn pbf_header_bbox_is_the_coverage() {
        let geocoder = monaco_geocoder_from(header_only_pbf(Some(MONACO_BBOX), true));
        let coverage = geocoder.coverage().expect("header bbox");
        assert!((coverage.min_lon - 7.36).abs() < 1e-9);
        assert!((coverage.max_lon - 7.47).abs() < 1e-9);
        assert!((coverage.min_lat - 43.71).abs() < 1e-9);
        assert!((coverage.max_lat - 43.78).abs() < 1e-9);
        // inside the declared box but away from the only record
        assert_eq!(geocoder.reverse(7.44, 43.75, 1).len(), 1);
        assert!(geocoder.reverse(-79.41, 43.647, 1).is_empty());
    }

    #[test]
    fn pbf_header_bbox_uncompressed() {
        let geocoder = monaco_geocoder_from(header_only_pbf(Some(MONACO_BBOX), false));
        let coverage = geocoder.coverage().expect("header bbox");
        assert!((coverage.max_lat - 43.78).abs() < 1e-9);
    }

    #[test]
    fn pbf_without_header_bbox_falls_back_to_record_extent() {
        let geocoder = monaco_geocoder_from(header_only_pbf(None, true));
        let coverage = geocoder.coverage().expect("record extent");
        assert!((coverage.min_lon - 7.4197).abs() < 1e-9);
        assert!((coverage.max_lat - 43.7355).abs() < 1e-9);
        assert!(geocoder.reverse(7.44, 43.75, 1).is_empty());
    }

    #[test]
    fn ingest_overpass_json() {
        let data = r#"{
            "elements": [
                {
                    "type": "node",
                    "lat": 48.8566,
                    "lon": 2.3522,
                    "tags": {
                        "addr:housenumber": "1",
                        "addr:street": "Rue de Rivoli",
                        "addr:city": "Paris",
                        "addr:postcode": "75001",
                        "addr:country": "FR"
                    }
                },
                {
                    "type": "way",
                    "center": { "lat": 51.5074, "lon": -0.1278 },
                    "tags": {
                        "addr:street": "Baker Street",
                        "addr:housenumber": "221B",
                        "addr:city": "London",
                        "addr:country": "GB"
                    }
                },
                {
                    "type": "node",
                    "lat": 40.0,
                    "lon": -74.0,
                    "tags": {}
                }
            ]
        }"#;

        let mut builder = GeocoderBuilder::new();
        let count = ingest_osm_overpass(data, &mut builder).unwrap();
        assert_eq!(count, 2);

        let geocoder = builder.build().unwrap();
        assert_eq!(geocoder.len(), 2);
    }

    #[test]
    fn ingest_overpass_with_name() {
        let data = r#"{
            "elements": [
                {
                    "type": "node",
                    "lat": 48.8584,
                    "lon": 2.2945,
                    "tags": {
                        "name": "Eiffel Tower",
                        "addr:city": "Paris",
                        "addr:country": "FR"
                    }
                }
            ]
        }"#;

        let mut builder = GeocoderBuilder::new();
        let count = ingest_osm_overpass(data, &mut builder).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn named_place_is_found_by_house_number_and_street() {
        // the name goes in front of the house number in `full`
        let data = r#"{
            "elements": [
                {
                    "type": "node",
                    "lat": 43.65313,
                    "lon": -79.3832344,
                    "tags": {
                        "name": "Toronto Public Library - City Hall",
                        "addr:housenumber": "100",
                        "addr:street": "Queen Street West",
                        "addr:postcode": "M5H 2N3"
                    }
                }
            ]
        }"#;

        let mut builder = GeocoderBuilder::new();
        assert_eq!(ingest_osm_overpass(data, &mut builder).unwrap(), 1);
        let geocoder = builder.build().unwrap();

        let results = geocoder.forward("100 Queen St W");
        assert_eq!(results.len(), 1, "got {results:?}");
        assert!(results[0].address.full.contains("Toronto Public Library"));
    }

    #[test]
    fn ingest_overpass_skips_no_address() {
        let data = r#"{
            "elements": [
                {
                    "type": "node",
                    "lat": 48.0,
                    "lon": 2.0
                }
            ]
        }"#;

        let mut builder = GeocoderBuilder::new();
        let count = ingest_osm_overpass(data, &mut builder).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn ingest_overpass_empty() {
        let data = r#"{"elements": []}"#;
        let mut builder = GeocoderBuilder::new();
        let result = ingest_osm_overpass(data, &mut builder);
        assert!(result.is_err());
    }

    #[test]
    fn overpass_place_node_becomes_a_place() {
        let data = r#"{"elements":[
            {"type":"node","lat":52.875,"lon":-118.082,
             "tags":{"place":"town","name":"Jasper","population":"4738"}}
        ]}"#;
        let mut builder = GeocoderBuilder::new();
        assert_eq!(ingest_osm_overpass(data, &mut builder).unwrap(), 1);
        let results = builder.build().unwrap().forward("jasper");
        assert_eq!(results[0].kind, geokode_core::address::FeatureKind::Place);
        assert!((results[0].lat - 52.875).abs() < 0.001);
    }

    #[test]
    fn overpass_place_tag_we_do_not_index_stays_an_address() {
        let data = r#"{"elements":[
            {"type":"node","lat":1.0,"lon":2.0,
             "tags":{"place":"farm","name":"Hill Farm"}}
        ]}"#;
        let mut builder = GeocoderBuilder::new();
        assert_eq!(ingest_osm_overpass(data, &mut builder).unwrap(), 1);
        let results = builder.build().unwrap().forward("hill farm");
        assert_eq!(results[0].kind, geokode_core::address::FeatureKind::Address);
    }

    fn node_at(id: i64, lat: f64, lon: f64) -> osmpbfreader::OsmObj {
        osmpbfreader::OsmObj::Node(osmpbfreader::Node {
            id: osmpbfreader::NodeId(id),
            tags: osmpbfreader::Tags::new(),
            decimicro_lat: (lat * 1e7) as i32,
            decimicro_lon: (lon * 1e7) as i32,
        })
    }

    fn way_of(id: i64, nodes: &[i64]) -> osmpbfreader::OsmObj {
        osmpbfreader::OsmObj::Way(osmpbfreader::Way {
            id: osmpbfreader::WayId(id),
            tags: osmpbfreader::Tags::new(),
            nodes: nodes.iter().map(|n| osmpbfreader::NodeId(*n)).collect(),
        })
    }

    #[test]
    fn a_boundary_centroid_averages_its_outer_ways() {
        let mut objs = std::collections::BTreeMap::new();
        for (id, lat, lon) in [(1, 0.0, 0.0), (2, 2.0, 0.0), (3, 2.0, 2.0), (4, 0.0, 2.0)] {
            objs.insert(
                osmpbfreader::OsmId::Node(osmpbfreader::NodeId(id)),
                node_at(id, lat, lon),
            );
        }
        objs.insert(
            osmpbfreader::OsmId::Way(osmpbfreader::WayId(10)),
            way_of(10, &[1, 2]),
        );
        objs.insert(
            osmpbfreader::OsmId::Way(osmpbfreader::WayId(11)),
            way_of(11, &[3, 4]),
        );
        let relation = osmpbfreader::Relation {
            id: osmpbfreader::RelationId(100),
            tags: osmpbfreader::Tags::new(),
            refs: vec![
                osmpbfreader::Ref {
                    member: osmpbfreader::OsmId::Way(osmpbfreader::WayId(10)),
                    role: "outer".into(),
                },
                osmpbfreader::Ref {
                    member: osmpbfreader::OsmId::Way(osmpbfreader::WayId(11)),
                    role: "outer".into(),
                },
                // an inner ring and a way outside the extract both contribute nothing
                osmpbfreader::Ref {
                    member: osmpbfreader::OsmId::Way(osmpbfreader::WayId(12)),
                    role: "inner".into(),
                },
                osmpbfreader::Ref {
                    member: osmpbfreader::OsmId::Way(osmpbfreader::WayId(13)),
                    role: "outer".into(),
                },
            ],
        };
        let (lat, lon) = relation_centroid(&relation, &objs).unwrap();
        assert!((lat - 1.0).abs() < 1e-6);
        assert!((lon - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_boundary_with_no_ways_in_the_extract_has_no_centroid() {
        let relation = osmpbfreader::Relation {
            id: osmpbfreader::RelationId(100),
            tags: osmpbfreader::Tags::new(),
            refs: vec![osmpbfreader::Ref {
                member: osmpbfreader::OsmId::Way(osmpbfreader::WayId(10)),
                role: "outer".into(),
            }],
        };
        assert!(relation_centroid(&relation, &std::collections::BTreeMap::new()).is_none());
    }

    #[test]
    fn build_full_address_all_parts() {
        let tags = OsmTags {
            house_number: Some("42".to_string()),
            street: Some("Main St".to_string()),
            city: Some("Springfield".to_string()),
            state: Some("IL".to_string()),
            postcode: Some("62701".to_string()),
            country: Some("US".to_string()),
            name: None,
            place: None,
            population: None,
        };
        let full = build_full_address(&tags);
        assert_eq!(full, "42, Main St, Springfield, IL, 62701, US");
    }

    #[test]
    fn build_full_address_name_only() {
        let tags = OsmTags {
            house_number: None,
            street: None,
            city: None,
            state: None,
            postcode: None,
            country: None,
            name: Some("Central Park".to_string()),
            place: None,
            population: None,
        };
        let full = build_full_address(&tags);
        assert_eq!(full, "Central Park");
    }

    #[test]
    fn ingest_osm_csv_tab_separated() {
        let csv_data = "@lat\t@lon\tname\n48.8566\t2.3522\tLouvre Museum\n40.7128\t-74.0060\tStatue of Liberty\n";
        let mut builder = GeocoderBuilder::new();
        let count = ingest_osm_csv(csv_data.as_bytes(), &mut builder).unwrap();
        assert_eq!(count, 2);
    }
}
