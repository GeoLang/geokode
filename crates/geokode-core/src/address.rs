//! Address parsing and normalization.
//!
//! Decomposes raw address strings into structured components:
//! house number, street, city, state/province, postal code, country.

use serde::{Deserialize, Serialize};

/// A structured address with parsed components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Address {
    pub house_number: Option<String>,
    pub street: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub postcode: Option<String>,
    pub country: Option<String>,
    /// Original full address string.
    pub full: String,
}

/// A geocoding result with coordinates and confidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoResult {
    pub address: Address,
    pub lat: f64,
    pub lon: f64,
    /// Confidence score 0.0–1.0.
    pub confidence: f64,
}

/// Parse a raw address string into structured components.
pub fn parse_address(input: &str) -> Address {
    let input = input.trim();
    let parts: Vec<&str> = input.split(',').map(|s| s.trim()).collect();

    match parts.len() {
        0 => Address {
            house_number: None,
            street: None,
            city: None,
            state: None,
            postcode: None,
            country: None,
            full: input.to_string(),
        },
        1 => Address {
            house_number: None,
            street: Some(parts[0].to_string()),
            city: None,
            state: None,
            postcode: None,
            country: None,
            full: input.to_string(),
        },
        2 => Address {
            house_number: None,
            street: Some(parts[0].to_string()),
            city: Some(parts[1].to_string()),
            state: None,
            postcode: None,
            country: None,
            full: input.to_string(),
        },
        3 => {
            let (house, street) = split_house_number(parts[0]);
            Address {
                house_number: house,
                street: Some(street),
                city: Some(parts[1].to_string()),
                state: Some(parts[2].to_string()),
                postcode: None,
                country: None,
                full: input.to_string(),
            }
        }
        4 => {
            let (house, street) = split_house_number(parts[0]);
            Address {
                house_number: house,
                street: Some(street),
                city: Some(parts[1].to_string()),
                state: Some(parts[2].to_string()),
                postcode: None,
                country: Some(parts[3].to_string()),
                full: input.to_string(),
            }
        }
        _ => {
            let (house, street) = split_house_number(parts[0]);
            Address {
                house_number: house,
                street: Some(street),
                city: Some(parts[1].to_string()),
                state: Some(parts[2].to_string()),
                postcode: Some(parts[3].to_string()),
                country: parts.get(4).map(|s| s.to_string()),
                full: input.to_string(),
            }
        }
    }
}

/// Split "123 Main St" into (Some("123"), "Main St").
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

/// Common street suffix abbreviations for normalization.
pub fn normalize_street(s: &str) -> String {
    let s = s.to_lowercase();
    STREET_SUFFIXES
        .iter()
        .fold(s, |acc, &(full, abbr)| acc.replace(full, abbr))
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

/// Directional prefixes/suffixes commonly found in US addresses.
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

/// Unit/apartment designators.
const UNIT_DESIGNATORS: &[&str] = &[
    "apt",
    "apartment",
    "unit",
    "suite",
    "ste",
    "floor",
    "fl",
    "room",
    "rm",
    "#",
    "no",
    "bldg",
    "building",
    "dept",
];

/// Normalize a full address string for matching: lowercase, expand/abbreviate,
/// strip punctuation, collapse whitespace.
pub fn normalize_address(input: &str) -> String {
    let mut s = input.to_lowercase();

    // Remove common punctuation (periods, commas preserved for structure)
    s = s.replace('.', "");
    s = s.replace('#', " # ");

    // Normalize directionals
    for &(full, abbr) in DIRECTIONALS {
        s = replace_word(&s, full, abbr);
    }

    // Normalize street suffixes
    for &(full, abbr) in STREET_SUFFIXES {
        s = replace_word(&s, full, abbr);
    }

    // Collapse whitespace
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse unit/apartment number from an address string.
pub fn extract_unit(input: &str) -> (String, Option<String>) {
    let lower = input.to_lowercase();
    for designator in UNIT_DESIGNATORS {
        if let Some(pos) = lower.find(designator) {
            let before = input[..pos].trim().trim_end_matches(',').trim();
            let after = input[pos + designator.len()..]
                .trim()
                .trim_start_matches(['.', ' ', ':']);
            let unit = after
                .split([',', ' '])
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !unit.is_empty() {
                return (before.to_string(), Some(unit));
            }
        }
    }
    (input.to_string(), None)
}

/// Detect the likely country format of an address string.
pub fn detect_format(input: &str) -> AddressFormat {
    let trimmed = input.trim();
    let parts: Vec<&str> = trimmed.split(',').collect();

    // German/European: "Straße Nr, PLZ Stadt"
    if parts.len() >= 2 {
        let last = parts.last().unwrap().trim();
        if last.len() >= 4 && last.chars().take(5).all(|c| c.is_ascii_digit()) {
            return AddressFormat::European;
        }
    }

    // Japanese: contains CJK characters
    if trimmed
        .chars()
        .any(|c| ('\u{3000}'..='\u{9FFF}').contains(&c))
    {
        return AddressFormat::Japanese;
    }

    // Default to US/North American
    AddressFormat::NorthAmerican
}

/// Address format classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressFormat {
    NorthAmerican,
    European,
    Japanese,
}

/// Replace a whole word in a string (not part of a larger word).
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
            remaining = &remaining[after_pos..];
        } else {
            result.push_str(&remaining[..pos + word.len()]);
            remaining = &remaining[after_pos..];
        }
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
    fn normalize_street_abbreviations() {
        assert_eq!(normalize_street("Main Street"), "main st");
        assert_eq!(normalize_street("Park Avenue"), "park ave");
        assert_eq!(normalize_street("Sunset Boulevard"), "sunset blvd");
    }

    #[test]
    fn normalize_address_full() {
        assert_eq!(normalize_address("123 North Main Street"), "123 n main st");
        assert_eq!(
            normalize_address("456 Southeast  Oak  Avenue"),
            "456 se oak ave"
        );
    }

    #[test]
    fn extract_unit_apartment() {
        let (addr, unit) = extract_unit("123 Main St Apt 4B");
        assert_eq!(addr, "123 Main St");
        assert_eq!(unit, Some("4B".to_string()));
    }

    #[test]
    fn extract_unit_suite() {
        let (addr, unit) = extract_unit("456 Oak Ave, Suite 200");
        assert_eq!(addr, "456 Oak Ave");
        assert_eq!(unit, Some("200".to_string()));
    }

    #[test]
    fn extract_unit_none() {
        let (addr, unit) = extract_unit("789 Elm Drive");
        assert_eq!(addr, "789 Elm Drive");
        assert_eq!(unit, None);
    }

    #[test]
    fn detect_north_american_format() {
        assert_eq!(
            detect_format("123 Main St, Springfield, IL"),
            AddressFormat::NorthAmerican
        );
    }

    #[test]
    fn detect_european_format() {
        assert_eq!(
            detect_format("Hauptstraße 42, 10115 Berlin"),
            AddressFormat::European
        );
    }

    #[test]
    fn extended_street_suffixes() {
        assert_eq!(normalize_street("Oak Circle"), "oak cir");
        assert_eq!(normalize_street("Pine Terrace"), "pine ter");
        assert_eq!(normalize_street("US Highway 66"), "us hwy 66");
    }
}
