//! An equality component over the SAME instruction CFG. Bits say which
//! registers equal one proved align-up expression; joins intersect all paths.
//! Register effects are evaluated by the existing value engine, never here.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(in crate::values) fn analyze<'m>(
    bounds: &[Bound],
    closed_graph: bool,
    input: &FunctionInput<'_>,
    nodes: &[Node],
    edges: &[Edge],
    operations: &[Operation],
    states: &[Option<State>],
    isa: &dyn FunctionSemantics,
    symbols: &mut Symbols<'_>,
    abi: Option<CallAbi>,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<RecordBuffer<'m>> {
    let mut records = RecordBuffer::new(memory);
    if bounds.is_empty() || !closed_graph {
        return Ok(records);
    }
    let Ok(entry) = nodes.binary_search_by_key(&input.extent.start, |n| n.offset) else {
        return Ok(records);
    };
    let _workspace = memory.reserve(
        (nodes.len() * (std::mem::size_of::<Option<u32>>() + std::mem::size_of::<usize>() + 1))
            as u64,
        c.position(),
    )?;
    let mut facts = reserve_vec::<Option<u32>>(nodes.len())?;
    let mut queued = reserve_vec::<bool>(nodes.len())?;
    let mut queue = VecDeque::new();
    queue.try_reserve_exact(nodes.len()).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "alignment equality queue allocation refused",
        )
    })?;
    for (position, bound) in bounds.iter().enumerate() {
        c.checkpoint(1)?;
        let mut duplicate = false;
        for previous in &bounds[..position] {
            c.checkpoint(1)?;
            duplicate |= previous.proof == bound.proof;
        }
        if duplicate {
            continue;
        }
        let mask = u32::from(bound.proof.mask);
        // Existing RV32 arithmetic/DAG, owned by this function. These are
        // derived expressions at the proof site, not architectural writes.
        let sum = symbols.binary(
            bound.proof.back_edge,
            IntegerOp::Add,
            bound.initial,
            Value::Constant(mask),
            input,
            c,
        )?;
        let aligned = symbols.binary(
            bound.proof.back_edge,
            IntegerOp::And,
            sum,
            Value::Constant(!mask),
            input,
            c,
        )?;
        facts.clear();
        facts.resize(nodes.len(), None);
        queued.clear();
        queued.resize(nodes.len(), false);
        facts[entry] = Some(0);
        queued[entry] = true;
        queue.push_back(entry);
        while let Some(at) = queue.pop_front() {
            queued[at] = false;
            c.checkpoint(1)?;
            let incoming = facts[at].unwrap_or(0);
            let outgoing = transfer_equality(
                at, incoming, aligned, input, nodes, operations, states, symbols, abi, c,
            )?;
            let first = edges.partition_point(|edge| edge.from < nodes[at].offset);
            for edge in edges[first..]
                .iter()
                .take_while(|edge| edge.from == nodes[at].offset)
            {
                c.checkpoint(1)?;
                let Some(to) = local_target(edge, nodes) else {
                    continue;
                };
                let (bits, _) = edge_equality(
                    edge, at, outgoing, bound, input, nodes, operations, states, isa, symbols, c,
                )?;
                let joined = facts[to].map_or(bits, |old| old & bits);
                if facts[to] != Some(joined) {
                    facts[to] = Some(joined);
                    if !queued[to] {
                        queued[to] = true;
                        queue.push_back(to);
                    }
                }
            }
        }
        // Emit only after the intersection fixed point, never a preferred or
        // first-visited predecessor. Unknown paths contribute zero bits.
        for (at, bits) in facts.iter().enumerate() {
            c.checkpoint(1)?;
            let Some(bits) = bits else { continue };
            emit(
                &mut records,
                *bits,
                ValuePoint::Entry {
                    offset: nodes[at].offset,
                },
                AlignmentValueOrigin::AllIncoming,
                bound,
                aligned,
                input,
                symbols,
                c,
            )?;
            let outgoing = transfer_equality(
                at, *bits, aligned, input, nodes, operations, states, symbols, abi, c,
            )?;
            let first = edges.partition_point(|edge| edge.from < nodes[at].offset);
            for edge in edges[first..]
                .iter()
                .take_while(|edge| edge.from == nodes[at].offset)
            {
                c.checkpoint(1)?;
                let Some(to) = local_target(edge, nodes) else {
                    continue;
                };
                let (outgoing, origin) = edge_equality(
                    edge, at, outgoing, bound, input, nodes, operations, states, isa, symbols, c,
                )?;
                emit(
                    &mut records,
                    outgoing,
                    ValuePoint::Edge {
                        from: edge.from,
                        target: nodes[to].offset,
                    },
                    origin,
                    bound,
                    aligned,
                    input,
                    symbols,
                    c,
                )?;
            }
        }
    }
    Ok(records)
}

fn local_target(edge: &Edge, nodes: &[Node]) -> Option<usize> {
    if !edge.external
        && matches!(
            edge.relation,
            EdgeKind::Fallthrough
                | EdgeKind::Taken
                | EdgeKind::Jump
                | EdgeKind::PossibleContinuation
        )
    {
        edge.target
            .and_then(|pc| nodes.binary_search_by_key(&pc, |n| n.offset).ok())
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn transfer_equality(
    at: usize,
    bits: u32,
    aligned: Value,
    input: &FunctionInput<'_>,
    nodes: &[Node],
    operations: &[Operation],
    states: &[Option<State>],
    symbols: &mut Symbols<'_>,
    abi: Option<CallAbi>,
    c: &mut dyn RunControl,
) -> Result<u32> {
    c.checkpoint(1)?;
    let operation = operations[at];
    if operation.gap.is_some()
        || operation.opaque_call
        || operation.relocation.is_some()
        || operation.op == SemanticOp::Unsupported
        || nodes[at].conflict
    {
        return Ok(0);
    }
    let Some(dest) = operation.op.destination_register().filter(|r| *r != 0) else {
        return Ok(bits);
    };
    let mut outgoing = bits & !(1u32 << dest);
    if bits != 0
        && let Some(mut state) = states[at]
    {
        for (register, value) in state.iter_mut().enumerate() {
            if bits & (1u32 << register) != 0 {
                *value = aligned;
            }
        }
        // Reuse the full shared effect semantics for copies, arithmetic,
        // loads and opaque destinations, including architectural x0 rules.
        let (out, _) = evaluate(operation, &nodes[at], input, state, c, symbols, true, abi)?;
        if out[usize::from(dest)] == aligned {
            outgoing |= 1u32 << dest;
        }
    }
    Ok(outgoing)
}

#[allow(clippy::too_many_arguments)]
fn edge_equality(
    edge: &Edge,
    at: usize,
    mut bits: u32,
    bound: &Bound,
    input: &FunctionInput<'_>,
    nodes: &[Node],
    operations: &[Operation],
    states: &[Option<State>],
    isa: &dyn FunctionSemantics,
    symbols: &Symbols<'_>,
    c: &mut dyn RunControl,
) -> Result<(u32, AlignmentValueOrigin)> {
    c.checkpoint(1)?;
    if edge.from == bound.proof.back_edge
        && edge.target == Some(bound.exit)
        && edge.relation == EdgeKind::Fallthrough
    {
        return Ok((
            bits | (1u32 << bound.proof.pointer),
            AlignmentValueOrigin::LoopExit,
        ));
    }
    let op = operations[at];
    if !matches!(nodes[at].decoded.flow, InstructionFlow::Branch { .. })
        || op.gap.is_some()
        || op.opaque_call
        || op.relocation.is_some()
        || nodes[at].conflict
    {
        return Ok((bits, AlignmentValueOrigin::Propagation));
    }
    let Some(state) = states[at] else {
        return Ok((bits, AlignmentValueOrigin::Propagation));
    };
    let start = (nodes[at].offset - input.extent.start) as usize;
    let Some((test, left, right)) =
        isa.branch(&input.bytes[start..start + nodes[at].decoded.length as usize])
    else {
        return Ok((bits, AlignmentValueOrigin::Propagation));
    };
    let zero_edge = matches!(
        (test, edge.relation),
        (BranchTest::Eq, EdgeKind::Taken) | (BranchTest::Ne, EdgeKind::Fallthrough)
    );
    if !zero_edge {
        return Ok((bits, AlignmentValueOrigin::Propagation));
    }
    let (left, right) = (operand(&state, left), operand(&state, right));
    let residue = if right == Value::Constant(0) {
        left
    } else if left == Value::Constant(0) {
        right
    } else {
        return Ok((bits, AlignmentValueOrigin::Propagation));
    };
    let initial = public(bound.initial, input, &symbols.sets);
    let mask = u32::from(bound.proof.mask);
    let is_residue = match residue {
        Value::Expr(id) => matches!(symbols.records.get(id as usize),
            Some((_, Expression::Integer { op: IntegerOp::And, left, right }))
            if (left == &initial && *right == AbstractValue::Constant { value: mask })
                || (right == &initial && *left == AbstractValue::Constant { value: mask })),
        Value::Constant(residue) => {
            // The equality is conditional on this zero edge. A nonzero
            // constant makes the edge infeasible; it never proves reachability.
            matches!(bound.initial, Value::Constant(v) | Value::Image(v) if v & mask == residue)
        }
        _ => false,
    };
    if is_residue {
        for (register, value) in state.iter().enumerate().skip(1) {
            c.checkpoint(1)?;
            if *value == bound.initial {
                bits |= 1u32 << register;
            }
        }
        return Ok((bits, AlignmentValueOrigin::ZeroResidue));
    }
    Ok((bits, AlignmentValueOrigin::Propagation))
}

#[allow(clippy::too_many_arguments)]
fn emit(
    records: &mut RecordBuffer<'_>,
    bits: u32,
    point: ValuePoint,
    origin: AlignmentValueOrigin,
    bound: &Bound,
    aligned: Value,
    input: &FunctionInput<'_>,
    symbols: &Symbols<'_>,
    c: &mut dyn RunControl,
) -> Result<()> {
    for register in 1..32 {
        if bits & (1u32 << register) != 0 {
            c.checkpoint(1)?;
            let _payload = symbols
                .memory
                .reserve(std::mem::size_of::<AlignedValue>() as u64, c.position())?;
            records.push(
                FunctionRecord::AlignedValue {
                    fact: Box::new(AlignedValue {
                        owner: input.extent.start,
                        point,
                        register,
                        initial: public(bound.initial, input, &symbols.sets),
                        value: public(aligned, input, &symbols.sets),
                        proof: bound.proof,
                        origin,
                    }),
                },
                c.position(),
            )?;
        }
    }
    Ok(())
}
