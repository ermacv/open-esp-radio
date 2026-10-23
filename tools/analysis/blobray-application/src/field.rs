//! Memory accesses as fields of a base: the address of an access folded into
//! a root and a path of displacements through loaded pointers.
//!
//! An access `sb a4, 148(a5)` after `a5 = *(g_ic + 16)` has the root symbol
//! `g_ic` and the path `[16, 148]`: the root plus 16 is loaded, and the
//! access lands 148 bytes after the loaded pointer. Additions of constants
//! fold into the current displacement; every load starts the next one.

use oer_analysis_blobray_domain::LibraryFunction;
use oer_analysis_riscv_model::{
    AbstractValue, Expression, FunctionIndex, FunctionRecord, IntegerOp, MemoryKind, Result,
    RunControl, SymbolId,
};
use serde::{Deserialize, Serialize};

/// What an access's address chain starts from.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FieldRoot {
    /// A pointer loaded from this absolute address; its first dereference is
    /// represented by the root itself, followed by the path's displacements.
    Address { address: u32 },
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

/// The memory accesses of one function's indexed records whose address folds to
/// a root and a path ending at `offset`, and, when given, of `width` bytes.
/// An unresolved base is the [`FieldRoot::Unknown`] root. With `absolute_roots`,
/// pointers loaded from absolute addresses can root a field chain; a direct
/// access to an absolute address is never a field.
pub fn field_accesses(
    facts: &FunctionIndex<'_, '_>,
    offset: i64,
    width: Option<u8>,
    absolute_roots: bool,
    c: &mut dyn RunControl,
) -> Result<Vec<FieldAccess>> {
    let mut accesses = Vec::new();
    facts.accesses(c, &mut |access, c| {
        if width.is_some_and(|width| width != access.width) {
            return Ok(());
        }
        let Some((root, mut path, last)) = fold(access.address, facts, absolute_roots, 0, c)?
        else {
            return Ok(());
        };
        if last != offset {
            return Ok(());
        }
        path.push(last);
        let root = match root {
            FieldRoot::Symbol { symbol, .. } => FieldRoot::Symbol {
                name: facts
                    .reference(&symbol, c)?
                    .map(|target| String::from_utf8_lossy(&target.name).into_owned()),
                symbol,
            },
            other => other,
        };
        accesses.push(FieldAccess {
            offset: access.offset,
            access: access.access,
            width: access.width,
            root,
            path,
        });
        Ok(())
    })?;
    Ok(accesses)
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

/// The deepest folds a chain may take, including dictionary inputs with cycles.
const MAX_DEPTH: usize = 64;

/// `value` as a root, the displacements loaded before the last, and the
/// last displacement.
fn fold(
    value: &AbstractValue,
    facts: &FunctionIndex<'_, '_>,
    absolute_roots: bool,
    depth: usize,
    c: &mut dyn RunControl,
) -> Result<Option<(FieldRoot, Vec<i64>, i64)>> {
    c.checkpoint(1)?;
    if depth > MAX_DEPTH {
        return Ok(None);
    }
    Ok(match value {
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
        AbstractValue::Expression { id } => match facts.expression(*id, c)? {
            Some(Expression::EntryRegister { register }) => Some((
                FieldRoot::EntryRegister {
                    register: *register,
                },
                Vec::new(),
                0,
            )),
            Some(Expression::CallResult { callsite, register }) => Some((
                FieldRoot::CallResult {
                    callsite: *callsite,
                    register: *register,
                },
                Vec::new(),
                0,
            )),
            Some(Expression::Integer {
                op: IntegerOp::Add,
                left,
                right,
            }) => match (left, right) {
                (base, AbstractValue::Constant { value })
                | (AbstractValue::Constant { value }, base) => {
                    let Some((root, path, last)) = fold(base, facts, absolute_roots, depth + 1, c)?
                    else {
                        return Ok(None);
                    };
                    Some((root, path, last + i64::from(*value as i32)))
                }
                _ => Some((FieldRoot::Unknown, Vec::new(), 0)),
            },
            Some(Expression::Load { address, .. }) => {
                if absolute_roots
                    && let AbstractValue::Constant { value: address }
                    | AbstractValue::ImageAddress { address } = address
                {
                    return Ok(Some((
                        FieldRoot::Address { address: *address },
                        Vec::new(),
                        0,
                    )));
                }
                let Some((root, mut path, last)) =
                    fold(address, facts, absolute_roots, depth + 1, c)?
                else {
                    return Ok(None);
                };
                path.push(last);
                Some((root, path, 0))
            }
            Some(Expression::Integer { .. } | Expression::CallWord { .. }) | None => {
                Some((FieldRoot::Unknown, Vec::new(), 0))
            }
        },
        // Missing expressions and unresolved bases keep exact displacements.
        AbstractValue::Alternatives { .. } | AbstractValue::Unknown => {
            Some((FieldRoot::Unknown, Vec::new(), 0))
        }
        // A known absolute address is no field of anything.
        AbstractValue::Constant { .. } | AbstractValue::ImageAddress { .. } => None,
    })
}

#[cfg(test)]
mod tests;

/// One function's accesses of the requested field.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldAccessFunction {
    pub function: LibraryFunction,
    pub accesses: Vec<crate::field::FieldAccess>,
}
