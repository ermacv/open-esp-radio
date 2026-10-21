//! Cited reserved masks are source annotations, never SVD fields.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use svd_rs::{Device, Peripheral};

use crate::{Error, RegisterGeometry, RegisterModel, RegisterModelFragment, Result};

/// One additional register property in the native fragment format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservedBits {
    #[serde(with = "mask")]
    pub mask: u128,
    pub sources: Vec<String>,
}

mod mask {
    use serde::{Deserializer, Serializer, de};
    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        if *value <= i64::MAX as u128 {
            serializer.serialize_u64(*value as u64)
        } else {
            serializer.serialize_str(&format!("{value:#x}"))
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        struct Mask;
        impl de::Visitor<'_> for Mask {
            type Value = u128;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an unsigned mask or a hexadecimal string")
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<u128, E> {
                Ok(u128::from(value))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<u128, E> {
                u128::try_from(value).map_err(E::custom)
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<u128, E> {
                let hex = value
                    .strip_prefix("0x")
                    .ok_or_else(|| E::custom("mask string requires 0x prefix"))?;
                u128::from_str_radix(hex, 16).map_err(E::custom)
            }
        }
        deserializer.deserialize_any(Mask)
    }
}

type Annotations = BTreeMap<String, ReservedBits>;

/// Remove the reserved annotation before the existing SVD deserializer.
pub(crate) fn extract(mut values: Vec<toml::Value>) -> Result<(Vec<Peripheral>, Annotations)> {
    let mut annotations = BTreeMap::new();
    visit(&mut values, &mut |name, register| {
        if let Some(value) = register.remove("reserved") {
            let reserved: ReservedBits = value
                .try_into()
                .map_err(|e| Error::message(format!("{name} reserved: {e}")))?;
            if annotations.insert(name.to_owned(), reserved).is_some() {
                return Err(Error::message(format!(
                    "duplicate reserved annotation {name}"
                )));
            }
        }
        Ok(())
    })?;
    // svd-rs accepts extension keys. Do not silently discard our annotation
    // when a source author places it on a field, cluster or peripheral.
    for value in &values {
        reject_misplaced(value)?;
    }
    let peripherals = values
        .into_iter()
        .map(|value| {
            value
                .try_into()
                .map_err(|e| Error::message(format!("register peripheral: {e}")))
        })
        .collect::<Result<_>>()?;
    Ok((peripherals, annotations))
}

fn reject_misplaced(value: &toml::Value) -> Result<()> {
    match value {
        toml::Value::Table(table) => {
            if table.contains_key("reserved") {
                return Err(Error::message(
                    "reserved is a register property, not a peripheral, cluster or field property",
                ));
            }
            for value in table.values() {
                reject_misplaced(value)?;
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                reject_misplaced(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn visit(
    values: &mut [toml::Value],
    f: &mut impl FnMut(&str, &mut toml::Table) -> Result<()>,
) -> Result<()> {
    for peripheral in values {
        let name = name(peripheral)?;
        if let Some(children) = peripheral
            .get_mut("registers")
            .and_then(toml::Value::as_array_mut)
        {
            visit_children(&name, children, f)?;
        }
    }
    Ok(())
}

fn name(value: &toml::Value) -> Result<String> {
    value
        .get("name")
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::message("peripheral/register/cluster without a name"))
}

fn visit_children(
    parent: &str,
    children: &mut [toml::Value],
    f: &mut impl FnMut(&str, &mut toml::Table) -> Result<()>,
) -> Result<()> {
    for child in children {
        if let Some(register) = child.get_mut("register") {
            let path = format!("{parent}.{}", name(register)?);
            if let Some(register) = register.as_table_mut() {
                f(&path, register)?;
            }
        } else if let Some(cluster) = child.get_mut("cluster") {
            let path = format!("{parent}.{}", name(cluster)?);
            if let Some(children) = cluster
                .get_mut("children")
                .and_then(toml::Value::as_array_mut)
            {
                visit_children(&path, children, f)?;
            }
        }
    }
    Ok(())
}

impl Serialize for RegisterModelFragment {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        #[derive(Serialize)]
        struct Source<'a> {
            schema: u32,
            peripherals: Vec<toml::Value>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            review: &'a Vec<crate::ReviewAnnotation>,
        }
        let mut peripherals = self
            .peripherals
            .iter()
            .map(toml::Value::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(S::Error::custom)?;
        let mut remaining = self.reserved.clone();
        visit(&mut peripherals, &mut |name, register| {
            if let Some(reserved) = remaining.remove(name) {
                register.insert(
                    "reserved".into(),
                    toml::Value::try_from(reserved).map_err(|e| Error::message(e.to_string()))?,
                );
            }
            Ok(())
        })
        .map_err(S::Error::custom)?;
        if !remaining.is_empty() {
            return Err(S::Error::custom(
                "reserved annotation for an absent register",
            ));
        }
        Source {
            schema: self.schema,
            peripherals,
            review: &self.review,
        }
        .serialize(serializer)
    }
}

pub(crate) fn expand(
    device: &Device,
    annotations: &Annotations,
) -> Result<BTreeMap<(u64, u32), ReservedBits>> {
    let mut geometry = crate::geometry::device_geometry(device)?;
    let mut found = BTreeSet::new();
    let mut output = BTreeMap::new();
    for register in &mut geometry {
        if let Some(reserved) = annotations.get(&register.template) {
            found.insert(register.template.clone());
            register.reserved = Some(reserved.clone());
            let width = register
                .width
                .ok_or_else(|| Error::message("reserved register has no inherited width"))?;
            if output
                .insert((register.address, width), reserved.clone())
                .is_some()
            {
                return Err(Error::message("duplicate physical reserved declaration"));
            }
        }
    }
    if let Some(name) = annotations.keys().find(|name| !found.contains(*name)) {
        return Err(Error::message(format!(
            "reserved annotation targets absent register {name}"
        )));
    }
    validate(&geometry)?;
    Ok(output)
}

pub(crate) fn validate(geometry: &[RegisterGeometry]) -> Result<()> {
    for register in geometry {
        let Some(reserved) = &register.reserved else {
            continue;
        };
        let width = register
            .width
            .ok_or_else(|| Error::message("reserved register has no width"))?;
        if reserved.mask == 0 || (width < 128 && reserved.mask >> width != 0) {
            return Err(Error::message(format!(
                "{} reserved mask is empty or exceeds register width",
                register.name
            )));
        }
        let mut sources = BTreeSet::new();
        if reserved.sources.is_empty()
            || reserved
                .sources
                .iter()
                .any(|source| source.trim().is_empty() || !sources.insert(source))
        {
            return Err(Error::message(format!(
                "{} reserved mask requires nonempty distinct sources",
                register.name
            )));
        }
        for field in &register.fields {
            let bits = if field.width == 128 {
                u128::MAX
            } else {
                ((1u128 << field.width) - 1) << field.offset
            };
            if bits & reserved.mask != 0 {
                return Err(Error::message(format!(
                    "{} reserved mask overlaps field {}",
                    register.name, field.name
                )));
            }
        }
    }
    Ok(())
}

impl RegisterModel {
    /// Evidence references are checked against the publication's catalogs.
    pub fn reserved_sources(&self) -> impl Iterator<Item = &str> {
        self.reserved
            .values()
            .flat_map(|reserved| reserved.sources.iter().map(String::as_str))
    }
}
