//! The argument registers at every call site of named functions.
//!
//! Analog PHY-I2C, PBus and RFPLL registers are written through calls such as
//! `phy_i2c_writeReg(block, host, register, value)`, which no memory access
//! records. The analysis keeps the register state before each opaque
//! transfer (`CallInputs`); this view reads `a0`..`a7` there.
//!
//! The values are may-values of one call site, without interprocedural
//! expansion: a value does not prove that the path reaching the call runs.
use crate::references::{self, Kind, Target, Targets};
use oer_analysis_blobray_domain::{ArgumentValue, LibraryFunction};
use oer_analysis_riscv_model::{
    AbstractValue, Expression, FunctionIndex, Result, RunControl, ValueAlternative,
};
use serde::{Deserialize, Serialize};

/// The integer argument registers `a0`..`a7`, `x10`..`x17`.
const ARGUMENTS: std::ops::Range<usize> = 10..18;

/// One call or tail call of a named function.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallSite {
    /// The transfer instruction's offset, or its image address.
    pub offset: u64,
    #[serde(with = "oer_analysis_riscv_model::symbol_name")]
    pub target: Vec<u8>,
    pub kind: Kind,
    pub destination: Option<Target>,
    pub alternatives: Option<usize>,
    /// `a0`..`a7` before the transfer; empty when the analysis kept no
    /// register state at this site.
    pub arguments: Vec<ArgumentValue>,
}

/// Arguments at the same relocation, exact-transfer and may-transfer sites
/// selected by `callers`. Input state always belongs to this transfer.
pub fn call_sites(
    facts: &FunctionIndex<'_, '_>,
    names: &[String],
    targets: &Targets<'_>,
    c: &mut dyn RunControl,
) -> Result<Vec<CallSite>> {
    references::call_sites(facts.records(), names, targets, c)?
        .into_iter()
        .filter(|site| {
            site.kind != Kind::Relocation
                || matches!(
                    site.relocation_kind,
                    Some(
                        oer_analysis_riscv_model::ReferenceKind::Call
                            | oer_analysis_riscv_model::ReferenceKind::Branch
                    )
                )
        })
        .map(|site| {
            Ok(CallSite {
                offset: site.site,
                target: site.target,
                kind: site.kind,
                destination: site.destination,
                alternatives: site.alternatives,
                arguments: arguments(facts, site.site, c)?,
            })
        })
        .collect()
}

pub(crate) fn arguments(
    facts: &FunctionIndex<'_, '_>,
    offset: u64,
    c: &mut dyn RunControl,
) -> Result<Vec<ArgumentValue>> {
    let Some(input) = facts.call_inputs(offset, c)? else {
        return Ok(Vec::new());
    };
    input
        .registers
        .get(ARGUMENTS)
        .unwrap_or_default()
        .iter()
        .map(|value| argument(value, facts, c))
        .collect()
}

fn argument(
    value: &AbstractValue,
    facts: &FunctionIndex<'_, '_>,
    c: &mut dyn RunControl,
) -> Result<ArgumentValue> {
    c.checkpoint(1)?;
    Ok(match value {
        AbstractValue::Unknown => ArgumentValue::Unknown,
        AbstractValue::Expression { id } => match facts.expression(*id, c)? {
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
                .map(|value| leaf(value, facts, c))
                .collect::<Result<_>>()?,
        },
        _ => match ValueAlternative::from_value(value) {
            Some(value) => leaf(&value, facts, c)?,
            None => ArgumentValue::Unknown,
        },
    })
}

fn leaf(
    value: &ValueAlternative,
    facts: &FunctionIndex<'_, '_>,
    c: &mut dyn RunControl,
) -> Result<ArgumentValue> {
    c.checkpoint(1)?;
    Ok(match value {
        ValueAlternative::Constant { value } => ArgumentValue::Constant { value: *value },
        ValueAlternative::ImageAddress { address } => {
            ArgumentValue::ImageAddress { address: *address }
        }
        ValueAlternative::Section { section, offset } => ArgumentValue::Section {
            section: *section,
            offset: *offset,
        },
        ValueAlternative::Symbol { symbol, addend } => ArgumentValue::Symbol {
            name: facts
                .reference(symbol, c)?
                .map(|target| target.name.clone())
                .unwrap_or_else(|| format!("symbol{}", symbol.index).into_bytes()),
            addend: *addend,
        },
        ValueAlternative::EntryStack { offset } => ArgumentValue::EntryStack { offset: *offset },
    })
}

#[cfg(test)]
mod tests;

/// One function with its call sites of the named functions.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallingFunction {
    pub function: LibraryFunction,
    pub sites: Vec<crate::call_arguments::CallSite>,
}
