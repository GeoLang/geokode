use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Address {
    pub house_number: Option<String>,
    pub street: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub postcode: Option<String>,
    pub country: Option<String>,
    pub full: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchType {
    Exact,
    Prefix,
    Fuzzy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureKind {
    Address,
    Place,
    Street,
    Poi,
    Boundary,
}

impl FeatureKind {
    pub const ALL: [FeatureKind; 5] = [
        FeatureKind::Address,
        FeatureKind::Place,
        FeatureKind::Street,
        FeatureKind::Poi,
        FeatureKind::Boundary,
    ];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsmType {
    Node,
    Way,
    Relation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaceClass {
    Country,
    State,
    County,
    City,
    Town,
    Municipality,
    Village,
    Island,
    Suburb,
    Hamlet,
    Neighbourhood,
    Other,
}

impl PlaceClass {
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "country" => Self::Country,
            "state" | "province" | "region" => Self::State,
            "county" | "district" => Self::County,
            "city" => Self::City,
            "town" => Self::Town,
            "municipality" => Self::Municipality,
            "village" => Self::Village,
            "island" | "archipelago" => Self::Island,
            "suburb" | "borough" => Self::Suburb,
            "hamlet" => Self::Hamlet,
            "quarter" | "neighbourhood" | "neighborhood" | "city_block" => Self::Neighbourhood,
            _ => Self::Other,
        }
    }

    pub fn is_settlement(self) -> bool {
        matches!(
            self,
            Self::City | Self::Town | Self::Village | Self::Hamlet | Self::Suburb
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GeoResult {
    pub name: Option<String>,
    pub display_name: String,
    pub address: Address,
    pub country_code: Option<String>,
    pub lat: f64,
    pub lon: f64,
    pub bbox: Option<[f64; 4]>,
    pub kind: FeatureKind,
    pub osm_type: Option<OsmType>,
    pub osm_id: Option<i64>,
    pub osm_key: Option<String>,
    pub osm_value: Option<String>,
    pub admin_level: Option<u8>,
    pub population: Option<u64>,
    pub confidence: f64,
    pub match_type: MatchType,
}

pub fn parse_address(input: &str) -> Address {
    let input = input.trim();
    let parts: Vec<&str> = input.split(',').map(|s| s.trim()).collect();
    let owned = |part: &str| Some(part.to_string());
    let mut address = Address {
        full: input.to_string(),
        ..Address::default()
    };
    match parts.as_slice() {
        [] => {}
        [street] => address.street = owned(street),
        [street, city] => {
            address.street = owned(street);
            address.city = owned(city);
        }
        [first, city, state, rest @ ..] => {
            let (house, street) = split_house_number(first);
            address.house_number = house;
            address.street = Some(street);
            address.city = owned(city);
            address.state = owned(state);
            match rest {
                [] => {}
                [country] => address.country = owned(country),
                [postcode, country, ..] => {
                    address.postcode = owned(postcode);
                    address.country = owned(country);
                }
            }
        }
    }
    address
}

fn split_house_number(s: &str) -> (Option<String>, String) {
    let s = s.trim();
    if let Some(pos) = s.find(|c: char| !c.is_ascii_digit()) {
        let prefix = &s[..pos];
        let rest = s[pos..].trim();
        if !prefix.is_empty() && !rest.is_empty() {
            return (Some(prefix.to_string()), rest.to_string());
        }
    }
    (None, s.to_string())
}

const STREET_SUFFIXES: &[(&str, &str)] = &[
    ("street", "st"),
    ("avenue", "ave"),
    ("boulevard", "blvd"),
    ("drive", "dr"),
    ("road", "rd"),
    ("lane", "ln"),
    ("court", "ct"),
    ("place", "pl"),
    ("circle", "cir"),
    ("terrace", "ter"),
    ("highway", "hwy"),
    ("parkway", "pkwy"),
    ("expressway", "expy"),
    ("freeway", "fwy"),
    ("trail", "trl"),
    ("way", "wy"),
    ("alley", "aly"),
    ("crescent", "cres"),
    ("square", "sq"),
];

const DIRECTIONALS: &[(&str, &str)] = &[
    ("north", "n"),
    ("south", "s"),
    ("east", "e"),
    ("west", "w"),
    ("northeast", "ne"),
    ("northwest", "nw"),
    ("southeast", "se"),
    ("southwest", "sw"),
];

// "no" and "fl" sit inside other words or are too short to trust
const UNIT_DESIGNATORS: &[&str] = &[
    "apartment",
    "apt",
    "suite",
    "ste",
    "unit",
    "room",
    "building",
    "bldg",
];

const MIN_PARTIAL_SUFFIX_CHARS: usize = 3;

// letters NFD leaves whole
const LETTER_FOLDS: &[(char, &str)] = &[
    ('ß', "ss"),
    ('æ', "ae"),
    ('œ', "oe"),
    ('ø', "o"),
    ('ł', "l"),
    ('đ', "d"),
    ('ð', "d"),
    ('þ', "th"),
    ('ı', "i"),
];

fn fold_diacritics(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.nfd() {
        if unicode_normalization::char::is_combining_mark(c) {
            continue;
        }
        match LETTER_FOLDS.iter().find(|(letter, _)| *letter == c) {
            Some((_, folded)) => out.push_str(folded),
            None => out.push(c),
        }
    }
    out
}

// "123 North Main Street Apt 4" and "123 Main St" become the same key
pub fn normalize_for_match(input: &str) -> String {
    let mut s = fold_diacritics(&input.to_lowercase());
    s = s.replace('.', "");
    s = s.replace([',', '#', '-', '\'', '’', '/', '(', ')'], " ");
    for &(full, abbr) in DIRECTIONALS {
        s = replace_word(&s, full, " ");
        s = replace_word(&s, abbr, " ");
    }
    // "Ste" without a house number is a saint, not a suite
    if s.trim_start().starts_with(|c: char| c.is_ascii_digit()) {
        for designator in UNIT_DESIGNATORS {
            s = strip_designator_and_token(&s, designator);
        }
    }
    for &(full, abbr) in STREET_SUFFIXES {
        s = replace_word(&s, full, abbr);
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// "avenu" normalizes to itself while keys hold "ave"
pub fn partial_suffix_keys(input: &str) -> Vec<String> {
    let key = normalize_for_match(input);
    let (head, last) = key.rsplit_once(' ').unwrap_or(("", &key));
    STREET_SUFFIXES
        .iter()
        .filter(|(full, abbr)| {
            last.len() >= MIN_PARTIAL_SUFFIX_CHARS
                && full.starts_with(last)
                && last != *full
                && !abbr.starts_with(last)
        })
        .map(|(_, abbr)| format!("{head} {abbr}").trim_start().to_string())
        .collect()
}

fn strip_designator_and_token(s: &str, designator: &str) -> String {
    let Some(pos) = s.find(designator) else {
        return s.to_string();
    };
    let before_ok = pos == 0 || !s.as_bytes()[pos - 1].is_ascii_alphanumeric();
    let after = pos + designator.len();
    let after_ok = after >= s.len() || !s.as_bytes()[after].is_ascii_alphanumeric();
    if !before_ok || !after_ok {
        return s.to_string();
    }
    let rest = s[after..].trim_start();
    let skip = rest.split_whitespace().next().map_or(0, |t| t.len());
    format!("{} {}", &s[..pos], &rest[skip..])
}

pub fn directional_mask(input: &str) -> u8 {
    let lower = input.to_lowercase();
    DIRECTIONALS
        .iter()
        .enumerate()
        .filter(|(_, (full, abbr))| contains_word(&lower, full) || contains_word(&lower, abbr))
        .fold(0, |mask, (bit, _)| mask | (1 << bit))
}

fn contains_word(s: &str, word: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|token| token == word)
}

fn replace_word(s: &str, word: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut remaining = s;

    while let Some(pos) = remaining.find(word) {
        let before = pos == 0 || !remaining.as_bytes()[pos - 1].is_ascii_alphanumeric();
        let after_pos = pos + word.len();
        let after = after_pos >= remaining.len()
            || !remaining.as_bytes()[after_pos].is_ascii_alphanumeric();

        if before && after {
            result.push_str(&remaining[..pos]);
            result.push_str(replacement);
        } else {
            result.push_str(&remaining[..after_pos]);
        }
        remaining = &remaining[after_pos..];
    }
    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_address() {
        let addr = parse_address("123 Main St, Springfield, IL");
        assert_eq!(addr.house_number.as_deref(), Some("123"));
        assert_eq!(addr.street.as_deref(), Some("Main St"));
        assert_eq!(addr.city.as_deref(), Some("Springfield"));
        assert_eq!(addr.state.as_deref(), Some("IL"));
    }

    #[test]
    fn parse_full_address() {
        let addr = parse_address("456 Oak Ave, Portland, OR, 97201, US");
        assert_eq!(addr.house_number.as_deref(), Some("456"));
        assert_eq!(addr.street.as_deref(), Some("Oak Ave"));
        assert_eq!(addr.postcode.as_deref(), Some("97201"));
        assert_eq!(addr.country.as_deref(), Some("US"));
    }

    #[test]
    fn split_house_number_works() {
        let (num, street) = split_house_number("42 Elm Drive");
        assert_eq!(num.as_deref(), Some("42"));
        assert_eq!(street, "Elm Drive");
    }

    #[test]
    fn directionals_read_whole_words_only() {
        assert_eq!(
            directional_mask("Queen Street West"),
            directional_mask("100 Queen St W")
        );
        assert_ne!(directional_mask("Queen Street West"), 0);
        assert_ne!(
            directional_mask("Queen Street East"),
            directional_mask("Queen Street West")
        );
        assert_eq!(directional_mask("Westminster Bridge"), 0);
    }

    #[test]
    fn normalize_for_match_strips_commas() {
        assert_eq!(
            normalize_for_match("123, Main St, Springfield, IL"),
            normalize_for_match("123 Main St, Springfield, IL")
        );
    }

    #[test]
    fn accents_and_hyphens_fold_away() {
        assert_eq!(normalize_for_match("Zürich"), "zurich");
        assert_eq!(normalize_for_match("Genève"), "geneve");
        assert_eq!(normalize_for_match("Cap-d'Ail"), "cap d ail");
        assert_eq!(normalize_for_match("Straße"), "strasse");
    }

    #[test]
    fn a_half_typed_suffix_also_searches_its_abbreviation() {
        assert_eq!(partial_suffix_keys("Avenu"), vec!["ave"]);
        assert_eq!(partial_suffix_keys("rue du bouleva"), vec!["rue du blvd"]);
        assert!(partial_suffix_keys("Main Street").is_empty());
        assert!(partial_suffix_keys("ma").is_empty());
    }

    #[test]
    fn a_saint_is_not_a_suite() {
        assert_eq!(normalize_for_match("Ste-Croix"), "ste croix");
        assert_eq!(normalize_for_match("12 Main St Ste 200"), "12 main st");
    }

    #[test]
    fn abbreviations_and_units_match_the_long_form() {
        assert_eq!(
            normalize_for_match("123 North Main Street Apt 4"),
            normalize_for_match("123 Main St")
        );
    }
}
