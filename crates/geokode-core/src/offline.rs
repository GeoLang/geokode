//! Offline mode — serialize/deserialize geocoder index to disk.
//!
//! Enables fully offline geocoding by saving the FST and spatial indexes
//! to a compact binary format that can be loaded without network access.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::address::Address;
use crate::geocode::{Geocoder, GeocoderBuilder};

/// Serializable representation of the address database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineDatabase {
    pub records: Vec<OfflineRecord>,
    pub version: u32,
    pub metadata: OfflineMetadata,
}

/// Metadata about the offline database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineMetadata {
    pub record_count: usize,
    pub created_at: String,
    pub source: String,
    pub coverage: Option<String>,
}

/// A serializable address record for offline storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineRecord {
    pub address: Address,
    pub lat: f64,
    pub lon: f64,
}

impl OfflineDatabase {
    /// Create a new offline database from address records.
    pub fn from_records(records: Vec<OfflineRecord>, source: String) -> Self {
        let count = records.len();
        Self {
            records,
            version: 1,
            metadata: OfflineMetadata {
                record_count: count,
                created_at: chrono::Utc::now().to_rfc3339(),
                source,
                coverage: None,
            },
        }
    }

    /// Save the database to a binary file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let data = bincode::serialize(self).map_err(|e| io::Error::other(e.to_string()))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, data)
    }

    /// Load a database from a binary file.
    pub fn load(path: &Path) -> io::Result<Self> {
        let data = fs::read(path)?;
        bincode::deserialize(&data).map_err(|e| io::Error::other(e.to_string()))
    }

    /// Build a Geocoder from this offline database.
    pub fn into_geocoder(self) -> io::Result<Geocoder> {
        let mut builder = GeocoderBuilder::new();
        for record in self.records {
            builder.add(record.address, record.lat, record.lon);
        }
        builder.build()
    }

    /// Extract records from an existing geocoder (for serialization).
    pub fn from_geocoder(geocoder: &Geocoder, source: String) -> Self {
        let records: Vec<OfflineRecord> = geocoder
            .records()
            .iter()
            .map(|r| OfflineRecord {
                address: r.address.clone(),
                lat: r.lat,
                lon: r.lon,
            })
            .collect();

        Self::from_records(records, source)
    }

    /// Number of records in the database.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_records() -> Vec<OfflineRecord> {
        vec![
            OfflineRecord {
                address: Address {
                    house_number: Some("123".into()),
                    street: Some("Main St".into()),
                    city: Some("Springfield".into()),
                    state: Some("IL".into()),
                    postcode: Some("62701".into()),
                    country: Some("US".into()),
                    full: "123 Main St, Springfield, IL 62701".into(),
                },
                lat: 39.78,
                lon: -89.65,
            },
            OfflineRecord {
                address: Address {
                    house_number: Some("456".into()),
                    street: Some("Oak Ave".into()),
                    city: Some("Portland".into()),
                    state: Some("OR".into()),
                    postcode: Some("97201".into()),
                    country: Some("US".into()),
                    full: "456 Oak Ave, Portland, OR 97201".into(),
                },
                lat: 45.51,
                lon: -122.67,
            },
        ]
    }

    #[test]
    fn test_offline_database_create() {
        let db = OfflineDatabase::from_records(sample_records(), "test".into());
        assert_eq!(db.len(), 2);
        assert_eq!(db.metadata.record_count, 2);
    }

    #[test]
    fn test_offline_save_load_roundtrip() {
        let db = OfflineDatabase::from_records(sample_records(), "test".into());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.geokode");

        db.save(&path).unwrap();
        let loaded = OfflineDatabase::load(&path).unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded.records[0].address.full,
            "123 Main St, Springfield, IL 62701"
        );
    }

    #[test]
    fn test_offline_into_geocoder() {
        let db = OfflineDatabase::from_records(sample_records(), "test".into());
        let geocoder = db.into_geocoder().unwrap();
        assert_eq!(geocoder.len(), 2);
    }
}
