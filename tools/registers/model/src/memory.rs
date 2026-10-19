//! A chip register model's memory map (`chips/<chip>/registers/model/memory.toml`):
//! its address spaces and named regions, among them the MMIO windows.
use crate::{Error, RegisterEvidenceSet, RegisterModel, Result};
use serde::Deserialize;
use std::{collections::BTreeSet, path::Path};

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

/// A memory map: address spaces and named, non-overlapping regions.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Memory {
    schema: u32,
    default_address_space: String,
    address_spaces: Vec<Space>,
    regions: Vec<Region>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Space {
    id: String,
    address_width: u8,
    endianness: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Region {
    name: String,
    address_space: String,
    kind: String,
    start: u64,
    end_exclusive: u64,
    permissions: String,
    #[serde(default, rename = "volatile")]
    _volatile: bool,
    alias_of: Option<String>,
}
impl Memory {
    /// The `start..end` of the MMIO region `name`.
    pub fn mmio(&self, name: &str) -> Option<(u64, u64)> {
        self.regions
            .iter()
            .find(|region| region.name == name && region.kind == "mmio")
            .map(|region| (region.start, region.end_exclusive))
    }

    /// Every MMIO region of the default address space that is no alias, as
    /// `(name, start, end)`, in map order.
    pub fn mmio_regions(&self) -> Vec<(&str, u64, u64)> {
        self.regions
            .iter()
            .filter(|r| {
                r.kind == "mmio"
                    && r.address_space == self.default_address_space
                    && r.alias_of.is_none()
            })
            .map(|r| (r.name.as_str(), r.start, r.end_exclusive))
            .collect()
    }

    /// Every RAM region of the default address space that is no alias, by
    /// name, as `start..end`: the regions a memory layout's placement names.
    pub fn ram_regions(&self) -> std::collections::BTreeMap<String, (u64, u64)> {
        self.regions
            .iter()
            .filter(|r| {
                r.kind == "ram"
                    && r.address_space == self.default_address_space
                    && r.alias_of.is_none()
            })
            .map(|r| (r.name.clone(), (r.start, r.end_exclusive)))
            .collect()
    }

    pub fn load(path: &Path) -> Result<Self> {
        Ok(toml_edit::de::from_str(&std::fs::read_to_string(path)?)?)
    }
    pub fn validate(
        &self,
        model: &RegisterModel,
        owned: &[String],
        evidence: &RegisterEvidenceSet,
    ) -> Result<()> {
        if self.schema != 1 || self.default_address_space != model.address_space() {
            return Err(invalid("invalid memory map schema/address space"));
        }
        let mut ids = BTreeSet::new();
        for space in &self.address_spaces {
            if !valid_id(&space.id)
                || !ids.insert(&space.id)
                || !(1..=64).contains(&space.address_width)
                || !matches!(space.endianness.as_str(), "little" | "big")
            {
                return Err(invalid("invalid memory address space"));
            }
        }
        if !ids.contains(&self.default_address_space) {
            return Err(invalid("default memory address space is absent"));
        }
        let mut names = BTreeSet::new();
        for region in &self.regions {
            let space = self
                .address_spaces
                .iter()
                .find(|s| s.id == region.address_space)
                .ok_or_else(|| invalid("unknown region address space"))?;
            if !valid_id(&region.name)
                || !names.insert(&region.name)
                || region.start >= region.end_exclusive
                || (space.address_width < 64
                    && region.end_exclusive > (1u64 << space.address_width))
                || !matches!(
                    region.kind.as_str(),
                    "code" | "rodata" | "ram" | "mmio" | "device" | "unknown"
                )
                || ['r', 'w', 'x']
                    .into_iter()
                    .any(|p| region.permissions.matches(p).count() > 1)
                || region.permissions.chars().any(|c| !"rwx".contains(c))
            {
                return Err(invalid(format!("invalid memory region {}", region.name)));
            }
            if let Some(alias) = &region.alias_of
                && (alias == &region.name
                    || !self.regions.iter().any(|r| {
                        &r.name == alias
                            && r.address_space == region.address_space
                            && r.start == region.start
                            && r.end_exclusive == region.end_exclusive
                    }))
            {
                return Err(invalid("invalid memory alias"));
            }
        }
        for (index, left) in self.regions.iter().enumerate() {
            for right in &self.regions[index + 1..] {
                if left.address_space == right.address_space
                    && left.start < right.end_exclusive
                    && right.start < left.end_exclusive
                    && left.alias_of.as_deref() != Some(right.name.as_str())
                    && right.alias_of.as_deref() != Some(left.name.as_str())
                {
                    return Err(invalid("memory regions overlap without explicit alias"));
                }
            }
        }
        let ranges: Vec<_> = self
            .regions
            .iter()
            .filter(|r| {
                r.kind == "mmio" && r.address_space == model.address_space() && r.alias_of.is_none()
            })
            .collect();
        let mut selected = BTreeSet::new();
        for name in owned {
            if !selected.insert(name) || !ranges.iter().any(|r| &r.name == name) {
                return Err(invalid(format!("invalid owned MMIO range {name}")));
            }
        }
        for ((address, width), name) in model.register_identities()? {
            let end = address
                .checked_add(u64::from(width).div_ceil(8))
                .ok_or_else(|| invalid("register range overflow"))?;
            if !ranges
                .iter()
                .any(|r| selected.contains(&r.name) && r.start <= address && end <= r.end_exclusive)
            {
                return Err(invalid(format!(
                    "register {name} is outside owned MMIO ranges"
                )));
            }
        }
        let ram = self.ram_regions();
        for layout in model.layouts() {
            let fields = layout
                .units
                .iter()
                .flat_map(|unit| unit.fields.iter())
                .flat_map(|field| field.placement.iter());
            if let Some(region) = layout
                .placement
                .iter()
                .chain(fields)
                .find(|region| !ram.contains_key(region.as_str()))
            {
                return Err(invalid(format!(
                    "layout {} is placed in {region}, which is not a RAM region of the memory map",
                    layout.name
                )));
            }
        }
        for range in &evidence.ranges {
            if !ranges
                .iter()
                .any(|r| r.start <= range.start && range.end_exclusive <= r.end_exclusive)
            {
                return Err(invalid(format!(
                    "evidence range {} is outside MMIO map",
                    range.name
                )));
            }
        }
        Ok(())
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mmio_windows_are_the_default_space_s_mmio_regions_but_aliases() {
        let memory: Memory = toml_edit::de::from_str(
            r#"
schema = 1
default-address-space = "cpu"

[[address-spaces]]
id = "cpu"
address-width = 32
endianness = "little"

[[address-spaces]]
id = "dma"
address-width = 32
endianness = "little"

[[regions]]
name = "modem"
address-space = "cpu"
kind = "mmio"
start = 0x20000000
end-exclusive = 0x20001000
permissions = "rw"

[[regions]]
name = "modem-alias"
address-space = "cpu"
kind = "mmio"
start = 0x20000000
end-exclusive = 0x20001000
permissions = "rw"
alias-of = "modem"

[[regions]]
name = "sram"
address-space = "cpu"
kind = "ram"
start = 0x30000000
end-exclusive = 0x30001000
permissions = "rwx"

[[regions]]
name = "dma-window"
address-space = "dma"
kind = "mmio"
start = 0x0
end-exclusive = 0x1000
permissions = "rw"
"#,
        )
        .unwrap();
        assert_eq!(memory.mmio_regions(), [("modem", 0x2000_0000, 0x2000_1000)]);
    }
}
