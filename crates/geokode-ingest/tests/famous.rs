use geokode_core::address::{GeoResult, OsmType};
use geokode_core::geocode::Geocoder;
use geokode_ingest::build::{BuildInput, build};
use std::path::Path;
use std::sync::OnceLock;

const EIFFEL_TOWER_WAY: i64 = 5_013_364;
const CENTRAL_PARK_WAY: i64 = 427_818_536;
const KILIMANJARO_WAY: i64 = 355_306_860;
const PARIS_NODE: i64 = 17_807_753;
const PORTLAND_OREGON_NODE: i64 = 1_666_626_393;
const SPRINGFIELD_MISSOURI_NODE: i64 = 151_340_686;

fn geocoder() -> &'static Geocoder {
    static INDEX: OnceLock<(tempfile::TempDir, Geocoder)> = OnceLock::new();
    &INDEX
        .get_or_init(|| {
            let directory = tempfile::tempdir().unwrap();
            let pbf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/famous.osm.pbf");
            build(&BuildInput {
                pbf: &pbf,
                addresses: &[],
                out: directory.path(),
            })
            .unwrap();
            let geocoder = Geocoder::open(directory.path()).unwrap();
            (directory, geocoder)
        })
        .1
}

fn first(query: &str) -> GeoResult {
    geocoder().forward(query, 1, None).remove(0)
}

#[test]
fn the_famous_tower_beats_a_replica_of_the_same_name() {
    let tower = first("Eiffel Tower");
    assert_eq!(
        (tower.osm_type, tower.osm_id),
        (Some(OsmType::Way), Some(EIFFEL_TOWER_WAY))
    );
}

#[test]
fn a_famous_park_beats_a_village_of_the_same_name() {
    assert_eq!(first("Central Park").osm_id, Some(CENTRAL_PARK_WAY));
}

#[test]
fn mount_finds_the_mountain_tagged_without_the_word() {
    let mountain = first("Mount Kilimanjaro");
    assert_eq!(mountain.osm_id, Some(KILIMANJARO_WAY));
    assert_eq!(mountain.osm_value.as_deref(), Some("massif"));
}

#[test]
fn display_name_leads_with_the_english_name() {
    let tokyo = first("Tokyo");
    assert_eq!(tokyo.name.as_deref(), Some("東京都"));
    assert!(
        tokyo.display_name.starts_with("Tokyo"),
        "{}",
        tokyo.display_name
    );
    let kyiv = first("Київ");
    assert_eq!(kyiv.name.as_deref(), Some("Київ"));
    assert!(
        kyiv.display_name.starts_with("Kyiv"),
        "{}",
        kyiv.display_name
    );
    assert_eq!(first("Kyiv").osm_id, kyiv.osm_id);
}

#[test]
fn big_cities_still_win_their_names() {
    assert_eq!(first("Paris").osm_id, Some(PARIS_NODE));
    assert_eq!(first("Portland").osm_id, Some(PORTLAND_OREGON_NODE));
    assert_eq!(first("Springfield").osm_id, Some(SPRINGFIELD_MISSOURI_NODE));
}
