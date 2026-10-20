//! A structural induction proof over the existing CFG and prepared semantics.
//! No execution samples, instruction decoding or second value interpreter.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn emit(
    store: usize,
    input: &FunctionInput<'_>,
    nodes: &[Node],
    edges: &[Edge],
    operations: &[Operation],
    states: &[Option<State>],
    isa: &dyn FunctionSemantics,
    sets: &Sets<'_>,
    sink: &mut dyn FunctionSink,
    c: &mut dyn RunControl,
) -> Result<()> {
    let SemanticOp::Memory {
        kind: MemoryKind::Store,
        base: pointer,
        displacement,
        width: 1,
        ..
    } = operations[store].op
    else {
        return Ok(());
    };
    if pointer == 0 {
        return Ok(());
    }
    // The supported cycle is one straight body followed by a masked test.
    let mut latch = store;
    while matches!(nodes[latch].decoded.flow, InstructionFlow::Next) {
        c.checkpoint(1)?;
        latch += 1;
        if latch == nodes.len() {
            return Ok(());
        }
    }
    let InstructionFlow::Branch { displacement: back } = nodes[latch].decoded.flow else {
        return Ok(());
    };
    let Some(header) = target(nodes[latch].offset, i64::from(back))
        .and_then(|pc| nodes.binary_search_by_key(&pc, |n| n.offset).ok())
    else {
        return Ok(());
    };
    if header > store || store + 1 >= latch || nodes[header].offset <= input.extent.start {
        return Ok(());
    }
    let guard = latch - 1;
    let SemanticOp::Integer {
        op: IntegerOp::And,
        dest: residue,
        left: Operand::Register(base),
        right: Operand::Immediate(mask),
    } = operations[guard].op
    else {
        return Ok(());
    };
    // Bounded expansion, with a power-of-two modulus dividing 2^32.
    if base != pointer || residue == 0 || residue == pointer || !matches!(mask, 1 | 3 | 7) {
        return Ok(());
    }
    let start = (nodes[latch].offset - input.extent.start) as usize;
    let condition = isa.branch(&input.bytes[start..start + nodes[latch].decoded.length as usize]);
    if !matches!(
        condition,
        Some((BranchTest::Ne, Operand::Register(r), Operand::Register(0))) if r == residue
    ) {
        return Ok(());
    }
    let mut step = None;
    for at in header..=latch {
        c.checkpoint(1)?;
        let operation = operations[at];
        if operation.gap.is_some()
            || operation.opaque_call
            || operation.relocation.is_some()
            || nodes[at].conflict
            || operation.op == SemanticOp::Unsupported
        {
            return Ok(());
        }
        if operation.op.destination_register() == Some(pointer) {
            if step.is_some()
                || !matches!(operation.op, SemanticOp::Integer {
                    op: IntegerOp::Add, dest, left: Operand::Register(source),
                    right: Operand::Immediate(1),
                } if dest == pointer && source == pointer)
            {
                return Ok(());
            }
            step = Some(at);
        }
        // Require the exact body edges, including no side exit or call.
        let first = edges.partition_point(|e| e.from < nodes[at].offset);
        let mut outgoing = 0u8;
        for edge in edges[first..]
            .iter()
            .take_while(|e| e.from == nodes[at].offset)
        {
            c.checkpoint(1)?;
            let (expected, bit) = if at == latch && edge.relation == EdgeKind::Taken {
                (Some(nodes[header].offset), 2)
            } else if edge.relation == EdgeKind::Fallthrough {
                (
                    Some(nodes[at].offset + u64::from(nodes[at].decoded.length)),
                    1,
                )
            } else {
                (None, 0)
            };
            if edge.external || expected.is_none() || edge.target != expected || outgoing & bit != 0
            {
                return Ok(());
            }
            outgoing |= bit;
        }
        if outgoing != if at == latch { 3 } else { 1 }
            || (at < latch
                && (!matches!(nodes[at].decoded.flow, InstructionFlow::Next)
                    || nodes[at].offset + u64::from(nodes[at].decoded.length)
                        != nodes[at + 1].offset))
        {
            return Ok(());
        }
    }
    let Some(step) = step.filter(|at| *at < guard) else {
        return Ok(());
    };
    // Audit ALL incoming paths, not just a dominating-looking predecessor.
    // An external entry may only reach the test, before the first body.
    let mut entry = None;
    for edge in edges {
        c.checkpoint(1)?;
        if edge
            .target
            .is_some_and(|pc| pc >= nodes[header].offset && pc <= nodes[latch].offset)
            && (edge.from < nodes[header].offset || edge.from > nodes[latch].offset)
        {
            if entry.is_some()
                || edge.target != Some(nodes[guard].offset)
                || edge.external
                || !matches!(
                    edge.relation,
                    EdgeKind::Jump | EdgeKind::Fallthrough | EdgeKind::Taken
                )
            {
                return Ok(());
            }
            entry = nodes.binary_search_by_key(&edge.from, |n| n.offset).ok();
            if entry.is_none() {
                return Ok(());
            }
        }
    }
    let Some(entry) = entry else { return Ok(()) };
    let operation = operations[entry];
    if operation.op.destination_register() == Some(pointer)
        || operation.op == SemanticOp::Unsupported
        || operation.gap.is_some()
        || operation.opaque_call
        || operation.relocation.is_some()
        || nodes[entry].conflict
    {
        return Ok(());
    }
    let Some(state) = states[entry] else {
        return Ok(());
    };
    let initial = state[usize::from(pointer)];
    if !matches!(
        initial,
        Value::Expr(_) | Value::Constant(_) | Value::Image(_) | Value::Stack(_)
    ) {
        return Ok(());
    }
    // Invariant at each test: p = initial + n (mod 2^32). For residue r,
    // r=0 skips the body; otherwise the rank (mask+1)-r is in 1..=mask
    // and decreases by one per unique +1 step until the next zero test.
    // A store observes n or n+1 according to its position relative to step.
    let first_offset = (displacement as u32).wrapping_add(u32::from(step < store));
    sink.record(
        &FunctionRecord::StoreAddressRange {
            offset: nodes[store].offset,
            initial: public(initial, input, sets),
            first_offset,
            count: mask as u8,
            proof: AlignmentLoopProof {
                header: nodes[header].offset,
                guard: nodes[guard].offset,
                back_edge: nodes[latch].offset,
                step: nodes[step].offset,
                entry_edge: nodes[entry].offset,
                pointer,
                mask: mask as u8,
            },
        },
        c,
    )
}
