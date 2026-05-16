//! GeoJSON point feature ingest.

use geokode_core::address::parse_address;
use geokode_core::geocode::GeocoderBuilder;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GeoJsonError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not a FeatureCollection")]
    NotFeatureCollection,
}

/// Ingest a GeoJSON FeatureCollection of Point features.
/// Extracts address from the "address" or "name" property.
pub fn ingest_geojson(data: &str, builder: &mut GeocoderBuilder) -> Result<usize, GeoJsonError> {
    let json: Value = serde_json::from_str(data)?;

    let features = json
        .get("features")
        .and_then(|f| f.as_array())
        .ok_or(GeoJsonError::NotFeatureCollection)?;

    let mut count = 0;

    for feature in features {
        let coords = feature
            .pointer("/geometry/coordinates")
            .and_then(|c| c.as_array());

        let (lon, lat) = match coords {
            Some(c) if c.len() >= 2 => {
                let lon = c[0].as_f64().unwrap_or(0.0);
                let lat = c[1].as_f64().unwrap_or(0.0);
                (lon, lat)
            }
            _ => continue,
        };

        let props = feature.get("properties");
        let addr_str = props
            .and_then(|p| p.get("address").or_else(|| p.get("name")))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if addr_str.is_empty() {
            continue;
        }

        let address = parse_address(addr_str);
        builder.add(address, lat, lon);
        count += 1;
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_geojson_points() {
        let geojson = r#"{
            "type": "FeatureCollection",
            "features": [
                {
                    "type": "Feature",
                    "geometry": { "type": "Point", "coordinates": [-74.0, 40.7] },
                    "properties": { "address": "123 Broadway, New York, NY" }
                },
                {
                    "type": "Feature",
                    "geometry": { "type": "Point", "coordinates": [-87.6, 41.9] },
                    "properties": { "name": "456 Michigan Ave, Chicago, IL" }
                }
            ]
        }"#;

        let mut builder = GeocoderBuilder::new();
        let count = ingest_geojson(geojson, &mut builder).unwrap();
        assert_eq!(count, 2);
    }
}
