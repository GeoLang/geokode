use crate::classify::Tagged;
use crate::containment::{
    AdminArea, AreaName, Containment, Located, MAX_CONTEXT_LEVEL, Settlement,
};
use crate::geojson::{GeoJsonError, read_geojson};
use crate::geometry::{
    Area, BandedArea, Coord, bbox, join_rings, line_length, line_midpoint, union,
};
use crate::openaddresses::{IngestError, read_openaddresses};
use crate::pbf::{self, RelationCandidate, Role, Scan, Selection, WayCandidate};
use geokode_core::address::{FeatureKind, OsmType, PlaceClass, normalize_for_match};
use geokode_core::index::{
    Coverage, IndexSummary, IndexWriter, PreparedRecord, Record, padded_extent,
};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;
use thiserror::Error;

const SCRATCH_DIRECTORY: &str = "build.tmp";
const BATCH: usize = 50_000;
const MIN_MUNICIPAL_LEVEL: u8 = 7;
// streets outside every admin area merge within this cell, in degrees
const STREET_CELL_DEGREES: f64 = 0.1;
const LINEAR_VALUES: &[(&str, &[&str])] = &[
    (
        "waterway",
        &[
            "river",
            "stream",
            "canal",
            "drain",
            "ditch",
            "brook",
            "tidal_channel",
        ],
    ),
    (
        "natural",
        &["coastline", "cliff", "ridge", "arete", "tree_row", "valley"],
    ),
    (
        "man_made",
        &["pipeline", "embankment", "dyke", "breakwater", "groyne"],
    ),
];

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{path}: {source}")]
    Csv { path: PathBuf, source: IngestError },
    #[error("{path}: {source}")]
    GeoJson { path: PathBuf, source: GeoJsonError },
}

pub struct BuildInput<'a> {
    pub pbf: &'a Path,
    pub addresses: &'a [PathBuf],
    pub out: &'a Path,
}

struct Placed {
    absorbed: Option<OsmKey>,
    prepared: PreparedRecord,
    admin_areas: Vec<u32>,
    settlement: Option<usize>,
    named_areas: Vec<String>,
    anchored: bool,
}

type OsmKey = (OsmType, i64);

struct AddressFill {
    house_number: Option<String>,
    street: Option<String>,
    postcode: Option<String>,
}

// address objects from address inputs that a named record of the same osm id may take in
type Absorbable = HashMap<OsmKey, AddressFill>;

struct Sink<'a> {
    writer: &'a mut IndexWriter,
    settlement_areas: HashMap<usize, u32>,
    absorbed: HashSet<OsmKey>,
}

impl Sink<'_> {
    fn commit(&mut self, containment: &Containment, placed: Placed) -> io::Result<()> {
        let mut areas = placed.admin_areas;
        if let Some(index) = placed.settlement {
            let writer = &mut *self.writer;
            let area = *self.settlement_areas.entry(index).or_insert_with(|| {
                writer.add_area(
                    &containment.settlements[index].name.variants,
                    MAX_CONTEXT_LEVEL,
                )
            });
            areas.push(area);
        }
        for name in &placed.named_areas {
            areas.push(self.writer.area_named(name));
        }
        self.absorbed.extend(placed.absorbed);
        self.writer.add(placed.prepared, areas, placed.anchored)?;
        Ok(())
    }

    fn commit_all(&mut self, containment: &Containment, placed: Vec<Placed>) -> io::Result<()> {
        placed
            .into_iter()
            .try_for_each(|placed| self.commit(containment, placed))
    }
}

fn context_max_level(record: &Record) -> u8 {
    match (record.kind, record.place) {
        (FeatureKind::Boundary, _) => record.admin_level.unwrap_or(u8::MAX),
        (FeatureKind::Place, Some(PlaceClass::Country)) => 2,
        (FeatureKind::Place, Some(PlaceClass::State)) => 4,
        (FeatureKind::Place, Some(PlaceClass::County)) => 6,
        _ => u8::MAX,
    }
}

struct Location {
    admin_areas: Vec<u32>,
    settlement: Option<usize>,
    anchored: bool,
}

// input values win over containment
fn locate(containment: &Containment, record: &mut Record) -> Location {
    let located = containment.locate([record.lon, record.lat]);
    let context = containment.context(&located, context_max_level(record));
    let address = &mut record.address;
    let fill = |field: &mut Option<String>, value: Option<&str>| {
        if field.is_none() {
            *field = value.map(str::to_string);
        }
    };
    let own_names: Vec<&str> = record
        .name
        .iter()
        .chain(record.name_en.iter())
        .map(String::as_str)
        .collect();
    let lead = record.name_en.as_deref().or(record.name.as_deref());
    let read = |name: &AreaName| name.read_for(&own_names, lead);
    let city = context.city.map(read);
    let state = context.state.map(|a| read(&a.name));
    let country = context.country.map(|a| read(&a.name));
    fill(&mut address.city, city.as_deref());
    fill(&mut address.state, state.as_deref());
    fill(&mut address.country, country.as_deref());
    if record.country_code.is_none() {
        record.country_code = context.country.and_then(|a| a.country_code.clone());
    }
    let settlement = if containment.has_admin_city(&located) || context.city.is_none() {
        None
    } else {
        located.settlement
    };
    Location {
        admin_areas: located
            .admin
            .iter()
            .map(|index| containment.admin[*index].area_id)
            .collect(),
        settlement,
        anchored: containment.anchored(&located),
    }
}

impl Placed {
    fn new(record: Record, location: Location, named_areas: Vec<String>) -> Placed {
        Placed {
            absorbed: None,
            anchored: location.anchored
                || record.address.state.is_some()
                || record.address.country.is_some(),
            admin_areas: location.admin_areas,
            settlement: location.settlement,
            named_areas,
            prepared: PreparedRecord::new(record),
        }
    }
}

fn place(containment: &Containment, mut record: Record, named_areas: Vec<String>) -> Placed {
    let location = locate(containment, &mut record);
    Placed::new(record, location, named_areas)
}

// the named record fills its empty address fields from the address object it replaces
fn place_named(containment: &Containment, absorbable: &Absorbable, mut record: Record) -> Placed {
    let absorbed = record.osm.filter(|key| absorbable.contains_key(key));
    if let Some(fill) = absorbed.and_then(|key| absorbable.get(&key)) {
        let address = &mut record.address;
        for (field, value) in [
            (&mut address.house_number, &fill.house_number),
            (&mut address.street, &fill.street),
            (&mut address.postcode, &fill.postcode),
        ] {
            if field.is_none() {
                field.clone_from(value);
            }
        }
        // still found by house number, as the address record it replaces was
        if let (Some(number), Some(street)) = (&address.house_number, &address.street) {
            record
                .name_variants
                .extend([format!("{number} {street}"), format!("{street} {number}")]);
        }
    }
    Placed {
        absorbed,
        ..place(containment, record, Vec::new())
    }
}

fn named_record(
    tagged: &Tagged,
    osm: (OsmType, i64),
    point: Coord,
    bbox: Option<[f64; 4]>,
) -> Record {
    let kind = tagged.kind();
    let mut record = Record::new(kind, point[0], point[1]);
    record.name = tagged.name.clone();
    record.name_variants = tagged.variants.clone();
    record.name_en = tagged.name_en.clone();
    record.languages = tagged.languages;
    record.osm = Some(osm);
    record.osm_key = tagged.key.clone();
    record.osm_value = tagged.value.clone();
    record.admin_level = tagged.admin_level;
    if matches!(kind, FeatureKind::Place | FeatureKind::Boundary) {
        record.place = tagged.place_class();
    }
    record.population = tagged.population;
    record.notable = tagged.notable;
    record.bbox = bbox;
    record.address.house_number = tagged.house_number.clone();
    record.address.street = if kind == FeatureKind::Street {
        tagged.name.clone()
    } else {
        tagged.street.clone()
    };
    record.address.postcode = tagged.postcode.clone();
    record
}

fn is_area(tagged: &Tagged, refs: &[i64]) -> bool {
    let closed = refs.len() >= 4 && refs.first() == refs.last();
    let linear = tagged.kind() == FeatureKind::Street
        || LINEAR_VALUES.iter().any(|(key, values)| {
            tagged.key.as_deref() == Some(*key)
                && tagged.value.as_deref().is_some_and(|v| values.contains(&v))
        });
    closed && !linear
}

struct WayShape {
    point: Coord,
    bbox: [f64; 4],
    length: f64,
}

fn way_shape(scan: &Scan, way: &WayCandidate) -> Option<WayShape> {
    let coords = scan.node_store.coords(&way.refs);
    let bbox = bbox(coords.iter().copied())?;
    let area_point = is_area(&way.tagged, &way.refs)
        .then(|| Area::from_rings(vec![coords.clone()])?.point_on_surface())
        .flatten();
    Some(WayShape {
        point: area_point.or_else(|| line_midpoint(&coords))?,
        bbox,
        length: line_length(&coords),
    })
}

struct RelationShape {
    area: Option<Area>,
    point: Option<Coord>,
    bbox: Option<[f64; 4]>,
}

fn relation_shape(scan: &Scan, relation: &RelationCandidate) -> RelationShape {
    let ways: Vec<Vec<i64>> = relation
        .members
        .iter()
        .filter(|m| m.way && matches!(m.role, Role::Outer | Role::Inner))
        .filter_map(|m| scan.member_way(m.id))
        .collect();
    let lines: Vec<Vec<Coord>> = ways
        .iter()
        .map(|refs| scan.node_store.coords(refs))
        .collect();
    let bbox = bbox(lines.iter().flatten().copied());
    let rings = join_rings(ways)
        .iter()
        .map(|ring| scan.node_store.coords(ring))
        .collect();
    let area = Area::from_rings(rings);
    let point = area.as_ref().and_then(Area::point_on_surface).or_else(|| {
        let longest = lines
            .iter()
            .max_by(|a, b| line_length(a).total_cmp(&line_length(b)))?;
        line_midpoint(longest)
    });
    RelationShape { area, point, bbox }
}

struct RelationRecords<'a> {
    scan: &'a Scan,
    containment: &'a Containment,
    admin_index_by_relation: &'a HashMap<i64, usize>,
    member_nodes: &'a HashMap<i64, MemberNode>,
}

impl RelationRecords<'_> {
    // the record and the member nodes it absorbed
    fn record(&self, relation: &RelationCandidate) -> Option<(Placed, Vec<i64>)> {
        let admin_area = self
            .admin_index_by_relation
            .get(&relation.id)
            .map(|index| self.containment.admin[*index].geometry.area());
        let shape = match admin_area {
            Some(area) => RelationShape {
                point: area.point_on_surface(),
                bbox: Some(area.bbox()),
                area: None,
            },
            None => relation_shape(self.scan, relation),
        };
        let area = admin_area.or(shape.area.as_ref());
        // a boundary whose rings never close was cut by the extract edge
        if is_admin(relation) && area.is_none() {
            return None;
        }
        let label = merged_node(relation, self.member_nodes, Role::Label);
        let centre = merged_node(relation, self.member_nodes, Role::AdminCentre);
        let inside = |node: &MemberNode| area.is_none_or(|a| a.contains(node.coord));
        let point = label
            .filter(|(_, n)| inside(n))
            .or(centre.filter(|(_, n)| inside(n)))
            .map(|(_, n)| n.coord)
            .or(shape.point)?;
        let mut record = named_record(
            &relation.tagged,
            (OsmType::Relation, relation.id),
            point,
            area.map(Area::bbox).or(shape.bbox),
        );
        let mut merged = Vec::new();
        if is_admin(relation) {
            for (id, node) in label.iter().chain(centre.iter()) {
                record.place = record.place.or(node.tagged.place_class());
                record.population = record.population.or(node.tagged.population);
                record.notable |= node.tagged.notable;
                record.languages = record.languages.max(node.tagged.languages);
                merged.push(*id);
            }
        }
        Some((place(self.containment, record, Vec::new()), merged))
    }
}

struct MemberNode {
    coord: Coord,
    tagged: Tagged,
}

// a label or admin_centre node that names the same place merges into its boundary
fn merged_node<'a>(
    relation: &RelationCandidate,
    member_nodes: &'a HashMap<i64, MemberNode>,
    role: Role,
) -> Option<(i64, &'a MemberNode)> {
    // a canton's admin_centre is its capital city, even when the two share a name
    let municipal = relation.tagged.admin_level.unwrap_or(0) >= MIN_MUNICIPAL_LEVEL;
    if role == Role::AdminCentre && !municipal {
        return None;
    }
    let own_name = normalize_for_match(relation.tagged.name.as_deref()?);
    relation
        .members
        .iter()
        .filter(|m| !m.way && m.role == role)
        .find_map(|m| {
            let node = member_nodes.get(&m.id)?;
            let same = node
                .tagged
                .name
                .as_deref()
                .is_some_and(|name| normalize_for_match(name) == own_name);
            same.then_some((m.id, node))
        })
}

fn is_admin(relation: &RelationCandidate) -> bool {
    relation.tagged.kind() == FeatureKind::Boundary
}

fn area_names(tagged: &Tagged) -> Vec<String> {
    let mut names = tagged.all_names();
    names.extend(tagged.country_code.clone());
    names.extend(tagged.subdivision_code.clone());
    names
}

fn log_step(started: Instant, message: String) {
    eprintln!("[{:>7.1} s] {message}", started.elapsed().as_secs_f64());
}

pub fn build(input: &BuildInput) -> Result<IndexSummary, BuildError> {
    let started = Instant::now();
    let scratch = input.out.join(SCRATCH_DIRECTORY);
    fs::create_dir_all(&scratch)?;
    let mut writer = IndexWriter::create(input.out)?;

    let scan = pbf::scan(input.pbf, Selection::NamedObjects, &scratch)?;
    log_step(
        started,
        format!(
            "scanned {}: {} relations",
            input.pbf.display(),
            scan.relations.len()
        ),
    );

    let merge_candidates: HashSet<i64> = scan
        .relations
        .iter()
        .filter(|r| is_admin(r))
        .flat_map(|r| r.members.iter())
        .filter(|m| !m.way)
        .map(|m| m.id)
        .collect();
    let mut member_nodes = HashMap::new();
    let mut settlements = Vec::new();
    for node in scan.nodes()? {
        let node = node?;
        if let Some(class) = node.tagged.place_class()
            && Settlement::radius_km(class).is_some()
            && let Some(display) = node.tagged.context_name()
        {
            settlements.push(Settlement {
                name: AreaName::new(display, &node.tagged.all_names()),
                class,
                coord: node.coord,
            });
        }
        if merge_candidates.contains(&node.id) {
            member_nodes.insert(
                node.id,
                MemberNode {
                    coord: node.coord,
                    tagged: node.tagged,
                },
            );
        }
    }

    let admin_shapes: Vec<(usize, Area)> = scan
        .relations
        .par_iter()
        .enumerate()
        .filter(|(_, relation)| {
            is_admin(relation)
                && relation.tagged.admin_level.unwrap_or(u8::MAX) <= MAX_CONTEXT_LEVEL
        })
        .filter_map(|(index, relation)| Some((index, relation_shape(&scan, relation).area?)))
        .collect();
    let mut admin = Vec::new();
    let mut admin_index_by_relation = HashMap::new();
    for (index, area) in admin_shapes {
        let relation = &scan.relations[index];
        let level = relation.tagged.admin_level.unwrap_or(u8::MAX);
        let area_id = writer.add_area(&area_names(&relation.tagged), level);
        admin_index_by_relation.insert(relation.id, admin.len());
        admin.push(AdminArea {
            area_id,
            level,
            name: AreaName::new(
                relation.tagged.context_name().unwrap_or_default(),
                &relation.tagged.all_names(),
            ),
            country_code: relation.tagged.country_code.clone(),
            geometry: BandedArea::new(area),
        });
    }
    let admin_vertices: usize = admin.iter().map(|a| a.geometry.area().vertex_count()).sum();
    log_step(
        started,
        format!(
            "assembled {} admin areas ({admin_vertices} vertices), {} settlements",
            admin.len(),
            settlements.len()
        ),
    );
    let containment = Containment::new(admin, settlements);
    let mut address_scans = HashMap::new();
    let mut absorbable = Absorbable::new();
    for (index, path) in input.addresses.iter().enumerate() {
        if !is_pbf(path) {
            continue;
        }
        let address_scratch = scratch.join(format!("addresses-{index}"));
        fs::create_dir_all(&address_scratch)?;
        let address_scan = pbf::scan(path, Selection::Addresses, &address_scratch)?;
        collect_absorbable(&address_scan, &mut absorbable)?;
        address_scans.insert(index, address_scan);
    }
    let mut sink = Sink {
        writer: &mut writer,
        settlement_areas: HashMap::new(),
        absorbed: HashSet::new(),
    };

    let relations = RelationRecords {
        scan: &scan,
        containment: &containment,
        admin_index_by_relation: &admin_index_by_relation,
        member_nodes: &member_nodes,
    };
    let mut merged_nodes = HashSet::new();
    for chunk in scan.relations.chunks(BATCH) {
        let outcomes: Vec<(Placed, Vec<i64>)> = chunk
            .par_iter()
            .filter_map(|relation| relations.record(relation))
            .collect();
        for (placed, merged) in outcomes {
            merged_nodes.extend(merged);
            sink.commit(&containment, placed)?;
        }
    }
    log_step(started, "relations indexed".to_string());

    let mut batch = Vec::with_capacity(BATCH);
    let flush_nodes = |sink: &mut Sink, batch: &mut Vec<pbf::NodeCandidate>| -> io::Result<()> {
        let placed: Vec<Placed> = batch
            .par_drain(..)
            .filter(|node| !merged_nodes.contains(&node.id))
            .map(|node| {
                let record = named_record(&node.tagged, (OsmType::Node, node.id), node.coord, None);
                place_named(&containment, &absorbable, record)
            })
            .collect();
        sink.commit_all(&containment, placed)
    };
    for node in scan.nodes()? {
        batch.push(node?);
        if batch.len() >= BATCH {
            flush_nodes(&mut sink, &mut batch)?;
        }
    }
    flush_nodes(&mut sink, &mut batch)?;
    log_step(started, "nodes indexed".to_string());

    let mut streets: HashMap<(String, StreetGroupArea), StreetGroup> = HashMap::new();
    let mut way_batch = Vec::with_capacity(BATCH);
    let mut flush_ways = |sink: &mut Sink, batch: &mut Vec<WayCandidate>| -> io::Result<()> {
        let outcomes: Vec<WayOutcome> = batch
            .par_drain(..)
            .filter_map(|way| way_outcome(&scan, &containment, &absorbable, way))
            .collect();
        for outcome in outcomes {
            match outcome {
                WayOutcome::Record(placed) => sink.commit(&containment, *placed)?,
                WayOutcome::Street(key, part) => match streets.get_mut(&key) {
                    Some(group) => group.absorb(part),
                    None => {
                        streets.insert(key, part);
                    }
                },
            }
        }
        Ok(())
    };
    for way in scan.ways()? {
        way_batch.push(way?);
        if way_batch.len() >= BATCH {
            flush_ways(&mut sink, &mut way_batch)?;
        }
    }
    flush_ways(&mut sink, &mut way_batch)?;
    let street_count = streets.len();
    let street_records: Vec<Placed> = streets
        .into_par_iter()
        .map(|(_, group)| place_named(&containment, &absorbable, group.record()))
        .collect();
    sink.commit_all(&containment, street_records)?;
    log_step(
        started,
        format!("ways indexed, {street_count} merged streets"),
    );
    drop(scan);

    for (index, path) in input.addresses.iter().enumerate() {
        let coverage = match address_scans.remove(&index) {
            Some(address_scan) => add_pbf_addresses(&mut sink, &containment, &address_scan)?,
            None => add_addresses(&mut sink, &containment, path)?,
        };
        if let Some(coverage) = coverage {
            sink.writer.add_coverage(coverage);
        }
        log_step(
            started,
            format!("addresses from {} indexed", path.display()),
        );
    }

    let summary = writer.finish()?;
    fs::remove_dir_all(&scratch)?;
    log_step(started, format!("index written to {}", input.out.display()));
    Ok(summary)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum StreetGroupArea {
    Admin(u32),
    Cell(i32, i32),
}

struct StreetGroup {
    name: String,
    variants: Vec<String>,
    way_id: i64,
    value: Option<String>,
    point: Coord,
    length: f64,
    bbox: [f64; 4],
}

impl StreetGroup {
    fn absorb(&mut self, part: StreetGroup) {
        self.bbox = union(self.bbox, part.bbox);
        for variant in part.variants.iter().chain([&part.name]) {
            if *variant != self.name && !self.variants.contains(variant) {
                self.variants.push(variant.clone());
            }
        }
        if part.length > self.length {
            let previous_name = std::mem::replace(&mut self.name, part.name);
            if !self.variants.contains(&previous_name) {
                self.variants.push(previous_name);
            }
            self.variants.retain(|v| *v != self.name);
            self.way_id = part.way_id;
            self.value = part.value;
            self.point = part.point;
            self.length = part.length;
        }
    }

    fn record(self) -> Record {
        let mut record = Record::new(FeatureKind::Street, self.point[0], self.point[1]);
        record.address.street = Some(self.name.clone());
        record.name = Some(self.name);
        record.name_variants = self.variants;
        record.osm = Some((OsmType::Way, self.way_id));
        record.osm_key = Some("highway".to_string());
        record.osm_value = self.value;
        record.bbox = Some(self.bbox);
        record
    }
}

enum WayOutcome {
    Record(Box<Placed>),
    Street((String, StreetGroupArea), StreetGroup),
}

fn way_outcome(
    scan: &Scan,
    containment: &Containment,
    absorbable: &Absorbable,
    way: WayCandidate,
) -> Option<WayOutcome> {
    let shape = way_shape(scan, &way)?;
    if way.tagged.kind() != FeatureKind::Street {
        let record = named_record(
            &way.tagged,
            (OsmType::Way, way.id),
            shape.point,
            Some(shape.bbox),
        );
        return Some(WayOutcome::Record(Box::new(place_named(
            containment,
            absorbable,
            record,
        ))));
    }
    let name = way.tagged.name.clone()?;
    let located: Located = containment.locate(shape.point);
    let group_area = match located.admin.last() {
        Some(index) => StreetGroupArea::Admin(containment.admin[*index].area_id),
        None => StreetGroupArea::Cell(
            (shape.point[0] / STREET_CELL_DEGREES).floor() as i32,
            (shape.point[1] / STREET_CELL_DEGREES).floor() as i32,
        ),
    };
    let key = (normalize_for_match(&name), group_area);
    Some(WayOutcome::Street(
        key,
        StreetGroup {
            name,
            variants: way.tagged.variants,
            way_id: way.id,
            value: way.tagged.value,
            point: shape.point,
            length: shape.length,
            bbox: shape.bbox,
        },
    ))
}

fn address_named_areas(record: &Record) -> Vec<String> {
    [
        record.address.city.clone(),
        record.address.state.clone(),
        record.address.country.clone(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn place_addresses(containment: &Containment, records: Vec<Record>) -> Vec<Placed> {
    records
        .into_par_iter()
        .map(|record| {
            let named_areas = address_named_areas(&record);
            place(containment, record, named_areas)
        })
        .collect()
}

fn is_pbf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pbf"))
}

fn collect_absorbable(scan: &Scan, absorbable: &mut Absorbable) -> io::Result<()> {
    let mut keep = |key: OsmKey, tagged: Tagged| {
        if tagged.also_named {
            absorbable.insert(
                key,
                AddressFill {
                    house_number: tagged.house_number,
                    street: tagged.street,
                    postcode: tagged.postcode,
                },
            );
        }
    };
    for node in scan.nodes()? {
        let node = node?;
        keep((OsmType::Node, node.id), node.tagged);
    }
    for way in scan.ways()? {
        let way = way?;
        keep((OsmType::Way, way.id), way.tagged);
    }
    Ok(())
}

fn add_addresses(
    sink: &mut Sink,
    containment: &Containment,
    path: &Path,
) -> Result<Option<Coverage>, BuildError> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let mut points = Vec::new();
    match extension.as_str() {
        "geojson" | "json" => {
            let text = fs::read_to_string(path)?;
            let records = read_geojson(&text).map_err(|source| BuildError::GeoJson {
                path: path.to_path_buf(),
                source,
            })?;
            points.extend(records.iter().map(|r| (r.lon, r.lat)));
            sink.commit_all(containment, place_addresses(containment, records))?;
        }
        _ => {
            let mut batch = Vec::with_capacity(BATCH);
            read_openaddresses(fs::File::open(path)?, |record| {
                points.push((record.lon, record.lat));
                batch.push(record);
                if batch.len() >= BATCH {
                    let records = std::mem::take(&mut batch);
                    sink.commit_all(containment, place_addresses(containment, records))?;
                }
                Ok(())
            })
            .map_err(|source| BuildError::Csv {
                path: path.to_path_buf(),
                source,
            })?;
            sink.commit_all(containment, place_addresses(containment, batch))?;
        }
    }
    Ok(padded_extent(&points))
}

fn add_pbf_addresses(
    sink: &mut Sink,
    containment: &Containment,
    scan: &Scan,
) -> io::Result<Option<Coverage>> {
    let mut points = Vec::new();
    let commit = |sink: &mut Sink, batch: Vec<(Tagged, (OsmType, i64), Coord)>| {
        let placed: Vec<Placed> = batch
            .into_par_iter()
            .map(|(tagged, osm, point)| {
                let mut record = named_record(&tagged, osm, point, None);
                record.kind = FeatureKind::Address;
                let location = locate(containment, &mut record);
                let mut named_areas = Vec::new();
                // addr:city only when no boundary or settlement named the city
                if record.address.city.is_none()
                    && let Some(city) = tagged.city
                {
                    record.address.city = Some(city.clone());
                    named_areas.push(city);
                }
                Placed::new(record, location, named_areas)
            })
            .collect();
        sink.commit_all(containment, placed)
    };
    let mut batch = Vec::with_capacity(BATCH);
    for node in scan.nodes()? {
        let node = node?;
        points.push((node.coord[0], node.coord[1]));
        if sink.absorbed.contains(&(OsmType::Node, node.id)) {
            continue;
        }
        batch.push((node.tagged, (OsmType::Node, node.id), node.coord));
        if batch.len() >= BATCH {
            commit(sink, std::mem::take(&mut batch))?;
        }
    }
    for way in scan.ways()? {
        let way = way?;
        let Some(shape) = way_shape(scan, &way) else {
            continue;
        };
        points.push((shape.point[0], shape.point[1]));
        if sink.absorbed.contains(&(OsmType::Way, way.id)) {
            continue;
        }
        batch.push((way.tagged, (OsmType::Way, way.id), shape.point));
        if batch.len() >= BATCH {
            commit(sink, std::mem::take(&mut batch))?;
        }
    }
    commit(sink, batch)?;
    Ok(scan.header_bbox.or_else(|| padded_extent(&points)))
}
