use crate::classify::{ObjectType, Tagged, Tags, classify_address, classify_named};
use crate::codec::{Decoder, Encoder};
use crate::geometry::Coord;
use geokode_core::index::Coverage;
use geokode_core::sort::{ExternalSorter, SortedItems};
use memmap2::{Mmap, MmapMut};
use osmpbf::{BlobDecode, BlobReader, Element, PrimitiveBlock, RelMemberType};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const BLOB_HAS_NODES: u8 = 1;
const BLOB_HAS_WAYS: u8 = 2;
const BLOB_HAS_RELATIONS: u8 = 4;
const NODE_ID_SORT_MEMORY: usize = 512 * 1024 * 1024;
const COORD_BYTES: usize = 8;
const ID_BYTES: usize = 8;
const DECIMICROS_PER_DEGREE: f64 = 1e7;
// offsets keep every stored coordinate non-zero, so zero bytes mean the node is missing
const STORED_LON_OFFSET: i64 = 2_000_000_000;
const STORED_LAT_OFFSET: i64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    NamedObjects,
    Addresses,
}

pub struct NodeCandidate {
    pub id: i64,
    pub coord: Coord,
    pub tagged: Tagged,
}

pub struct WayCandidate {
    pub id: i64,
    pub tagged: Tagged,
    pub refs: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Outer,
    Inner,
    Label,
    AdminCentre,
}

#[derive(Debug, Clone, Copy)]
pub struct Member {
    pub way: bool,
    pub id: i64,
    pub role: Role,
}

pub struct RelationCandidate {
    pub id: i64,
    pub tagged: Tagged,
    pub members: Vec<Member>,
}

fn osm_error(error: osmpbf::Error) -> io::Error {
    io::Error::other(error.to_string())
}

fn degrees(decimicro: i32) -> f64 {
    f64::from(decimicro) / DECIMICROS_PER_DEGREE
}

pub trait Candidate: Sized {
    fn write(&self, out: &mut Encoder<BufWriter<File>>) -> io::Result<()>;
    fn read(input: &mut Decoder<BufReader<File>>) -> io::Result<Option<Self>>;
}

impl Candidate for NodeCandidate {
    fn write(&self, out: &mut Encoder<BufWriter<File>>) -> io::Result<()> {
        out.signed(self.id)?;
        out.signed(i64::from(spatial_units(self.coord[0])))?;
        out.signed(i64::from(spatial_units(self.coord[1])))?;
        self.tagged.write(out)
    }

    fn read(input: &mut Decoder<BufReader<File>>) -> io::Result<Option<Self>> {
        let Some(id) = input.next_signed()? else {
            return Ok(None);
        };
        let lon = degrees(input.signed()? as i32);
        let lat = degrees(input.signed()? as i32);
        Ok(Some(NodeCandidate {
            id,
            coord: [lon, lat],
            tagged: Tagged::read(input)?,
        }))
    }
}

impl Candidate for WayCandidate {
    fn write(&self, out: &mut Encoder<BufWriter<File>>) -> io::Result<()> {
        out.unsigned(self.id as u64)?;
        self.tagged.write(out)?;
        out.ids(&self.refs)
    }

    fn read(input: &mut Decoder<BufReader<File>>) -> io::Result<Option<Self>> {
        let Some(id) = input.next_unsigned()? else {
            return Ok(None);
        };
        Ok(Some(WayCandidate {
            id: id as i64,
            tagged: Tagged::read(input)?,
            refs: input.ids()?,
        }))
    }
}

fn spatial_units(degrees: f64) -> i32 {
    (degrees * DECIMICROS_PER_DEGREE).round() as i32
}

pub struct CandidateFile<T> {
    decoder: Decoder<BufReader<File>>,
    item: PhantomData<T>,
}

impl<T: Candidate> Iterator for CandidateFile<T> {
    type Item = io::Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        T::read(&mut self.decoder).transpose()
    }
}

pub struct NodeStore {
    ids: Mmap,
    coords: Mmap,
}

impl NodeStore {
    fn id_at(ids: &[u8], index: usize) -> i64 {
        let mut bytes = [0u8; ID_BYTES];
        bytes.copy_from_slice(&ids[index * ID_BYTES..(index + 1) * ID_BYTES]);
        i64::from_le_bytes(bytes)
    }

    fn lower_bound(ids: &[u8], id: i64) -> usize {
        let (mut low, mut high) = (0, ids.len() / ID_BYTES);
        while low < high {
            let middle = (low + high) / 2;
            if Self::id_at(ids, middle) < id {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low
    }

    pub fn get(&self, id: i64) -> Option<Coord> {
        let index = Self::lower_bound(&self.ids, id);
        if index * ID_BYTES >= self.ids.len() || Self::id_at(&self.ids, index) != id {
            return None;
        }
        let bytes = &self.coords[index * COORD_BYTES..(index + 1) * COORD_BYTES];
        let lon = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let lat = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        if lon == 0 && lat == 0 {
            return None;
        }
        Some([
            degrees((i64::from(lon) - STORED_LON_OFFSET) as i32),
            degrees((i64::from(lat) - STORED_LAT_OFFSET) as i32),
        ])
    }

    pub fn coords(&self, refs: &[i64]) -> Vec<Coord> {
        refs.iter().filter_map(|id| self.get(*id)).collect()
    }
}

fn stored_coord(lon: i32, lat: i32) -> [u8; COORD_BYTES] {
    let mut out = [0u8; COORD_BYTES];
    out[..4].copy_from_slice(&((i64::from(lon) + STORED_LON_OFFSET) as u32).to_le_bytes());
    out[4..].copy_from_slice(&((i64::from(lat) + STORED_LAT_OFFSET) as u32).to_le_bytes());
    out
}

pub struct Scan {
    pub header_bbox: Option<Coverage>,
    nodes_path: PathBuf,
    ways_path: PathBuf,
    pub relations: Vec<RelationCandidate>,
    member_way_index: Vec<(i64, u64)>,
    member_ways: Option<Mmap>,
    pub node_store: NodeStore,
}

impl Scan {
    fn open<T>(path: &Path) -> io::Result<CandidateFile<T>> {
        Ok(CandidateFile {
            decoder: Decoder::new(BufReader::new(File::open(path)?)),
            item: PhantomData,
        })
    }

    pub fn nodes(&self) -> io::Result<CandidateFile<NodeCandidate>> {
        Self::open(&self.nodes_path)
    }

    pub fn ways(&self) -> io::Result<CandidateFile<WayCandidate>> {
        Self::open(&self.ways_path)
    }

    pub fn member_way(&self, id: i64) -> Option<Vec<i64>> {
        let index = self
            .member_way_index
            .binary_search_by_key(&id, |(way, _)| *way)
            .ok()?;
        let offset = self.member_way_index[index].1 as usize;
        let bytes = self.member_ways.as_ref()?;
        Decoder::new(&bytes[offset..]).ids().ok()
    }
}

// the scratch files are ours alone and are not truncated while mapped
fn map_read(path: &Path) -> io::Result<Mmap> {
    let file = File::open(path)?;
    unsafe { Mmap::map(&file) }
}

fn read_header_bbox(path: &Path) -> io::Result<Option<Coverage>> {
    let mut blobs = BlobReader::from_path(path).map_err(osm_error)?;
    let Some(blob) = blobs.next() else {
        return Ok(None);
    };
    let BlobDecode::OsmHeader(header) = blob.map_err(osm_error)?.decode().map_err(osm_error)?
    else {
        return Ok(None);
    };
    Ok(header.bbox().map(|bbox| Coverage {
        min_lon: bbox.left.min(bbox.right),
        min_lat: bbox.bottom.min(bbox.top),
        max_lon: bbox.left.max(bbox.right),
        max_lat: bbox.bottom.max(bbox.top),
    }))
}

fn for_each_block<T: Send>(
    path: &Path,
    wanted: impl Fn(usize) -> bool + Sync,
    read_block: impl Fn(usize, &PrimitiveBlock) -> T + Sync,
    absorb: impl FnMut(T) -> io::Result<()> + Send,
) -> io::Result<()> {
    let absorb = Mutex::new(absorb);
    BlobReader::from_path(path)
        .map_err(osm_error)?
        .enumerate()
        .par_bridge()
        .try_for_each(|(index, blob)| {
            let blob = blob.map_err(osm_error)?;
            if !wanted(index) {
                return Ok(());
            }
            if let BlobDecode::OsmData(block) = blob.decode().map_err(osm_error)? {
                let output = read_block(index, &block);
                (absorb.lock().expect("a scan worker panicked"))(output)?;
            }
            Ok(())
        })
}

#[derive(Default)]
struct FirstPassBlock {
    index: usize,
    kinds: u8,
    nodes: Vec<NodeCandidate>,
    ways: Vec<WayCandidate>,
    relations: Vec<RelationCandidate>,
}

fn member_role(way: bool, role: &str) -> Option<Role> {
    match (way, role) {
        (true, "outer" | "" | "main_stream") => Some(Role::Outer),
        (true, "inner") => Some(Role::Inner),
        (false, "label") => Some(Role::Label),
        (false, "admin_centre") => Some(Role::AdminCentre),
        _ => None,
    }
}

fn first_pass_block(index: usize, block: &PrimitiveBlock, selection: Selection) -> FirstPassBlock {
    let mut out = FirstPassBlock {
        index,
        ..FirstPassBlock::default()
    };
    let classify = |tags: &Tags, object| match selection {
        Selection::NamedObjects => classify_named(tags, object),
        Selection::Addresses => classify_address(tags),
    };
    for element in block.elements() {
        match element {
            Element::DenseNode(node) => {
                out.kinds |= BLOB_HAS_NODES;
                let tags = Tags::new(node.tags());
                if tags.is_empty() {
                    continue;
                }
                if let Some(tagged) = classify(&tags, ObjectType::Node) {
                    out.nodes.push(NodeCandidate {
                        id: node.id(),
                        coord: [degrees(node.decimicro_lon()), degrees(node.decimicro_lat())],
                        tagged,
                    });
                }
            }
            Element::Node(node) => {
                out.kinds |= BLOB_HAS_NODES;
                if let Some(tagged) = classify(&Tags::new(node.tags()), ObjectType::Node) {
                    out.nodes.push(NodeCandidate {
                        id: node.id(),
                        coord: [degrees(node.decimicro_lon()), degrees(node.decimicro_lat())],
                        tagged,
                    });
                }
            }
            Element::Way(way) => {
                out.kinds |= BLOB_HAS_WAYS;
                if let Some(tagged) = classify(&Tags::new(way.tags()), ObjectType::Way) {
                    out.ways.push(WayCandidate {
                        id: way.id(),
                        tagged,
                        refs: way.refs().collect(),
                    });
                }
            }
            Element::Relation(relation) => {
                out.kinds |= BLOB_HAS_RELATIONS;
                if selection == Selection::Addresses {
                    continue;
                }
                let tags = Tags::new(relation.tags());
                if !tags.relation_type_is_indexed() {
                    continue;
                }
                let Some(tagged) = classify(&tags, ObjectType::Relation) else {
                    continue;
                };
                let members = relation
                    .members()
                    .filter_map(|member| {
                        let way = match member.member_type {
                            RelMemberType::Way => true,
                            RelMemberType::Node => false,
                            RelMemberType::Relation => return None,
                        };
                        let role = member_role(way, member.role().unwrap_or_default())?;
                        Some(Member {
                            way,
                            id: member.member_id,
                            role,
                        })
                    })
                    .collect();
                out.relations.push(RelationCandidate {
                    id: relation.id(),
                    tagged,
                    members,
                });
            }
        }
    }
    out
}

fn write_node_ids(sorted: SortedItems<i64>, path: &Path) -> io::Result<usize> {
    let mut out = BufWriter::new(File::create(path)?);
    let mut previous = None;
    let mut count = 0;
    for id in sorted {
        let id = id?;
        if previous == Some(id) {
            continue;
        }
        out.write_all(&id.to_le_bytes())?;
        previous = Some(id);
        count += 1;
    }
    out.flush()?;
    Ok(count)
}

// the passes the brief orders: candidates and relations, member ways, then node coordinates
pub fn scan(path: &Path, selection: Selection, scratch: &Path) -> io::Result<Scan> {
    let name = |suffix: &str| scratch.join(format!("{selection:?}.{suffix}").to_lowercase());
    let header_bbox = read_header_bbox(path)?;
    let nodes_path = name("nodes");
    let ways_path = name("ways");
    let mut needed = ExternalSorter::<i64>::new(
        scratch,
        &format!("{selection:?}-needed"),
        NODE_ID_SORT_MEMORY,
    );
    let mut relations = Vec::new();
    let mut blob_kinds: Vec<u8> = Vec::new();
    {
        let mut node_out = Encoder::new(BufWriter::new(File::create(&nodes_path)?));
        let mut way_out = Encoder::new(BufWriter::new(File::create(&ways_path)?));
        for_each_block(
            path,
            |_| true,
            |index, block| first_pass_block(index, block, selection),
            |block: FirstPassBlock| {
                if blob_kinds.len() <= block.index {
                    blob_kinds.resize(block.index + 1, 0);
                }
                blob_kinds[block.index] = block.kinds;
                for node in &block.nodes {
                    node.write(&mut node_out)?;
                }
                for way in &block.ways {
                    way.write(&mut way_out)?;
                    for id in &way.refs {
                        needed.push(*id)?;
                    }
                }
                relations.extend(block.relations);
                Ok(())
            },
        )?;
        node_out.finish()?;
        way_out.finish()?;
    }
    relations.sort_by_key(|relation: &RelationCandidate| relation.id);

    let mut member_way_ids: Vec<i64> = relations
        .iter()
        .flat_map(|relation| relation.members.iter())
        .filter(|member| member.way)
        .map(|member| member.id)
        .collect();
    member_way_ids.sort_unstable();
    member_way_ids.dedup();
    for member in relations.iter().flat_map(|r| r.members.iter()) {
        if !member.way {
            needed.push(member.id)?;
        }
    }

    let member_ways_path = name("member-ways");
    let mut member_way_index = Vec::new();
    if !member_way_ids.is_empty() {
        let mut out = BufWriter::new(File::create(&member_ways_path)?);
        let mut offset = 0u64;
        let has_ways = |index: usize| {
            blob_kinds
                .get(index)
                .is_some_and(|k| k & BLOB_HAS_WAYS != 0)
        };
        for_each_block(
            path,
            has_ways,
            |_, block| {
                block
                    .elements()
                    .filter_map(|element| match element {
                        Element::Way(way) if member_way_ids.binary_search(&way.id()).is_ok() => {
                            Some((way.id(), way.refs().collect::<Vec<i64>>()))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            },
            |ways| {
                for (id, refs) in ways {
                    let mut bytes = Vec::new();
                    let mut encoder = Encoder::new(&mut bytes);
                    encoder.ids(&refs)?;
                    encoder.finish()?;
                    out.write_all(&bytes)?;
                    member_way_index.push((id, offset));
                    offset += bytes.len() as u64;
                    for node in refs {
                        needed.push(node)?;
                    }
                }
                Ok(())
            },
        )?;
        out.flush()?;
        member_way_index.sort_unstable();
    }

    let ids_path = name("node-ids");
    let coords_path = name("node-coords");
    let count = write_node_ids(needed.finish()?, &ids_path)?;
    let ids = map_read(&ids_path)?;
    let coords_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&coords_path)?;
    coords_file.set_len((count * COORD_BYTES) as u64)?;
    if count > 0 {
        // the coordinate file is ours alone, created just above
        let coords = Mutex::new(unsafe { MmapMut::map_mut(&coords_file)? });
        let has_nodes = |index: usize| {
            blob_kinds
                .get(index)
                .is_some_and(|k| k & BLOB_HAS_NODES != 0)
        };
        for_each_block(
            path,
            has_nodes,
            |_, block| located_nodes(block, &ids),
            |found| {
                let mut coords = coords.lock().expect("a scan worker panicked");
                for (slot, bytes) in found {
                    coords[slot * COORD_BYTES..(slot + 1) * COORD_BYTES].copy_from_slice(&bytes);
                }
                Ok(())
            },
        )?;
        coords
            .into_inner()
            .expect("a scan worker panicked")
            .flush()?;
    }
    let member_ways = if member_way_index.is_empty() {
        None
    } else {
        Some(map_read(&member_ways_path)?)
    };
    Ok(Scan {
        header_bbox,
        nodes_path,
        ways_path,
        relations,
        member_way_index,
        member_ways,
        node_store: NodeStore {
            ids,
            coords: map_read(&coords_path)?,
        },
    })
}

// walks the block's nodes against the sorted needed ids, a binary search only when ids step back
fn located_nodes(block: &PrimitiveBlock, ids: &[u8]) -> Vec<(usize, [u8; COORD_BYTES])> {
    let total = ids.len() / ID_BYTES;
    let mut found = Vec::new();
    let mut cursor = 0;
    let mut previous = i64::MIN;
    let mut visit = |id: i64, lon: i32, lat: i32| {
        if id < previous || previous == i64::MIN {
            cursor = NodeStore::lower_bound(ids, id);
        }
        previous = id;
        while cursor < total && NodeStore::id_at(ids, cursor) < id {
            cursor += 1;
        }
        if cursor < total && NodeStore::id_at(ids, cursor) == id {
            found.push((cursor, stored_coord(lon, lat)));
        }
    };
    for element in block.elements() {
        match element {
            Element::DenseNode(node) => {
                visit(node.id(), node.decimicro_lon(), node.decimicro_lat())
            }
            Element::Node(node) => visit(node.id(), node.decimicro_lon(), node.decimicro_lat()),
            _ => {}
        }
    }
    found
}
