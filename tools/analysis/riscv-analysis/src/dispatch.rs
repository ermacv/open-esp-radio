//! Jump-table dispatches the value analysis locates, whose entries the
//! facts of a build prove ([`TableEntries`]).
//!
//! A compiler dispatches a RV32 `switch` as
//! `lui/addi table; slli index, 2; add; lw target; jalr zero, 0(target)`
//! with the table in read-only data. The value analysis records such a jump
//! as a `Dispatch` record: its base, a symbol of an object or an image
//! address, and its index, possibly unknown. Where
//! the index is known, the bounds check whose in-range edge is the only way
//! into the dispatch block, such as `bltu limit, index`, fixes the entry
//! count; without one, only a table the build's facts delimit (an image's
//! labelled table) is proven. A dispatch whose table the facts do not
//! prove, or whose entries leave the function, stays an indirect gap.
use crate::KnownJump;
use crate::tables::{JumpTableSource, ProvenTables};
use oer_analysis_riscv_model::*;

#[cfg(test)]
mod tests;

/// Table lengths beyond this are not taken from a bound.
const MAX_ENTRIES: u32 = 4096;

/// The tables the value records of a function of `section` locate and the
/// build's facts prove.
pub struct RecognizedTables<'e> {
    pub entries: &'e dyn TableEntries,
    pub section: u32,
}

impl JumpTableSource for RecognizedTables<'_> {
    type Table = JumpDispatch;

    fn same(&self, earlier: &JumpDispatch, later: &JumpDispatch) -> bool {
        match (earlier, later) {
            (
                JumpDispatch::Table {
                    site: a,
                    entries: x,
                    ..
                },
                JumpDispatch::Table {
                    site: b,
                    entries: y,
                    ..
                },
            ) => a == b && x == y,
            (
                JumpDispatch::TargetSet {
                    site: a,
                    targets: x,
                },
                JumpDispatch::TargetSet {
                    site: b,
                    targets: y,
                },
            ) => a == b && x == y,
            _ => false,
        }
    }

    fn tables<'m>(
        &self,
        records: &[FunctionRecord],
        extent: CodeRange,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<ProvenTables<'m, JumpDispatch>> {
        jump_tables(records, self.section, extent, self.entries, memory, control)
    }

    fn jump(&self, table: &JumpDispatch) -> KnownJump {
        let mut targets = table.addresses().to_vec();
        targets.sort_unstable();
        targets.dedup();
        KnownJump {
            site: table.site(),
            targets,
        }
    }
}

/// The proofs of the `Dispatch` records among the `records` of one pass over
/// a function of `section` spanning `extent`: one table retains its index
/// order, while multiple possible bases yield only a complete target set.
/// The value analysis records no dispatch for a return. The
/// expression and block indexes this builds are admitted in `memory` and
/// every record it visits is charged to `c`.
pub fn jump_tables<'r, 'm>(
    records: &'r [FunctionRecord],
    section: u32,
    extent: CodeRange,
    facts: &dyn TableEntries,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<ProvenTables<'m, JumpDispatch>> {
    let mut expressions: AdmittedVec<'_, &'r Expression> = AdmittedVec::new(memory);
    let mut blocks: AdmittedVec<'_, (u64, u64)> = AdmittedVec::new(memory);
    let mut dispatches: AdmittedVec<'_, (u64, &'r AbstractValue, &'r AbstractValue, u8, i32)> =
        AdmittedVec::new(memory);
    for record in records {
        c.checkpoint(1)?;
        match record {
            // Expression ids are dense and emitted in order; anything else
            // leaves the index unusable, and then no table is proven.
            FunctionRecord::Expression { id, expression, .. } => {
                if *id as usize != expressions.len() {
                    return Ok(ProvenTables::none(memory));
                }
                expressions.push(expression, c.position())?;
            }
            FunctionRecord::Block { start, end, .. } => {
                blocks.push((*start, *end), c.position())?
            }
            // A table of words.
            FunctionRecord::Dispatch {
                offset,
                base,
                index,
                scale: scale @ 2,
                displacement,
            } => dispatches.push((*offset, base, index, *scale, *displacement), c.position())?,
            _ => {}
        }
    }
    let inside = |target: u64| target >= extent.start && target < extent.start + extent.length;
    // One proof per dispatch at most, and the next pass's jump per proof:
    // both are reserved before the list is allocated at its exact capacity,
    // so it never grows.
    let mut reserved = AdmittedVec::new(memory);
    let per_site = (std::mem::size_of::<JumpDispatch>() + std::mem::size_of::<KnownJump>()) as u64;
    reserved.push(
        memory.reserve(dispatches.len() as u64 * per_site, c.position())?,
        c.position(),
    )?;
    let mut jumps = Vec::new();
    jumps
        .try_reserve_exact(dispatches.len())
        .map_err(|_| Error::new(ErrorCode::ResourceLimited, "jump table allocation refused"))?;
    for &(site, table, index, scale, displacement) in dispatches.iter() {
        let mut block = None;
        for &(start, end) in blocks.iter() {
            c.checkpoint(1)?;
            if start <= site && site < end {
                block = Some(start);
                break;
            }
        }
        let Some(start) = block else {
            continue;
        };
        // An index the pass does not know states no bound: a check against
        // an unknown value is not a check against it. Without a bound, only
        // a table the build's facts delimit, such as an image's labelled
        // jump table, is proven.
        let bound = if *index == AbstractValue::Unknown {
            None
        } else {
            guarding_bound(records, index, start, c)?
        };
        let alternatives = match table {
            AbstractValue::Alternatives { values } => Some(values.values()),
            _ => None,
        };
        let mut targets = Vec::new();
        let mut complete = true;
        let mut case = first_case(index, &expressions);
        let bases = alternatives
            .into_iter()
            .flatten()
            .map(ValueAlternative::as_value)
            .chain(alternatives.is_none().then(|| table.clone()));
        for base in bases {
            c.checkpoint(1)?;
            let Some(effective) = displaced(&base, displacement) else {
                complete = false;
                break;
            };
            let direct = facts.entries(&effective, bound, section, memory, c)?;
            // Query the unshifted base separately: only a label can justify
            // skipping the excluded prefix of the index's domain. Objects
            // and bounds never establish that second interpretation.
            let labelled_base = if displacement == 0 {
                None
            } else {
                facts
                    .entries(&base, None, section, memory, c)?
                    .filter(|words| words.source == TableSource::Labelled)
            };
            let direct_labelled = direct
                .as_ref()
                .is_some_and(|words| words.source == TableSource::Labelled);
            let (words, bias, expected) = match (direct, labelled_base) {
                (Some(_), Some(_)) if direct_labelled => {
                    // Both candidates start labelled tables: ambiguous.
                    complete = false;
                    break;
                }
                (_, Some(words)) => {
                    let Some(bias) = displaced_cases(displacement, scale) else {
                        complete = false;
                        break;
                    };
                    // The label delimits the whole chain, including when
                    // the value pass no longer knows the index expression.
                    (words, bias, None)
                }
                (Some(words), None) => (words, 0, bound),
                (None, None) => {
                    complete = false;
                    break;
                }
            };
            case =
                first_case(index, &expressions).and_then(|case| case.checked_add(i64::from(bias)));
            complete &= match expected {
                Some(entries) => words.entries.len() == entries as usize,
                None => !words.entries.is_empty(),
            };
            // Every possible base must prove its complete table. Admit both
            // the combined entries and the next pass's sorted KnownJump copy.
            let count = targets.len() + words.entries.len();
            let bytes = count as u64 * size_of::<u64>() as u64;
            reserved.push(memory.reserve(bytes * 2, c.position())?, c.position())?;
            targets
                .try_reserve_exact(words.entries.len())
                .map_err(|_| {
                    Error::new(ErrorCode::ResourceLimited, "jump target allocation refused")
                })?;
            for &target in &words.entries {
                c.checkpoint(1)?;
                // Dispatch records describe jalr zero, 0(reg). RV32 clears
                // bit zero AFTER computing the address; never round a target
                // before reading the table word.
                let Ok(target) = u32::try_from(target) else {
                    complete = false;
                    break;
                };
                let target = u64::from(target & !1);
                complete &= inside(target);
                targets.push(target);
            }
            // The source owns its temporary words/reservation. Only our
            // copied targets and the next pass's jump survive this iteration.
        }
        if complete {
            if alternatives.is_some() {
                targets.sort_unstable();
                targets.dedup();
            }
            jumps.push(if alternatives.is_some() {
                JumpDispatch::TargetSet { site, targets }
            } else {
                JumpDispatch::Table {
                    site,
                    first_case: case,
                    entries: targets,
                }
            });
        }
    }
    Ok(ProvenTables {
        tables: jumps,
        reserved,
    })
}

/// Apply an RV32 load displacement before asking the build for table words.
fn displaced(base: &AbstractValue, displacement: i32) -> Option<AbstractValue> {
    Some(match base {
        AbstractValue::Constant { value } => AbstractValue::Constant {
            value: value.wrapping_add_signed(displacement),
        },
        AbstractValue::ImageAddress { address } => AbstractValue::ImageAddress {
            address: address.wrapping_add_signed(displacement),
        },
        AbstractValue::Symbol { symbol, addend } => AbstractValue::Symbol {
            symbol: symbol.clone(),
            addend: addend.checked_add(i64::from(displacement))?,
        },
        _ => return None,
    })
}

/// The excluded index prefix for a table labelled at the unshifted base.
fn displaced_cases(displacement: i32, scale: u8) -> Option<u32> {
    let step = 1_u32.checked_shl(u32::from(scale))?;
    let prefix = displacement.unsigned_abs();
    let cases = prefix >> scale;
    (displacement < 0 && prefix.is_multiple_of(step) && cases <= MAX_ENTRIES).then_some(cases)
}

fn constant(value: &AbstractValue) -> Option<u32> {
    match value {
        AbstractValue::Constant { value } => Some(*value),
        _ => None,
    }
}

/// The expression `value` names in a dense id index.
fn expression_of<'a>(
    value: &AbstractValue,
    expressions: &[&'a Expression],
) -> Option<&'a Expression> {
    match value {
        AbstractValue::Expression { id } => expressions.get(*id as usize).copied(),
        _ => None,
    }
}

/// The entry count the bounds check guarding the dispatch block at `block`
/// states. The block must have exactly one incoming edge, from a conditional
/// branch on `index` against a constant, and that edge must be the branch's
/// in-range side: the fall-through of `bltu limit, index` / `bgeu index, limit`
/// or the taken edge of `bltu index, limit` / `bgeu limit, index`. Any other
/// shape, such as a check elsewhere in the function, states no bound. Every
/// visited record is charged.
fn guarding_bound(
    records: &[FunctionRecord],
    index: &AbstractValue,
    block: u64,
    c: &mut dyn RunControl,
) -> Result<Option<u32>> {
    let mut incoming = None;
    for record in records {
        c.checkpoint(1)?;
        if let FunctionRecord::Edge {
            from,
            target: Some(target),
            relation,
            external: false,
        } = record
            && *target == block
        {
            if incoming.is_some() {
                return Ok(None);
            }
            incoming = Some((*from, *relation));
        }
    }
    let Some((branch, relation)) = incoming else {
        return Ok(None);
    };
    let mut condition = None;
    for record in records {
        c.checkpoint(1)?;
        if let FunctionRecord::Condition {
            offset,
            test,
            left,
            right,
        } = record
            && *offset == branch
        {
            condition = Some((*test, left, right));
            break;
        }
    }
    let Some((test, left, right)) = condition else {
        return Ok(None);
    };
    let entries = match (test, relation) {
        // `bltu limit, index` leaves the range when taken.
        (BranchTest::Ltu, EdgeKind::Fallthrough) if right == index => {
            constant(left).and_then(|limit| limit.checked_add(1))
        }
        // `bgeu index, limit` leaves the range when taken.
        (BranchTest::Geu, EdgeKind::Fallthrough) if left == index => constant(right),
        // `bltu index, limit` enters the range when taken.
        (BranchTest::Ltu, EdgeKind::Taken) if left == index => constant(right),
        // `bgeu limit, index` enters the range when taken.
        (BranchTest::Geu, EdgeKind::Taken) if right == index => {
            constant(left).and_then(|limit| limit.checked_add(1))
        }
        _ => None,
    };
    Ok(entries.filter(|entries| (1..=MAX_ENTRIES).contains(entries)))
}

/// `value` without a zero extension to 8 or 16 bits (`andi 0xff`,
/// `zext.h`'s `and 0xffff`, or `slli k; srli k`), which GCC emits for a
/// narrow switch value.
fn unextended<'a>(value: &'a AbstractValue, expressions: &[&'a Expression]) -> &'a AbstractValue {
    match expression_of(value, expressions) {
        Some(Expression::Integer {
            op: IntegerOp::And,
            left,
            right,
        }) => match (constant(left), constant(right)) {
            (_, Some(0xff | 0xffff)) => left,
            (Some(0xff | 0xffff), _) => right,
            _ => value,
        },
        Some(Expression::Integer {
            op: IntegerOp::Shr,
            left,
            right,
        }) => match (expression_of(left, expressions), constant(right)) {
            (
                Some(Expression::Integer {
                    op: IntegerOp::Shl,
                    left: inner,
                    right: by,
                }),
                Some(k @ (16 | 24)),
            ) if constant(by) == Some(k) => inner,
            _ => value,
        },
        _ => value,
    }
}

/// The case value of entry zero: `c` when GCC rebased the switch value as
/// `index = value - c` (an addition of `-c` with the constant on either side
/// or a subtraction of `c`, under any narrow zero extension), zero when the
/// index is an opaque value itself, and unknown for any other shape: an
/// unrecognized index never claims case values.
fn first_case(index: &AbstractValue, expressions: &[&Expression]) -> Option<i64> {
    let index = unextended(index, expressions);
    // An index the pass does not know claims no case value.
    let expression = expression_of(index, expressions)?;
    match expression {
        Expression::Integer {
            op: IntegerOp::Add,
            left,
            right,
        } => match (constant(left), constant(right)) {
            (_, Some(c)) | (Some(c), _) => Some(-i64::from(c as i32)),
            _ => None,
        },
        Expression::Integer {
            op: IntegerOp::Sub,
            right,
            ..
        } => constant(right).map(|c| i64::from(c as i32)),
        Expression::Integer { .. } => None,
        _ => Some(0),
    }
}
