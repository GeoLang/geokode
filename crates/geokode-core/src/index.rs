use crate::address::{
    Address, FeatureKind, OsmType, PlaceClass, directional_mask, normalize_for_match,
};
use crate::details::{Details, LABEL_FIELDS, encode_labels};
use crate::rank::{Importance, feature_code};
use crate::sort::ExternalSorter;
use crate::spatial::{IndexedPoint, encode_kd_tree, to_degrees, to_units};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

pub const FORMAT_VERSION: u32 = 2;
pub const UNKNOWN_AREA_LEVEL: u8 = 0;

pub(crate) const META_FILE: &str = "meta.json";
pub(crate) const ROWS_FILE: &str = "records.bin";
pub(crate) const DETAILS_FILE: &str = "details.bin";
pub(crate) const NAMES_FILE: &str = "names.fst";
pub(crate) const POSTINGS_FILE: &str = "postings.bin";
pub(crate) const CONTEXT_FILE: &str = "context.json";
pub(crate) const ADDRESS_POINTS_FILE: &str = "addresses.kd";
pub(crate) const SETTLEMENT_POINTS_FILE: &str = "settlements.kd";
const KEY_SORT_DIRECTORY: &str = "key-sort.tmp";

const KEY_SORT_MEMORY: usize = 512 * 1024 * 1024;
const MAX_KEY_BYTES: usize = 200;
const IMPORTANCE_SHIFT: u32 = 48;
const POSTING_OFFSET_MASK: u64 = (1 << IMPORTANCE_SHIFT) - 1;
const IMPORTANCE_SCALE: f32 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
pub(crate) struct Meta {
    pub format_version: u32,
    pub records: u64,
    pub address_coverage: Vec<Coverage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Chain {
    pub areas: Vec<u32>,
    // a known state or country lets a qualifier contradict the record
    pub anchored: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct ContextFile {
    pub area_names: Vec<Vec<String>>,
    pub area_levels: Vec<u8>,
    pub chains: Vec<Chain>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Record {
    pub name: Option<String>,
    // leads display_name when tagged
    pub name_en: Option<String>,
    pub name_variants: Vec<String>,
    pub address: Address,
    pub country_code: Option<String>,
    pub lon: f64,
    pub lat: f64,
    pub bbox: Option<[f64; 4]>,
    pub kind: FeatureKind,
    pub osm: Option<(OsmType, i64)>,
    pub osm_key: Option<String>,
    pub osm_value: Option<String>,
    pub admin_level: Option<u8>,
    pub place: Option<PlaceClass>,
    pub population: Option<u64>,
    pub notable: bool,
    pub languages: u16,
}

impl Record {
    pub fn new(kind: FeatureKind, lon: f64, lat: f64) -> Self {
        Self {
            name: None,
            name_en: None,
            name_variants: Vec::new(),
            address: Address::default(),
            country_code: None,
            lon,
            lat,
            bbox: None,
            kind,
            osm: None,
            osm_key: None,
            osm_value: None,
            admin_level: None,
            place: None,
            population: None,
            notable: false,
            languages: 0,
        }
    }

    fn keys(&self) -> Vec<String> {
        let mut sources: Vec<String> = self
            .name
            .iter()
            .chain(self.name_variants.iter())
            .cloned()
            .collect();
        if self.kind == FeatureKind::Address
            && let (Some(number), Some(street)) = (&self.address.house_number, &self.address.street)
        {
            sources.push(format!("{number} {street}"));
            sources.push(format!("{street} {number}"));
        }
        let mut keys: Vec<String> = sources
            .iter()
            .map(|source| truncate_key(normalize_for_match(source)))
            .filter(|key| !key.is_empty())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn reverse_layer(&self) -> Option<ReverseLayer> {
        if self.kind == FeatureKind::Address {
            return Some(ReverseLayer::Address);
        }
        let settlement = matches!(self.kind, FeatureKind::Place | FeatureKind::Boundary)
            && self.place.is_some_and(PlaceClass::is_settlement);
        settlement.then_some(ReverseLayer::Settlement)
    }
}

fn truncate_key(mut key: String) -> String {
    if key.len() > MAX_KEY_BYTES {
        let mut end = MAX_KEY_BYTES;
        while !key.is_char_boundary(end) {
            end -= 1;
        }
        key.truncate(end);
    }
    key
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReverseLayer {
    Address,
    Settlement,
}

pub(crate) const ROW_BYTES: usize = 48;
const NO_BBOX: i32 = i32::MIN;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Row {
    pub lon: i32,
    pub lat: i32,
    pub bbox: Option<[i32; 4]>,
    pub importance: f32,
    pub chain: u32,
    pub detail_offset: u64,
    pub detail_len: u32,
    pub kind: FeatureKind,
    pub directionals: u8,
    pub feature: u8,
}

impl Row {
    fn encode(&self) -> [u8; ROW_BYTES] {
        let mut out = [0u8; ROW_BYTES];
        let bbox = self.bbox.unwrap_or([NO_BBOX; 4]);
        out[0..4].copy_from_slice(&self.lon.to_le_bytes());
        out[4..8].copy_from_slice(&self.lat.to_le_bytes());
        for (i, value) in bbox.iter().enumerate() {
            out[8 + i * 4..12 + i * 4].copy_from_slice(&value.to_le_bytes());
        }
        out[24..28].copy_from_slice(&self.importance.to_le_bytes());
        out[28..32].copy_from_slice(&self.chain.to_le_bytes());
        out[32..40].copy_from_slice(&self.detail_offset.to_le_bytes());
        out[40..44].copy_from_slice(&self.detail_len.to_le_bytes());
        out[44] = self.kind.code();
        out[45] = self.directionals;
        out[46] = self.feature;
        out
    }

    pub fn decode(bytes: &[u8]) -> Self {
        let word = |at: usize| [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
        let int = |at: usize| i32::from_le_bytes(word(at));
        let bbox = [int(8), int(12), int(16), int(20)];
        let mut offset = [0u8; 8];
        offset.copy_from_slice(&bytes[32..40]);
        Row {
            lon: int(0),
            lat: int(4),
            bbox: (bbox[0] != NO_BBOX).then_some(bbox),
            importance: f32::from_le_bytes(word(24)),
            chain: u32::from_le_bytes(word(28)),
            detail_offset: u64::from_le_bytes(offset),
            detail_len: u32::from_le_bytes(word(40)),
            kind: FeatureKind::from_code(bytes[44]).expect("rows are written by IndexWriter"),
            directionals: bytes[45],
            feature: bytes[46],
        }
    }

    pub fn bbox_degrees(&self) -> Option<[f64; 4]> {
        self.bbox.map(|b| b.map(to_degrees))
    }
}

// needs no shared state, so callers build it in parallel
pub struct PreparedRecord {
    row: Row,
    labels: [Option<String>; LABEL_FIELDS],
    inline_details: Vec<u8>,
    keys: Vec<String>,
    reverse_layer: Option<ReverseLayer>,
}

impl PreparedRecord {
    pub fn new(record: Record) -> Self {
        let importance = Importance {
            kind: record.kind,
            place: record.place,
            admin_level: record.admin_level,
            population: record.population,
            notable: record.notable,
            languages: record.languages,
        }
        .score();
        let directional_source = record
            .address
            .street
            .clone()
            .or_else(|| record.name.clone())
            .unwrap_or_default();
        let keys = record.keys();
        let reverse_layer = record.reverse_layer();
        let row = Row {
            lon: to_units(record.lon),
            lat: to_units(record.lat),
            bbox: record.bbox.map(|b| b.map(to_units)),
            importance,
            chain: 0,
            detail_offset: 0,
            detail_len: 0,
            kind: record.kind,
            directionals: directional_mask(&directional_source),
            feature: feature_code(record.osm_value.as_deref()),
        };
        let details = Details {
            name: record.name,
            name_en: record.name_en,
            address: record.address,
            country_code: record.country_code,
            osm: record.osm,
            osm_key: record.osm_key,
            osm_value: record.osm_value,
            admin_level: record.admin_level,
            population: record.population,
        };
        PreparedRecord {
            row,
            labels: details.labels(),
            inline_details: details.encode_inline(),
            keys,
            reverse_layer,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexSummary {
    pub records: u64,
    pub keys: u64,
    pub by_kind: Vec<(FeatureKind, u64)>,
}

pub struct IndexWriter {
    directory: PathBuf,
    rows: BufWriter<File>,
    details: BufWriter<File>,
    detail_offset: u64,
    keys: ExternalSorter<(String, u32)>,
    importances: Vec<f32>,
    address_points: Vec<IndexedPoint>,
    settlement_points: Vec<IndexedPoint>,
    area_names: Vec<Vec<String>>,
    area_levels: Vec<u8>,
    areas_by_name: HashMap<String, u32>,
    labels: Vec<String>,
    label_ids: HashMap<String, u32>,
    chains: Vec<Chain>,
    chain_ids: HashMap<Chain, u32>,
    coverage: Vec<Coverage>,
    kind_counts: [u64; FeatureKind::ALL.len()],
}

impl IndexWriter {
    pub fn create(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        // serve refuses a directory without meta.json
        match fs::remove_file(directory.join(META_FILE)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let sort_directory = directory.join(KEY_SORT_DIRECTORY);
        fs::create_dir_all(&sort_directory)?;
        let mut writer = IndexWriter {
            directory: directory.to_path_buf(),
            rows: BufWriter::new(File::create(directory.join(ROWS_FILE))?),
            details: BufWriter::new(File::create(directory.join(DETAILS_FILE))?),
            detail_offset: 0,
            keys: ExternalSorter::new(&sort_directory, "keys", KEY_SORT_MEMORY),
            importances: Vec::new(),
            address_points: Vec::new(),
            settlement_points: Vec::new(),
            area_names: Vec::new(),
            area_levels: Vec::new(),
            areas_by_name: HashMap::new(),
            labels: Vec::new(),
            label_ids: HashMap::new(),
            chains: Vec::new(),
            chain_ids: HashMap::new(),
            coverage: Vec::new(),
            kind_counts: [0; FeatureKind::ALL.len()],
        };
        writer.chain_id(Chain::default());
        Ok(writer)
    }

    pub fn add_area(&mut self, names: &[String], level: u8) -> u32 {
        let mut normalized: Vec<String> = names
            .iter()
            .map(|name| normalize_for_match(name))
            .filter(|name| !name.is_empty())
            .collect();
        normalized.sort_unstable();
        normalized.dedup();
        self.area_names.push(normalized);
        self.area_levels.push(level);
        (self.area_names.len() - 1) as u32
    }

    pub fn area_named(&mut self, name: &str) -> u32 {
        let key = normalize_for_match(name);
        if let Some(id) = self.areas_by_name.get(&key) {
            return *id;
        }
        let id = self.add_area(&[name.to_string()], UNKNOWN_AREA_LEVEL);
        self.areas_by_name.insert(key, id);
        id
    }

    pub fn add_coverage(&mut self, coverage: Coverage) {
        self.coverage.push(coverage);
    }

    fn label_id(&mut self, text: String) -> u32 {
        if let Some(id) = self.label_ids.get(&text) {
            return *id;
        }
        let id = self.labels.len() as u32;
        self.labels.push(text.clone());
        self.label_ids.insert(text, id);
        id
    }

    fn chain_id(&mut self, chain: Chain) -> u32 {
        if let Some(id) = self.chain_ids.get(&chain) {
            return *id;
        }
        let id = self.chains.len() as u32;
        self.chains.push(chain.clone());
        self.chain_ids.insert(chain, id);
        id
    }

    pub fn add(
        &mut self,
        record: PreparedRecord,
        mut areas: Vec<u32>,
        anchored: bool,
    ) -> io::Result<u32> {
        let id = u32::try_from(self.importances.len())
            .map_err(|_| io::Error::other("more records than a u32 id can hold"))?;
        areas.sort_unstable();
        areas.dedup();
        let mut row = record.row;
        row.chain = self.chain_id(Chain { areas, anchored });
        let label_ids = record
            .labels
            .map(|label| label.map(|text| self.label_id(text)));
        let mut details = encode_labels(label_ids);
        details.extend_from_slice(&record.inline_details);
        row.detail_offset = self.detail_offset;
        row.detail_len = details.len() as u32;
        self.details.write_all(&details)?;
        self.detail_offset += details.len() as u64;
        self.rows.write_all(&row.encode())?;
        self.importances.push(row.importance);
        self.kind_counts[usize::from(row.kind.code())] += 1;
        for key in record.keys {
            self.keys.push((key, id))?;
        }
        let point = IndexedPoint {
            lon: row.lon,
            lat: row.lat,
            id,
        };
        match record.reverse_layer {
            Some(ReverseLayer::Address) => self.address_points.push(point),
            Some(ReverseLayer::Settlement) => self.settlement_points.push(point),
            None => {}
        }
        Ok(id)
    }

    pub fn finish(self) -> io::Result<IndexSummary> {
        let IndexWriter {
            directory,
            mut rows,
            mut details,
            keys,
            importances,
            address_points,
            settlement_points,
            area_names,
            area_levels,
            chains,
            labels,
            coverage,
            kind_counts,
            ..
        } = self;
        rows.flush()?;
        details.flush()?;
        let key_count = write_names(&directory, keys, &importances)?;
        fs::write(
            directory.join(ADDRESS_POINTS_FILE),
            encode_kd_tree(address_points),
        )?;
        fs::write(
            directory.join(SETTLEMENT_POINTS_FILE),
            encode_kd_tree(settlement_points),
        )?;
        let context = ContextFile {
            area_names,
            area_levels,
            chains,
            labels,
        };
        fs::write(
            directory.join(CONTEXT_FILE),
            serde_json::to_vec(&context).map_err(io::Error::other)?,
        )?;
        let meta = Meta {
            format_version: FORMAT_VERSION,
            records: importances.len() as u64,
            address_coverage: coverage,
        };
        fs::write(
            directory.join(META_FILE),
            serde_json::to_vec_pretty(&meta).map_err(io::Error::other)?,
        )?;
        fs::remove_dir_all(directory.join(KEY_SORT_DIRECTORY))?;
        Ok(IndexSummary {
            records: meta.records,
            keys: key_count,
            by_kind: FeatureKind::ALL
                .iter()
                .map(|kind| (*kind, kind_counts[usize::from(kind.code())]))
                .collect(),
        })
    }
}

fn write_names(
    directory: &Path,
    keys: ExternalSorter<(String, u32)>,
    importances: &[f32],
) -> io::Result<u64> {
    let mut postings = BufWriter::new(File::create(directory.join(POSTINGS_FILE))?);
    let mut names = fst::MapBuilder::new(BufWriter::new(File::create(directory.join(NAMES_FILE))?))
        .map_err(io::Error::other)?;
    let mut posting_offset = 0u64;
    let mut key_count = 0u64;
    let mut flush = |key: String, mut ids: Vec<u32>| -> io::Result<()> {
        ids.dedup();
        ids.sort_by(|a, b| importances[*b as usize].total_cmp(&importances[*a as usize]));
        let best = importances[ids[0] as usize];
        let quantized = (best.max(0.0) * IMPORTANCE_SCALE).min(f32::from(u16::MAX)) as u64;
        names
            .insert(
                key.as_bytes(),
                posting_offset | (quantized << IMPORTANCE_SHIFT),
            )
            .map_err(io::Error::other)?;
        postings.write_all(&(ids.len() as u32).to_le_bytes())?;
        for id in &ids {
            postings.write_all(&id.to_le_bytes())?;
        }
        posting_offset += 4 + 4 * ids.len() as u64;
        key_count += 1;
        Ok(())
    };
    let mut current: Option<(String, Vec<u32>)> = None;
    for item in keys.finish()? {
        let (key, id) = item?;
        match &mut current {
            Some((current_key, ids)) if *current_key == key => ids.push(id),
            _ => {
                if let Some((done_key, ids)) = current.replace((key, vec![id])) {
                    flush(done_key, ids)?;
                }
            }
        }
    }
    if let Some((key, ids)) = current {
        flush(key, ids)?;
    }
    names.finish().map_err(io::Error::other)?;
    postings.flush()?;
    Ok(key_count)
}

pub(crate) fn posting_offset(value: u64) -> u64 {
    value & POSTING_OFFSET_MASK
}

pub(crate) fn key_importance(value: u64) -> u16 {
    (value >> IMPORTANCE_SHIFT) as u16
}

pub fn padded_extent(points: &[(f64, f64)]) -> Option<Coverage> {
    let (&(first_lon, first_lat), rest) = points.split_first()?;
    let mut extent = Coverage {
        min_lon: first_lon,
        min_lat: first_lat,
        max_lon: first_lon,
        max_lat: first_lat,
    };
    for &(lon, lat) in rest {
        extent.min_lon = extent.min_lon.min(lon);
        extent.min_lat = extent.min_lat.min(lat);
        extent.max_lon = extent.max_lon.max(lon);
        extent.max_lat = extent.max_lat.max(lat);
    }
    let tree = crate::spatial::KdTree::new(encode_kd_tree(
        points
            .iter()
            .enumerate()
            .map(|(i, &(lon, lat))| IndexedPoint::new(lon, lat, i as u32))
            .collect(),
    ));
    // the widest gap the data already tolerates between two neighbouring addresses
    let pad = points
        .iter()
        .enumerate()
        .filter_map(|(i, &(lon, lat))| {
            let neighbour = tree
                .nearest(lon, lat, 2, f64::INFINITY)
                .into_iter()
                .find(|n| n.id != i as u32)?;
            Some((neighbour.lon - lon).hypot(neighbour.lat - lat))
        })
        .fold(0.0, f64::max);
    Some(Coverage {
        min_lon: extent.min_lon - pad,
        min_lat: extent.min_lat - pad,
        max_lon: extent.max_lon + pad,
        max_lat: extent.max_lat + pad,
    })
}
