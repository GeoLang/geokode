//! Forward and reverse geocoding operations.

use crate::address::{Address, GeoResult, MatchType, directionals_in, normalize_for_match};
use crate::fuzzy::{FuzzyConfig, FuzzySearcher};
use crate::index::{TextIndex, TextIndexBuilder};
use crate::spatial::{SpatialIndex, SpatialRecord};

/// Most fuzzy fallback results returned for one query.
const FUZZY_LIMIT: usize = 5;

/// A record is indexed under up to 6 keys, so over-fetch before deduping by
/// record id, otherwise a single street can crowd out the other candidates.
const FUZZY_CANDIDATES: usize = FUZZY_LIMIT * 6;

fn fuzzy_config() -> FuzzyConfig {
    FuzzyConfig {
        max_distance: 2,
        // soundex over a whole address string collides too easily, so garbage
        // queries would come back with unrelated addresses. Edit distance only.
        phonetic_fallback: false,
        min_score: 0.6,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coverage {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
}

impl Coverage {
    pub fn contains(&self, lon: f64, lat: f64) -> bool {
        lon >= self.min_lon && lon <= self.max_lon && lat >= self.min_lat && lat <= self.max_lat
    }
}

fn record_coverage(records: &[AddressRecord], spatial_index: &SpatialIndex) -> Option<Coverage> {
    let extent = record_extent(records)?;
    // the widest gap the data already tolerates between two addresses
    let pad = largest_neighbor_gap(records, spatial_index);
    Some(Coverage {
        min_lon: extent.min_lon - pad,
        min_lat: extent.min_lat - pad,
        max_lon: extent.max_lon + pad,
        max_lat: extent.max_lat + pad,
    })
}

fn largest_neighbor_gap(records: &[AddressRecord], spatial_index: &SpatialIndex) -> f64 {
    records
        .iter()
        .enumerate()
        .filter_map(|(i, rec)| {
            let neighbor = spatial_index
                .nearest(rec.lon, rec.lat, 2)
                .into_iter()
                .find(|sr| sr.id != i as u64)?;
            Some((neighbor.lon - rec.lon).hypot(neighbor.lat - rec.lat))
        })
        .fold(0.0, f64::max)
}

fn record_extent(records: &[AddressRecord]) -> Option<Coverage> {
    let first = records.first()?;
    let mut extent = Coverage {
        min_lon: first.lon,
        min_lat: first.lat,
        max_lon: first.lon,
        max_lat: first.lat,
    };
    for rec in &records[1..] {
        extent.min_lon = extent.min_lon.min(rec.lon);
        extent.min_lat = extent.min_lat.min(rec.lat);
        extent.max_lon = extent.max_lon.max(rec.lon);
        extent.max_lat = extent.max_lat.max(rec.lat);
    }
    Some(extent)
}

/// A geocoding engine combining text and spatial indexes.
pub struct Geocoder {
    text_index: TextIndex,
    spatial_index: SpatialIndex,
    fuzzy: FuzzySearcher,
    records: Vec<AddressRecord>,
    coverage: Option<Coverage>,
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
    declared_coverage: Option<Coverage>,
}

impl GeocoderBuilder {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            declared_coverage: None,
        }
    }

    /// Add an address record.
    pub fn add(&mut self, address: Address, lat: f64, lon: f64) {
        self.records.push(AddressRecord { address, lat, lon });
    }

    pub fn set_coverage(&mut self, coverage: Coverage) {
        self.declared_coverage = Some(coverage);
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
                // OSM puts the place name before the house number in `full`
                if let Some(house_number) = &rec.address.house_number {
                    keys.push(index_key(&format!("{house_number} {street}")));
                    if let Some(city) = &rec.address.city {
                        keys.push(index_key(&format!("{house_number} {street} {city}")));
                    }
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
        let coverage = self
            .declared_coverage
            .or_else(|| record_coverage(&self.records, &spatial_index));

        Ok(Geocoder {
            text_index,
            spatial_index,
            fuzzy,
            records: self.records,
            coverage,
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

fn directional_rank(query_directionals: &[&str], address: &Address) -> u8 {
    let street = address.street.as_deref().unwrap_or(&address.full);
    let record_directionals = directionals_in(street);
    if record_directionals.is_empty() {
        return 1;
    }
    if record_directionals
        .iter()
        .any(|d| query_directionals.contains(d))
    {
        2
    } else {
        0
    }
}

impl Geocoder {
    /// Forward geocode: text query → coordinates. Falls back to fuzzy matching
    /// when the text index has no exact or prefix hit.
    pub fn forward(&self, query: &str) -> Vec<GeoResult> {
        let normalized = index_key(query);
        let matches = self.text_index.prefix_search(&normalized);

        // A record can be indexed under several keys, so dedup by record id.
        let mut seen = std::collections::HashSet::new();
        let mut exact: Vec<GeoResult> = matches
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

        // index_key drops directionals, so West and East share a key
        let query_directionals = directionals_in(query);
        if !query_directionals.is_empty() {
            exact.sort_by_key(|r| {
                std::cmp::Reverse(directional_rank(&query_directionals, &r.address))
            });
        }

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

    /// Reverse geocode: coordinates → nearest address within the coverage.
    pub fn reverse(&self, lon: f64, lat: f64, k: usize) -> Vec<GeoResult> {
        if !self.coverage.is_some_and(|c| c.contains(lon, lat)) {
            return Vec::new();
        }
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

    pub fn coverage(&self) -> Option<Coverage> {
        self.coverage
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
    fn reverse_geocode_outside_coverage_is_empty() {
        let gc = build_test_geocoder();
        // Monaco, thousands of kilometres from the extent of the records
        let results = gc.reverse(7.4197, 43.7355, 5);
        assert!(results.is_empty(), "expected no results, got {results:?}");
    }

    #[test]
    fn reverse_geocode_next_to_record() {
        let gc = build_test_geocoder();
        // ~850 m west of the Springfield record, inside the extent
        let results = gc.reverse(-89.66, 39.7817, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].address.city.as_deref(), Some("Springfield"));
        assert!(results[0].confidence > 0.9);
    }

    #[test]
    fn reverse_geocode_uses_declared_coverage() {
        let mut builder = GeocoderBuilder::new();
        builder.add(
            parse_address("1 Avenue Grimaldi, Monaco, MC"),
            43.7355,
            7.4197,
        );
        builder.set_coverage(Coverage {
            min_lon: 7.36,
            min_lat: 43.71,
            max_lon: 7.47,
            max_lat: 43.78,
        });
        let gc = builder.build().unwrap();

        // inside the declared box, off the only record
        let inside = gc.reverse(7.44, 43.75, 1);
        assert_eq!(inside.len(), 1);
        let outside = gc.reverse(7.6, 43.75, 1);
        assert!(outside.is_empty());
    }

    fn build_queen_street_geocoder() -> Geocoder {
        let mut builder = GeocoderBuilder::new();
        builder.add(
            parse_address("100 Queen Street East, Toronto, ON"),
            43.6537,
            -79.3740,
        );
        builder.add(
            parse_address("100 Queen Street West, Brampton, ON"),
            43.6841,
            -79.7622,
        );
        builder.add(
            parse_address("100 North Queen Street, Toronto, ON"),
            43.6215,
            -79.5457,
        );
        builder.add(
            Address {
                house_number: Some("100".to_string()),
                street: Some("Queen Street West".to_string()),
                city: Some("Toronto".to_string()),
                state: None,
                postcode: Some("M5H 2N2".to_string()),
                country: None,
                full: "Toronto City Hall, 100, Queen Street West, Toronto, M5H 2N2".to_string(),
            },
            43.6534,
            -79.3841,
        );
        builder.build().unwrap()
    }

    #[test]
    fn forward_finds_named_place_by_house_number() {
        let gc = build_queen_street_geocoder();
        let results = gc.forward("100 Queen Street West");
        assert!(
            results
                .iter()
                .any(|r| r.address.full.contains("Toronto City Hall")),
            "got {:?}",
            results.iter().map(|r| &r.address.full).collect::<Vec<_>>()
        );
    }

    #[test]
    fn forward_ranks_the_queried_directional_first() {
        let gc = build_queen_street_geocoder();

        let west = gc.forward("100 Queen St W");
        assert!(
            west[0].address.full.contains("Queen Street West"),
            "got {:?}",
            west[0].address.full
        );
        let west_order: Vec<&str> = west.iter().map(|r| r.address.full.as_str()).collect();
        let last_west = west_order
            .iter()
            .rposition(|f| f.contains("Queen Street West"))
            .unwrap();
        let first_other = west_order
            .iter()
            .position(|f| f.contains("Queen Street East") || f.contains("North Queen Street"))
            .unwrap();
        assert!(last_west < first_other, "got {west_order:?}");

        let east = gc.forward("100 Queen St E");
        assert!(
            east[0].address.full.contains("Queen Street East"),
            "got {:?}",
            east[0].address.full
        );
    }

    #[test]
    fn reverse_just_outside_the_extent_is_covered() {
        let gc = build_queen_street_geocoder();
        // the records span 43.62–43.68 N, and this sits just south of that
        let results = gc.reverse(-79.3740, 43.60, 1);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn reverse_far_beyond_the_padded_extent_is_empty() {
        let gc = build_queen_street_geocoder();
        let results = gc.reverse(-79.3740, 40.0, 1);
        assert!(results.is_empty(), "got {results:?}");
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
