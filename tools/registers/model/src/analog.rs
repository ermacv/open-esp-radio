//! Native analog-I2C register space. A transport address is never a CPU
//! address. Field declarations and their semantic validation are shared with
//! MMIO; route profiles describe how a request reaches the physical byte.
use crate::{
    BitStates, Error, NamingRequirement, ReservedBits, Result, ReviewAnnotation, ReviewMetadata,
};
use oer_registers_contracts::SemanticEntityId;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use svd_rs::{
    Access, FieldInfo, ModifiedWriteValues, ReadAction, RegisterInfo, RegisterProperties,
    ValidateLevel,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalogAddress {
    pub block: u8,
    pub index: u8,
}

impl AnalogAddress {
    /// Canonical numeric transport identity for the existing semantic-ID
    /// grammar. This encoding does not create a CPU address or pointer.
    pub const fn identity(self) -> u64 {
        ((self.block as u64) << 8) | self.index as u64
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Route {
    pub profile: String,
    pub request_code: u8,
    pub host: u8,
    pub read_select: u16,
    pub read_mask_bits: u8,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReadSafety {
    /// A finite lifecycle qualification; access=readable alone is insufficient.
    pub epoch: String,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AnalogRegister {
    pub name: String,
    pub index: u8,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub access: Option<Access>,
    #[serde(default)]
    pub read_action: Option<ReadAction>,
    #[serde(default)]
    pub modified_write_values: Option<ModifiedWriteValues>,
    #[serde(default)]
    pub reset_value: Option<u8>,
    #[serde(default)]
    pub reset_known_mask: Option<u8>,
    #[serde(default)]
    pub reserved: Option<ReservedBits>,
    #[serde(default)]
    pub read_safe: Option<ReadSafety>,
    /// The same native field declaration as the CPU model, including enums,
    /// access, constraints and read/write side effects.
    #[serde(default, deserialize_with = "field_declarations")]
    pub fields: Vec<FieldInfo>,
}

fn field_declarations<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<FieldInfo>, D::Error> {
    use serde::de::Error as _;
    let values = Vec::<toml::Value>::deserialize(d)?;
    values.into_iter().map(|value| {
        let table = value.as_table().ok_or_else(|| D::Error::custom("analog field must be a table"))?;
        for key in table.keys() {
            if !matches!(key.as_str(), "name" | "description" | "bitOffset" | "bitWidth" | "bitRange" | "msb" | "lsb" | "access" | "modifiedWriteValues" | "writeConstraint" | "readAction" | "enumeratedValues") {
                return Err(D::Error::custom(format!("unsupported analog field key {key:?}; field inheritance/arrays require explicit expanded geometry")));
            }
        }
        let field: FieldInfo = value.try_into().map_err(D::Error::custom)?;
        if field.enumerated_values.iter().any(|v| v.derived_from.is_some()) {
            return Err(D::Error::custom("analog enumerations require explicit expanded values"));
        }
        Ok(field)
    }).collect()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AnalogBlock {
    pub name: String,
    pub code: u8,
    pub sources: Vec<String>,
    #[serde(default)]
    pub routes: Vec<Route>,
    #[serde(default)]
    pub registers: Vec<AnalogRegister>,
    /// Whole-record review default, resolved with the common review resolver.
    #[serde(default)]
    pub review: Option<ReviewMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AnalogSpace {
    pub schema: u32,
    pub kind: String,
    pub blocks: Vec<AnalogBlock>,
    #[serde(default)]
    pub review: Vec<ReviewAnnotation>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalogGeometry {
    pub address: AnalogAddress,
    pub identity: String,
    pub name: String,
    pub states: BitStates,
    pub reset_value: Option<u8>,
    pub reset_known_mask: Option<u8>,
    pub read_safe_epoch: Option<String>,
    pub fields: Vec<AnalogFieldGeometry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalogFieldGeometry {
    pub identity: String,
    pub name: String,
    pub declaration: FieldInfo,
}

fn source_ids(kind: &str, sources: &[String]) -> Result<()> {
    let distinct: BTreeSet<_> = sources.iter().collect();
    if sources.is_empty()
        || distinct.len() != sources.len()
        || sources.iter().any(|s| s.trim().is_empty())
    {
        return Err(Error::message(format!(
            "{kind} requires nonempty distinct sources"
        )));
    }
    Ok(())
}

fn component(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        || name.as_bytes().first().is_none_or(u8::is_ascii_digit)
    {
        return Err(Error::message(format!(
            "invalid analog model name {name:?}"
        )));
    }
    Ok(())
}

impl AnalogRegister {
    fn declaration(&self) -> Result<RegisterInfo> {
        RegisterInfo::builder()
            .name(self.name.clone())
            .description(self.description.clone())
            .address_offset(u32::from(self.index))
            .properties(
                RegisterProperties::new()
                    .size(Some(8))
                    .access(self.access)
                    .reset_value(self.reset_value.map(u64::from))
                    .reset_mask(self.reset_known_mask.map(u64::from))
                    .build(ValidateLevel::Strict)?,
            )
            .read_action(self.read_action)
            .modified_write_values(self.modified_write_values)
            .fields(Some(
                self.fields
                    .iter()
                    .cloned()
                    .map(svd_rs::MaybeArray::Single)
                    .collect(),
            ))
            .build(ValidateLevel::Strict)
            .map_err(Error::from)
    }

    pub fn bit_states(&self) -> Result<BitStates> {
        let mut fields = 0;
        let mut opaque = 0;
        for field in &self.fields {
            let offset = field.bit_offset();
            let width = field.bit_width();
            if width == 0 || offset.checked_add(width).is_none_or(|n| n > 8) {
                return Err(Error::message("invalid analog field width"));
            }
            let mask = ((1u128 << width) - 1) << offset;
            fields |= mask;
            if self.name.ends_with(crate::OPAQUE_SUFFIX)
                || field.name.ends_with(crate::OPAQUE_SUFFIX)
            {
                opaque |= mask;
            }
        }
        BitStates::new(
            8,
            fields,
            opaque,
            self.reserved.as_ref().map_or(0, |r| r.mask),
        )
    }
}

impl AnalogSpace {
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path)?, NamingRequirement::Complete)
            .map_err(|e| Error::manifest("analog-I2C model", path, e))
    }

    fn parse(text: &str, naming: NamingRequirement) -> Result<Self> {
        let mut space: Self = toml_edit::de::from_str(text)?;
        if space.schema != 1 || space.kind != "analog-i2c" || space.blocks.is_empty() {
            return Err(Error::message(
                "analog model requires schema1, kind=analog-i2c and blocks",
            ));
        }
        let mut codes = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut routes = BTreeSet::new();
        let mut entities = BTreeMap::new();
        let mut defaults = BTreeMap::new();
        let mut positions = BTreeMap::new();
        for block in &space.blocks {
            component(&block.name)?;
            source_ids("analog block", &block.sources)?;
            if !codes.insert(block.code) || !names.insert(&block.name) {
                return Err(Error::message("duplicate analog block code/name"));
            }
            for route in &block.routes {
                source_ids("analog route", &route.sources)?;
                if route.profile.is_empty()
                    || !route
                        .profile
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                    || route.host > 1
                    || route.read_select.count_ones() > 1
                    || !matches!(route.read_mask_bits, 24 | 32)
                {
                    return Err(Error::message(
                        "invalid analog route profile, host or one-hot read-select",
                    ));
                }
                if !routes.insert((route.profile.clone(), route.request_code)) {
                    return Err(Error::message("ambiguous analog request route"));
                }
            }
            let mut indices = BTreeSet::new();
            let mut register_names = BTreeSet::new();
            for register in &block.registers {
                component(&register.name)?;
                if !indices.insert(register.index) || !register_names.insert(&register.name) {
                    return Err(Error::message("duplicate analog register index/name"));
                }
                let declaration = register.declaration()?;
                crate::model_validation::validate_register(
                    &block.name,
                    &declaration,
                    declaration.properties,
                )?;
                match (register.reset_value, register.reset_known_mask) {
                    (None, None) => {}
                    (Some(value), Some(mask)) if mask != 0 && value & !mask == 0 => {}
                    _ => {
                        return Err(Error::message(
                            "analog reset requires a value restricted to a nonempty known mask, or both absent",
                        ));
                    }
                }
                if let Some(reserved) = &register.reserved {
                    source_ids("analog reserved mask", &reserved.sources)?;
                    if reserved.mask == 0 {
                        return Err(Error::message("empty analog reserved mask"));
                    }
                }
                register.bit_states()?;
                if let Some(safe) = &register.read_safe {
                    source_ids("analog read safety", &safe.sources)?;
                    if safe.epoch.trim().is_empty()
                        || !matches!(
                            register.access,
                            Some(Access::ReadOnly | Access::ReadWrite | Access::ReadWriteOnce)
                        )
                        || register.read_action.is_some()
                        || register.fields.iter().any(|f| f.read_action.is_some())
                    {
                        return Err(Error::message(
                            "analog read safety requires a finite epoch and explicit readable access without read actions",
                        ));
                    }
                }
                let entity = format!("{}.{}", block.name, register.name);
                positions.insert(
                    entity.clone(),
                    format!("BYTE_{:02X}_OPAQUE", register.index),
                );
                entities.insert(
                    entity.clone(),
                    register.name.ends_with(crate::OPAQUE_SUFFIX),
                );
                defaults.insert(entity.clone(), block.review.clone());
                for field in &register.fields {
                    component(&field.name)?;
                    let entity = format!("{entity}.{}", field.name);
                    positions.insert(
                        entity.clone(),
                        crate::opaque_position_name(
                            crate::Position::Field {
                                offset: field.bit_offset(),
                                width: field.bit_width(),
                            },
                            false,
                        ),
                    );
                    entities.insert(entity.clone(), field.name.ends_with(crate::OPAQUE_SUFFIX));
                    defaults.insert(entity, block.review.clone());
                }
            }
        }
        space.review = crate::review_defaults::resolve(
            space.review,
            &entities,
            |entity, _| defaults.get(entity).cloned().flatten(),
            "analog block",
        )?;
        let mut seen = BTreeSet::new();
        for review in &space.review {
            if !entities.contains_key(&review.entity) || !seen.insert(&review.entity) {
                return Err(Error::message("unknown or duplicate analog review entity"));
            }
            source_ids("analog review", &review.sources)?;
            let classifications = usize::from(review.provenance.is_some())
                + usize::from(review.accuracy.is_some())
                + usize::from(review.completeness.is_some());
            if !matches!(classifications, 0 | 3)
                || (naming == NamingRequirement::Complete
                    && (classifications != 3
                        || review.provenance
                            == Some(oer_registers_contracts::FactProvenance::Hint)))
            {
                return Err(Error::message(
                    "analog review needs provenance, accuracy and completeness",
                ));
            }
        }
        for entity in entities.keys() {
            let review = space.review.iter().find(|r| r.entity == *entity);
            if naming == NamingRequirement::Complete || review.is_some_and(|r| r.naming.is_some()) {
                crate::validate_naming(
                    entity,
                    review,
                    positions
                        .get(entity)
                        .ok_or_else(|| Error::message("missing analog naming position"))?,
                )?;
            }
        }
        Ok(space)
    }

    pub fn sources(&self) -> impl Iterator<Item = &str> {
        self.blocks
            .iter()
            .flat_map(|b| {
                b.sources
                    .iter()
                    .chain(b.routes.iter().flat_map(|r| &r.sources))
                    .chain(b.registers.iter().flat_map(|r| {
                        r.reserved
                            .iter()
                            .flat_map(|v| &v.sources)
                            .chain(r.read_safe.iter().flat_map(|v| &v.sources))
                    }))
            })
            .chain(self.review.iter().flat_map(|r| &r.sources))
            .map(String::as_str)
    }

    pub fn geometry(&self, chip: &str) -> Result<Vec<AnalogGeometry>> {
        let mut output = Vec::new();
        for block in &self.blocks {
            for register in &block.registers {
                let address = AnalogAddress {
                    block: block.code,
                    index: register.index,
                };
                let identity =
                    SemanticEntityId::register(chip, "analog-i2c", address.identity(), 8)
                        .map_err(|e| Error::message(e.to_string()))?
                        .to_string();
                let fields = register
                    .fields
                    .iter()
                    .map(|field| {
                        let identity = SemanticEntityId::register_field(
                            chip,
                            "analog-i2c",
                            address.identity(),
                            8,
                            field.bit_offset(),
                            field.bit_width(),
                        )
                        .map_err(|e| Error::message(e.to_string()))?
                        .to_string();
                        Ok(AnalogFieldGeometry {
                            identity,
                            name: field.name.clone(),
                            declaration: field.clone(),
                        })
                    })
                    .collect::<Result<_>>()?;
                output.push(AnalogGeometry {
                    address,
                    identity,
                    name: format!("{}.{}", block.name, register.name),
                    states: register.bit_states()?,
                    reset_value: register.reset_value,
                    reset_known_mask: register.reset_known_mask,
                    read_safe_epoch: register.read_safe.as_ref().map(|r| r.epoch.clone()),
                    fields,
                });
            }
        }
        output.sort_by_key(|r| r.address);
        Ok(output)
    }
}

pub(crate) fn load(
    base: &Path,
    paths: &[String],
    inputs: &mut BTreeMap<PathBuf, String>,
    naming: NamingRequirement,
) -> Result<Vec<AnalogSpace>> {
    if paths.len() > 1 {
        return Err(Error::message(
            "one analog-i2c space per chip; combine its blocks in one model",
        ));
    }
    paths
        .iter()
        .map(|relative| {
            crate::validate_relative_fragment(relative)?;
            let path = base.join(relative);
            let text = fs::read_to_string(&path)?;
            if inputs.insert(path.canonicalize()?, text.clone()).is_some() {
                return Err(Error::message("duplicate model input path"));
            }
            AnalogSpace::parse(&text, naming)
                .map_err(|e| Error::manifest("analog-I2C model", &path, e))
        })
        .collect()
}

#[cfg(test)]
#[path = "analog_tests.rs"]
mod tests;
