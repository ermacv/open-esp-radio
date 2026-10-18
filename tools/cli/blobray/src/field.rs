//! Memory accesses as fields of a base: the address of an access folded into
//! a root and a path of displacements through loaded pointers.
//!
//! An access `sb a4, 148(a5)` after `a5 = *(g_ic + 16)` has the root symbol
//! `g_ic` and the path `[16, 148]`: the root plus 16 is loaded, and the
//! access lands 148 bytes after the loaded pointer. Additions of constants
//! fold into the current displacement; every load starts the next one.

use std::collections::HashMap;

use oer_riscv_model::{AbstractValue, Expression, FunctionRecord, IntegerOp, MemoryKind, SymbolId};
use serde::{Deserialize, Serialize};

/// What an access's address chain starts from.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FieldRoot {
    /// A symbol; `name` when the function's references name it.
    Symbol {
        symbol: SymbolId,
        name: Option<String>,
    },
    /// A register on function entry (an argument or a callee-saved value).
    EntryRegister { register: u8 },
    /// The stack pointer on function entry.
    EntryStack,
    /// A register a call returned.
    CallResult { callsite: u64, register: u8 },
    /// A section of the function's object.
    Section { section: u32 },
    /// A base the analysis could not resolve; the displacements after it are
    /// still exact, so the access is a candidate, not a proof.
    Unknown,
}

/// One memory access of a function, as a field of its root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldAccess {
    /// The access instruction's offset in the function.
    pub offset: u64,
    pub access: MemoryKind,
    pub width: u8,
    pub root: FieldRoot,
    /// Displacements from the root: each but the last is loaded as a
    /// pointer, and the last is the accessed field.
    pub path: Vec<i64>,
}

/// The memory accesses of one function's `records` whose address folds to
/// a root and a path ending at `offset`, and, when given, of `width` bytes.
/// An unresolved base is the [`FieldRoot::Unknown`] root; absolute addresses
/// are not fields.
pub fn field_accesses(
    records: &[FunctionRecord],
    offset: i64,
    width: Option<u8>,
) -> Vec<FieldAccess> {
    let expressions: HashMap<u32, &Expression> = records
        .iter()
        .filter_map(|record| match record {
            FunctionRecord::Expression { id, expression, .. } => Some((*id, expression)),
            _ => None,
        })
        .collect();
    let names: HashMap<SymbolId, String> = records
        .iter()
        .filter_map(|record| match record {
            FunctionRecord::Reference { target, .. } => Some((
                target.symbol.clone(),
                String::from_utf8_lossy(&target.name).into_owned(),
            )),
            _ => None,
        })
        .collect();
    records
        .iter()
        .filter_map(|record| {
            let FunctionRecord::MemoryAccess {
                offset: at,
                access,
                width: accessed,
                address,
                ..
            } = record
            else {
                return None;
            };
            if width.is_some_and(|width| width != *accessed) {
                return None;
            }
            let (root, mut path, last) = fold(address, &expressions, 0)?;
            if last != offset {
                return None;
            }
            path.push(last);
            let root = match root {
                FieldRoot::Symbol { symbol, .. } => FieldRoot::Symbol {
                    name: names.get(&symbol).cloned(),
                    symbol,
                },
                other => other,
            };
            Some(FieldAccess {
                offset: *at,
                access: *access,
                width: *accessed,
                root,
                path,
            })
        })
        .collect()
}

/// The memory accesses of `records` whose address is not known at all:
/// neither a root nor a displacement, so no field selection can see them.
pub fn unknown_addresses(records: &[FunctionRecord]) -> u64 {
    records
        .iter()
        .filter(|record| {
            matches!(
                record,
                FunctionRecord::MemoryAccess {
                    address: AbstractValue::Unknown,
                    ..
                }
            )
        })
        .count() as u64
}

/// The deepest folds a chain may take; the expressions form a DAG of earlier
/// records, so this only bounds pathological inputs.
const MAX_DEPTH: usize = 64;

/// `value` as a root, the displacements loaded before the last, and the
/// last displacement.
fn fold(
    value: &AbstractValue,
    expressions: &HashMap<u32, &Expression>,
    depth: usize,
) -> Option<(FieldRoot, Vec<i64>, i64)> {
    if depth > MAX_DEPTH {
        return None;
    }
    match value {
        AbstractValue::Symbol { symbol, addend } => Some((
            FieldRoot::Symbol {
                symbol: symbol.clone(),
                name: None,
            },
            Vec::new(),
            *addend,
        )),
        AbstractValue::EntryStack { offset } => Some((FieldRoot::EntryStack, Vec::new(), *offset)),
        AbstractValue::Section { section, offset } => Some((
            FieldRoot::Section { section: *section },
            Vec::new(),
            *offset,
        )),
        AbstractValue::Expression { id } if expressions.contains_key(id) => match expressions[id] {
            Expression::EntryRegister { register } => Some((
                FieldRoot::EntryRegister {
                    register: *register,
                },
                Vec::new(),
                0,
            )),
            Expression::CallResult { callsite, register } => Some((
                FieldRoot::CallResult {
                    callsite: *callsite,
                    register: *register,
                },
                Vec::new(),
                0,
            )),
            Expression::Integer {
                op: IntegerOp::Add,
                left,
                right,
            } => match (left, right) {
                (base, AbstractValue::Constant { value })
                | (AbstractValue::Constant { value }, base) => {
                    let (root, path, last) = fold(base, expressions, depth + 1)?;
                    Some((root, path, last + i64::from(*value as i32)))
                }
                _ => Some((FieldRoot::Unknown, Vec::new(), 0)),
            },
            Expression::Load { address, .. } => {
                let (root, mut path, last) = fold(address, expressions, depth + 1)?;
                path.push(last);
                Some((root, path, 0))
            }
            Expression::Integer { .. } => Some((FieldRoot::Unknown, Vec::new(), 0)),
        },
        // An expression the records do not define, a base with several
        // possible values or none known: the displacements after it stay
        // exact.
        AbstractValue::Expression { .. }
        | AbstractValue::Alternatives { .. }
        | AbstractValue::Unknown => Some((FieldRoot::Unknown, Vec::new(), 0)),
        // A known absolute address is no field of anything.
        AbstractValue::Constant { .. } | AbstractValue::ImageAddress { .. } => None,
    }
}

#[cfg(test)]
mod tests;
