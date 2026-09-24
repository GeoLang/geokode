use std::cmp::Ordering;
use std::collections::BinaryHeap;

const POINT_BYTES: usize = 12;
const UNITS_PER_DEGREE: f64 = 1e7;
const KM_PER_DEGREE: f64 = 111.32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedPoint {
    pub lon: i32,
    pub lat: i32,
    pub id: u32,
}

impl IndexedPoint {
    pub fn new(lon: f64, lat: f64, id: u32) -> Self {
        Self {
            lon: to_units(lon),
            lat: to_units(lat),
            id,
        }
    }

    fn axis(&self, axis: usize) -> i32 {
        if axis == 0 { self.lon } else { self.lat }
    }
}

pub fn to_units(degrees: f64) -> i32 {
    (degrees * UNITS_PER_DEGREE).round() as i32
}

pub fn to_degrees(units: i32) -> f64 {
    f64::from(units) / UNITS_PER_DEGREE
}

// equirectangular around the first point, good enough under a few hundred km
pub fn distance_km(lon_a: f64, lat_a: f64, lon_b: f64, lat_b: f64) -> f64 {
    let x = (lon_b - lon_a) * lat_a.to_radians().cos();
    let y = lat_b - lat_a;
    x.hypot(y) * KM_PER_DEGREE
}

fn arrange(points: &mut [IndexedPoint], depth: usize) {
    if points.len() <= 1 {
        return;
    }
    let axis = depth % 2;
    let middle = points.len() / 2;
    points.select_nth_unstable_by_key(middle, |point| point.axis(axis));
    let (left, right) = points.split_at_mut(middle);
    arrange(left, depth + 1);
    arrange(&mut right[1..], depth + 1);
}

// an implicit kd-tree: each subrange keeps its median at its middle index
pub fn encode_kd_tree(mut points: Vec<IndexedPoint>) -> Vec<u8> {
    arrange(&mut points, 0);
    let mut bytes = Vec::with_capacity(points.len() * POINT_BYTES);
    for point in points {
        bytes.extend_from_slice(&point.lon.to_le_bytes());
        bytes.extend_from_slice(&point.lat.to_le_bytes());
        bytes.extend_from_slice(&point.id.to_le_bytes());
    }
    bytes
}

pub struct KdTree<D> {
    bytes: D,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Neighbour {
    pub id: u32,
    pub lon: f64,
    pub lat: f64,
    pub distance_km: f64,
}

struct Candidate(f64, usize);

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

struct Search {
    lon: f64,
    lat: f64,
    lon_scale: f64,
    limit: usize,
    max_distance: f64,
    best: BinaryHeap<Candidate>,
}

impl Search {
    fn worst(&self) -> f64 {
        if self.best.len() < self.limit {
            return self.max_distance;
        }
        self.best.peek().map_or(self.max_distance, |c| c.0)
    }
}

impl<D: AsRef<[u8]>> KdTree<D> {
    pub fn new(bytes: D) -> Self {
        Self { bytes }
    }

    pub fn len(&self) -> usize {
        self.bytes.as_ref().len() / POINT_BYTES
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn point(&self, index: usize) -> IndexedPoint {
        let bytes = &self.bytes.as_ref()[index * POINT_BYTES..(index + 1) * POINT_BYTES];
        let field = |at: usize| [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
        IndexedPoint {
            lon: i32::from_le_bytes(field(0)),
            lat: i32::from_le_bytes(field(4)),
            id: u32::from_le_bytes(field(8)),
        }
    }

    pub fn nearest(&self, lon: f64, lat: f64, limit: usize, max_km: f64) -> Vec<Neighbour> {
        if limit == 0 || self.is_empty() {
            return Vec::new();
        }
        let mut search = Search {
            lon,
            lat,
            lon_scale: lat.to_radians().cos(),
            limit,
            max_distance: max_km / KM_PER_DEGREE,
            best: BinaryHeap::new(),
        };
        self.visit(0, self.len(), 0, &mut search);
        let mut found: Vec<Neighbour> = search
            .best
            .into_iter()
            .map(|Candidate(distance, index)| {
                let point = self.point(index);
                Neighbour {
                    id: point.id,
                    lon: to_degrees(point.lon),
                    lat: to_degrees(point.lat),
                    distance_km: distance * KM_PER_DEGREE,
                }
            })
            .collect();
        found.sort_by(|a, b| a.distance_km.total_cmp(&b.distance_km));
        found
    }

    fn visit(&self, start: usize, end: usize, depth: usize, search: &mut Search) {
        if start >= end {
            return;
        }
        let middle = start + (end - start) / 2;
        let point = self.point(middle);
        let dx = (to_degrees(point.lon) - search.lon) * search.lon_scale;
        let dy = to_degrees(point.lat) - search.lat;
        let distance = dx.hypot(dy);
        if distance <= search.worst() {
            search.best.push(Candidate(distance, middle));
            if search.best.len() > search.limit {
                search.best.pop();
            }
        }
        let split = if depth.is_multiple_of(2) { dx } else { dy };
        let (near, far) = if split > 0.0 {
            ((start, middle), (middle + 1, end))
        } else {
            ((middle + 1, end), (start, middle))
        };
        self.visit(near.0, near.1, depth + 1, search);
        if split.abs() <= search.worst() {
            self.visit(far.0, far.1, depth + 1, search);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cities() -> KdTree<Vec<u8>> {
        KdTree::new(encode_kd_tree(vec![
            IndexedPoint::new(-0.128, 51.507, 0),
            IndexedPoint::new(2.352, 48.857, 1),
            IndexedPoint::new(-74.006, 40.713, 2),
            IndexedPoint::new(139.759, 35.682, 3),
            IndexedPoint::new(151.209, -33.868, 4),
        ]))
    }

    #[test]
    fn nearest_comes_first() {
        let found = cities().nearest(0.0, 50.0, 3, f64::INFINITY);
        let ids: Vec<u32> = found.iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![0, 1, 2]);
    }

    #[test]
    fn max_distance_drops_far_points() {
        let found = cities().nearest(-0.1, 51.5, 5, 500.0);
        let ids: Vec<u32> = found.iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![0, 1]);
    }

    #[test]
    fn matches_a_brute_force_scan() {
        let points: Vec<IndexedPoint> = (0..500u32)
            .map(|i| {
                let lon = f64::from((i * 7919) % 360) - 180.0;
                let lat = f64::from((i * 104_729) % 170) - 85.0;
                IndexedPoint::new(lon, lat, i)
            })
            .collect();
        let tree = KdTree::new(encode_kd_tree(points.clone()));
        let (lon, lat) = (12.5, 41.9);
        let found: Vec<u32> = tree
            .nearest(lon, lat, 10, f64::INFINITY)
            .iter()
            .map(|n| n.id)
            .collect();
        let scale = lat.to_radians().cos();
        let mut brute = points.clone();
        brute.sort_by(|a, b| {
            let distance = |p: &IndexedPoint| {
                ((to_degrees(p.lon) - lon) * scale).hypot(to_degrees(p.lat) - lat)
            };
            distance(a).total_cmp(&distance(b))
        });
        let expected: Vec<u32> = brute.iter().take(10).map(|p| p.id).collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn an_empty_tree_finds_nothing() {
        let tree = KdTree::new(encode_kd_tree(Vec::new()));
        assert!(tree.is_empty());
        assert!(tree.nearest(0.0, 0.0, 5, f64::INFINITY).is_empty());
    }
}
