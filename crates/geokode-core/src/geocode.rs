//! Forward and reverse geocoding operations.

use crate::address::{
    Address, FeatureKind, GeoResult, MatchType, Place, PlaceClass, directionals_in,
    normalize_for_match,
};
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

fn record_coverage(records: &[FeatureRecord], spatial_index: &SpatialIndex) -> Option<Coverage> {
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

fn largest_neighbor_gap(records: &[FeatureRecord], spatial_index: &SpatialIndex) -> f64 {
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

fn record_extent(records: &[FeatureRecord]) -> Option<Coverage> {
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
    records: Vec<FeatureRecord>,
    coverage: Option<Coverage>,
}

/// Internal record stored in the geocoder: a street address, or a place when
/// `place` is set.
#[derive(Debug, Clone)]
pub struct FeatureRecord {
    pub address: Address,
    pub lat: f64,
    pub lon: f64,
    pub place: Option<Place>,
}

impl FeatureRecord {
    fn kind(&self) -> FeatureKind {
        if self.place.is_some() {
            FeatureKind::Place
        } else {
            FeatureKind::Address
        }
    }

    fn result(&self, confidence: f64, match_type: MatchType) -> GeoResult {
        GeoResult {
            address: self.address.clone(),
            lat: self.lat,
            lon: self.lon,
            confidence,
            match_type,
            kind: self.kind(),
        }
    }
}

/// Builder for constructing a Geocoder from address data.
pub struct GeocoderBuilder {
    records: Vec<FeatureRecord>,
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
        self.records.push(FeatureRecord {
            address,
            lat,
            lon,
            place: None,
        });
    }

    /// Add a settlement or administrative area.
    pub fn add_place(&mut self, place: Place, lat: f64, lon: f64) {
        self.records.push(FeatureRecord {
            address: place.as_address(),
            lat,
            lon,
            place: Some(place),
        });
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
            if let Some(place) = &rec.place {
                for qualifier in [place.state.as_deref(), place.country.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    keys.push(index_key(&format!("{} {qualifier}", place.name)));
                }
            }
            for key in keys {
                fuzzy.add_entry(key.clone(), i as u64);
                text_builder.insert(format!("{key}{KEY_ID_SEPARATOR}{i}"), i as u64);
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

// the unit separator that keeps FST keys unique per record
const KEY_ID_SEPARATOR: char = '\u{1f}';

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

const PARTIAL_QUERY_CONFIDENCE: f64 = 0.6;

// a record naming neither a state nor a country cannot contradict anything
fn contradicts(address: &Address, part: &str) -> bool {
    let known: Vec<String> = [address.state.as_deref(), address.country.as_deref()]
        .into_iter()
        .flatten()
        .map(normalize_for_match)
        .collect();
    if known.is_empty() {
        return false;
    }
    !known.iter().any(|value| value == part)
}

fn starts_with_house_number(query: &str) -> bool {
    query.trim_start().starts_with(|c: char| c.is_ascii_digit())
}

// directional the query asked for, place before street, largest settlement
type RankKey = (u8, u8, u8, std::cmp::Reverse<u64>);

impl Geocoder {
    fn rank_key(&self, id: usize, query_directionals: &[&str], numbered: bool) -> RankKey {
        let Some(record) = self.records.get(id) else {
            return (u8::MAX, u8::MAX, u8::MAX, std::cmp::Reverse(0));
        };
        let directional = if query_directionals.is_empty() {
            0
        } else {
            2 - directional_rank(query_directionals, &record.address)
        };
        let group = match (numbered, record.place.is_some()) {
            (false, true) | (true, false) => 0,
            (false, false) | (true, true) => 1,
        };
        let (class, population) = match &record.place {
            Some(place) => (place.class as u8, place.population.unwrap_or(0)),
            None => (PlaceClass::Other as u8 + 1, 0),
        };
        (directional, group, class, std::cmp::Reverse(population))
    }

    /// Forward geocode: text query → coordinates. Falls back to fuzzy matching
    /// when the text index has no exact or prefix hit, then to the query's
    /// leading part, since OSM rarely tags a town with the province a caller
    /// names it by.
    pub fn forward(&self, query: &str) -> Vec<GeoResult> {
        let results = self.search(query);
        if !results.is_empty() {
            return results;
        }
        self.search_leading_part(query)
    }

    fn search_leading_part(&self, query: &str) -> Vec<GeoResult> {
        let mut parts = query.split(',').map(str::trim).filter(|p| !p.is_empty());
        let Some(head) = parts.next() else {
            return Vec::new();
        };
        let dropped: Vec<String> = parts.map(normalize_for_match).collect();
        if dropped.is_empty() {
            return Vec::new();
        }
        self.search(head)
            .into_iter()
            .filter(|result| {
                dropped
                    .iter()
                    .all(|part| !contradicts(&result.address, part))
            })
            .map(|mut result| {
                result.confidence = result.confidence.min(PARTIAL_QUERY_CONFIDENCE);
                result
            })
            .collect()
    }

    fn search(&self, query: &str) -> Vec<GeoResult> {
        let normalized = index_key(query);
        let matches = self.text_index.prefix_search(&normalized);

        // A record can be indexed under several keys, so dedup by record id.
        // a record carries several keys, the query may be the whole of one
        let mut whole_key = std::collections::HashMap::new();
        let mut order = Vec::new();
        for (key, id) in matches {
            let matched_whole = key.split(KEY_ID_SEPARATOR).next() == Some(normalized.as_str());
            match whole_key.entry(id) {
                std::collections::hash_map::Entry::Occupied(mut seen) => {
                    *seen.get_mut() |= matched_whole;
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(matched_whole);
                    order.push(id);
                }
            }
        }
        let mut exact: Vec<(usize, GeoResult)> = order
            .into_iter()
            .filter_map(|id| {
                let rec = self.records.get(id as usize)?;
                let match_type = if whole_key[&id] {
                    MatchType::Exact
                } else {
                    MatchType::Prefix
                };
                Some((id as usize, rec.result(1.0, match_type)))
            })
            .collect();

        // index_key drops directionals, so West and East share a key
        let query_directionals = directionals_in(query);
        let numbered = starts_with_house_number(query);
        exact.sort_by_key(|(id, _)| self.rank_key(*id, &query_directionals, numbered));

        if !exact.is_empty() {
            return exact.into_iter().map(|(_, result)| result).collect();
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
                Some(rec.result(m.score, MatchType::Fuzzy))
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
                Some(rec.result(confidence, MatchType::Exact))
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
                Some(rec.result(1.0, MatchType::Exact))
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
    pub fn records(&self) -> &[FeatureRecord] {
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

    const JASPER_ALBERTA: (f64, f64) = (52.875, -118.082);

    fn place(name: &str, class: PlaceClass, population: Option<u64>, state: Option<&str>) -> Place {
        Place {
            name: name.to_string(),
            class,
            population,
            state: state.map(ToString::to_string),
            country: None,
        }
    }

    fn build_jasper_geocoder() -> Geocoder {
        let mut builder = GeocoderBuilder::new();
        builder.add(
            parse_address("2 Jasper Avenue, Toronto, ON"),
            43.6835,
            -79.4830,
        );
        builder.add_place(
            place("Jasper", PlaceClass::Town, Some(4738), None),
            JASPER_ALBERTA.0,
            JASPER_ALBERTA.1,
        );
        builder.build().unwrap()
    }

    #[test]
    fn a_town_outranks_a_street_that_starts_with_its_name() {
        let gc = build_jasper_geocoder();
        let results = gc.forward("jasper");
        assert_eq!(results[0].kind, FeatureKind::Place);
        assert!((results[0].lat - JASPER_ALBERTA.0).abs() < 0.001);
    }

    #[test]
    fn a_whole_street_name_is_an_exact_match() {
        let gc = build_jasper_geocoder();
        let results = gc.forward("jasper avenue");
        assert_eq!(results[0].match_type, MatchType::Exact);
    }

    #[test]
    fn a_name_that_only_starts_a_street_is_a_prefix_match() {
        let mut builder = GeocoderBuilder::new();
        builder.add(
            parse_address("2 Jasper Avenue, Toronto, ON"),
            43.6835,
            -79.4830,
        );
        let gc = builder.build().unwrap();
        let results = gc.forward("jasper");
        assert_eq!(results[0].match_type, MatchType::Prefix);
    }

    #[test]
    fn a_house_number_still_puts_the_street_first() {
        let gc = build_jasper_geocoder();
        let results = gc.forward("2 jasper avenue");
        assert_eq!(results[0].kind, FeatureKind::Address);
        assert!((results[0].lat - 43.6835).abs() < 0.001);
    }

    #[test]
    fn a_province_the_town_does_not_carry_still_finds_it() {
        let gc = build_jasper_geocoder();
        let results = gc.forward("Jasper, Alberta");
        assert_eq!(results[0].kind, FeatureKind::Place);
        assert!((results[0].lat - JASPER_ALBERTA.0).abs() < 0.001);
        assert!(
            results[0].confidence <= PARTIAL_QUERY_CONFIDENCE,
            "a hit that ignored part of the query must say so"
        );
    }

    #[test]
    fn a_qualifier_the_record_contradicts_drops_it() {
        let mut builder = GeocoderBuilder::new();
        builder.add_place(
            place("Jasper", PlaceClass::Town, None, Some("Alberta")),
            JASPER_ALBERTA.0,
            JASPER_ALBERTA.1,
        );
        let gc = builder.build().unwrap();
        assert!(gc.forward("Jasper, Texas").is_empty());
        assert!(!gc.forward("Jasper, Alberta").is_empty());
    }

    #[test]
    fn the_larger_settlement_of_two_sorts_first() {
        let mut builder = GeocoderBuilder::new();
        builder.add_place(
            place("Springfield", PlaceClass::Village, Some(200), None),
            1.0,
            1.0,
        );
        builder.add_place(
            place("Springfield", PlaceClass::City, Some(116_000), None),
            2.0,
            2.0,
        );
        let gc = builder.build().unwrap();
        let results = gc.forward("springfield");
        assert!((results[0].lat - 2.0).abs() < 0.001, "city before village");
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
