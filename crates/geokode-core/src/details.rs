use crate::address::{Address, OsmType};
use crate::codec::{Decoder, Encoder};
use std::io;

pub(crate) const LABEL_FIELDS: usize = 6;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Details {
    pub name: Option<String>,
    pub name_en: Option<String>,
    pub address: Address,
    pub country_code: Option<String>,
    pub osm: Option<(OsmType, i64)>,
    pub osm_key: Option<String>,
    pub osm_value: Option<String>,
    pub admin_level: Option<u8>,
    pub population: Option<u64>,
}

pub(crate) fn display_name(name: Option<&str>, address: &Address) -> String {
    let leading = name.map(str::to_string).or_else(|| {
        let parts: Vec<&str> = [address.house_number.as_deref(), address.street.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    });
    let context = [
        address.city.as_deref(),
        address.state.as_deref(),
        address.country.as_deref(),
    ];
    let mut parts: Vec<&str> = Vec::new();
    for part in leading
        .as_deref()
        .into_iter()
        .chain(context.into_iter().flatten())
    {
        if !part.is_empty() && !parts.contains(&part) {
            parts.push(part);
        }
    }
    parts.join(", ")
}

fn osm_type_code(osm_type: OsmType) -> u64 {
    match osm_type {
        OsmType::Node => 1,
        OsmType::Way => 2,
        OsmType::Relation => 3,
    }
}

fn osm_type_from_code(code: u64) -> Option<OsmType> {
    match code {
        1 => Some(OsmType::Node),
        2 => Some(OsmType::Way),
        3 => Some(OsmType::Relation),
        _ => None,
    }
}

impl Details {
    pub fn lead(&self) -> Option<&str> {
        self.name_en.as_deref().or(self.name.as_deref())
    }

    pub fn labels(&self) -> [Option<String>; LABEL_FIELDS] {
        [
            self.address.city.clone(),
            self.address.state.clone(),
            self.address.country.clone(),
            self.country_code.clone(),
            self.osm_key.clone(),
            self.osm_value.clone(),
        ]
    }

    pub fn encode_inline(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut out = Encoder::new(&mut bytes);
        let address = &self.address;
        let derived_full = display_name(self.lead(), address);
        let full = (!address.full.is_empty() && address.full != derived_full)
            .then_some(address.full.as_str());
        let write = |out: &mut Encoder<&mut Vec<u8>>| -> io::Result<()> {
            out.optional_text(self.name.as_deref())?;
            out.optional_text(self.name_en.as_deref())?;
            out.optional_text(address.house_number.as_deref())?;
            out.optional_text(address.street.as_deref())?;
            out.optional_text(address.postcode.as_deref())?;
            out.optional_text(full)?;
            match self.osm {
                Some((osm_type, id)) => {
                    out.unsigned(osm_type_code(osm_type))?;
                    out.signed(id)?;
                }
                None => out.unsigned(0)?,
            }
            out.unsigned(self.admin_level.map_or(0, u64::from))?;
            out.unsigned(self.population.map_or(0, |p| p.saturating_add(1)))
        };
        write(&mut out).expect("writing to a Vec cannot fail");
        bytes
    }

    pub fn decode(bytes: &[u8], labels: &[String]) -> io::Result<Details> {
        let mut input = Decoder::new(bytes);
        let mut label = || -> io::Result<Option<String>> {
            let id = input.unsigned()?;
            Ok(id
                .checked_sub(1)
                .and_then(|id| labels.get(id as usize).cloned()))
        };
        let (city, state, country) = (label()?, label()?, label()?);
        let (country_code, osm_key, osm_value) = (label()?, label()?, label()?);
        let name = input.optional_text()?;
        let name_en = input.optional_text()?;
        let mut address = Address {
            house_number: input.optional_text()?,
            street: input.optional_text()?,
            city,
            state,
            postcode: input.optional_text()?,
            country,
            full: String::new(),
        };
        address.full = input
            .optional_text()?
            .unwrap_or_else(|| display_name(name_en.as_deref().or(name.as_deref()), &address));
        let osm = match osm_type_from_code(input.unsigned()?) {
            Some(osm_type) => Some((osm_type, input.signed()?)),
            None => None,
        };
        Ok(Details {
            name,
            name_en,
            address,
            country_code,
            osm,
            osm_key,
            osm_value,
            admin_level: u8::try_from(input.unsigned()?).ok().filter(|l| *l != 0),
            population: input.unsigned()?.checked_sub(1),
        })
    }
}

pub(crate) fn encode_labels(ids: [Option<u32>; LABEL_FIELDS]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut out = Encoder::new(&mut bytes);
    for id in ids {
        out.unsigned(id.map_or(0, |id| u64::from(id) + 1))
            .expect("writing to a Vec cannot fail");
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_round_trip_through_labels() {
        let details = Details {
            name: Some("Musée Océanographique".to_string()),
            name_en: Some("Oceanographic Museum".to_string()),
            address: Address {
                street: Some("Avenue Saint-Martin".to_string()),
                city: Some("Monaco".to_string()),
                country: Some("Monaco".to_string()),
                postcode: Some("98000".to_string()),
                ..Address::default()
            },
            country_code: Some("mc".to_string()),
            osm: Some((OsmType::Way, 23_715_051)),
            osm_key: Some("tourism".to_string()),
            osm_value: Some("museum".to_string()),
            admin_level: None,
            population: Some(0),
        };
        let mut labels: Vec<String> = Vec::new();
        let ids = details.labels().map(|label| {
            label.map(|text| {
                labels.push(text);
                (labels.len() - 1) as u32
            })
        });
        let mut bytes = encode_labels(ids);
        bytes.extend(details.encode_inline());
        let back = Details::decode(&bytes, &labels).unwrap();
        assert_eq!(back.address.full, "Oceanographic Museum, Monaco");
        assert_eq!(
            back,
            Details {
                address: Address {
                    full: "Oceanographic Museum, Monaco".to_string(),
                    ..details.address.clone()
                },
                ..details
            }
        );
    }
}
