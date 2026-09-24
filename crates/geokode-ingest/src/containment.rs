use crate::geometry::{BandedArea, Coord};
use geokode_core::address::{PlaceClass, normalize_for_match};
use geokode_core::spatial::{IndexedPoint, KdTree, encode_kd_tree};
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{AABB, RTree};

pub const COUNTRY_LEVEL: u8 = 2;
pub const STATE_LEVEL: u8 = 4;
pub const MAX_CONTEXT_LEVEL: u8 = 8;
// most local first
const CITY_LEVELS: [u8; 3] = [8, 7, 6];
const SETTLEMENT_NEIGHBOURS: usize = 16;
const SETTLEMENT_RADII_KM: &[(PlaceClass, f64)] = &[
    (PlaceClass::City, 10.0),
    (PlaceClass::Town, 5.0),
    (PlaceClass::Village, 2.0),
];

// the English name when tagged, and every normalized variant to spot the record itself
#[derive(Debug)]
pub struct AreaName {
    pub display: String,
    pub variants: Vec<String>,
}

impl AreaName {
    pub fn new(display: String, names: &[String]) -> Self {
        AreaName {
            display,
            variants: names.iter().map(|n| normalize_for_match(n)).collect(),
        }
    }

    // a record named like its own area reads its own lead there
    pub fn read_for(&self, own_names: &[&str], lead: Option<&str>) -> String {
        let own = own_names
            .iter()
            .any(|name| self.variants.contains(&normalize_for_match(name)));
        match lead {
            Some(lead) if own => lead.to_string(),
            _ => self.display.clone(),
        }
    }
}

pub struct AdminArea {
    pub area_id: u32,
    pub level: u8,
    pub name: AreaName,
    pub country_code: Option<String>,
    pub geometry: BandedArea,
}

pub struct Settlement {
    pub name: AreaName,
    pub class: PlaceClass,
    pub coord: Coord,
}

impl Settlement {
    pub fn radius_km(class: PlaceClass) -> Option<f64> {
        SETTLEMENT_RADII_KM
            .iter()
            .find(|(c, _)| *c == class)
            .map(|(_, radius)| *radius)
    }
}

pub struct Containment {
    pub admin: Vec<AdminArea>,
    tree: RTree<GeomWithData<Rectangle<[f64; 2]>, usize>>,
    pub settlements: Vec<Settlement>,
    settlement_tree: KdTree<Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Located {
    // lowest level first
    pub admin: Vec<usize>,
    pub settlement: Option<usize>,
}

pub struct Context<'a> {
    pub country: Option<&'a AdminArea>,
    pub state: Option<&'a AdminArea>,
    pub city: Option<&'a AreaName>,
}

impl Containment {
    pub fn new(admin: Vec<AdminArea>, settlements: Vec<Settlement>) -> Self {
        let tree = RTree::bulk_load(
            admin
                .iter()
                .enumerate()
                .map(|(index, area)| {
                    let [min_x, min_y, max_x, max_y] = area.geometry.area().bbox();
                    GeomWithData::new(
                        Rectangle::from_corners([min_x, min_y], [max_x, max_y]),
                        index,
                    )
                })
                .collect(),
        );
        let settlement_tree = KdTree::new(encode_kd_tree(
            settlements
                .iter()
                .enumerate()
                .map(|(index, s)| IndexedPoint::new(s.coord[0], s.coord[1], index as u32))
                .collect(),
        ));
        Containment {
            admin,
            tree,
            settlements,
            settlement_tree,
        }
    }

    pub fn locate(&self, coord: Coord) -> Located {
        let mut admin: Vec<usize> = self
            .tree
            .locate_in_envelope_intersecting(AABB::from_point(coord))
            .map(|found| found.data)
            .filter(|index| self.admin[*index].geometry.contains(coord))
            .collect();
        admin.sort_by_key(|index| (self.admin[*index].level, *index));
        let settlement = self.nearest_settlement(coord);
        Located { admin, settlement }
    }

    fn nearest_settlement(&self, [lon, lat]: Coord) -> Option<usize> {
        let widest = SETTLEMENT_RADII_KM
            .iter()
            .map(|(_, radius)| *radius)
            .fold(0.0, f64::max);
        self.settlement_tree
            .nearest(lon, lat, SETTLEMENT_NEIGHBOURS, widest)
            .into_iter()
            .find(|neighbour| {
                let class = self.settlements[neighbour.id as usize].class;
                Settlement::radius_km(class).is_some_and(|radius| neighbour.distance_km <= radius)
            })
            .map(|neighbour| neighbour.id as usize)
    }

    // a state is not placed in one of its towns
    pub fn context(&self, located: &Located, max_level: u8) -> Context<'_> {
        let at_level = |level: u8| {
            located
                .admin
                .iter()
                .map(|index| &self.admin[*index])
                .find(|area| area.level == level && level <= max_level)
        };
        let admin_city = CITY_LEVELS.iter().find_map(|level| at_level(*level));
        let city = match admin_city {
            Some(area) => Some(&area.name),
            None if max_level >= MAX_CONTEXT_LEVEL => located
                .settlement
                .map(|index| &self.settlements[index].name),
            None => None,
        };
        Context {
            country: at_level(COUNTRY_LEVEL),
            state: at_level(STATE_LEVEL),
            city,
        }
    }

    pub fn has_admin_city(&self, located: &Located) -> bool {
        located
            .admin
            .iter()
            .any(|index| CITY_LEVELS.contains(&self.admin[*index].level))
    }

    pub fn anchored(&self, located: &Located) -> bool {
        located.admin.iter().any(|index| {
            let level = self.admin[*index].level;
            level == COUNTRY_LEVEL || level == STATE_LEVEL
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Area;

    fn square_area(area_id: u32, level: u8, name: &str, x: f64, y: f64, size: f64) -> AdminArea {
        let ring = vec![
            [x, y],
            [x + size, y],
            [x + size, y + size],
            [x, y + size],
            [x, y],
        ];
        AdminArea {
            area_id,
            level,
            name: AreaName::new(name.to_string(), &[name.to_string()]),
            country_code: (level == COUNTRY_LEVEL).then(|| "xx".to_string()),
            geometry: BandedArea::new(Area::from_rings(vec![ring]).unwrap()),
        }
    }

    fn containment() -> Containment {
        Containment::new(
            vec![
                square_area(0, 8, "Townsville", 0.2, 0.2, 0.2),
                square_area(1, 2, "Country", 0.0, 0.0, 1.0),
                square_area(2, 4, "State", 0.0, 0.0, 0.5),
            ],
            vec![Settlement {
                name: AreaName::new(
                    "Hamlet Village".to_string(),
                    &["Hamlet Village".to_string()],
                ),
                class: PlaceClass::Village,
                coord: [0.8, 0.8],
            }],
        )
    }

    #[test]
    fn a_point_takes_every_containing_level() {
        let containment = containment();
        let located = containment.locate([0.3, 0.3]);
        let context = containment.context(&located, u8::MAX);
        assert_eq!(
            context.country.map(|a| a.name.display.as_str()),
            Some("Country")
        );
        assert_eq!(
            context.state.map(|a| a.name.display.as_str()),
            Some("State")
        );
        assert_eq!(context.city.map(|c| c.display.as_str()), Some("Townsville"));
        assert!(containment.anchored(&located));
    }

    #[test]
    fn with_no_admin_city_the_nearest_settlement_in_range_is_the_city() {
        let containment = containment();
        let near = containment.locate([0.805, 0.8]);
        assert_eq!(
            containment
                .context(&near, u8::MAX)
                .city
                .map(|c| c.display.as_str()),
            Some("Hamlet Village")
        );
        let far = containment.locate([0.7, 0.9]);
        assert!(containment.context(&far, u8::MAX).city.is_none());
    }

    #[test]
    fn a_record_named_like_its_area_keeps_its_own_spelling() {
        let names = ["Zürich".to_string(), "Zurich".to_string()];
        let area = AreaName::new("Zurich".to_string(), &names);
        assert_eq!(area.read_for(&["Zürich"], Some("Zürich")), "Zürich");
        assert_eq!(
            area.read_for(&["東京都", "Zurich"], Some("Zurich")),
            "Zurich"
        );
        assert_eq!(area.read_for(&["Zürich HB"], Some("Zürich HB")), "Zurich");
        assert_eq!(area.read_for(&[], None), "Zurich");
    }

    #[test]
    fn a_state_level_record_gets_no_city() {
        let containment = containment();
        let located = containment.locate([0.3, 0.3]);
        let context = containment.context(&located, STATE_LEVEL);
        assert!(context.city.is_none());
        assert_eq!(
            context.state.map(|a| a.name.display.as_str()),
            Some("State")
        );
    }
}
