use std::collections::HashMap;

pub type Coord = [f64; 2];

const MIN_RING_NODES: usize = 4;
const EDGES_PER_BAND: usize = 8;
const MAX_BANDS: usize = 1 << 16;
// fractions of the area height
const SCAN_LINES: &[f64] = &[0.5, 0.25, 0.75, 0.375, 0.625, 0.125, 0.875];

pub fn bbox(coords: impl IntoIterator<Item = Coord>) -> Option<[f64; 4]> {
    coords.into_iter().fold(None, |bbox, [x, y]| {
        Some(match bbox {
            None => [x, y, x, y],
            Some([min_x, min_y, max_x, max_y]) => {
                [min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]
            }
        })
    })
}

pub fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn segment_length(a: Coord, b: Coord) -> f64 {
    let scale = ((a[1] + b[1]) / 2.0).to_radians().cos();
    ((b[0] - a[0]) * scale).hypot(b[1] - a[1])
}

pub fn line_length(coords: &[Coord]) -> f64 {
    coords.windows(2).map(|w| segment_length(w[0], w[1])).sum()
}

// a vertex, not an interpolated point
pub fn line_midpoint(coords: &[Coord]) -> Option<Coord> {
    let half = line_length(coords) / 2.0;
    let mut walked = 0.0;
    for window in coords.windows(2) {
        walked += segment_length(window[0], window[1]);
        if walked >= half {
            return Some(window[1]);
        }
    }
    coords.first().copied()
}

// chains that never close are dropped
pub fn join_rings(mut segments: Vec<Vec<i64>>) -> Vec<Vec<i64>> {
    segments.retain(|segment| segment.len() >= 2);
    let mut rings = Vec::new();
    let mut by_endpoint: HashMap<i64, Vec<usize>> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        if segment.first() == segment.last() {
            continue;
        }
        by_endpoint.entry(segment[0]).or_default().push(index);
        by_endpoint
            .entry(*segment.last().unwrap())
            .or_default()
            .push(index);
    }
    let mut used = vec![false; segments.len()];
    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut ring = segments[start].clone();
        while ring.first() != ring.last() {
            let end = *ring.last().unwrap();
            let next = by_endpoint
                .get(&end)
                .and_then(|candidates| candidates.iter().find(|i| !used[**i]).copied());
            let Some(next) = next else {
                break;
            };
            used[next] = true;
            let segment = &segments[next];
            if segment[0] == end {
                ring.extend_from_slice(&segment[1..]);
            } else {
                ring.extend(segment.iter().rev().skip(1));
            }
        }
        if ring.first() == ring.last() && ring.len() >= MIN_RING_NODES {
            rings.push(ring);
        }
    }
    rings
}

// even-odd over all rings, so inner roles are not needed
#[derive(Debug, Clone)]
pub struct Area {
    vertices: Vec<Coord>,
    ring_ends: Vec<usize>,
    bbox: [f64; 4],
}

impl Area {
    pub fn from_rings(rings: Vec<Vec<Coord>>) -> Option<Area> {
        let mut vertices = Vec::new();
        let mut ring_ends = Vec::new();
        for ring in rings {
            if ring.len() < MIN_RING_NODES {
                continue;
            }
            vertices.extend(ring);
            ring_ends.push(vertices.len());
        }
        let bbox = bbox(vertices.iter().copied())?;
        Some(Area {
            vertices,
            ring_ends,
            bbox,
        })
    }

    pub fn bbox(&self) -> [f64; 4] {
        self.bbox
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    fn edges(&self) -> impl Iterator<Item = usize> + '_ {
        let mut start = 0;
        self.ring_ends.iter().flat_map(move |&end| {
            let ring = start..end.saturating_sub(1);
            start = end;
            ring
        })
    }

    fn crossing(&self, edge: usize, y: f64) -> Option<f64> {
        let [ax, ay] = self.vertices[edge];
        let [bx, by] = self.vertices[edge + 1];
        ((ay > y) != (by > y)).then(|| ax + (y - ay) * (bx - ax) / (by - ay))
    }

    pub fn contains(&self, [x, y]: Coord) -> bool {
        self.edges()
            .filter(|edge| self.crossing(*edge, y).is_some_and(|cross| x < cross))
            .count()
            % 2
            == 1
    }

    // middle of the widest inside span on a few scan lines
    pub fn point_on_surface(&self) -> Option<Coord> {
        let [_, min_y, _, max_y] = self.bbox;
        let mut best: Option<(f64, Coord)> = None;
        for fraction in SCAN_LINES {
            let y = min_y + (max_y - min_y) * fraction;
            let mut crossings: Vec<f64> = self
                .edges()
                .filter_map(|edge| self.crossing(edge, y))
                .collect();
            crossings.sort_by(f64::total_cmp);
            for [left, right] in crossings.as_chunks::<2>().0 {
                let width = right - left;
                if best.is_none_or(|(widest, _)| width > widest) {
                    best = Some((width, [(left + right) / 2.0, y]));
                }
            }
        }
        best.map(|(_, point)| point)
    }
}

// edges bucketed by horizontal band
pub struct BandedArea {
    area: Area,
    band_height: f64,
    band_starts: Vec<u32>,
    band_edges: Vec<u32>,
}

impl BandedArea {
    pub fn new(area: Area) -> Self {
        let [_, min_y, _, max_y] = area.bbox;
        let edge_count = area.edges().count();
        let bands = (edge_count / EDGES_PER_BAND).clamp(1, MAX_BANDS);
        let band_height = ((max_y - min_y) / bands as f64).max(f64::MIN_POSITIVE);
        let band_of = |y: f64| (((y - min_y) / band_height) as usize).min(bands - 1);
        let span = |edge: usize| {
            let (a, b) = (area.vertices[edge][1], area.vertices[edge + 1][1]);
            band_of(a.min(b))..=band_of(a.max(b))
        };
        let mut counts = vec![0u32; bands + 1];
        for edge in area.edges() {
            for band in span(edge) {
                counts[band + 1] += 1;
            }
        }
        for band in 0..bands {
            counts[band + 1] += counts[band];
        }
        let mut fill = counts.clone();
        let mut band_edges = vec![0u32; counts[bands] as usize];
        for edge in area.edges() {
            for band in span(edge) {
                band_edges[fill[band] as usize] = edge as u32;
                fill[band] += 1;
            }
        }
        BandedArea {
            area,
            band_height,
            band_starts: counts,
            band_edges,
        }
    }

    pub fn area(&self) -> &Area {
        &self.area
    }

    pub fn contains(&self, [x, y]: Coord) -> bool {
        let [min_x, min_y, max_x, max_y] = self.area.bbox;
        if x < min_x || x > max_x || y < min_y || y > max_y {
            return false;
        }
        let bands = self.band_starts.len() - 1;
        let band = (((y - min_y) / self.band_height) as usize).min(bands - 1);
        let edges =
            &self.band_edges[self.band_starts[band] as usize..self.band_starts[band + 1] as usize];
        edges
            .iter()
            .filter(|edge| {
                self.area
                    .crossing(**edge as usize, y)
                    .is_some_and(|cross| x < cross)
            })
            .count()
            % 2
            == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, size: f64) -> Vec<Coord> {
        vec![
            [x, y],
            [x + size, y],
            [x + size, y + size],
            [x, y + size],
            [x, y],
        ]
    }

    #[test]
    fn open_ways_join_into_a_ring_in_either_direction() {
        let rings = join_rings(vec![vec![1, 2, 3], vec![5, 4, 3], vec![5, 6, 1]]);
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].first(), rings[0].last());
        assert_eq!(rings[0].len(), 7);
    }

    #[test]
    fn a_chain_that_never_closes_is_dropped() {
        let rings = join_rings(vec![vec![1, 2, 3], vec![3, 4], vec![10, 11, 12, 10]]);
        assert_eq!(rings, vec![vec![10, 11, 12, 10]]);
    }

    #[test]
    fn a_hole_is_outside_under_even_odd() {
        let area = Area::from_rings(vec![square(0.0, 0.0, 10.0), square(4.0, 4.0, 2.0)]).unwrap();
        assert!(area.contains([1.0, 1.0]));
        assert!(!area.contains([5.0, 5.0]));
        assert!(!area.contains([11.0, 1.0]));
    }

    #[test]
    fn the_surface_point_avoids_the_hole() {
        let area = Area::from_rings(vec![square(0.0, 0.0, 10.0), square(2.0, 2.0, 6.0)]).unwrap();
        let point = area.point_on_surface().unwrap();
        assert!(area.contains(point), "{point:?}");
    }

    #[test]
    fn a_crescent_gets_a_point_inside_where_its_bbox_centre_is_not() {
        let crescent = vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 1.0],
            [1.0, 1.0],
            [1.0, 9.0],
            [10.0, 9.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [0.0, 0.0],
        ];
        let area = Area::from_rings(vec![crescent]).unwrap();
        assert!(!area.contains([5.0, 5.0]));
        let point = area.point_on_surface().unwrap();
        assert!(area.contains(point), "{point:?}");
    }

    #[test]
    fn banded_containment_agrees_with_the_plain_test() {
        let mut ring: Vec<Coord> = (0..400)
            .map(|i| {
                let angle = f64::from(i) / 400.0 * std::f64::consts::TAU;
                let radius = 5.0 + (angle * 7.0).sin();
                [radius * angle.cos(), radius * angle.sin()]
            })
            .collect();
        ring.push(ring[0]);
        let area = Area::from_rings(vec![ring, square(-1.0, -1.0, 2.0)]).unwrap();
        let banded = BandedArea::new(area.clone());
        for i in 0..40 {
            for j in 0..40 {
                let point = [f64::from(i) * 0.4 - 8.0, f64::from(j) * 0.4 - 8.0];
                assert_eq!(banded.contains(point), area.contains(point), "{point:?}");
            }
        }
    }

    #[test]
    fn the_line_midpoint_is_a_vertex_of_the_line() {
        let line = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [10.0, 0.0]];
        assert_eq!(line_midpoint(&line), Some([10.0, 0.0]));
        let even = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]];
        assert_eq!(line_midpoint(&even), Some([1.0, 0.0]));
    }
}
