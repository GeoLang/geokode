//! OpenAddresses CSV ingest.
//!
//! Parses OpenAddresses data (CSV with LON, LAT, NUMBER, STREET, CITY, REGION, POSTCODE).

use csv::ReaderBuilder;
use geokode_core::address::Address;
use geokode_core::geocode::GeocoderBuilder;
use std::io::Read;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("missing required column: {0}")]
    MissingColumn(String),
}

/// Ingest OpenAddresses CSV data into a GeocoderBuilder.
pub fn ingest_openaddresses(
    reader: impl Read,
    builder: &mut GeocoderBuilder,
) -> Result<usize, IngestError> {
    let mut csv_reader = ReaderBuilder::new().has_headers(true).from_reader(reader);

    let headers = csv_reader.headers()?.clone();
    let lon_idx = find_column(&headers, &["LON", "lon", "longitude", "x"])?;
    let lat_idx = find_column(&headers, &["LAT", "lat", "latitude", "y"])?;
    let number_idx = find_column(&headers, &["NUMBER", "number", "house_number"]).ok();
    let street_idx = find_column(&headers, &["STREET", "street"]).ok();
    let city_idx = find_column(&headers, &["CITY", "city"]).ok();
    let region_idx = find_column(&headers, &["REGION", "region", "state"]).ok();
    let postcode_idx = find_column(&headers, &["POSTCODE", "postcode", "zip"]).ok();

    let mut count = 0;

    for result in csv_reader.records() {
        let record = result?;

        let lon: f64 = match record.get(lon_idx).and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let lat: f64 = match record.get(lat_idx).and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };

        let number = number_idx
            .and_then(|i| record.get(i))
            .map(|s| s.to_string());
        let street = street_idx
            .and_then(|i| record.get(i))
            .map(|s| s.to_string());
        let city = city_idx.and_then(|i| record.get(i)).map(|s| s.to_string());
        let region = region_idx
            .and_then(|i| record.get(i))
            .map(|s| s.to_string());
        let postcode = postcode_idx
            .and_then(|i| record.get(i))
            .map(|s| s.to_string());

        let full = [
            number.as_deref().unwrap_or(""),
            street.as_deref().unwrap_or(""),
            city.as_deref().unwrap_or(""),
            region.as_deref().unwrap_or(""),
        ]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(", ");

        let address = Address {
            house_number: number.filter(|s| !s.is_empty()),
            street: street.filter(|s| !s.is_empty()),
            city: city.filter(|s| !s.is_empty()),
            state: region.filter(|s| !s.is_empty()),
            postcode: postcode.filter(|s| !s.is_empty()),
            country: None,
            full,
        };

        builder.add(address, lat, lon);
        count += 1;
    }

    Ok(count)
}

fn find_column(headers: &csv::StringRecord, names: &[&str]) -> Result<usize, IngestError> {
    for name in names {
        if let Some(pos) = headers.iter().position(|h| h == *name) {
            return Ok(pos);
        }
    }
    Err(IngestError::MissingColumn(names[0].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_csv() {
        let csv_data = "LON,LAT,NUMBER,STREET,CITY,REGION,POSTCODE\n\
                        -89.65,39.78,123,Main St,Springfield,IL,62701\n\
                        -122.68,45.52,456,Oak Ave,Portland,OR,97201\n";

        let mut builder = GeocoderBuilder::new();
        let count = ingest_openaddresses(csv_data.as_bytes(), &mut builder).unwrap();
        assert_eq!(count, 2);

        let geocoder = builder.build().unwrap();
        assert_eq!(geocoder.len(), 2);
    }
}
