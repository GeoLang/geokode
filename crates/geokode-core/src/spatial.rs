//! Spatial index using R-tree for reverse geocoding.

use rstar::{AABB, PointDistance, RTree, RTreeObject};

/// A spatial record associating a location with an address record ID.
#[derive(Debug, Clone, Copy)]
pub struct SpatialRecord {
    pub lat: f64,
    pub lon: f64,
    pub id: u64,
}

impl RTreeObject for SpatialRecord {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_point([self.lon, self.lat])
    }
}

impl PointDistance for SpatialRecord {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let dx = self.lon - point[0];
        let dy = self.lat - point[1];
        dx * dx + dy * dy
    }
}

/// R-tree spatial index for nearest-neighbor queries.
pub struct SpatialIndex {
    tree: RTree<SpatialRecord>,
}

impl SpatialIndex {
    /// Build from a set of records.
    pub fn build(records: Vec<SpatialRecord>) -> Self {
        Self {
            tree: RTree::bulk_load(records),
        }
    }

    /// Find the k nearest neighbors to a point.
    pub fn nearest(&self, lon: f64, lat: f64, k: usize) -> Vec<&SpatialRecord> {
        self.tree
            .nearest_neighbor_iter(&[lon, lat])
            .take(k)
            .collect()
    }

    /// Find all records within a bounding box.
    pub fn within_bbox(
        &self,
        min_lon: f64,
        min_lat: f64,
        max_lon: f64,
        max_lat: f64,
    ) -> Vec<&SpatialRecord> {
        let envelope = AABB::from_corners([min_lon, min_lat], [max_lon, max_lat]);
        self.tree.locate_in_envelope(&envelope).collect()
    }

    /// Number of records.
    pub fn len(&self) -> usize {
        self.tree.size()
    }

    /// Whether empty.
    pub fn is_empty(&self) -> bool {
        self.tree.size() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_neighbor() {
        let records = vec![
            SpatialRecord {
                lat: 40.7128,
                lon: -74.0060,
                id: 0,
            }, // NYC
            SpatialRecord {
                lat: 34.0522,
                lon: -118.2437,
                id: 1,
            }, // LA
            SpatialRecord {
                lat: 41.8781,
                lon: -87.6298,
                id: 2,
            }, // Chicago
        ];
        let index = SpatialIndex::build(records);

        let nearest = index.nearest(-73.9, 40.8, 1);
        assert_eq!(nearest.len(), 1);
        assert_eq!(nearest[0].id, 0); // NYC is closest
    }

    #[test]
    fn bbox_query() {
        let records = vec![
            SpatialRecord {
                lat: 40.7,
                lon: -74.0,
                id: 0,
            },
            SpatialRecord {
                lat: 34.0,
                lon: -118.2,
                id: 1,
            },
            SpatialRecord {
                lat: 41.8,
                lon: -87.6,
                id: 2,
            },
        ];
        let index = SpatialIndex::build(records);

        // bbox around NYC
        let results = index.within_bbox(-75.0, 40.0, -73.0, 41.0);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, 0);
    }
}
