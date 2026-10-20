//! A bounded, optional value pass over the existing nodes/edges/operations.
//! It uses the shared evaluator and #191's conditional all-input equalities.
//! Reachability starts ONLY at function entry, so a back edge is not a root.
use super::*;
use crate::writes::{BranchProof, CallContext, ContextProof};

#[allow(clippy::too_many_arguments)]
pub(super) fn analyze<'m>(
    context: &CallContext,
    closed: bool,
    input: &FunctionInput<'_>,
    nodes: &[Node],
    edges: &[Edge],
    operations: &[Operation],
    original: &[Option<State>],
    relations: &RecordBuffer<'_>,
    isa: &dyn FunctionSemantics,
    symbols: &mut Symbols<'_>,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Option<ContextProof<'m>>> {
    if !closed || context.entry != input.extent.start {
        return Ok(None);
    }
    for (node, op) in nodes.iter().zip(operations) {
        c.checkpoint(1)?;
        if node.conflict || op.gap.is_some() || op.op == SemanticOp::Unsupported {
            return Ok(None);
        }
    }
    let Ok(entry) = nodes.binary_search_by_key(&input.extent.start, |n| n.offset) else {
        return Ok(None);
    };
    let _workspace = memory.reserve(
        (nodes.len() * (std::mem::size_of::<Option<State>>() + std::mem::size_of::<usize>() + 1))
            as u64,
        c.position(),
    )?;
    let mut states = reserve_vec::<Option<State>>(nodes.len())?;
    states.resize(nodes.len(), None);
    let mut queued = reserve_vec::<bool>(nodes.len())?;
    queued.resize(nodes.len(), false);
    let mut queue = VecDeque::new();
    queue.try_reserve_exact(nodes.len()).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "context queue allocation refused",
        )
    })?;
    let Some(mut initial) = original[entry] else {
        return Ok(None);
    };
    for (value, register) in context.arguments.iter().zip(context.abi.arguments()) {
        if let Some(value) = value {
            initial[usize::from(register)] = Value::Constant(*value);
        }
    }
    // Instantiate entry expressions, never replace a LIVE register using its
    // entry argument number. In particular live a2 may already equal 32.
    let mut equalities = AdmittedVec::new(memory);
    for record in relations.iter() {
        c.checkpoint(1)?;
        if let FunctionRecord::AlignedValue { fact } = record
            && fact.owner == context.entry
            && let ValuePoint::Entry { offset } = fact.point
            && let Ok(at) = nodes.binary_search_by_key(&offset, |n| n.offset)
        {
            let value = instantiate(&fact.value, context, symbols, input, 32, c)?;
            if value != Value::Unknown {
                equalities.push((at, fact.register, value), c.position())?;
            }
        }
    }
    // Facts from different loop witnesses need not arrive in PC order.
    equalities.sort_unstable_by_key(|(at, register, _)| (*at, *register));
    states[entry] = Some(initial);
    queued[entry] = true;
    queue.push_back(entry);
    while let Some(at) = queue.pop_front() {
        queued[at] = false;
        c.checkpoint(1)?;
        let mut state = states[at].expect("queued context state");
        let first = equalities.partition_point(|(node, _, _)| *node < at);
        for (_, register, value) in equalities[first..]
            .iter()
            .take_while(|(node, _, _)| *node == at)
        {
            c.checkpoint(1)?;
            state[usize::from(*register)] = *value;
        }
        // Store the strengthened entry for final branch witnesses as well.
        states[at] = Some(state);
        let (out, _) = evaluate(
            operations[at],
            &nodes[at],
            input,
            state,
            c,
            symbols,
            true,
            Some(context.abi),
        )?;
        let branch = branch_proof(at, state, input, nodes, isa, symbols, c)?;
        let first = edges.partition_point(|e| e.from < nodes[at].offset);
        for edge in edges[first..]
            .iter()
            .take_while(|e| e.from == nodes[at].offset)
        {
            c.checkpoint(1)?;
            if !allowed(edge, branch.as_ref()) {
                continue;
            }
            let Some(to) = alignment::relations::local_target(edge, nodes) else {
                continue;
            };
            let changed = match &mut states[to] {
                None => {
                    states[to] = Some(out);
                    true
                }
                Some(old) => join(old, &out, &mut symbols.sets, c)?,
            };
            if changed && !queued[to] {
                queued[to] = true;
                queue.push_back(to);
            }
        }
    }
    let mut proof = ContextProof {
        context: *context,
        owner: context.entry,
        branches: AdmittedVec::new(memory),
        unreachable: AdmittedVec::new(memory),
    };
    for (at, state) in states.iter().enumerate() {
        c.checkpoint(1)?;
        match state {
            None => proof.unreachable.push(nodes[at].offset, c.position())?,
            Some(state) => {
                if let Some(branch) = branch_proof(at, *state, input, nodes, isa, symbols, c)? {
                    proof.branches.push(branch, c.position())?;
                }
            }
        }
    }
    // Audit EVERY edge from the reached set into the excluded set. Internal
    // edges (including cycles) have no reached source and cannot seed it.
    for edge in edges {
        c.checkpoint(1)?;
        let Some(to) = alignment::relations::local_target(edge, nodes) else {
            continue;
        };
        let Ok(from) = nodes.binary_search_by_key(&edge.from, |n| n.offset) else {
            return Ok(None);
        };
        if let Some(state) = states[from]
            && states[to].is_none()
        {
            let branch = branch_proof(from, state, input, nodes, isa, symbols, c)?;
            if allowed(edge, branch.as_ref()) {
                return Ok(None);
            }
        }
    }
    Ok(Some(proof))
}

fn allowed(edge: &Edge, branch: Option<&BranchProof>) -> bool {
    match (branch, edge.relation) {
        (Some(branch), EdgeKind::Taken) => branch.taken,
        (Some(branch), EdgeKind::Fallthrough) => !branch.taken,
        _ => true,
    }
}

fn instantiate(
    value: &AbstractValue,
    context: &CallContext,
    symbols: &mut Symbols<'_>,
    input: &FunctionInput<'_>,
    depth: u8,
    c: &mut dyn RunControl,
) -> Result<Value> {
    c.checkpoint(1)?;
    if depth == 0 {
        return Ok(Value::Unknown);
    }
    Ok(match value {
        AbstractValue::Constant { value } => Value::Constant(*value),
        AbstractValue::EntryStack { offset } => Value::Stack(*offset),
        AbstractValue::Expression { id } => {
            let Some((site, expression)) = symbols.records.get(*id as usize).cloned() else {
                return Ok(Value::Unknown);
            };
            match expression {
                Expression::EntryRegister { register } => context
                    .abi
                    .argument_word(register)
                    .and_then(|word| context.arguments[usize::from(word)])
                    .map_or(Value::Expr(*id), Value::Constant),
                Expression::Integer { op, left, right } => {
                    let left = instantiate(&left, context, symbols, input, depth - 1, c)?;
                    let right = instantiate(&right, context, symbols, input, depth - 1, c)?;
                    symbols.binary(site, op, left, right, input, c)?
                }
                _ => Value::Unknown,
            }
        }
        _ => Value::Unknown,
    })
}

#[allow(clippy::too_many_arguments)]
fn branch_proof(
    at: usize,
    state: State,
    input: &FunctionInput<'_>,
    nodes: &[Node],
    isa: &dyn FunctionSemantics,
    symbols: &Symbols<'_>,
    c: &mut dyn RunControl,
) -> Result<Option<BranchProof>> {
    if !matches!(nodes[at].decoded.flow, InstructionFlow::Branch { .. }) {
        return Ok(None);
    }
    let start = (nodes[at].offset - input.extent.start) as usize;
    let Some((test, left, right)) =
        isa.branch(&input.bytes[start..start + nodes[at].decoded.length as usize])
    else {
        return Ok(None);
    };
    let Some((left, left_count)) = finite(operand(&state, left), symbols, c)? else {
        return Ok(None);
    };
    let Some((right, right_count)) = finite(operand(&state, right), symbols, c)? else {
        return Ok(None);
    };
    let taken = compare(test, left[0], right[0]);
    for a in &left[..usize::from(left_count)] {
        for b in &right[..usize::from(right_count)] {
            c.checkpoint(1)?;
            if compare(test, *a, *b) != taken {
                return Ok(None);
            }
        }
    }
    Ok(Some(BranchProof {
        site: nodes[at].offset,
        test,
        left,
        left_count,
        right,
        right_count,
        taken,
    }))
}
fn compare(test: BranchTest, a: u32, b: u32) -> bool {
    match test {
        BranchTest::Eq => a == b,
        BranchTest::Ne => a != b,
        BranchTest::Lt => (a as i32) < (b as i32),
        BranchTest::Ge => (a as i32) >= (b as i32),
        BranchTest::Ltu => a < b,
        BranchTest::Geu => a >= b,
    }
}

/// If A=mask+1 divides 2^32 and D=Q+r with Q aligned, then
/// ((D+a)&!mask)-((D+b)&!mask) = ((r+a)&!mask)-((r+b)&!mask)
/// in Z/(2^32). Enumerating ALL residues is exact even across wraparound;
/// signed interpretation is deferred to the actual branch test.
fn finite(
    value: Value,
    symbols: &Symbols<'_>,
    c: &mut dyn RunControl,
) -> Result<Option<([u32; 8], u8)>> {
    c.checkpoint(1)?;
    let mut values = [0; 8];
    if let Value::Constant(value) | Value::Image(value) = value {
        values[0] = value;
        return Ok(Some((values, 1)));
    }
    let Value::Expr(id) = value else {
        return Ok(None);
    };
    let Some((
        _,
        Expression::Integer {
            op: IntegerOp::Sub,
            left,
            right,
        },
    )) = symbols.records.get(id as usize)
    else {
        return Ok(None);
    };
    let Some((left_base, a, mask)) = masked(left, symbols) else {
        return Ok(None);
    };
    let Some((right_base, b, other_mask)) = masked(right, symbols) else {
        return Ok(None);
    };
    if left_base != right_base || mask != other_mask || !matches!(mask, 1 | 3 | 7) {
        return Ok(None);
    }
    let mut count = 0usize;
    for r in 0..=mask {
        c.checkpoint(1)?;
        let result = (r.wrapping_add(a) & !mask).wrapping_sub(r.wrapping_add(b) & !mask);
        if !values[..count].contains(&result) {
            values[count] = result;
            count += 1;
        }
    }
    values[..count].sort_unstable();
    Ok(Some((values, count as u8)))
}
fn masked<'a>(
    value: &'a AbstractValue,
    symbols: &'a Symbols<'_>,
) -> Option<(&'a AbstractValue, u32, u32)> {
    let AbstractValue::Expression { id } = value else {
        return None;
    };
    let (
        _,
        Expression::Integer {
            op: IntegerOp::And,
            left,
            right,
        },
    ) = symbols.records.get(*id as usize)?
    else {
        return None;
    };
    let (value, mask) = match (left, right) {
        (value, AbstractValue::Constant { value: mask })
        | (AbstractValue::Constant { value: mask }, value) => (value, !mask),
        _ => return None,
    };
    let (base, offset) = affine(value, symbols, 16)?;
    Some((base, offset, mask))
}
fn affine<'a>(
    value: &'a AbstractValue,
    symbols: &'a Symbols<'_>,
    depth: u8,
) -> Option<(&'a AbstractValue, u32)> {
    if depth == 0 {
        return None;
    }
    if let AbstractValue::Expression { id } = value
        && let Some((
            _,
            Expression::Integer {
                op: IntegerOp::Add,
                left,
                right,
            },
        )) = symbols.records.get(*id as usize)
    {
        let (base, delta) = match (left, right) {
            (base, AbstractValue::Constant { value })
            | (AbstractValue::Constant { value }, base) => (base, value),
            _ => return Some((value, 0)),
        };
        let (base, offset) = affine(base, symbols, depth - 1)?;
        return Some((base, offset.wrapping_add(*delta)));
    }
    Some((value, 0))
}
