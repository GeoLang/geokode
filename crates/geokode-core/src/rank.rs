use crate::address::{FeatureKind, MatchType, PlaceClass};

// population and the notable boost never cross a settlement tier
const PLACE_TIERS: &[(PlaceClass, f32)] = &[
    (PlaceClass::Country, 220.0),
    (PlaceClass::City, 200.0),
    (PlaceClass::State, 190.0),
    (PlaceClass::Town, 180.0),
    (PlaceClass::Village, 160.0),
    (PlaceClass::County, 150.0),
    (PlaceClass::Municipality, 140.0),
    (PlaceClass::Island, 140.0),
    (PlaceClass::Suburb, 130.0),
    (PlaceClass::Hamlet, 120.0),
    (PlaceClass::Neighbourhood, 110.0),
    // localities, farms and squares rank with pois
    (PlaceClass::Other, 72.0),
];

const ADMIN_LEVEL_TIERS: &[(u8, f32)] = &[
    (2, 220.0),
    (4, 190.0),
    (6, 150.0),
    (8, 140.0),
    (u8::MAX, 110.0),
];

// a notable poi outranks a same-named street, a plain one does not
const STREET_TIER: f32 = 76.0;
const POI_TIER: f32 = 74.0;
const ADDRESS_TIER: f32 = 60.0;

// 15 on Central Park and 49 on the Eiffel Tower, 0 on the village and the replica they lost to
pub const FAMOUS_LANGUAGES: u16 = 10;
// between village and town
const FAMOUS_TIER: f32 = 170.0;
const LANGUAGE_WEIGHT: f32 = 0.1;
const MAX_LANGUAGE_SCORE: f32 = 4.0;

const MAX_POPULATION_SCORE: f32 = 6.9;
const NOTABLE_BOOST: f32 = 3.0;

const EXACT_MATCH: f64 = 300.0;
const PREFIX_MATCH: f64 = 150.0;
// a typo of a city still beats a prefix of a peak
const FUZZY_MATCH: f64 = 100.0;
const HOUSE_NUMBER_ADDRESS_BOOST: f64 = 400.0;
const DIRECTIONAL_MISMATCH_PENALTY: f64 = 1000.0;
const DIRECTIONAL_ABSENT_PENALTY: f64 = 500.0;
const IGNORED_QUALIFIER_PENALTY: f64 = 50.0;
const QUALIFIER_LEVEL_WEIGHT: f64 = 2.0;
// above any tier gap, so "mount x" puts the peak before the region named x
const FEATURE_WORD_FIT: f64 = 150.0;
// lifts a nearby town over a far city, never a poi over a settlement
const BIAS_WEIGHT: f64 = 35.0;
const BIAS_HALF_DISTANCE_KM: f64 = 20.0;

// a query led by one of these words, with no exact hit, is retried without it
pub const FEATURE_WORDS: &[(&str, &[&str])] = &[
    ("mount", MOUNTAIN_VALUES),
    ("mt", MOUNTAIN_VALUES),
    ("mountain", MOUNTAIN_VALUES),
    ("lake", &["water"]),
    ("river", &["river"]),
];
const MOUNTAIN_VALUES: &[&str] = &["peak", "volcano", "massif", "mountain_range", "hill"];

// the osm_value as a byte a row can hold, 0 for a value no feature word names
pub fn feature_code(osm_value: Option<&str>) -> u8 {
    let values = FEATURE_WORDS.iter().flat_map(|(_, values)| values.iter());
    osm_value
        .and_then(|value| values.enumerate().find(|(_, v)| **v == value))
        .map_or(0, |(index, _)| (index + 1) as u8)
}

pub fn feature_word(word: &str) -> Option<usize> {
    FEATURE_WORDS.iter().position(|(w, _)| *w == word)
}

pub fn fits_feature_word(word: usize, code: u8) -> bool {
    let start: usize = FEATURE_WORDS[..word].iter().map(|(_, v)| v.len()).sum();
    let end = start + FEATURE_WORDS[word].1.len();
    (start + 1..=end).contains(&usize::from(code))
}

pub struct Importance {
    pub kind: FeatureKind,
    pub place: Option<PlaceClass>,
    pub admin_level: Option<u8>,
    pub population: Option<u64>,
    pub notable: bool,
    // distinct name:xx language tags
    pub languages: u16,
}

impl Importance {
    fn settlement(&self) -> bool {
        self.place.is_some_and(|class| class != PlaceClass::Other)
    }

    fn tier(&self) -> f32 {
        let place_tier = self
            .place
            .and_then(|class| PLACE_TIERS.iter().find(|(c, _)| *c == class))
            .map(|(_, tier)| *tier);
        let tier = match self.kind {
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
        };
        if self.languages >= FAMOUS_LANGUAGES {
            tier.max(FAMOUS_TIER)
        } else {
            tier
        }
    }

    pub fn score(&self) -> f32 {
        let population = self.population.map_or(0.0, |p| {
            ((p as f32) + 1.0).log10().min(MAX_POPULATION_SCORE)
        });
        let notable = if self.notable { NOTABLE_BOOST } else { 0.0 };
        // settlements order by population instead
        let languages = if self.settlement() {
            0.0
        } else {
            (f32::from(self.languages) * LANGUAGE_WEIGHT).min(MAX_LANGUAGE_SCORE)
        };
        self.tier() + population + notable + languages
    }
}

pub struct QueryFit {
    pub match_type: MatchType,
    pub numbered_query: bool,
    // None when the query names no directional
    pub directional: Option<DirectionalFit>,
    pub distance_km: Option<f64>,
    pub ignored_qualifier: bool,
    // summed admin levels the qualifiers matched
    pub qualifier_level: u32,
    // the feature word the query led with, see FEATURE_WORDS
    pub feature_word: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectionalFit {
    Same,
    Absent,
    Different,
}

pub fn score(importance: f32, kind: FeatureKind, feature: u8, fit: &QueryFit) -> f64 {
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
    score += QUALIFIER_LEVEL_WEIGHT * f64::from(fit.qualifier_level);
    if fit
        .feature_word
        .is_some_and(|word| fits_feature_word(word, feature))
    {
        score += FEATURE_WORD_FIT;
    }
    if let Some(distance) = fit.distance_km {
        score += BIAS_WEIGHT * BIAS_HALF_DISTANCE_KM / (BIAS_HALF_DISTANCE_KM + distance);
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn importance(
        kind: FeatureKind,
        place: Option<PlaceClass>,
        population: Option<u64>,
    ) -> Importance {
        Importance {
            kind,
            place,
            admin_level: None,
            population,
            notable: true,
            languages: 0,
        }
    }

    fn settlement(place: PlaceClass, population: Option<u64>) -> f32 {
        importance(FeatureKind::Place, Some(place), population).score()
    }

    fn exact(distance_km: Option<f64>) -> QueryFit {
        QueryFit {
            match_type: MatchType::Exact,
            numbered_query: false,
            directional: None,
            distance_km,
            ignored_qualifier: false,
            qualifier_level: 0,
            feature_word: None,
        }
    }

    #[test]
    fn a_notable_large_town_stays_below_an_unknown_city() {
        let town = settlement(PlaceClass::Town, Some(9_000_000));
        let city = Importance {
            notable: false,
            ..importance(FeatureKind::Place, Some(PlaceClass::City), None)
        }
        .score();
        assert!(town < city);
    }

    #[test]
    fn a_famous_poi_sits_between_the_largest_village_and_the_smallest_town() {
        let famous = Importance {
            languages: u16::MAX,
            ..importance(FeatureKind::Poi, None, None)
        }
        .score();
        let barely_famous = Importance {
            languages: FAMOUS_LANGUAGES,
            ..importance(FeatureKind::Poi, None, None)
        }
        .score();
        assert!(barely_famous > settlement(PlaceClass::Village, Some(u64::MAX)));
        assert!(famous < settlement(PlaceClass::Town, None) - NOTABLE_BOOST);
        let replica = importance(FeatureKind::Poi, None, None).score();
        assert!(barely_famous > replica);
    }

    #[test]
    fn languages_do_not_reorder_settlements() {
        let many_languages = Importance {
            languages: 200,
            ..importance(FeatureKind::Place, Some(PlaceClass::City), Some(1_000))
        }
        .score();
        assert!(many_languages < settlement(PlaceClass::City, Some(100_000)));
    }

    #[test]
    fn a_feature_word_prefers_the_matching_value() {
        let mount = QueryFit {
            feature_word: feature_word("mount"),
            ..exact(None)
        };
        let peak = score(74.0, FeatureKind::Poi, feature_code(Some("massif")), &mount);
        let region = score(
            190.0,
            FeatureKind::Boundary,
            feature_code(Some("administrative")),
            &mount,
        );
        let lake = score(74.0, FeatureKind::Poi, feature_code(Some("water")), &mount);
        assert!(peak > region);
        assert!(lake < region);
    }

    #[test]
    fn nearby_bias_lifts_a_town_over_a_far_city_but_not_a_poi() {
        let far_city = score(
            settlement(PlaceClass::City, Some(2_000_000)),
            FeatureKind::Place,
            0,
            &exact(Some(5000.0)),
        );
        let near_town = score(
            settlement(PlaceClass::Town, Some(25_000)),
            FeatureKind::Place,
            0,
            &exact(Some(1.0)),
        );
        let poi = importance(FeatureKind::Poi, None, None).score();
        let near_poi = score(poi, FeatureKind::Poi, 0, &exact(Some(0.0)));
        assert!(near_town > far_city);
        assert!(near_poi < far_city);
    }
}
