use geokode_core::address::{FeatureKind, GeoResult, OsmType};
use geokode_core::geocode::{Geocoder, Point};
use geokode_ingest::build::{BuildInput, build};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MONACO_COUNTRY_RELATION: i64 = 1_124_039;
const MONACO_CITY_RELATION: i64 = 2_220_322;
const JARDIN_EXOTIQUE_QUARTER: i64 = 5_986_473;
const JARDIN_EXOTIQUE_GARDEN: i64 = 157_455_610;
const OCEANOGRAPHIC_MUSEUM_WAY: i64 = 23_715_051;
const BNP_LARVOTTO_NODE: i64 = 12_323_382_259;
const BNP_FONTVIEILLE_NODE: i64 = 1_704_201_320;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/monaco.osm.pbf")
}

fn index() -> &'static Path {
    static INDEX: OnceLock<tempfile::TempDir> = OnceLock::new();
    INDEX
        .get_or_init(|| {
            let directory = tempfile::tempdir().unwrap();
            build(&BuildInput {
                pbf: &fixture(),
                addresses: &[],
                out: directory.path(),
            })
            .unwrap();
            directory
        })
        .path()
}

fn geocoder() -> Geocoder {
    Geocoder::open(index()).unwrap()
}

fn osm_ids(results: &[GeoResult]) -> Vec<i64> {
    results.iter().filter_map(|r| r.osm_id).collect()
}

#[test]
fn the_country_and_city_come_before_anything_else_named_monaco() {
    let results = geocoder().forward("Monaco", 10, None);
    assert_eq!(
        osm_ids(&results[..2]),
        vec![MONACO_COUNTRY_RELATION, MONACO_CITY_RELATION]
    );
    assert_eq!(results[0].admin_level, Some(2));
    assert_eq!(results[1].admin_level, Some(8));
    assert!(
        results[2..]
            .iter()
            .all(|r| r.kind != FeatureKind::Boundary || r.admin_level > Some(8))
    );
}

#[test]
fn a_settlement_outranks_the_same_named_garden() {
    let results = geocoder().forward("Jardin Exotique", 5, None);
    assert_eq!(
        osm_ids(&results[..2]),
        vec![JARDIN_EXOTIQUE_QUARTER, JARDIN_EXOTIQUE_GARDEN]
    );
    assert_eq!(results[1].osm_key.as_deref(), Some("leisure"));
}

#[test]
fn a_named_poi_carries_its_osm_identity() {
    let results = geocoder().forward("Musée Océanographique", 1, None);
    let museum = &results[0];
    assert_eq!(museum.kind, FeatureKind::Poi);
    assert_eq!(museum.osm_type, Some(OsmType::Way));
    assert_eq!(museum.osm_id, Some(OCEANOGRAPHIC_MUSEUM_WAY));
    assert_eq!(museum.osm_key.as_deref(), Some("tourism"));
    assert_eq!(museum.osm_value.as_deref(), Some("museum"));
    assert!(museum.bbox.is_some());
}

#[test]
fn containment_fills_city_country_and_code() {
    let geocoder = geocoder();
    let museum = &geocoder.forward("Musee Oceanographique", 1, None)[0];
    assert_eq!(museum.address.city.as_deref(), Some("Monaco"));
    assert_eq!(museum.address.country.as_deref(), Some("Monaco"));
    assert_eq!(museum.country_code.as_deref(), Some("mc"));
    assert_eq!(museum.display_name, "Musée Océanographique, Monaco");
}

#[test]
fn the_point_lies_inside_the_boundary_bbox() {
    for result in geocoder().forward("Monaco", 2, None) {
        let [min_lon, min_lat, max_lon, max_lat] = result.bbox.unwrap();
        assert!(result.lon >= min_lon && result.lon <= max_lon);
        assert!(result.lat >= min_lat && result.lat <= max_lat);
    }
}

#[test]
fn a_bias_reorders_same_named_results() {
    let geocoder = geocoder();
    let near = |lon, lat| {
        let results = geocoder.forward("BNP Paribas", 3, Some(Point { lon, lat }));
        results[0].osm_id
    };
    assert_eq!(near(7.4318676, 43.7473891), Some(BNP_LARVOTTO_NODE));
    assert_eq!(near(7.415687, 43.7292733), Some(BNP_FONTVIEILLE_NODE));
}

#[test]
fn a_trailing_area_filters_by_containment() {
    let geocoder = geocoder();
    let within = geocoder.forward("Larvotto, Monaco", 5, None);
    assert!(!within.is_empty());
    assert!(
        within
            .iter()
            .all(|r| r.country_code.as_deref() == Some("mc"))
    );
    assert!(geocoder.forward("Larvotto, Italia", 5, None).is_empty());
}

#[test]
fn street_ways_merge_into_one_record_per_admin_area() {
    let results = geocoder().forward("Boulevard des Moulins", 10, None);
    let streets: Vec<&GeoResult> = results
        .iter()
        .filter(|r| r.kind == FeatureKind::Street)
        .collect();
    assert_eq!(streets.len(), 1, "{results:?}");
    assert_eq!(streets[0].osm_type, Some(OsmType::Way));
    assert_eq!(streets[0].osm_key.as_deref(), Some("highway"));
}

#[test]
fn reverse_without_addresses_names_the_nearest_settlement() {
    let results = geocoder().reverse(7.4270, 43.7405, 1);
    assert_eq!(results[0].name.as_deref(), Some("Monte-Carlo"));
}
