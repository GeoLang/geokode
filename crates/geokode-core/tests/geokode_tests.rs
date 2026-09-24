use geokode_core::address::*;
use geokode_core::geocode::{Geocoder, OpenError, Point};
use geokode_core::index::*;
use tempfile::TempDir;

struct TestIndex {
    _directory: TempDir,
    geocoder: Geocoder,
}

impl std::ops::Deref for TestIndex {
    type Target = Geocoder;

    fn deref(&self) -> &Geocoder {
        &self.geocoder
    }
}

fn build(records: Vec<Record>, coverage: Option<Coverage>) -> TestIndex {
    let directory = tempfile::tempdir().unwrap();
    let mut writer = IndexWriter::create(directory.path()).unwrap();
    if let Some(coverage) = coverage {
        writer.add_coverage(coverage);
    }
    for record in records {
        let names = [
            record.address.city.clone(),
            record.address.state.clone(),
            record.address.country.clone(),
        ];
        let anchored = names[1].is_some() || names[2].is_some();
        let areas = names
            .iter()
            .flatten()
            .map(|name| writer.area_named(name))
            .collect();
        writer
            .add(PreparedRecord::new(record), areas, anchored)
            .unwrap();
    }
    writer.finish().unwrap();
    let geocoder = Geocoder::open(directory.path()).unwrap();
    TestIndex {
        _directory: directory,
        geocoder,
    }
}

fn address(text: &str, lat: f64, lon: f64) -> Record {
    let mut record = Record::new(FeatureKind::Address, lon, lat);
    record.address = parse_address(text);
    record
}

fn place(name: &str, class: PlaceClass, population: Option<u64>, lat: f64, lon: f64) -> Record {
    let mut record = Record::new(FeatureKind::Place, lon, lat);
    record.name = Some(name.to_string());
    record.place = Some(class);
    record.population = population;
    record.address.city = Some(name.to_string());
    record
}

fn street(name: &str, city: &str, lat: f64, lon: f64) -> Record {
    let mut record = Record::new(FeatureKind::Street, lon, lat);
    record.name = Some(name.to_string());
    record.address.street = Some(name.to_string());
    record.address.city = Some(city.to_string());
    record
}

fn poi(name: &str, lat: f64, lon: f64) -> Record {
    let mut record = Record::new(FeatureKind::Poi, lon, lat);
    record.name = Some(name.to_string());
    record.osm = Some((OsmType::Node, 42));
    record.osm_key = Some("amenity".to_string());
    record.osm_value = Some("cafe".to_string());
    record
}

fn covering(min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> Option<Coverage> {
    Some(Coverage {
        min_lon,
        min_lat,
        max_lon,
        max_lat,
    })
}

const JASPER_ALBERTA: (f64, f64) = (52.875, -118.082);

fn jasper() -> TestIndex {
    build(
        vec![
            address("2 Jasper Avenue, Toronto, ON", 43.6835, -79.4830),
            street("Jasper Avenue", "Toronto", 43.6836, -79.4831),
            place(
                "Jasper",
                PlaceClass::Town,
                Some(4738),
                JASPER_ALBERTA.0,
                JASPER_ALBERTA.1,
            ),
        ],
        None,
    )
}

#[test]
fn parse_street_city() {
    let addr = parse_address("Baker Street, London");
    assert_eq!(addr.street, Some("Baker Street".into()));
    assert_eq!(addr.city, Some("London".into()));
}

#[test]
fn parse_full_address() {
    let addr = parse_address("221B Baker Street, London, England, UK");
    assert_eq!(addr.house_number, Some("221".into()));
    assert_eq!(addr.street.as_deref(), Some("B Baker Street"));
    assert_eq!(addr.city, Some("London".into()));
    assert_eq!(addr.country, Some("UK".into()));
}

#[test]
fn a_town_outranks_a_street_that_starts_with_its_name() {
    let results = jasper().forward("jasper", 10, None);
    assert_eq!(results[0].kind, FeatureKind::Place);
    assert!((results[0].lat - JASPER_ALBERTA.0).abs() < 0.001);
}

#[test]
fn a_whole_street_name_is_an_exact_match() {
    let results = jasper().forward("jasper avenue", 10, None);
    assert_eq!(results[0].kind, FeatureKind::Street);
    assert_eq!(results[0].match_type, MatchType::Exact);
}

#[test]
fn a_name_that_only_starts_a_street_is_a_prefix_match() {
    let index = build(
        vec![street("Jasper Avenue", "Toronto", 43.6835, -79.4830)],
        None,
    );
    let results = index.forward("jasper", 10, None);
    assert_eq!(results[0].match_type, MatchType::Prefix);
}

#[test]
fn a_house_number_puts_the_address_first() {
    let results = jasper().forward("2 jasper avenue", 10, None);
    assert_eq!(results[0].kind, FeatureKind::Address);
    assert!((results[0].lat - 43.6835).abs() < 0.001);
}

#[test]
fn a_province_the_town_does_not_carry_still_finds_it() {
    let results = jasper().forward("Jasper, Alberta", 10, None);
    assert_eq!(results[0].kind, FeatureKind::Place);
    assert!((results[0].lat - JASPER_ALBERTA.0).abs() < 0.001);
    assert!(
        results[0].confidence <= 0.6,
        "a hit that ignored part of the query must say so"
    );
}

#[test]
fn a_qualifier_the_record_contradicts_drops_it() {
    let mut town = place(
        "Jasper",
        PlaceClass::Town,
        None,
        JASPER_ALBERTA.0,
        JASPER_ALBERTA.1,
    );
    town.address.state = Some("Alberta".to_string());
    let index = build(vec![town], None);
    assert!(index.forward("Jasper, Texas", 10, None).is_empty());
    let found = index.forward("Jasper, Alberta", 10, None);
    assert_eq!(found[0].confidence, 1.0);
}

#[test]
fn a_trailing_area_name_filters_without_a_comma() {
    let mut illinois = place("Springfield", PlaceClass::City, Some(114_000), 39.8, -89.6);
    illinois.address.state = Some("Illinois".to_string());
    let mut missouri = place("Springfield", PlaceClass::City, Some(169_000), 37.2, -93.3);
    missouri.address.state = Some("Missouri".to_string());
    let index = build(vec![illinois, missouri], None);

    let results = index.forward("Springfield Illinois", 10, None);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].address.state.as_deref(), Some("Illinois"));
    let with_comma = index.forward("Springfield, Missouri", 10, None);
    assert_eq!(with_comma.len(), 1);
    assert_eq!(with_comma[0].address.state.as_deref(), Some("Missouri"));
}

#[test]
fn the_larger_settlement_of_two_sorts_first() {
    let index = build(
        vec![
            place("Springfield", PlaceClass::Village, Some(200), 1.0, 1.0),
            place("Springfield", PlaceClass::City, Some(116_000), 2.0, 2.0),
            place("Springfield", PlaceClass::City, Some(3_000), 3.0, 3.0),
        ],
        None,
    );
    let lats: Vec<f64> = index
        .forward("springfield", 10, None)
        .iter()
        .map(|r| r.lat)
        .collect();
    assert_eq!(lats, vec![2.0, 3.0, 1.0]);
}

#[test]
fn a_city_beats_a_same_named_street_and_poi() {
    let index = build(
        vec![
            poi("Lausanne", 46.0, 6.0),
            street("Lausanne", "Geneva", 46.2, 6.1),
            place("Lausanne", PlaceClass::City, Some(140_000), 46.52, 6.63),
        ],
        None,
    );
    let kinds: Vec<FeatureKind> = index
        .forward("Lausanne", 10, None)
        .iter()
        .map(|r| r.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![FeatureKind::Place, FeatureKind::Street, FeatureKind::Poi]
    );
}

#[test]
fn a_wikidata_tag_breaks_a_tie() {
    let mut notable = poi("Grand Hotel", 10.0, 10.0);
    notable.notable = true;
    let index = build(vec![poi("Grand Hotel", 20.0, 20.0), notable], None);
    let results = index.forward("grand hotel", 10, None);
    assert_eq!(results[0].lat, 10.0);
}

#[test]
fn a_bias_puts_the_nearer_same_named_object_first() {
    let index = build(
        vec![
            poi("Grand Hotel", 10.0, 10.0),
            poi("Grand Hotel", 20.0, 20.0),
        ],
        None,
    );
    for (bias, expected_lat) in [((10.1, 10.1), 10.0), ((19.9, 19.9), 20.0)] {
        let bias = Point {
            lon: bias.0,
            lat: bias.1,
        };
        assert_eq!(
            index.forward("grand hotel", 1, Some(bias))[0].lat,
            expected_lat
        );
        assert_eq!(
            index.autocomplete("grand h", 1, Some(bias))[0].lat,
            expected_lat
        );
    }
}

#[test]
fn every_osm_field_comes_back() {
    let index = build(vec![poi("Cafe Central", 48.21, 16.37)], None);
    let result = &index.forward("Cafe Central", 1, None)[0];
    assert_eq!(result.name.as_deref(), Some("Cafe Central"));
    assert_eq!(result.osm_type, Some(OsmType::Node));
    assert_eq!(result.osm_id, Some(42));
    assert_eq!(result.osm_key.as_deref(), Some("amenity"));
    assert_eq!(result.osm_value.as_deref(), Some("cafe"));
}

#[test]
fn display_name_drops_repeated_parts() {
    let mut record = place("Zürich", PlaceClass::City, None, 47.37, 8.54);
    record.address.state = Some("Zürich".to_string());
    record.address.country = Some("Switzerland".to_string());
    let index = build(vec![record], None);
    let result = &index.forward("zurich", 1, None)[0];
    assert_eq!(result.display_name, "Zürich, Switzerland");
}

#[test]
fn forward_geocode() {
    let index = build(
        vec![
            address("123 Main Street, Springfield, IL", 39.7817, -89.6501),
            address("456 Oak Avenue, Portland, OR", 45.5152, -122.6784),
        ],
        None,
    );
    let results = index.forward("123 main st", 10, None);
    assert_eq!(results.len(), 1);
    assert!((results[0].lat - 39.7817).abs() < 0.001);
    let results = index.forward("123 North Main Street Apt 4", 10, None);
    assert_eq!(results.len(), 1);
    let results = index.forward("123 Main St, Springfield, IL", 10, None);
    assert_eq!(results[0].confidence, 1.0);
    assert!(index.forward("xyznonexistent", 10, None).is_empty());
}

fn queen_street() -> TestIndex {
    let mut city_hall = address("100 Queen Street West, Toronto, ON", 43.6534, -79.3841);
    city_hall.name = Some("Toronto City Hall".to_string());
    build(
        vec![
            address("100 Queen Street East, Toronto, ON", 43.6537, -79.3740),
            address("100 Queen Street West, Brampton, ON", 43.6841, -79.7622),
            address("100 North Queen Street, Toronto, ON", 43.6215, -79.5457),
            city_hall,
        ],
        None,
    )
}

#[test]
fn forward_finds_a_named_address_by_house_number() {
    let results = queen_street().forward("100 Queen Street West", 10, None);
    assert!(
        results
            .iter()
            .any(|r| r.name.as_deref() == Some("Toronto City Hall")),
        "got {results:?}"
    );
}

#[test]
fn forward_ranks_the_queried_directional_first() {
    let index = queen_street();
    let west: Vec<String> = index
        .forward("100 Queen St W", 10, None)
        .into_iter()
        .map(|r| r.address.street.unwrap_or_default())
        .collect();
    let last_west = west.iter().rposition(|s| s == "Queen Street West").unwrap();
    let first_other = west.iter().position(|s| s != "Queen Street West").unwrap();
    assert!(last_west < first_other, "got {west:?}");

    let east = index.forward("100 Queen St E", 10, None);
    assert_eq!(east[0].address.street.as_deref(), Some("Queen Street East"));
}

#[test]
fn autocomplete_matches_a_prefix() {
    let index = build(
        vec![address("123 Main Street, Springfield, IL", 39.78, -89.65)],
        None,
    );
    assert!(!index.autocomplete("123", 10, None).is_empty());
    assert!(index.autocomplete("123 mian", 10, None).is_empty());
}

fn monaco() -> TestIndex {
    build(
        vec![
            street("Avenue Grimaldi", "Monaco", 43.7355, 7.4197),
            street("Boulevard des Moulins", "Monaco", 43.7396, 7.4278),
            street("Avenue de la Costa", "Monaco", 43.7402, 7.4266),
        ],
        None,
    )
}

#[test]
fn a_typo_falls_back_to_fuzzy() {
    let results = monaco().forward("Avenue Grimadli", 10, None);
    assert!(!results.is_empty(), "typo query should fall back to fuzzy");
    assert_eq!(results[0].name.as_deref(), Some("Avenue Grimaldi"));
    assert_eq!(results[0].match_type, MatchType::Fuzzy);
    assert!(results[0].confidence < 1.0);
    let json = serde_json::to_string(&results[0]).unwrap();
    assert!(json.contains(r#""match_type":"fuzzy""#), "got {json}");
}

#[test]
fn an_exact_hit_is_not_flagged_fuzzy() {
    let results = monaco().forward("Avenue Grimaldi", 10, None);
    assert!(!results.is_empty());
    assert!(results.iter().all(|r| r.match_type == MatchType::Exact));
    assert!(results.iter().all(|r| r.confidence == 1.0));
}

#[test]
fn garbage_returns_nothing() {
    let index = monaco();
    assert!(index.forward("zzqqwx flurbleglop", 10, None).is_empty());
    assert!(index.forward("xyznonexistent", 10, None).is_empty());
    assert!(index.forward(",,,", 10, None).is_empty());
}

#[test]
fn fuzzy_results_respect_the_limit() {
    let records = (0..8)
        .map(|i| {
            street(
                "Avenue Grimaldi",
                &format!("Town{i}"),
                43.73 + f64::from(i) * 0.001,
                7.41,
            )
        })
        .collect();
    let results = build(records, None).forward("Avenue Grimadli", 5, None);
    assert_eq!(results.len(), 5);
    assert!(results.iter().all(|r| r.match_type == MatchType::Fuzzy));
}

fn springfield_addresses() -> TestIndex {
    let records = vec![
        address("123 Main Street, Springfield, IL", 39.7817, -89.6501),
        address("456 Oak Avenue, Portland, OR", 45.5152, -122.6784),
        address("789 Main Drive, Denver, CO", 39.7392, -104.9903),
    ];
    let points: Vec<(f64, f64)> = records.iter().map(|r| (r.lon, r.lat)).collect();
    build(records, padded_extent(&points))
}

#[test]
fn reverse_answers_with_the_nearest_address() {
    let index = springfield_addresses();
    let results = index.reverse(-89.66, 39.7817, 1);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].address.city.as_deref(), Some("Springfield"));
    assert!(results[0].confidence > 0.9);
}

#[test]
fn reverse_outside_the_padded_extent_is_empty() {
    let index = springfield_addresses();
    assert!(index.reverse(7.4197, 43.7355, 5).is_empty());
    assert!(index.reverse(0.0, -75.0, 1).is_empty());
}

#[test]
fn autocomplete_bias_ranks_nearer_first() {
    let index = springfield_addresses();
    let near_denver = index.autocomplete(
        "main",
        10,
        Some(Point {
            lon: -104.99,
            lat: 39.74,
        }),
    );
    assert!(near_denver.len() >= 2);
    assert_eq!(near_denver[0].address.city.as_deref(), Some("Denver"));
    let near_springfield = index.autocomplete(
        "main",
        10,
        Some(Point {
            lon: -89.65,
            lat: 39.78,
        }),
    );
    assert_eq!(
        near_springfield[0].address.city.as_deref(),
        Some("Springfield")
    );
}

#[test]
fn padding_covers_a_point_just_past_the_outermost_address() {
    let points = [
        (-79.3740, 43.6537),
        (-79.7622, 43.6841),
        (-79.5457, 43.6215),
    ];
    let coverage = padded_extent(&points).unwrap();
    assert!(coverage.contains(-79.3740, 43.60));
    assert!(!coverage.contains(-79.3740, 40.0));
}

#[test]
fn inside_address_coverage_reverse_answers_only_with_addresses() {
    let index = build(
        vec![
            address("2 Jasper Avenue, Toronto, ON", 43.6835, -79.4830),
            address("20 Jasper Avenue, Toronto, ON", 43.6845, -79.4840),
            place("Jasper", PlaceClass::Town, None, 43.6840, -79.4835),
        ],
        covering(-79.5, 43.6, -79.4, 43.7),
    );
    let results = index.reverse(-79.4835, 43.6840, 3);
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.kind == FeatureKind::Address));
}

#[test]
fn outside_address_coverage_reverse_answers_with_the_nearest_settlement() {
    let index = build(
        vec![
            address("2 Jasper Avenue, Toronto, ON", 43.6835, -79.4830),
            place("Jasper", PlaceClass::Town, None, 52.8752, -118.0824),
        ],
        covering(-79.5, 43.6, -79.4, 43.7),
    );
    let near_jasper = index.reverse(-118.09, 52.87, 1);
    assert_eq!(near_jasper[0].kind, FeatureKind::Place);
    assert!(
        index.reverse(-100.0, 50.0, 1).is_empty(),
        "no settlement within reach"
    );
}

#[test]
fn opening_a_directory_without_an_index_says_so() {
    let directory = tempfile::tempdir().unwrap();
    let error = Geocoder::open(directory.path()).err().unwrap();
    assert!(matches!(error, OpenError::Missing(_)), "{error}");
}

#[test]
fn a_mismatched_format_version_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    IndexWriter::create(directory.path())
        .unwrap()
        .finish()
        .unwrap();
    let meta_path = directory.path().join("meta.json");
    let meta = std::fs::read_to_string(&meta_path).unwrap().replace(
        &format!("\"format_version\": {FORMAT_VERSION}"),
        "\"format_version\": 0",
    );
    std::fs::write(&meta_path, meta).unwrap();
    let error = Geocoder::open(directory.path()).err().unwrap();
    assert!(
        matches!(error, OpenError::Version { found: 0, .. }),
        "{error}"
    );
    assert!(error.to_string().contains("geokode build"));
}

#[test]
fn an_empty_index_opens_and_finds_nothing() {
    let index = build(Vec::new(), None);
    assert!(index.is_empty());
    assert!(index.forward("anything", 5, None).is_empty());
    assert!(index.reverse(0.0, 0.0, 5).is_empty());
}

#[test]
fn a_notable_poi_outranks_a_same_named_street() {
    let mut park = poi("Central Park", 40.78, -73.97);
    park.notable = true;
    let index = build(
        vec![street("Central Park", "Leeds", 53.8, -1.5), park],
        None,
    );
    assert_eq!(
        index.forward("central park", 1, None)[0].kind,
        FeatureKind::Poi
    );
}

#[test]
fn a_half_typed_street_suffix_still_autocompletes() {
    let names: Vec<String> = monaco()
        .autocomplete("Avenu", 5, None)
        .into_iter()
        .filter_map(|r| r.name)
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.iter().all(|name| name.starts_with("Avenue")));
}

#[test]
fn a_qualifier_prefers_the_city_over_the_same_named_state() {
    let directory = tempfile::tempdir().unwrap();
    let mut writer = IndexWriter::create(directory.path()).unwrap();
    let canton = writer.add_area(&["Zürich".to_string()], 4);
    let city = writer.add_area(&["Zürich".to_string()], 8);
    let town = writer.add_area(&["Küsnacht".to_string()], 8);
    for (lat, areas) in [(47.31, vec![canton, town]), (47.37, vec![canton, city])] {
        let record = address("1 Bahnhofstrasse, Somewhere, ZH", lat, 8.5);
        writer
            .add(PreparedRecord::new(record), areas, true)
            .unwrap();
    }
    writer.finish().unwrap();
    let geocoder = Geocoder::open(directory.path()).unwrap();
    let results = geocoder.forward("Bahnhofstrasse 1, Zürich", 2, None);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].lat, 47.37);
}

#[test]
fn a_qualifier_matches_the_english_or_the_local_name() {
    let directory = tempfile::tempdir().unwrap();
    let mut writer = IndexWriter::create(directory.path()).unwrap();
    let switzerland = writer.add_area(
        &[
            "Schweiz/Suisse/Svizzera/Svizra".to_string(),
            "Switzerland".to_string(),
        ],
        2,
    );
    let mut peak = poi("Matterhorn", 45.976, 7.659);
    peak.address.country = Some("Switzerland".to_string());
    writer
        .add(PreparedRecord::new(peak), vec![switzerland], true)
        .unwrap();
    writer.finish().unwrap();
    let geocoder = Geocoder::open(directory.path()).unwrap();
    for query in [
        "Matterhorn, Switzerland",
        "Matterhorn, Schweiz/Suisse/Svizzera/Svizra",
    ] {
        let results = geocoder.forward(query, 1, None);
        assert_eq!(results.len(), 1, "{query}");
        assert_eq!(results[0].confidence, 1.0, "{query}");
    }
    assert!(geocoder.forward("Matterhorn, Italia", 1, None).is_empty());
}
