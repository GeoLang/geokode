use crate::address::{FeatureKind, MatchType, PlaceClass};

// settlement tiers sit 10 apart so population and the notability boost reorder within a tier only
const PLACE_TIERS: &[(PlaceClass, f32)] = &[
    (PlaceClass::Country, 200.0),
    (PlaceClass::City, 180.0),
    (PlaceClass::State, 170.0),
    (PlaceClass::Town, 160.0),
    (PlaceClass::Village, 150.0),
    (PlaceClass::County, 140.0),
    (PlaceClass::Municipality, 130.0),
    (PlaceClass::Island, 130.0),
    (PlaceClass::Suburb, 120.0),
    (PlaceClass::Hamlet, 110.0),
    (PlaceClass::Neighbourhood, 100.0),
    (PlaceClass::Other, 90.0),
];

// a boundary with no place tag ranks by its admin_level
const ADMIN_LEVEL_TIERS: &[(u8, f32)] = &[
    (2, 200.0),
    (4, 170.0),
    (6, 140.0),
    (8, 130.0),
    (u8::MAX, 100.0),
];

// a notable poi outranks a same-named street, a plain one does not
const STREET_TIER: f32 = 76.0;
const POI_TIER: f32 = 74.0;
const ADDRESS_TIER: f32 = 60.0;

const MAX_POPULATION_SCORE: f32 = 6.9;
const NOTABLE_BOOST: f32 = 3.0;

const EXACT_MATCH: f64 = 300.0;
const PREFIX_MATCH: f64 = 150.0;
const FUZZY_MATCH: f64 = 0.0;
const HOUSE_NUMBER_ADDRESS_BOOST: f64 = 400.0;
const DIRECTIONAL_MISMATCH_PENALTY: f64 = 1000.0;
const DIRECTIONAL_ABSENT_PENALTY: f64 = 500.0;
const IGNORED_QUALIFIER_PENALTY: f64 = 50.0;
// lifts a nearby town over a far city, never a poi over a settlement
const BIAS_WEIGHT: f64 = 35.0;
const BIAS_HALF_DISTANCE_KM: f64 = 20.0;

pub struct Importance {
    pub kind: FeatureKind,
    pub place: Option<PlaceClass>,
    pub admin_level: Option<u8>,
    pub population: Option<u64>,
    pub notable: bool,
}

impl Importance {
    fn tier(&self) -> f32 {
        let place_tier = self
            .place
            .and_then(|class| PLACE_TIERS.iter().find(|(c, _)| *c == class))
            .map(|(_, tier)| *tier);
        match self.kind {
            FeatureKind::Place => place_tier.unwrap_or(PLACE_TIERS[PLACE_TIERS.len() - 1].1),
            FeatureKind::Boundary => place_tier.unwrap_or_else(|| {
                let level = self.admin_level.unwrap_or(u8::MAX);
                ADMIN_LEVEL_TIERS
                    .iter()
                    .find(|(max_level, _)| level <= *max_level)
                    .map_or(
                        ADMIN_LEVEL_TIERS[ADMIN_LEVEL_TIERS.len() - 1].1,
                        |(_, tier)| *tier,
                    )
            }),
            FeatureKind::Poi => POI_TIER,
            FeatureKind::Street => STREET_TIER,
            FeatureKind::Address => ADDRESS_TIER,
        }
    }

    pub fn score(&self) -> f32 {
        let population = self.population.map_or(0.0, |p| {
            ((p as f32) + 1.0).log10().min(MAX_POPULATION_SCORE)
        });
        let notable = if self.notable { NOTABLE_BOOST } else { 0.0 };
        self.tier() + population + notable
    }
}

pub struct QueryFit {
    pub match_type: MatchType,
    pub numbered_query: bool,
    // None when the query names no directional
    pub directional: Option<DirectionalFit>,
    pub distance_km: Option<f64>,
    pub ignored_qualifier: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectionalFit {
    Same,
    Absent,
    Different,
}

pub fn score(importance: f32, kind: FeatureKind, fit: &QueryFit) -> f64 {
    let mut score = f64::from(importance)
        + match fit.match_type {
            MatchType::Exact => EXACT_MATCH,
            MatchType::Prefix => PREFIX_MATCH,
            MatchType::Fuzzy => FUZZY_MATCH,
        };
    if fit.numbered_query && kind == FeatureKind::Address {
        score += HOUSE_NUMBER_ADDRESS_BOOST;
    }
    score -= match fit.directional {
        None | Some(DirectionalFit::Same) => 0.0,
        Some(DirectionalFit::Absent) => DIRECTIONAL_ABSENT_PENALTY,
        Some(DirectionalFit::Different) => DIRECTIONAL_MISMATCH_PENALTY,
    };
    if fit.ignored_qualifier {
        score -= IGNORED_QUALIFIER_PENALTY;
    }
    if let Some(distance) = fit.distance_km {
        score += BIAS_WEIGHT * BIAS_HALF_DISTANCE_KM / (BIAS_HALF_DISTANCE_KM + distance);
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settlement(place: PlaceClass, population: Option<u64>) -> f32 {
        Importance {
            kind: FeatureKind::Place,
            place: Some(place),
            admin_level: None,
            population,
            notable: true,
        }
        .score()
    }

    #[test]
    fn a_notable_large_town_stays_below_an_unknown_city() {
        let town = settlement(PlaceClass::Town, Some(9_000_000));
        let city = Importance {
            kind: FeatureKind::Place,
            place: Some(PlaceClass::City),
            admin_level: None,
            population: None,
            notable: false,
        }
        .score();
        assert!(town < city);
    }

    #[test]
    fn nearby_bias_lifts_a_town_over_a_far_city_but_not_a_poi() {
        let exact = |distance_km| QueryFit {
            match_type: MatchType::Exact,
            numbered_query: false,
            directional: None,
            distance_km,
            ignored_qualifier: false,
        };
        let far_city = score(
            settlement(PlaceClass::City, Some(2_000_000)),
            FeatureKind::Place,
            &exact(Some(5000.0)),
        );
        let near_town = score(
            settlement(PlaceClass::Town, Some(25_000)),
            FeatureKind::Place,
            &exact(Some(1.0)),
        );
        let poi = Importance {
            kind: FeatureKind::Poi,
            place: None,
            admin_level: None,
            population: None,
            notable: true,
        }
        .score();
        let near_poi = score(poi, FeatureKind::Poi, &exact(Some(0.0)));
        assert!(near_town > far_city);
        assert!(near_poi < far_city);
    }
}
