use geokode_core::address::{FeatureKind, PlaceClass};
use geokode_core::codec::{Decoder, Encoder};
use std::io::{self, Read, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    Node,
    Way,
    Relation,
}

enum Values {
    Any,
    Only(&'static [&'static str]),
    Except(&'static [&'static str]),
}

impl Values {
    fn accepts(&self, value: &str) -> bool {
        match self {
            Values::Any => true,
            Values::Only(values) => values.contains(&value),
            Values::Except(values) => !values.contains(&value),
        }
    }
}

struct KeyRule {
    key: &'static str,
    kind: FeatureKind,
    values: Values,
    objects: &'static [ObjectType],
}

const ANY_OBJECT: &[ObjectType] = &[ObjectType::Node, ObjectType::Way, ObjectType::Relation];

const STREET_HIGHWAYS: &[&str] = &[
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "unclassified",
    "residential",
    "living_street",
    "pedestrian",
    "service",
    "road",
    "track",
    "footway",
    "path",
    "cycleway",
    "bridleway",
    "steps",
];

const AEROWAY_PARTS: &[&str] = &[
    "runway",
    "taxiway",
    "taxilane",
    "apron",
    "stopway",
    "holding_position",
    "parking_position",
];

const fn poi(key: &'static str) -> KeyRule {
    KeyRule {
        key,
        kind: FeatureKind::Poi,
        values: Values::Any,
        objects: ANY_OBJECT,
    }
}

// the first matching rule wins
const CLASSIFICATION: &[KeyRule] = &[
    KeyRule {
        key: "boundary",
        kind: FeatureKind::Boundary,
        values: Values::Only(&["administrative"]),
        objects: &[ObjectType::Relation],
    },
    KeyRule {
        key: "place",
        kind: FeatureKind::Place,
        values: Values::Any,
        objects: ANY_OBJECT,
    },
    poi("amenity"),
    poi("tourism"),
    poi("historic"),
    poi("leisure"),
    poi("natural"),
    poi("waterway"),
    KeyRule {
        key: "aeroway",
        kind: FeatureKind::Poi,
        values: Values::Except(AEROWAY_PARTS),
        objects: ANY_OBJECT,
    },
    KeyRule {
        key: "railway",
        kind: FeatureKind::Poi,
        values: Values::Only(&["station", "halt", "tram_stop"]),
        objects: ANY_OBJECT,
    },
    KeyRule {
        key: "public_transport",
        kind: FeatureKind::Poi,
        values: Values::Only(&["station"]),
        objects: ANY_OBJECT,
    },
    poi("shop"),
    poi("office"),
    poi("man_made"),
    KeyRule {
        key: "highway",
        kind: FeatureKind::Street,
        values: Values::Only(STREET_HIGHWAYS),
        objects: &[ObjectType::Way],
    },
];

const NAME_VARIANT_KEYS: &[&str] = &[
    "name:en",
    "int_name",
    "alt_name",
    "official_name",
    "short_name",
];

const RELATION_TYPES: &[&str] = &["multipolygon", "boundary", "waterway"];
const MAX_ADMIN_LEVEL: u8 = 11;

pub struct Tags<'a>(Vec<(&'a str, &'a str)>);

impl<'a> Tags<'a> {
    pub fn new(tags: impl Iterator<Item = (&'a str, &'a str)>) -> Self {
        Tags(tags.collect())
    }

    pub fn get(&self, key: &str) -> Option<&'a str> {
        self.0
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
            .filter(|v| !v.is_empty())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn relation_type_is_indexed(&self) -> bool {
        self.get("type")
            .is_some_and(|value| RELATION_TYPES.contains(&value))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tagged {
    pub name: Option<String>,
    pub name_en: Option<String>,
    pub variants: Vec<String>,
    pub key: Option<String>,
    pub value: Option<String>,
    pub place: Option<String>,
    pub population: Option<u64>,
    pub notable: bool,
    pub admin_level: Option<u8>,
    pub country_code: Option<String>,
    pub subdivision_code: Option<String>,
    pub house_number: Option<String>,
    pub street: Option<String>,
    pub postcode: Option<String>,
    pub city: Option<String>,
}

impl Tagged {
    pub fn kind(&self) -> FeatureKind {
        let Some(key) = self.key.as_deref() else {
            return FeatureKind::Address;
        };
        CLASSIFICATION
            .iter()
            .find(|rule| rule.key == key)
            .map_or(FeatureKind::Poi, |rule| rule.kind)
    }

    pub fn place_class(&self) -> Option<PlaceClass> {
        self.place.as_deref().map(PlaceClass::from_tag)
    }

    // how the object reads as the city, state or country of something inside it
    pub fn context_name(&self) -> Option<String> {
        self.name_en.clone().or_else(|| self.name.clone())
    }

    pub fn all_names(&self) -> Vec<String> {
        self.name.iter().chain(&self.variants).cloned().collect()
    }

    fn common(tags: &Tags) -> Tagged {
        Tagged {
            name: tags.get("name").map(str::to_string),
            name_en: tags.get("name:en").map(str::to_string),
            place: tags.get("place").map(str::to_string),
            population: tags.get("population").and_then(parse_population),
            notable: tags.get("wikidata").is_some() || tags.get("wikipedia").is_some(),
            house_number: tags.get("addr:housenumber").map(str::to_string),
            street: tags.get("addr:street").map(str::to_string),
            postcode: tags.get("addr:postcode").map(str::to_string),
            city: tags.get("addr:city").map(str::to_string),
            ..Tagged::default()
        }
    }

    pub fn write(&self, out: &mut Encoder<impl Write>) -> io::Result<()> {
        out.optional_text(self.name.as_deref())?;
        out.optional_text(self.name_en.as_deref())?;
        out.texts(&self.variants)?;
        out.optional_text(self.key.as_deref())?;
        out.optional_text(self.value.as_deref())?;
        out.optional_text(self.place.as_deref())?;
        out.unsigned(self.population.map_or(0, |p| p + 1))?;
        out.unsigned(u64::from(self.notable))?;
        out.unsigned(self.admin_level.map_or(0, u64::from))?;
        out.optional_text(self.country_code.as_deref())?;
        out.optional_text(self.subdivision_code.as_deref())?;
        out.optional_text(self.house_number.as_deref())?;
        out.optional_text(self.street.as_deref())?;
        out.optional_text(self.postcode.as_deref())?;
        out.optional_text(self.city.as_deref())
    }

    pub fn read(input: &mut Decoder<impl Read>) -> io::Result<Tagged> {
        Ok(Tagged {
            name: input.optional_text()?,
            name_en: input.optional_text()?,
            variants: input.texts()?,
            key: input.optional_text()?,
            value: input.optional_text()?,
            place: input.optional_text()?,
            population: input.unsigned()?.checked_sub(1),
            notable: input.unsigned()? != 0,
            admin_level: u8::try_from(input.unsigned()?).ok().filter(|l| *l != 0),
            country_code: input.optional_text()?,
            subdivision_code: input.optional_text()?,
            house_number: input.optional_text()?,
            street: input.optional_text()?,
            postcode: input.optional_text()?,
            city: input.optional_text()?,
        })
    }
}

fn parse_population(raw: &str) -> Option<u64> {
    let digits: String = raw
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, ',' | ' ' | '.'))
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn name_variants(tags: &Tags, name: &str) -> Vec<String> {
    let mut variants: Vec<String> = NAME_VARIANT_KEYS
        .iter()
        .filter_map(|key| tags.get(key))
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != name)
        .map(str::to_string)
        .collect();
    variants.sort_unstable();
    variants.dedup();
    variants
}

pub fn classify_named(tags: &Tags, object: ObjectType) -> Option<Tagged> {
    let name = tags.get("name")?;
    let (rule, value) = CLASSIFICATION.iter().find_map(|rule| {
        if !rule.objects.contains(&object) {
            return None;
        }
        let value = tags.get(rule.key)?;
        if !rule.values.accepts(value) {
            return None;
        }
        if rule.kind == FeatureKind::Boundary && admin_level(tags).is_none() {
            return None;
        }
        Some((rule, value))
    })?;
    let mut tagged = Tagged::common(tags);
    tagged.variants = name_variants(tags, name);
    tagged.key = Some(rule.key.to_string());
    tagged.value = Some(value.to_string());
    if rule.kind == FeatureKind::Boundary {
        tagged.admin_level = admin_level(tags);
        tagged.country_code = tags
            .get("ISO3166-1:alpha2")
            .or_else(|| tags.get("ISO3166-1"))
            .map(str::to_lowercase);
        tagged.subdivision_code = tags
            .get("ISO3166-2")
            .and_then(|code| code.split_once('-'))
            .map(|(_, subdivision)| subdivision.to_string());
    }
    Some(tagged)
}

pub fn classify_address(tags: &Tags) -> Option<Tagged> {
    tags.get("addr:housenumber")?;
    tags.get("addr:street")?;
    Some(Tagged::common(tags))
}

fn admin_level(tags: &Tags) -> Option<u8> {
    tags.get("admin_level")?
        .parse()
        .ok()
        .filter(|level| (1..=MAX_ADMIN_LEVEL).contains(level))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags<'a>(pairs: &'a [(&'a str, &'a str)]) -> Tags<'a> {
        Tags::new(pairs.iter().copied())
    }

    #[test]
    fn the_first_matching_key_classifies() {
        let found = classify_named(
            &tags(&[
                ("name", "Monaco-Monte-Carlo"),
                ("public_transport", "station"),
                ("railway", "station"),
            ]),
            ObjectType::Node,
        )
        .unwrap();
        assert_eq!(found.key.as_deref(), Some("railway"));
        assert_eq!(found.kind(), FeatureKind::Poi);
    }

    #[test]
    fn a_value_outside_the_rule_falls_through_to_the_next_key() {
        let found = classify_named(
            &tags(&[
                ("name", "Boulevard du Larvotto"),
                ("railway", "abandoned"),
                ("highway", "primary"),
            ]),
            ObjectType::Way,
        )
        .unwrap();
        assert_eq!(found.kind(), FeatureKind::Street);
        assert_eq!(found.value.as_deref(), Some("primary"));
    }

    #[test]
    fn unnamed_and_unclassified_objects_are_skipped() {
        assert!(classify_named(&tags(&[("amenity", "bench")]), ObjectType::Node).is_none());
        assert!(classify_named(&tags(&[("name", "Hill Farm")]), ObjectType::Node).is_none());
        assert!(
            classify_named(
                &tags(&[("name", "Main Street"), ("highway", "residential")]),
                ObjectType::Node
            )
            .is_none(),
            "a street is a way"
        );
    }

    #[test]
    fn a_boundary_carries_its_level_and_codes() {
        let found = classify_named(
            &tags(&[
                ("name", "Illinois"),
                ("boundary", "administrative"),
                ("admin_level", "4"),
                ("ISO3166-2", "US-IL"),
            ]),
            ObjectType::Relation,
        )
        .unwrap();
        assert_eq!(found.kind(), FeatureKind::Boundary);
        assert_eq!(found.admin_level, Some(4));
        assert_eq!(found.subdivision_code.as_deref(), Some("IL"));
    }

    #[test]
    fn name_variants_split_and_skip_the_name() {
        let found = classify_named(
            &tags(&[
                ("name", "Genève"),
                ("name:en", "Geneva"),
                ("alt_name", "Genf;Ginevra;Genève"),
                ("place", "city"),
            ]),
            ObjectType::Node,
        )
        .unwrap();
        assert_eq!(found.variants, vec!["Geneva", "Genf", "Ginevra"]);
    }

    #[test]
    fn tagged_round_trips_through_the_codec() {
        let found = classify_named(
            &tags(&[
                ("name", "Musée Océanographique"),
                ("tourism", "museum"),
                ("wikidata", "Q1141"),
                ("name:en", "Oceanographic Museum"),
                ("population", "1,234"),
            ]),
            ObjectType::Way,
        )
        .unwrap();
        let mut bytes = Vec::new();
        let mut encoder = Encoder::new(&mut bytes);
        found.write(&mut encoder).unwrap();
        encoder.finish().unwrap();
        let back = Tagged::read(&mut Decoder::new(bytes.as_slice())).unwrap();
        assert_eq!(back, found);
        assert_eq!(back.population, Some(1234));
        assert_eq!(back.context_name().as_deref(), Some("Oceanographic Museum"));
        assert!(back.notable);
    }
}
