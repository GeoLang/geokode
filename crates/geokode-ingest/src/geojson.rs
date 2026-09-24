use geokode_core::address::{FeatureKind, parse_address};
use geokode_core::index::Record;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GeoJsonError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not a FeatureCollection")]
    NotFeatureCollection,
}

// Point features, the address text taken from the "address" or "name" property
pub fn read_geojson(data: &str) -> Result<Vec<Record>, GeoJsonError> {
    let json: Value = serde_json::from_str(data)?;
    let features = json
        .get("features")
        .and_then(|f| f.as_array())
        .ok_or(GeoJsonError::NotFeatureCollection)?;

    Ok(features
        .iter()
        .filter_map(|feature| {
            let coords = feature.pointer("/geometry/coordinates")?.as_array()?;
            let (lon, lat) = (coords.first()?.as_f64()?, coords.get(1)?.as_f64()?);
            let text = feature
                .get("properties")
                .and_then(|p| p.get("address").or_else(|| p.get("name")))
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())?;
            let mut record = Record::new(FeatureKind::Address, lon, lat);
            record.address = parse_address(text);
            Some(record)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_features_become_address_records() {
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
                },
                {
                    "type": "Feature",
                    "geometry": { "type": "Point", "coordinates": [0.0, 0.0] },
                    "properties": {}
                }
            ]
        }"#;
        let records = read_geojson(geojson).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].address.city.as_deref(), Some("Chicago"));
        assert_eq!(records[0].lon, -74.0);
    }
}
