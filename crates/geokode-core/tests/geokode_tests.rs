// Comprehensive integration tests for geokode-core.

use geokode_core::address::*;
use geokode_core::fuzzy::*;
use geokode_core::geocode::*;
use geokode_core::spatial::*;

// ═══════════════════════════════════════════════════════════════════════════
// Address parsing tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_parse_simple_street() {
    let addr = parse_address("Baker Street");
    assert_eq!(addr.street, Some("Baker Street".into()));
    assert!(addr.city.is_none());
}

#[test]
fn test_parse_street_city() {
    let addr = parse_address("Baker Street, London");
    assert_eq!(addr.street, Some("Baker Street".into()));
    assert_eq!(addr.city, Some("London".into()));
}

#[test]
fn test_parse_full_address() {
    let addr = parse_address("221B Baker Street, London, England, UK");
    assert_eq!(addr.house_number, Some("221".into()));
    assert_eq!(addr.street.as_deref(), Some("B Baker Street"));
    assert_eq!(addr.city, Some("London".into()));
    assert_eq!(addr.country, Some("UK".into()));
}

#[test]
fn test_parse_with_house_number() {
    let addr = parse_address("10 Downing St, Westminster, London");
    assert_eq!(addr.house_number, Some("10".into()));
    assert_eq!(addr.street, Some("Downing St".into()));
    assert_eq!(addr.city, Some("Westminster".into()));
}

#[test]
fn test_normalize_street() {
    assert_eq!(normalize_street("Main Street"), "main st");
    assert_eq!(normalize_street("Park Avenue"), "park ave");
    assert_eq!(normalize_street("Sunset Boulevard"), "sunset blvd");
    assert_eq!(normalize_street("Ocean Drive"), "ocean dr");
}

#[test]
fn test_address_serialization() {
    let addr = parse_address("123 Test Rd, Springfield, IL");
    let json = serde_json::to_string(&addr).unwrap();
    let back: Address = serde_json::from_str(&json).unwrap();
    assert_eq!(back.full, addr.full);
    assert_eq!(back.city, addr.city);
}

// ═══════════════════════════════════════════════════════════════════════════
// Spatial index tests
// ═══════════════════════════════════════════════════════════════════════════

fn sample_spatial_records() -> Vec<SpatialRecord> {
    vec![
        SpatialRecord {
            lat: 51.507,
            lon: -0.128,
            id: 0,
        },
        SpatialRecord {
            lat: 48.857,
            lon: 2.352,
            id: 1,
        },
        SpatialRecord {
            lat: 40.713,
            lon: -74.006,
            id: 2,
        },
        SpatialRecord {
            lat: 35.682,
            lon: 139.759,
            id: 3,
        },
        SpatialRecord {
            lat: -33.868,
            lon: 151.209,
            id: 4,
        },
    ]
}

#[test]
fn test_spatial_index_build() {
    let idx = SpatialIndex::build(sample_spatial_records());
    assert_eq!(idx.len(), 5);
    assert!(!idx.is_empty());
}

#[test]
fn test_spatial_index_nearest_single() {
    let idx = SpatialIndex::build(sample_spatial_records());
    // Query near London
    let results = idx.nearest(-0.1, 51.5, 1);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, 0); // London
}

#[test]
fn test_spatial_index_nearest_k() {
    let idx = SpatialIndex::build(sample_spatial_records());
    let results = idx.nearest(0.0, 50.0, 3);
    assert_eq!(results.len(), 3);
    // First result should be London (closest to 0°E, 50°N)
    assert_eq!(results[0].id, 0);
}

#[test]
fn test_spatial_index_within_bbox() {
    let idx = SpatialIndex::build(sample_spatial_records());
    // Bbox covering Europe
    let results = idx.within_bbox(-10.0, 40.0, 10.0, 55.0);
    // Should contain London (51.5, -0.1) and Paris (48.8, 2.3)
    let ids: Vec<u64> = results.iter().map(|r| r.id).collect();
    assert!(ids.contains(&0)); // London
    assert!(ids.contains(&1)); // Paris
    assert!(!ids.contains(&2)); // NYC is not in Europe
}

#[test]
fn test_spatial_index_empty() {
    let idx = SpatialIndex::build(Vec::new());
    assert!(idx.is_empty());
    assert_eq!(idx.len(), 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Fuzzy matching tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_fuzzy_exact_match() {
    let mut searcher = FuzzySearcher::new(FuzzyConfig::default());
    searcher.add_entry("Main Street".into(), 1);
    searcher.add_entry("Park Avenue".into(), 2);

    let results = searcher.search("Main Street", 5);
    assert!(!results.is_empty());
    assert_eq!(results[0].record_id, 1);
    assert_eq!(results[0].distance, 0);
}

#[test]
fn test_fuzzy_typo_match() {
    let mut searcher = FuzzySearcher::new(FuzzyConfig {
        max_distance: 2,
        phonetic_fallback: false,
        min_score: 0.5,
    });
    searcher.add_entry("Baker Street".into(), 1);
    searcher.add_entry("Park Avenue".into(), 2);

    let results = searcher.search("Bker Street", 5); // missing 'a'
    assert!(!results.is_empty());
    assert_eq!(results[0].record_id, 1);
}

#[test]
fn test_fuzzy_no_match_beyond_threshold() {
    let mut searcher = FuzzySearcher::new(FuzzyConfig {
        max_distance: 1,
        phonetic_fallback: false,
        min_score: 0.8,
    });
    searcher.add_entry("Oxford Street".into(), 1);

    let results = searcher.search("completely different", 5);
    assert!(results.is_empty());
}

#[test]
fn test_fuzzy_respects_limit() {
    let mut searcher = FuzzySearcher::new(FuzzyConfig {
        max_distance: 3,
        phonetic_fallback: false,
        min_score: 0.3,
    });
    for i in 0..100 {
        searcher.add_entry(format!("Street {i}"), i);
    }

    let results = searcher.search("Street 5", 3);
    assert!(results.len() <= 3);
}

// ═══════════════════════════════════════════════════════════════════════════
// Geocoder integration tests
// ═══════════════════════════════════════════════════════════════════════════

fn build_test_geocoder() -> Geocoder {
    let mut builder = GeocoderBuilder::new();
    builder.add(
        parse_address("10 Downing Street, London, UK"),
        51.503,
        -0.127,
    );
    builder.add(
        parse_address("1600 Pennsylvania Avenue, Washington, DC, USA"),
        38.897,
        -77.036,
    );
    builder.add(
        parse_address("55 Rue du Faubourg, Paris, France"),
        48.870,
        2.316,
    );
    builder.build().expect("failed to build geocoder")
}

#[test]
fn test_forward_geocode_match() {
    let geocoder = build_test_geocoder();
    // normalize_street converts the full address then prefix_search is used
    let results = geocoder.forward("10 downing st");
    assert!(!results.is_empty());
    assert!((results[0].lat - 51.503).abs() < 0.01);
}

#[test]
fn test_forward_geocode_no_match() {
    let geocoder = build_test_geocoder();
    let results = geocoder.forward("xyznonexistent");
    assert!(results.is_empty());
}

#[test]
fn test_reverse_geocode() {
    let geocoder = build_test_geocoder();
    // Query near Downing Street (lon, lat)
    let results = geocoder.reverse(-0.127, 51.503, 1);
    assert!(!results.is_empty());
    // Nearest result should be London record
    assert!((results[0].lat - 51.503).abs() < 0.01);
}

#[test]
fn test_reverse_geocode_far_away() {
    let geocoder = build_test_geocoder();
    // Antarctica (lon, lat) — should still return nearest
    let results = geocoder.reverse(0.0, -75.0, 1);
    assert_eq!(results.len(), 1); // returns nearest regardless of distance
}

// ═══════════════════════════════════════════════════════════════════════════
// Fuzzy forward-geocoding fallback tests
// ═══════════════════════════════════════════════════════════════════════════

fn build_monaco_geocoder() -> Geocoder {
    let mut builder = GeocoderBuilder::new();
    builder.add(
        parse_address("1 Avenue Grimaldi, Monaco, MC"),
        43.7355,
        7.4197,
    );
    builder.add(
        parse_address("2 Boulevard des Moulins, Monaco, MC"),
        43.7396,
        7.4278,
    );
    builder.add(
        parse_address("6 Avenue de la Costa, Monaco, MC"),
        43.7402,
        7.4266,
    );
    builder.build().expect("failed to build geocoder")
}

#[test]
fn test_forward_fuzzy_fallback_on_typo() {
    let geocoder = build_monaco_geocoder();
    let results = geocoder.forward("Avenue Grimadli");
    assert!(!results.is_empty(), "typo query should fall back to fuzzy");
    assert!(results[0].address.full.contains("Grimaldi"));
    assert!((results[0].lat - 43.7355).abs() < 0.001);
    assert_eq!(results[0].match_type, MatchType::Fuzzy);
    assert!(results[0].confidence < 1.0);
}

#[test]
fn test_forward_fuzzy_flag_serializes() {
    let geocoder = build_monaco_geocoder();
    let results = geocoder.forward("Avenue Grimadli");
    let json = serde_json::to_string(&results[0]).unwrap();
    assert!(json.contains(r#""match_type":"fuzzy""#), "got {json}");
}

#[test]
fn test_forward_exact_is_not_flagged_fuzzy() {
    let geocoder = build_monaco_geocoder();
    let results = geocoder.forward("Avenue Grimaldi");
    assert!(!results.is_empty());
    assert!(results.iter().all(|r| r.match_type == MatchType::Exact));
    assert!(results.iter().all(|r| r.confidence == 1.0));
}

#[test]
fn test_forward_garbage_returns_empty() {
    let geocoder = build_monaco_geocoder();
    assert!(geocoder.forward("zzqqwx flurbleglop").is_empty());
    assert!(geocoder.forward("xyznonexistent").is_empty());
}

#[test]
fn test_forward_fuzzy_results_are_bounded() {
    // Eight records share the street key, so all of them match the typo.
    let mut builder = GeocoderBuilder::new();
    for i in 0..8 {
        builder.add(
            parse_address(&format!("{i} Avenue Grimaldi, Town{i}, MC")),
            43.73 + f64::from(i) * 0.001,
            7.41,
        );
    }
    let geocoder = builder.build().unwrap();

    let results = geocoder.forward("Avenue Grimadli");
    assert_eq!(results.len(), 5, "fuzzy fallback must cap results");
    assert!(results.iter().all(|r| r.match_type == MatchType::Fuzzy));
}
