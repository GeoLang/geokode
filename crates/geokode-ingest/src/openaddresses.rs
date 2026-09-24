use csv::ReaderBuilder;
use geokode_core::address::{Address, FeatureKind};
use geokode_core::index::Record;
use std::io::{self, Read};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("missing required column: {0}")]
    MissingColumn(String),
    #[error("{0}")]
    Io(#[from] io::Error),
}

pub fn read_openaddresses(
    reader: impl Read,
    mut each: impl FnMut(Record) -> io::Result<()>,
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
        let row = result?;
        let number = |idx: usize| row.get(idx).and_then(|s| s.parse::<f64>().ok());
        let (Some(lon), Some(lat)) = (number(lon_idx), number(lat_idx)) else {
            continue;
        };
        let field = |idx: Option<usize>| {
            idx.and_then(|i| row.get(i))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let address = Address {
            house_number: field(number_idx),
            street: field(street_idx),
            city: field(city_idx),
            state: field(region_idx),
            postcode: field(postcode_idx),
            country: None,
            full: String::new(),
        };
        let full = [
            &address.house_number,
            &address.street,
            &address.city,
            &address.state,
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
        let mut record = Record::new(FeatureKind::Address, lon, lat);
        record.address = Address { full, ..address };
        each(record)?;
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

    fn read(csv: &str) -> Vec<Record> {
        let mut records = Vec::new();
        read_openaddresses(csv.as_bytes(), |record| {
            records.push(record);
            Ok(())
        })
        .unwrap();
        records
    }

    #[test]
    fn rows_become_address_records() {
        let records = read(
            "LON,LAT,NUMBER,STREET,CITY,REGION,POSTCODE\n\
             -89.65,39.78,123,Main St,Springfield,IL,62701\n\
             -122.68,45.52,456,Oak Ave,Portland,OR,97201\n\
             bad,45.52,1,Nowhere St,,,\n",
        );
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].address.house_number.as_deref(), Some("123"));
        assert_eq!(records[0].address.state.as_deref(), Some("IL"));
        assert_eq!(records[0].address.full, "123 Main St Springfield IL");
        assert!((records[1].lon - (-122.68)).abs() < 1e-9);
    }

    #[test]
    fn a_file_without_coordinates_is_refused() {
        let result = read_openaddresses("NUMBER,STREET\n1,Main St\n".as_bytes(), |_| Ok(()));
        assert!(matches!(result, Err(IngestError::MissingColumn(_))));
    }
}
