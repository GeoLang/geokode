//! Forward and reverse geocoding operations.

use crate::address::{Address, GeoResult, normalize_street};
use crate::index::{TextIndex, TextIndexBuilder};
use crate::spatial::{SpatialIndex, SpatialRecord};

/// A geocoding engine combining text and spatial indexes.
pub struct Geocoder {
    text_index: TextIndex,
    spatial_index: SpatialIndex,
    records: Vec<AddressRecord>,
}

/// Internal address record stored in the geocoder.
#[derive(Debug, Clone)]
pub struct AddressRecord {
    pub address: Address,
    pub lat: f64,
    pub lon: f64,
}

/// Builder for constructing a Geocoder from address data.
pub struct GeocoderBuilder {
    records: Vec<AddressRecord>,
}

impl GeocoderBuilder {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Add an address record.
    pub fn add(&mut self, address: Address, lat: f64, lon: f64) {
        self.records.push(AddressRecord { address, lat, lon });
    }

    /// Build the geocoder indexes.
    pub fn build(self) -> Result<Geocoder, std::io::Error> {
        let mut text_builder = TextIndexBuilder::new();
        let mut spatial_records = Vec::with_capacity(self.records.len());

        for (i, rec) in self.records.iter().enumerate() {
            let key = normalize_street(&rec.address.full);
            text_builder.insert(key, i as u64);
            spatial_records.push(SpatialRecord {
                lat: rec.lat,
                lon: rec.lon,
                id: i as u64,
            });
        }

        let text_index = text_builder.build()?;
        let spatial_index = SpatialIndex::build(spatial_records);

        Ok(Geocoder {
            text_index,
            spatial_index,
            records: self.records,
        })
    }
}

impl Default for GeocoderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Geocoder {
    /// Forward geocode: text query → coordinates.
    pub fn forward(&self, query: &str) -> Vec<GeoResult> {
        let normalized = normalize_street(query);
        let matches = self.text_index.prefix_search(&normalized);

        matches
            .into_iter()
            .filter_map(|(_, id)| {
                let rec = self.records.get(id as usize)?;
                Some(GeoResult {
                    address: rec.address.clone(),
                    lat: rec.lat,
                    lon: rec.lon,
                    confidence: 1.0,
                })
            })
            .collect()
    }

    /// Reverse geocode: coordinates → nearest address.
    pub fn reverse(&self, lon: f64, lat: f64, k: usize) -> Vec<GeoResult> {
        self.spatial_index
            .nearest(lon, lat, k)
            .into_iter()
            .filter_map(|sr| {
                let rec = self.records.get(sr.id as usize)?;
                let dist = ((rec.lat - lat).powi(2) + (rec.lon - lon).powi(2)).sqrt();
                // Confidence decays with distance (rough heuristic)
                let confidence = (1.0 - dist * 10.0).clamp(0.0, 1.0);
                Some(GeoResult {
                    address: rec.address.clone(),
                    lat: rec.lat,
                    lon: rec.lon,
                    confidence,
                })
            })
            .collect()
    }

    /// Autocomplete: prefix search for interactive UIs.
    pub fn autocomplete(&self, prefix: &str, limit: usize) -> Vec<GeoResult> {
        let normalized = normalize_street(prefix);
        let matches = self.text_index.prefix_search(&normalized);

        matches
            .into_iter()
            .take(limit)
            .filter_map(|(_, id)| {
                let rec = self.records.get(id as usize)?;
                Some(GeoResult {
                    address: rec.address.clone(),
                    lat: rec.lat,
                    lon: rec.lon,
                    confidence: 1.0,
                })
            })
            .collect()
    }

    /// Batch forward geocode.
    pub fn batch_forward(&self, queries: &[&str]) -> Vec<Vec<GeoResult>> {
        queries.iter().map(|q| self.forward(q)).collect()
    }

    /// Number of indexed records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Access the raw address records (for serialization/export).
    pub fn records(&self) -> &[AddressRecord] {
        &self.records
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::parse_address;

    fn build_test_geocoder() -> Geocoder {
        let mut builder = GeocoderBuilder::new();
        builder.add(
            parse_address("123 Main Street, Springfield, IL"),
            39.7817,
            -89.6501,
        );
        builder.add(
            parse_address("456 Oak Avenue, Portland, OR"),
            45.5152,
            -122.6784,
        );
        builder.add(
            parse_address("789 Main Drive, Denver, CO"),
            39.7392,
            -104.9903,
        );
        builder.build().unwrap()
    }

    #[test]
    fn forward_geocode() {
        let gc = build_test_geocoder();
        let results = gc.forward("123 main st");
        assert_eq!(results.len(), 1);
        assert!((results[0].lat - 39.7817).abs() < 0.001);
    }

    #[test]
    fn reverse_geocode() {
        let gc = build_test_geocoder();
        let results = gc.reverse(-89.65, 39.78, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].address.city.as_deref(), Some("Springfield"));
    }

    #[test]
    fn autocomplete_prefix() {
        let gc = build_test_geocoder();
        // Normalized: "123 main st, springfield, il" — search by "123"
        let results = gc.autocomplete("123", 10);
        assert!(!results.is_empty());
    }

    #[test]
    fn batch_forward_geocode() {
        let gc = build_test_geocoder();
        let results = gc.batch_forward(&["123 main st", "nonexistent"]);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), 1);
        assert_eq!(results[1].len(), 0);
    }
}
