//! Compare vendor bit uses with declared register fields.
//!
//! `cargo registers check inventory` runs Blobray on every pinned vendor
//! binary in the chip root's MMIO instance windows, using `riscv-integer`
//! and call summaries. The typed consumer in [`uses`] rejects uses of
//! absent bits and reports unknown bits and coverage limits separately.
//! This module supplies expanded geometry, inherited resets, and the same
//! bounded/indexed candidate addresses used by the MMIO inventory.
use crate::Result;
use oer_analysis_blobray_domain::IndexedAddress;
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
    pub reserved: Option<DeclaredReserved>,
    pub fields: Vec<DeclaredField>,
}

/// The bits a source marks reserved in a register.
#[derive(Clone, Debug)]
pub struct DeclaredReserved {
    pub mask: u128,
    pub sources: Vec<String>,
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
    /// Each cited source's reserved mask in this hardware word.
    pub reserved: BTreeMap<String, u32>,
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
        if let Some(reserved) = &register.reserved {
            for bit in 0..register.width_bits.min(128) {
                if reserved.mask & (1u128 << bit) == 0 {
                    continue;
                }
                let position = lane + bit;
                let word = words
                    .entry(base + position / WORD_BITS * WORD_BYTES)
                    .or_default();
                for source in &reserved.sources {
                    *word.reserved.entry(source.clone()).or_default() |=
                        1 << (position % WORD_BITS);
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
    Indexed(IndexedAddress),
}

/// Index values of a bounded progression enumerated word by word. A bound
/// from a mask (`& 7`) is this small; one from a type (a byte-wide queue
/// number reaches 256) is not, and enumerating it would list words far beyond
/// any register the vendor reaches, so a longer bound is matched against the
/// declared arrays like an unbounded index.
const MAX_ENUMERATED_INDICES: u32 = 64;

/// The addresses the progression touches: each index of a bound up to
/// [`MAX_ENUMERATED_INDICES`]; otherwise, inside the bound when there is
/// one, the elements of each declared array whose stride is the
/// progression's and that has its base as an element, and the run of
/// declared words the progression meets from its base on until the first
/// word no register declares (per-queue registers declared one by one).
fn addresses(
    indexed: IndexedAddress,
    arrays: &[Vec<u32>],
    declared: &BTreeMap<u32, DeclaredWord>,
) -> Vec<u32> {
    let stride = i64::from(indexed.stride);
    let at = |index: i64| (i64::from(indexed.base) + stride * index) as u32;
    if let Some(count) = indexed
        .count
        .filter(|count| *count <= MAX_ENUMERATED_INDICES)
    {
        return (0..i64::from(count)).map(at).collect();
    }
    let inside = |index: i64| {
        indexed
            .count
            .is_none_or(|count| (0..i64::from(count)).contains(&index))
    };
    let mut addresses: Vec<u32> = arrays
        .iter()
        .filter(|array| {
            i64::from(array[1] - array[0]) == stride.abs() && array.contains(&indexed.base)
        })
        .flatten()
        .copied()
        .filter(|element| inside((i64::from(*element) - i64::from(indexed.base)) / stride))
        .collect();
    let is_declared = |address: u32| declared.contains_key(&(address - address % WORD_BYTES));
    // An unbounded index may be negative: the run extends both ways.
    let directions: &[i64] = if indexed.count.is_none() {
        &[1, -1]
    } else {
        &[1]
    };
    if is_declared(indexed.base) {
        addresses.push(indexed.base);
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

/// The chip's register model and its MMIO instance windows: every
/// register element of the chip root (`chips/<chip>/registers/chip.toml`)
/// and every instance's window.
pub fn model(root: &Path, chip: &str) -> Result<(Vec<DeclaredRegister>, Vec<Region>)> {
    let chip = oer_registers_model::Chip::load(root, chip)?;
    let model = &chip.model;
    let regions = model
        .instances()
        .iter()
        .map(|instance| -> Result<Region> {
            let start = u32::try_from(instance.base).map_err(|_| "instance above 4 GiB")?;
            let end = instance
                .base
                .checked_add(instance.extent.size)
                .and_then(|end| u32::try_from(end).ok())
                .ok_or("instance window above 4 GiB")?;
            Ok(Region {
                name: instance.name.clone(),
                start,
                end,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut registers = Vec::new();
    for register in model.registers() {
        let oer_registers_model::chip_model::Location::Mmio { address } = register.location else {
            continue;
        };
        let template = model
            .instances()
            .iter()
            .find(|instance| instance.name == register.instance)
            .and_then(|instance| model.types().get(&instance.block_type))
            .and_then(|block| block.templates.get(&register.key));
        let reserved = (register.reserved != 0).then(|| DeclaredReserved {
            mask: u128::from(register.reserved),
            sources: template
                .and_then(|template| template.reserved.as_ref())
                .map(|reserved| reserved.sources.clone())
                .unwrap_or_default(),
        });
        registers.push(DeclaredRegister {
            address,
            width_bits: register.width,
            name: register.identity.clone(),
            template: format!("{}.{}", register.instance, register.key),
            reset_value: register.reset.map(|reset| u64::from(reset.value)),
            reset_mask: register.reset.and_then(|reset| reset.mask).map(u64::from),
            reserved,
            fields: register
                .fields
                .iter()
                .map(|field| DeclaredField {
                    name: field.name.clone(),
                    offset: field.low,
                    width: field.width,
                })
                .collect(),
        });
    }
    Ok((registers, regions))
}
