//! OpenStreetMap data ingest for geocoding indexes.
//!
//! Parses OSM JSON (Overpass API format) and extracts addressable features:
//! nodes, ways (centroids), and relations with addr:* tags.

use geokode_core::address::Address;
use geokode_core::geocode::GeocoderBuilder;
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OsmError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no elements found")]
    NoElements,
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
    fn build_full_address_all_parts() {
        let tags = OsmTags {
            house_number: Some("42".to_string()),
            street: Some("Main St".to_string()),
            city: Some("Springfield".to_string()),
            state: Some("IL".to_string()),
            postcode: Some("62701".to_string()),
            country: Some("US".to_string()),
            name: None,
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
