//! Bounded path driver for one normal-return word, over the prepared common
//! graph and evaluator. No decoding, private CFG or instruction interpreter.
use super::*;
use crate::writes::{
    WordContents, WordEvent, WordEventKind, WordPostcondition, WordQuery, WriteBase, WriteOrigin,
    WriteValue,
};
const MAX_STEPS: u32 = 4096;

#[derive(Clone, Copy)]
pub(super) struct Range {
    base: u32,
    extent: u32,
    needs_bounds: bool,
}

fn address(
    symbols: &Symbols<'_>,
    value: Value,
    c: &mut dyn RunControl,
) -> Result<Option<WordAddress>> {
    symbols.word_address(value, 32, c)
}

fn value_address(
    symbols: &mut Symbols<'_>,
    site: u64,
    at: WordAddress,
    c: &mut dyn RunControl,
) -> Result<Value> {
    Ok(match at {
        WordAddress::Absolute(at) => Value::Constant(at),
        WordAddress::Stack(at) => Value::Stack(at),
        WordAddress::Expression { base, offset: 0 } => Value::Expr(base),
        WordAddress::Expression { base, offset } => symbols.expression(
            site,
            Expression::Integer {
                op: IntegerOp::Add,
                left: AbstractValue::Expression { id: base },
                right: AbstractValue::Constant { value: offset },
            },
            c,
        )?,
    })
}

pub(super) fn binary(
    symbols: &mut Symbols<'_>,
    site: u64,
    op: IntegerOp,
    a: Value,
    b: Value,
    c: &mut dyn RunControl,
) -> Result<Option<Value>> {
    let Some(range) = symbols.word_range else {
        return Ok(None);
    };
    let (a_address, b_address) = (address(symbols, a, c)?, address(symbols, b, c)?);
    if op == IntegerOp::Sub
        && let (Some(a), Some(b)) = (a_address, b_address)
    {
        let delta = match (a, b) {
            (
                WordAddress::Expression { base: x, offset: a },
                WordAddress::Expression { base: y, offset: b },
            ) if x == y => Some(a.wrapping_sub(b)),
            (WordAddress::Stack(a), WordAddress::Stack(b)) => {
                Some((a as u32).wrapping_sub(b as u32))
            }
            _ => None,
        };
        if let Some(delta) = delta {
            return Ok(Some(Value::Constant(delta)));
        }
    }
    let pair = match (op, a_address, b) {
        (IntegerOp::Add, Some(at), Value::Constant(delta)) => Some((at, delta)),
        (IntegerOp::Sub, Some(at), Value::Constant(delta)) => Some((at, delta.wrapping_neg())),
        _ => match (op, a, b_address) {
            (IntegerOp::Add, Value::Constant(delta), Some(at)) => Some((at, delta)),
            _ => None,
        },
    };
    if let Some((WordAddress::Expression { base, offset }, delta)) = pair {
        return value_address(
            symbols,
            site,
            WordAddress::Expression {
                base,
                offset: offset.wrapping_add(delta),
            },
            c,
        )
        .map(Some);
    }
    if op == IntegerOp::And
        && let (Some(WordAddress::Expression { base, offset }), Value::Constant(mask)) =
            (a_address, b)
        && base == range.base
    {
        if matches!(mask, 1 | 3) {
            return Ok(Some(Value::Constant(offset & mask)));
        }
        if matches!(!mask, 1 | 3) {
            return value_address(
                symbols,
                site,
                WordAddress::Expression {
                    base,
                    offset: offset & mask,
                },
                c,
            )
            .map(Some);
        }
    }
    Ok(None)
}

// Ordering of two translated words changes only at a wrap or sign boundary.
// Check every interval boundary, including both neighbouring aligned values.
fn uniform(test: BranchTest, a: u32, b: u32, maximum: u32) -> Option<bool> {
    let mut result = None;
    let mut check = |value: u32| -> Option<()> {
        if value > maximum || value % 4 != 0 {
            return Some(());
        }
        let taken = test.evaluate(value.wrapping_add(a), value.wrapping_add(b));
        if result.is_some_and(|old| old != taken) {
            return None;
        }
        result = Some(taken);
        Some(())
    };
    check(0)?;
    check(maximum & !3)?;
    for offset in [a, b] {
        for threshold in [0u32, 0x80000000] {
            let cut = threshold.wrapping_sub(offset);
            check(cut & !3)?;
            check((cut & !3).wrapping_sub(4))?;
            check((cut & !3).wrapping_add(4))?;
        }
    }
    result
}

fn branch(
    test: BranchTest,
    a: Value,
    b: Value,
    symbols: &mut Symbols<'_>,
    c: &mut dyn RunControl,
) -> Result<Option<bool>> {
    if let (Value::Constant(a), Value::Constant(b)) = (a, b) {
        return Ok(Some(test.evaluate(a, b)));
    }
    let (
        Some(WordAddress::Expression { base: x, offset: a }),
        Some(WordAddress::Expression { base: y, offset: b }),
    ) = (address(symbols, a, c)?, address(symbols, b, c)?)
    else {
        return Ok(None);
    };
    let Some(range) = &mut symbols.word_range else {
        return Ok(None);
    };
    if x != y || x != range.base {
        return Ok(None);
    }
    let result = uniform(test, a, b, u32::MAX - range.extent);
    if result.is_some() && result != uniform(test, a, b, u32::MAX) {
        range.needs_bounds = true;
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Read {
    id: u32,
    site: u64,
    order: u32,
    address: WordAddress,
    width: u8,
    clean: bool,
}
struct Path<'m> {
    at: usize,
    state: State,
    steps: u32,
    word: Option<(Read, u64, u32)>,
    clean_source: bool,
    reads: AdmittedVec<'m, Read>,
    events: AdmittedVec<'m, WordEvent>,
}
impl<'m> Path<'m> {
    fn fork(&self, at: usize, memory: &'m WorkingMemory, c: &mut dyn RunControl) -> Result<Self> {
        let mut reads = AdmittedVec::new(memory);
        let mut events = AdmittedVec::new(memory);
        for event in self.events.iter() {
            c.checkpoint(1)?;
            events.push(*event, c.position())?;
        }
        for read in self.reads.iter() {
            c.checkpoint(1)?;
            reads.push(*read, c.position())?;
        }
        Ok(Self {
            at,
            reads,
            events,
            state: self.state,
            steps: self.steps,
            word: self.word,
            clean_source: self.clean_source,
        })
    }
}

fn read_value(
    value: Value,
    symbols: &Symbols<'_>,
    reads: &[Read],
    c: &mut dyn RunControl,
) -> Result<Option<Read>> {
    let mut value = value;
    for _ in 0..32 {
        c.checkpoint(1)?;
        let Value::Expr(id) = value else {
            return Ok(None);
        };
        if let Some(read) = reads.iter().find(|r| r.id == id) {
            return Ok(Some(*read));
        }
        match symbols.records.get(id as usize).map(|(_, e)| e) {
            Some(Expression::Integer {
                op: IntegerOp::And,
                left: AbstractValue::Expression { id },
                right: AbstractValue::Constant { value: u32::MAX },
            }) => value = Value::Expr(*id),
            _ => return Ok(None),
        }
    }
    Ok(None)
}

fn write_value(
    at: WordAddress,
    owner: u64,
    abi: CallAbi,
    symbols: &Symbols<'_>,
) -> Option<WriteValue> {
    Some(match at {
        WordAddress::Absolute(address) => WriteValue::base(WriteBase::Address(address)),
        WordAddress::Stack(offset) => WriteValue::base(WriteBase::Stack {
            function: owner,
            offset,
        }),
        WordAddress::Expression { base, offset } => {
            let Some((_, Expression::EntryRegister { register })) =
                symbols.records.get(base as usize)
            else {
                return None;
            };
            WriteValue::Base {
                base: WriteBase::Argument(abi.argument_word(*register)? as u8),
                offset,
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn analyze<'m>(
    context: &writes::CallContext,
    query: WordQuery,
    closed: bool,
    input: &FunctionInput<'_>,
    nodes: &[Node],
    edges: &[Edge],
    operations: &[Operation],
    original: &[Option<State>],
    isa: &dyn FunctionSemantics,
    symbols: &mut Symbols<'_>,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Option<WordPostcondition<'m>>> {
    if !closed
        || [query.source, query.destination, query.length]
            .iter()
            .any(|word| *word >= 8)
    {
        return Ok(None);
    }
    let Some(extent) = context.arguments()[usize::from(query.length)] else {
        return Ok(None);
    };
    if extent > 4096 || query.offset.checked_add(4).is_none_or(|end| end > extent) {
        return Ok(None);
    }
    if operations
        .iter()
        .any(|op| op.gap.is_some() || op.op == SemanticOp::Unsupported || op.opaque_call)
    {
        return Ok(None);
    }
    let Ok(entry) = nodes.binary_search_by_key(&input.extent.start, |n| n.offset) else {
        return Ok(None);
    };
    let Some(mut initial) = original[entry] else {
        return Ok(None);
    };
    let destination_register = context.abi.arguments()[usize::from(query.destination)];
    let source_register = context.abi.arguments()[usize::from(query.source)];
    let Some(WordAddress::Expression { base, offset: 0 }) =
        address(symbols, initial[usize::from(destination_register)], c)?
    else {
        return Ok(None);
    };
    for (value, register) in context.arguments().iter().zip(context.abi.arguments()) {
        if let Some(value) = value {
            initial[usize::from(register)] = Value::Constant(*value);
        }
    }
    if matches!(initial[usize::from(destination_register)], Value::Constant(v) if v % 4 != 0) {
        return Ok(None);
    }
    let query_address =
        |value, symbols: &mut Symbols<'_>, c: &mut dyn RunControl| -> Result<Option<WordAddress>> {
            let value = symbols.binary(
                input.extent.start,
                IntegerOp::Add,
                value,
                Value::Constant(query.offset),
                input,
                c,
            )?;
            address(symbols, value, c)
        };
    let Some(destination) = query_address(initial[usize::from(destination_register)], symbols, c)?
    else {
        return Ok(None);
    };
    let Some(source) = query_address(initial[usize::from(source_register)], symbols, c)? else {
        return Ok(None);
    };
    // The common DAG's usual per-instruction bound assumes a fixed point.
    // This bounded path proof can revisit an instruction with new offsets.
    symbols.limit = symbols
        .limit
        .checked_add(MAX_STEPS as usize * 8)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimited,
                "word expression capacity overflow",
            )
        })?;
    let old_range = symbols.word_range.replace(Range {
        base,
        extent,
        needs_bounds: false,
    });
    let (old_spills, old_words) = (symbols.symbolic_spills, symbols.memory_words);
    symbols.symbolic_spills = false;
    symbols.memory_words = false;
    let result = run(
        context,
        query,
        extent,
        destination,
        source,
        entry,
        initial,
        input,
        nodes,
        edges,
        operations,
        isa,
        symbols,
        memory,
        c,
    );
    symbols.word_range = old_range;
    symbols.symbolic_spills = old_spills;
    symbols.memory_words = old_words;
    result
}

#[allow(clippy::too_many_arguments)]
fn run<'m>(
    context: &writes::CallContext,
    query: WordQuery,
    extent: u32,
    destination: WordAddress,
    source: WordAddress,
    entry: usize,
    initial: State,
    input: &FunctionInput<'_>,
    nodes: &[Node],
    edges: &[Edge],
    operations: &[Operation],
    isa: &dyn FunctionSemantics,
    symbols: &mut Symbols<'_>,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Option<WordPostcondition<'m>>> {
    let mut pending = AdmittedVec::new(memory);
    pending.push(
        Path {
            at: entry,
            state: initial,
            steps: 0,
            word: None,
            clean_source: true,
            reads: AdmittedVec::new(memory),
            events: AdmittedVec::new(memory),
        },
        c.position(),
    )?;
    let (mut paths, mut returned, mut contents) = (0, false, None);
    let mut events = AdmittedVec::new(memory);
    let mut returned_paths = 0;
    while let Some(mut path) = pending.pop() {
        paths += 1;
        if paths > 64 {
            return Ok(None);
        }
        loop {
            c.checkpoint(1)?;
            path.steps += 1;
            if path.steps > MAX_STEPS {
                return Ok(None);
            }
            let at = path.at;
            let node = &nodes[at];
            let (out, effects) = evaluate(
                operations[at],
                node,
                input,
                path.state,
                c,
                symbols,
                true,
                Some(context.abi),
            )?;
            if effects.gap.is_some() {
                return Ok(None);
            }
            if let Some((kind, width, location, value)) = effects.memory {
                let location = address(symbols, location, c)?;
                let read = if kind == MemoryKind::Store {
                    read_value(value.unwrap_or(Value::Unknown), symbols, &path.reads, c)?
                        .map(|r| (r.site, r.order, r.width))
                } else {
                    None
                };
                path.events.push(
                    WordEvent {
                        owner: input.extent.start,
                        path: 0,
                        site: node.offset,
                        order: path.steps,
                        kind: if kind == MemoryKind::Load {
                            WordEventKind::Load
                        } else {
                            WordEventKind::Store
                        },
                        address: location
                            .and_then(|at| {
                                write_value(at, input.extent.start, context.abi, symbols)
                            })
                            .unwrap_or(WriteValue::UNKNOWN),
                        width,
                        read,
                    },
                    c.position(),
                )?;
                if kind == MemoryKind::Load {
                    if let (Some(location), Some((_, Value::Expr(id)))) =
                        (location, effects.register)
                    {
                        if path.reads.iter().any(|read| read.id == id) {
                            return Ok(None);
                        }
                        path.reads.push(
                            Read {
                                id,
                                site: node.offset,
                                order: path.steps,
                                address: location,
                                width,
                                clean: path.clean_source,
                            },
                            c.position(),
                        )?;
                    }
                } else if kind == MemoryKind::Store {
                    path.clean_source &= location.is_some_and(|at| at.disjoint(width, source, 4));
                    if !location.is_some_and(|at| at.disjoint(width, destination, 4)) {
                        path.word = None;
                        if width == 4
                            && location == Some(destination)
                            && let Some(read) = read_value(
                                value.unwrap_or(Value::Unknown),
                                symbols,
                                &path.reads,
                                c,
                            )?
                            && read.width == 4
                            && read.address == source
                            && read.order < path.steps
                        {
                            path.word = Some((read, node.offset, path.steps));
                        }
                    }
                } else {
                    return Ok(None);
                }
            }
            if matches!(node.decoded.flow, InstructionFlow::Return { .. }) {
                let Some((read, store, store_order)) = path.word else {
                    return Ok(None);
                };
                let Some(destination) =
                    write_value(destination, input.extent.start, context.abi, symbols)
                else {
                    return Ok(None);
                };
                let Some(source) = write_value(source, input.extent.start, context.abi, symbols)
                else {
                    return Ok(None);
                };
                let candidate = WordContents {
                    origin: WriteOrigin {
                        function: input.extent.start,
                        tail: None,
                    },
                    destination,
                    source,
                    load: read.site,
                    load_order: read.order,
                    store,
                    store_order,
                    width: 4,
                    source_unchanged_before_load: read.clean,
                };
                if returned && contents != Some(candidate) {
                    return Ok(None);
                }
                contents = Some(candidate);
                returned = true;
                returned_paths += 1;
                for event in path.events.iter() {
                    c.checkpoint(1)?;
                    events.push(
                        WordEvent {
                            path: returned_paths,
                            ..*event
                        },
                        c.position(),
                    )?;
                }
                events.push(
                    WordEvent {
                        owner: input.extent.start,
                        path: returned_paths,
                        order: path.steps,
                        site: node.offset,
                        kind: WordEventKind::Return,
                        width: 0,
                        address: WriteValue::UNKNOWN,
                        read: None,
                    },
                    c.position(),
                )?;
                break;
            }
            let mut taken = None;
            if matches!(node.decoded.flow, InstructionFlow::Branch { .. }) {
                let start = (node.offset - input.extent.start) as usize;
                let Some((test, a, b)) =
                    isa.branch(&input.bytes[start..start + usize::from(node.decoded.length)])
                else {
                    return Ok(None);
                };
                taken = branch(
                    test,
                    operand(&path.state, a),
                    operand(&path.state, b),
                    symbols,
                    c,
                )?;
            }
            let mut next = None;
            let first = edges.partition_point(|e| e.from < node.offset);
            for edge in edges[first..].iter().take_while(|e| e.from == node.offset) {
                c.checkpoint(1)?;
                if matches!(
                    (taken, edge.relation),
                    (Some(true), EdgeKind::Fallthrough) | (Some(false), EdgeKind::Taken)
                ) {
                    continue;
                }
                let Some(to) = alignment::relations::local_target(edge, nodes) else {
                    return Ok(None);
                };
                if let Some(previous) = next {
                    if paths + pending.len() >= 64 {
                        return Ok(None);
                    }
                    let mut fork = path.fork(previous, memory, c)?;
                    fork.state = out;
                    pending.push(fork, c.position())?;
                }
                next = Some(to);
            }
            let Some(next) = next else {
                return Ok(None);
            };
            path.at = next;
            path.state = out;
        }
    }
    Ok(contents.map(|contents| WordPostcondition {
        contents,
        context: *context,
        query,
        extent,
        needs_bounds: symbols.word_range.is_some_and(|range| range.needs_bounds),
        events,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translated_predicates_cover_every_small_domain_and_rv32_boundaries() {
        let offsets = [0, 4, 32, 0x7ffffffc, 0x80000000, 0xffffffe0, 0xfffffffc];
        for test in [
            BranchTest::Eq,
            BranchTest::Ne,
            BranchTest::Lt,
            BranchTest::Ge,
            BranchTest::Ltu,
            BranchTest::Geu,
        ] {
            for a in offsets {
                for b in offsets {
                    let first = test.evaluate(a, b);
                    let expected = (0..=65532u32)
                        .step_by(4)
                        .all(|base| {
                            test.evaluate(base.wrapping_add(a), base.wrapping_add(b)) == first
                        })
                        .then_some(first);
                    assert_eq!(
                        uniform(test, a, b, 65532),
                        expected,
                        "{test:?} {a:#x} {b:#x}"
                    );
                    if let Some(taken) = uniform(test, a, b, u32::MAX - 68) {
                        for base in [0u32, 4, 0x7fffffbc, 0x7ffffffc, 0x80000000, 0xffffffb8] {
                            assert_eq!(
                                test.evaluate(base.wrapping_add(a), base.wrapping_add(b)),
                                taken
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(uniform(BranchTest::Ltu, 4, 68, u32::MAX - 68), Some(true));
        assert_eq!(uniform(BranchTest::Ltu, 4, 68, u32::MAX), None);
        assert_eq!(uniform(BranchTest::Lt, 4, 68, u32::MAX - 68), None);
    }
}
