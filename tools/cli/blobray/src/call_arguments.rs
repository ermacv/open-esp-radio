//! The argument registers at every call site of named functions.
//!
//! Analog PHY-I2C, PBus and RFPLL registers are written through calls such as
//! `phy_i2c_writeReg(block, host, register, value)`, which no memory access
//! records. The analysis keeps the register state before each opaque
//! transfer (`CallInputs`); this view reads `a0`..`a7` there.
//!
//! The values are may-values of one call site, without interprocedural
//! expansion: a value does not prove that the path reaching the call runs.
use crate::listing::{ImageSymbols, signed_hex};
use oer_riscv_model::{
    AbstractValue, Expression, FunctionRecord, ReferenceKind, SymbolId, ValueAlternative,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The integer argument registers `a0`..`a7`, `x10`..`x17`.
const ARGUMENTS: std::ops::Range<usize> = 10..18;

/// One call or tail call of a named function.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallSite {
    /// The transfer instruction's offset, or its image address.
    pub offset: u64,
    #[serde(with = "oer_riscv_model::symbol_name")]
    pub target: Vec<u8>,
    /// `a0`..`a7` before the transfer; empty when the analysis kept no
    /// register state at this site.
    pub arguments: Vec<ArgumentValue>,
}

/// One argument register's value at a call site.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ArgumentValue {
    Constant {
        value: u32,
    },
    ImageAddress {
        address: u32,
    },
    Symbol {
        #[serde(with = "oer_riscv_model::symbol_name")]
        name: Vec<u8>,
        addend: i64,
    },
    Section {
        section: u32,
        offset: i64,
    },
    EntryStack {
        offset: i64,
    },
    /// One of a finite set; no element is selected by the analysis.
    Alternatives {
        values: Vec<ArgumentValue>,
    },
    /// The calling function's own argument `a<index>` at its entry.
    EntryArgument {
        index: u8,
    },
    /// Another register's value at the calling function's entry.
    EntryRegister {
        register: u8,
    },
    Unknown,
}

impl ArgumentValue {
    /// `0x6b`, `arg0`, `phy_param+0xef`, `one-of{0x2 | 0x40}`, `?`.
    pub fn human(&self) -> String {
        match self {
            Self::Constant { value } => format!("{value:#x}"),
            Self::ImageAddress { address } => format!("{address:#x}"),
            Self::Symbol { name, addend } => {
                format!("{}{}", String::from_utf8_lossy(name), signed_hex(*addend))
            }
            Self::Section { section, offset } => format!("section{section}{}", signed_hex(*offset)),
            Self::EntryStack { offset } => format!("sp{}", signed_hex(*offset)),
            Self::Alternatives { values } => format!(
                "one-of{{{}}}",
                values
                    .iter()
                    .map(Self::human)
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            Self::EntryArgument { index } => format!("arg{index}"),
            Self::EntryRegister { register } => {
                format!("entry {}", oer_riscv_lift::register_name(*register))
            }
            Self::Unknown => String::from("?"),
        }
    }
}

/// Every call site among `records` whose target `targets` names.
///
/// In a relocatable object a call is a `Call` relocation to the name, whose
/// register state is at the `jalr` four bytes after its `auipc`, or a
/// `Branch` (JAL or branch) relocation, whose state is at its own offset. Without relocations, `image` supplies the addresses of the named
/// functions and a transfer to one of them is a call site.
pub fn call_sites(
    records: &[FunctionRecord],
    targets: &[String],
    image: Option<&ImageSymbols>,
) -> Vec<CallSite> {
    let mut names: BTreeMap<&SymbolId, &[u8]> = BTreeMap::new();
    let mut expressions: BTreeMap<u32, &Expression> = BTreeMap::new();
    let mut inputs: BTreeMap<u64, &[AbstractValue]> = BTreeMap::new();
    for record in records {
        match record {
            FunctionRecord::Reference { target, .. } => {
                names.insert(&target.symbol, &target.name);
            }
            FunctionRecord::Expression { id, expression, .. } => {
                expressions.insert(*id, expression);
            }
            FunctionRecord::CallInputs { offset, registers } => {
                inputs.insert(*offset, registers);
            }
            _ => {}
        }
    }
    let wanted = |name: &[u8]| targets.iter().any(|target| target.as_bytes() == name);
    let arguments = |registers: Option<&&[AbstractValue]>| {
        registers
            .map(|registers| {
                registers
                    .get(ARGUMENTS)
                    .unwrap_or_default()
                    .iter()
                    .map(|value| argument(value, &names, &expressions))
                    .collect()
            })
            .unwrap_or_default()
    };
    let image_targets: BTreeMap<u64, &str> = image
        .map(|image| image.starts_named(targets))
        .unwrap_or_default();
    let mut sites = Vec::new();
    for record in records {
        match record {
            FunctionRecord::Reference {
                raw,
                reference_kind: kind @ (ReferenceKind::Call | ReferenceKind::Branch),
                target,
                ..
            } if wanted(&target.name) => {
                // A CALL pair relocates its `auipc` and transfers at the
                // `jalr` four bytes on; a JAL or branch relocation is its
                // own transfer. Never borrow another transfer's state.
                let site = match kind {
                    ReferenceKind::Call => raw.offset + 4,
                    _ => raw.offset,
                };
                sites.push(CallSite {
                    offset: site,
                    target: target.name.clone(),
                    arguments: arguments(inputs.get(&site)),
                });
            }
            FunctionRecord::Transfer {
                offset,
                target: AbstractValue::ImageAddress { address },
                ..
            } => {
                if let Some(name) = image_targets.get(&u64::from(*address)) {
                    sites.push(CallSite {
                        offset: *offset,
                        target: name.as_bytes().to_vec(),
                        arguments: arguments(inputs.get(offset)),
                    });
                }
            }
            _ => {}
        }
    }
    sites.sort_by_key(|site| site.offset);
    sites.dedup();
    sites
}

fn argument(
    value: &AbstractValue,
    names: &BTreeMap<&SymbolId, &[u8]>,
    expressions: &BTreeMap<u32, &Expression>,
) -> ArgumentValue {
    match value {
        AbstractValue::Unknown => ArgumentValue::Unknown,
        AbstractValue::Expression { id } => match expressions.get(id) {
            Some(Expression::EntryRegister { register })
                if ARGUMENTS.contains(&usize::from(*register)) =>
            {
                ArgumentValue::EntryArgument {
                    index: register - 10,
                }
            }
            Some(Expression::EntryRegister { register }) => ArgumentValue::EntryRegister {
                register: *register,
            },
            _ => ArgumentValue::Unknown,
        },
        AbstractValue::Alternatives { values } => ArgumentValue::Alternatives {
            values: values
                .values()
                .iter()
                .map(|value| leaf(value, names))
                .collect(),
        },
        _ => ValueAlternative::from_value(value)
            .map_or(ArgumentValue::Unknown, |value| leaf(&value, names)),
    }
}

fn leaf(value: &ValueAlternative, names: &BTreeMap<&SymbolId, &[u8]>) -> ArgumentValue {
    match value {
        ValueAlternative::Constant { value } => ArgumentValue::Constant { value: *value },
        ValueAlternative::ImageAddress { address } => {
            ArgumentValue::ImageAddress { address: *address }
        }
        ValueAlternative::Section { section, offset } => ArgumentValue::Section {
            section: *section,
            offset: *offset,
        },
        ValueAlternative::Symbol { symbol, addend } => ArgumentValue::Symbol {
            name: names
                .get(symbol)
                .map(|name| name.to_vec())
                .unwrap_or_else(|| format!("symbol{}", symbol.index).into_bytes()),
            addend: *addend,
        },
        ValueAlternative::EntryStack { offset } => ArgumentValue::EntryStack { offset: *offset },
    }
}

#[cfg(test)]
mod tests;
