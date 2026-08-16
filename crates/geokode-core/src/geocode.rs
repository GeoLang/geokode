//! Forward and reverse geocoding operations.

use crate::address::{Address, GeoResult, MatchType, normalize_for_match};
use crate::fuzzy::{FuzzyConfig, FuzzySearcher};
use crate::index::{TextIndex, TextIndexBuilder};
use crate::spatial::{SpatialIndex, SpatialRecord};

/// Most fuzzy fallback results returned for one query.
const FUZZY_LIMIT: usize = 5;

/// A record is indexed under up to 4 keys, so over-fetch before deduping by
/// record id, otherwise a single street can crowd out the other candidates.
const FUZZY_CANDIDATES: usize = FUZZY_LIMIT * 4;

fn fuzzy_config() -> FuzzyConfig {
    FuzzyConfig {
        max_distance: 2,
        // soundex over a whole address string collides too easily, so garbage
        // queries would come back with unrelated addresses. Edit distance only.
        phonetic_fallback: false,
        min_score: 0.6,
    }
}

/// A geocoding engine combining text and spatial indexes.
pub struct Geocoder {
    text_index: TextIndex,
    spatial_index: SpatialIndex,
    fuzzy: FuzzySearcher,
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
        let mut fuzzy = FuzzySearcher::new(fuzzy_config());
        let mut spatial_records = Vec::with_capacity(self.records.len());

        for (i, rec) in self.records.iter().enumerate() {
            // Index several prefix-searchable variants so queries by street or
            // place name match — not only the house-number-led full address.
            // Each key is suffixed with a unit separator + id so FST keys stay
            // unique while the human-readable prefix still matches.
            let mut keys: Vec<String> = vec![index_key(&rec.address.full)];
            if let Some(street) = &rec.address.street {
                keys.push(index_key(street));
                if let Some(city) = &rec.address.city {
                    keys.push(index_key(&format!("{street} {city}")));
                }
            }
            if let Some(city) = &rec.address.city {
                keys.push(index_key(city));
            }
            for key in keys {
                fuzzy.add_entry(key.clone(), i as u64);
                text_builder.insert(format!("{key}\u{1f}{i}"), i as u64);
            }
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
            fuzzy,
            records: self.records,
        })
    }
}

impl Default for GeocoderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn index_key(s: &str) -> String {
    normalize_for_match(s)
}

impl Geocoder {
    /// Forward geocode: text query → coordinates. Falls back to fuzzy matching
    /// when the text index has no exact or prefix hit.
    pub fn forward(&self, query: &str) -> Vec<GeoResult> {
        let normalized = index_key(query);
        let matches = self.text_index.prefix_search(&normalized);

        // A record can be indexed under several keys, so dedup by record id.
        let mut seen = std::collections::HashSet::new();
        let exact: Vec<GeoResult> = matches
            .into_iter()
            .filter_map(|(_, id)| {
                if !seen.insert(id) {
                    return None;
                }
                let rec = self.records.get(id as usize)?;
                Some(GeoResult {
                    address: rec.address.clone(),
                    lat: rec.lat,
                    lon: rec.lon,
                    confidence: 1.0,
                    match_type: MatchType::Exact,
                })
            })
            .collect();

        if !exact.is_empty() {
            return exact;
        }
        self.forward_fuzzy(&normalized)
    }

    /// Fuzzy fallback over the indexed keys. Confidence carries the fuzzy score
    /// so callers can rank these below exact hits.
    fn forward_fuzzy(&self, normalized: &str) -> Vec<GeoResult> {
        let mut seen = std::collections::HashSet::new();
        let mut results: Vec<GeoResult> = self
            .fuzzy
            .search(normalized, FUZZY_CANDIDATES)
            .into_iter()
            .filter_map(|m| {
                if !seen.insert(m.record_id) {
                    return None;
                }
                let rec = self.records.get(m.record_id as usize)?;
                Some(GeoResult {
                    address: rec.address.clone(),
                    lat: rec.lat,
                    lon: rec.lon,
                    confidence: m.score,
                    match_type: MatchType::Fuzzy,
                })
            })
            .collect();
        results.truncate(FUZZY_LIMIT);
        results
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
                    match_type: MatchType::Exact,
                })
            })
            .collect()
    }

    /// Autocomplete: prefix search. Optional `(lon, lat)` ranks nearer hits first.
    pub fn autocomplete(&self, prefix: &str, limit: usize) -> Vec<GeoResult> {
        self.autocomplete_biased(prefix, limit, None)
    }

    /// Prefix search with optional spatial bias for interactive UIs.
    pub fn autocomplete_biased(
        &self,
        prefix: &str,
        limit: usize,
        bias: Option<(f64, f64)>,
    ) -> Vec<GeoResult> {
        let normalized = index_key(prefix);
        let matches = self.text_index.prefix_search(&normalized);
        let take = if bias.is_some() {
            limit.saturating_mul(8).max(limit)
        } else {
            limit
        };

        let mut seen = std::collections::HashSet::new();
        let mut results: Vec<GeoResult> = matches
            .into_iter()
            .filter_map(|(_, id)| {
                if !seen.insert(id) {
                    return None;
                }
                let rec = self.records.get(id as usize)?;
                Some(GeoResult {
                    address: rec.address.clone(),
                    lat: rec.lat,
                    lon: rec.lon,
                    confidence: 1.0,
                    match_type: MatchType::Exact,
                })
            })
            .take(take)
            .collect();

        if let Some((lon, lat)) = bias {
            results.sort_by(|a, b| {
                let da = (a.lon - lon).hypot(a.lat - lat);
                let db = (b.lon - lon).hypot(b.lat - lat);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        results.truncate(limit);
        results
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
    fn forward_by_street_name() {
        // Querying by street name (no house number) must match — this is the
        // common case and previously returned nothing for number-led addresses.
        let gc = build_test_geocoder();
        let results = gc.forward("main street");
        assert!(
            results.iter().any(|r| (r.lat - 39.7817).abs() < 0.001),
            "expected Main Street, Springfield in results"
        );
        // A record must not be returned more than once across its index keys.
        let mut lats: Vec<_> = results.iter().map(|r| (r.lat * 1e4) as i64).collect();
        lats.sort_unstable();
        let deduped = {
            let mut l = lats.clone();
            l.dedup();
            l
        };
        assert_eq!(lats, deduped, "results contain duplicate records");
    }

    #[test]
    fn forward_by_city() {
        let gc = build_test_geocoder();
        let results = gc.forward("portland");
        assert_eq!(results.len(), 1);
        assert!((results[0].lat - 45.5152).abs() < 0.001);
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
    fn forward_matches_directional_and_unit() {
        let gc = build_test_geocoder();
        let results = gc.forward("123 North Main Street Apt 4");
        assert_eq!(results.len(), 1);
        assert!((results[0].lat - 39.7817).abs() < 0.001);
    }

    #[test]
    fn autocomplete_spatial_bias_ranks_nearer_first() {
        let gc = build_test_geocoder();
        // "1" matches 123 Main St Springfield and 100 Broadway Portland
        let near_denver = gc.autocomplete_biased("main", 10, Some((-104.99, 39.74)));
        assert!(near_denver.len() >= 2);
        assert_eq!(near_denver[0].address.city.as_deref(), Some("Denver"));
        let near_springfield = gc.autocomplete_biased("main", 10, Some((-89.65, 39.78)));
        assert_eq!(
            near_springfield[0].address.city.as_deref(),
            Some("Springfield")
        );
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
