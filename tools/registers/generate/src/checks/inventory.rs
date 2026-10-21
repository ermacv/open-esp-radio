//! Compare vendor bit uses with declared register fields.
//!
//! `cargo registers check inventory` runs Blobray on every pinned vendor
//! binary in the publication's owned MMIO ranges, using `riscv-integer`
//! and `--call-returns`. The schema-9 consumer in [`uses`] rejects uses of
//! absent bits and reports unknown bits and coverage limits separately.
//! This module supplies expanded geometry, inherited resets, and the same
//! bounded/indexed candidate addresses used by the MMIO inventory.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

mod run;
#[cfg(test)]
mod tests;
pub mod uses;

pub use run::run;

/// Bits of the word the inventory compares; register-model registers are
/// split into words of this size.
const WORD_BITS: u32 = 32;
/// Bytes of one compared word.
const WORD_BYTES: u32 = WORD_BITS / 8;
/// A register as the model declares it, independent of its SVD form.
#[derive(Clone, Debug)]
pub struct DeclaredRegister {
    pub address: u64,
    pub width_bits: u32,
    pub name: String,
    /// The declaration the register expands from: equal for the elements
    /// of one array.
    pub template: String,
    pub reset_value: Option<u64>,
    pub reset_mask: Option<u64>,
    pub fields: Vec<DeclaredField>,
}

#[derive(Clone, Debug)]
pub struct DeclaredField {
    pub name: String,
    pub offset: u32,
    pub width: u32,
}

/// What the model declares about one word.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeclaredWord {
    /// Names of the registers overlapping the word.
    pub registers: BTreeSet<String>,
    /// Bits of declared fields.
    pub fields: u32,
    /// Declared bits whose name, or whose register's name, is opaque.
    pub opaque: u32,
    /// Reset bits established by the inherited model properties.
    pub reset_value: u32,
    pub reset_known: u32,
}

/// Word-granular view of the declared registers.
pub fn declared_words(registers: &[DeclaredRegister]) -> BTreeMap<u32, DeclaredWord> {
    let mut words: BTreeMap<u32, DeclaredWord> = BTreeMap::new();
    let mut reset_conflicts = BTreeMap::<u32, u32>::new();
    let opaque = |name: &str| name.ends_with(oer_registers_model::OPAQUE_SUFFIX);
    for register in registers {
        let Ok(address) = u32::try_from(register.address) else {
            continue;
        };
        let lane = (address % WORD_BYTES) * 8;
        let base = address - address % WORD_BYTES;
        let span = lane + register.width_bits;
        for word in 0..span.div_ceil(WORD_BITS) {
            words
                .entry(base + word * WORD_BYTES)
                .or_default()
                .registers
                .insert(register.name.clone());
        }
        if let Some(reset) = register.reset_value {
            let known = register.reset_mask.unwrap_or(u64::MAX);
            for bit in 0..register.width_bits.min(64) {
                if known & (1u64 << bit) == 0 {
                    continue;
                }
                let position = lane + bit;
                let address = base + position / WORD_BITS * WORD_BYTES;
                let word = words.entry(address).or_default();
                let mask = 1 << (position % WORD_BITS);
                if word.reset_known & mask != 0
                    && (word.reset_value & mask != 0) != (reset & (1u64 << bit) != 0)
                {
                    *reset_conflicts.entry(address).or_default() |= mask;
                }
                word.reset_known |= mask;
                if reset & (1u64 << bit) != 0 {
                    word.reset_value |= mask;
                }
            }
        }
        for field in &register.fields {
            for bit in lane + field.offset..lane + field.offset + field.width {
                let word = words
                    .entry(base + bit / WORD_BITS * WORD_BYTES)
                    .or_default();
                word.fields |= 1 << (bit % WORD_BITS);
                if opaque(&register.name) || opaque(&field.name) {
                    word.opaque |= 1 << (bit % WORD_BITS);
                }
            }
        }
    }
    for (address, conflicts) in reset_conflicts {
        if let Some(word) = words.get_mut(&address) {
            word.reset_known &= !conflicts;
        }
    }
    words
}

/// A vendor function: its artifact id and symbol.
pub type Function = (String, String);

/// Where one access goes: a constant address, or Blobray's progression
/// `base + stride * index` of an address computed from one run-time index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Address(u32),
    Indexed(Indexed),
}

/// `base + stride * index` modulo 2^32, the index below `count` when Blobray
/// proves a bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Indexed {
    pub base: u32,
    pub stride: i32,
    pub count: Option<u32>,
}

/// Index values of a bounded progression enumerated word by word. A bound
/// from a mask (`& 7`) is this small; one from a type (a byte-wide queue
/// number reaches 256) is not, and enumerating it would list words far beyond
/// any register the vendor reaches, so a longer bound is matched against the
/// declared arrays like an unbounded index.
const MAX_ENUMERATED_INDICES: u32 = 64;

impl Indexed {
    /// The addresses the progression touches: each index of a bound up to
    /// [`MAX_ENUMERATED_INDICES`]; otherwise, inside the bound when there is
    /// one, the elements of each declared array whose stride is the
    /// progression's and that has its base as an element, and the run of
    /// declared words the progression meets from its base on until the first
    /// word no register declares (per-queue registers declared one by one).
    fn addresses(&self, arrays: &[Vec<u32>], declared: &BTreeMap<u32, DeclaredWord>) -> Vec<u32> {
        let stride = i64::from(self.stride);
        let at = |index: i64| (i64::from(self.base) + stride * index) as u32;
        if let Some(count) = self.count.filter(|count| *count <= MAX_ENUMERATED_INDICES) {
            return (0..i64::from(count)).map(at).collect();
        }
        let inside = |index: i64| {
            self.count
                .is_none_or(|count| (0..i64::from(count)).contains(&index))
        };
        let mut addresses: Vec<u32> = arrays
            .iter()
            .filter(|array| {
                i64::from(array[1] - array[0]) == stride.abs() && array.contains(&self.base)
            })
            .flatten()
            .copied()
            .filter(|element| inside((i64::from(*element) - i64::from(self.base)) / stride))
            .collect();
        let is_declared = |address: u32| declared.contains_key(&(address - address % WORD_BYTES));
        // An unbounded index may be negative: the run extends both ways.
        let directions: &[i64] = if self.count.is_none() { &[1, -1] } else { &[1] };
        if is_declared(self.base) {
            addresses.push(self.base);
            for direction in directions {
                let mut index = *direction;
                while inside(index) && index.abs() <= i64::from(MAX_RUN) && is_declared(at(index)) {
                    addresses.push(at(index));
                    index += direction;
                }
            }
        }
        addresses.sort_unstable();
        addresses.dedup();
        addresses
    }
}

/// Elements of a declared run followed from a progression's base, each way.
const MAX_RUN: u32 = 256;

/// The register arrays of the model: each array's element addresses in
/// ascending order, for every template with two or more elements at one
/// stride.
pub fn arrays(registers: &[DeclaredRegister]) -> Vec<Vec<u32>> {
    let mut templates: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    for register in registers {
        if let Ok(address) = u32::try_from(register.address) {
            templates
                .entry(&register.template)
                .or_default()
                .push(address);
        }
    }
    templates
        .into_values()
        .filter_map(|mut elements| {
            elements.sort_unstable();
            elements.dedup();
            let stride = elements.get(1)?.checked_sub(elements[0])?;
            elements
                .windows(2)
                .all(|pair| pair[1] - pair[0] == stride)
                .then_some(elements)
        })
        .collect()
}

/// One owned MMIO range of the publication.
#[derive(Clone, Debug)]
pub struct Region {
    pub name: String,
    pub start: u32,
    pub end: u32,
}

fn region_of(regions: &[Region], address: u32) -> Option<&str> {
    regions
        .iter()
        .find(|r| r.start <= address && address < r.end)
        .map(|r| r.name.as_str())
}

/// The chip's register model and owned MMIO ranges, through its
/// publication manifest `chips/<chip>/registers/publication/registers.toml`.
pub fn model(root: &Path, chip: &str) -> Result<(Vec<DeclaredRegister>, Vec<Region>)> {
    let manifest = root.join(format!("chips/{chip}/registers/publication/registers.toml"));
    let publication = crate::ChipSources::load(&manifest)?;
    let regions = publication
        .owned_mmio()?
        .into_iter()
        .map(|(name, start, end)| -> Result<Region> {
            Ok(Region {
                name,
                start: u32::try_from(start).map_err(|_| "owned range above 4 GiB")?,
                end: u32::try_from(end).map_err(|_| "owned range above 4 GiB")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let model = oer_registers_model::RegisterModel::load(&publication.model)?;
    let registers = model
        .register_geometry()?
        .into_iter()
        .map(DeclaredRegister::from)
        .collect();
    Ok((registers, regions))
}

impl From<oer_registers_model::RegisterGeometry> for DeclaredRegister {
    fn from(r: oer_registers_model::RegisterGeometry) -> Self {
        Self {
            address: r.address,
            width_bits: r.width.unwrap_or(WORD_BITS),
            name: r.name,
            template: r.template,
            reset_value: r.declaration.properties.reset_value,
            reset_mask: r.declaration.properties.reset_mask,
            fields: r
                .fields
                .into_iter()
                .map(|f| DeclaredField {
                    name: f.name,
                    offset: f.offset,
                    width: f.width,
                })
                .collect(),
        }
    }
}
