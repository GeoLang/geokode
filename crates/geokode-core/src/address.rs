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
    s.to_lowercase()
        .replace("street", "st")
        .replace("avenue", "ave")
        .replace("boulevard", "blvd")
        .replace("drive", "dr")
        .replace("road", "rd")
        .replace("lane", "ln")
        .replace("court", "ct")
        .replace("place", "pl")
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
}
