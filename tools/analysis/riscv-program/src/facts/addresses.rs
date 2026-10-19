//! Structural address uses of one ELF, with no callback-resolution policy.
use super::invalid;
use oer_analysis_elf::{Elf, Function, Relocation, Symbol, Target, rv32};
use oer_analysis_riscv_model::*;

/// A relocation's numerical ELF value. An undefined symbol's value is kept
/// for consumers whose existing policy reads it, without calling it resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetValue {
    Resolved(u64),
    External(u64),
    Unsupported,
}

impl TargetValue {
    /// The ELF arithmetic value, including an undefined symbol's placeholder.
    /// This is an explicit projection, not proof of an external address.
    pub fn elf_value(self) -> Result<u64> {
        match self {
            Self::Resolved(value) | Self::External(value) => Ok(value),
            Self::Unsupported => Err(invalid("relocation without a target")),
        }
    }

    /// The numerical RV32 address with the instruction low bit cleared.
    pub fn canonical(self) -> Option<u32> {
        let value = match self {
            Self::Resolved(value) | Self::External(value) => value,
            Self::Unsupported => return None,
        };
        u32::try_from(value).ok().map(|value| value & !1)
    }
}

/// One relationship to a function in this ELF. Offset zero is an exact
/// entry; odd and interior addresses retain their original byte offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionAddress {
    pub entry: u32,
    pub offset: u32,
}

/// All containing functions are counted. `nearest` preserves the nearest
/// preceding entry even when overlapping extents make ownership ambiguous.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FunctionRelation {
    pub nearest: Option<FunctionAddress>,
    pub matches: usize,
}

#[derive(Clone, Debug)]
pub struct AddressUse<'a> {
    /// Original site, section, type, target identity, addend and REL/RELA form.
    pub relocation: Relocation,
    pub role: rv32::Role,
    /// The original symbol, including its alias identity, kind, definition
    /// state and section. Section and symbol indexes belong to this ELF.
    pub symbol: Option<Symbol<'a>>,
    pub value: TargetValue,
    /// Relation of the numerical value to this ELF's functions.
    pub function: FunctionRelation,
    /// Relation of the original symbol before adding the addend. Keeping
    /// both relations exposes arithmetic that crosses function boundaries.
    pub symbol_function: FunctionRelation,
}

impl<'a> AddressUse<'a> {
    pub(super) fn new(
        file: &Elf<'a>,
        relocation: Relocation,
        lookup: &FunctionLookup<'_, '_>,
        control: &mut dyn RunControl,
    ) -> Result<Self> {
        let symbol = match relocation.target {
            Target::Symbol(index) => Some(
                file.symbol_at(index)
                    .map_err(|error| invalid(error.to_string()))?,
            ),
            _ => None,
        };
        let value = if relocation.target == Target::Other {
            TargetValue::Unsupported
        } else {
            let value = file
                .target_address(&relocation)
                .map_err(|error| invalid(error.to_string()))?;
            if symbol.as_ref().is_some_and(|symbol| !symbol.defined) {
                TargetValue::External(value)
            } else {
                TargetValue::Resolved(value)
            }
        };
        let function = match value {
            TargetValue::Resolved(value) => lookup.relate(value, control)?,
            // A placeholder cannot establish a function relationship.
            _ => FunctionRelation::default(),
        };
        let symbol_function = match &symbol {
            Some(symbol) if symbol.defined => lookup.relate(symbol.address, control)?,
            _ => FunctionRelation::default(),
        };
        Ok(Self {
            role: rv32::kind(relocation.r_type).role,
            relocation,
            symbol,
            value,
            function,
            symbol_function,
        })
    }
}

/// Prefix maxima bound overlap searches without a full function scan per use.
pub(super) struct FunctionLookup<'f, 'm> {
    functions: &'f [Function],
    ends: AdmittedVec<'m, u64>,
}

impl<'f, 'm> FunctionLookup<'f, 'm> {
    pub(super) fn new(
        functions: &'f [Function],
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut ends = AdmittedVec::new(memory);
        let mut end = 0;
        for function in functions {
            control.checkpoint(1)?;
            end = end.max(u64::from(function.address) + u64::from(function.size));
            ends.push(end, control.position())?;
        }
        Ok(Self { functions, ends })
    }

    fn relate(&self, address: u64, control: &mut dyn RunControl) -> Result<FunctionRelation> {
        control.checkpoint(1)?;
        let Ok(raw) = u32::try_from(address) else {
            return Ok(FunctionRelation::default());
        };
        let canonical = raw & !1;
        let at = self.functions.partition_point(|f| f.address <= canonical);
        let mut relation = FunctionRelation::default();
        for (function, &end) in self.functions.iter().zip(self.ends.iter()).take(at).rev() {
            control.checkpoint(1)?;
            if end <= u64::from(canonical) {
                break;
            }
            if u64::from(canonical) < u64::from(function.address) + u64::from(function.size) {
                relation.matches += 1;
                relation.nearest.get_or_insert(FunctionAddress {
                    entry: function.address,
                    offset: raw - function.address,
                });
            }
        }
        Ok(relation)
    }
}

#[cfg(test)]
mod tests;
