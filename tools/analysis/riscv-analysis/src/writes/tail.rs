//! One-level composition over canonical machine observations, without a new
//! decoder or graph. The first supported trampoline is exactly one jump.
use super::*;

fn transparent(
    facts: &FunctionIndex<'_, '_>,
    entry: u64,
    c: &mut dyn RunControl,
) -> Result<Option<TailTransfer>> {
    let Some(transfer) = facts.transfer(entry, c)? else {
        return Ok(None);
    };
    let AbstractValue::ImageAddress { address } = transfer.target else {
        return Ok(None);
    };
    if transfer.call {
        return Ok(None);
    }
    let target = u64::from(*address);
    let (mut instructions, mut edges, mut zero_writes) = (0, 0, 0);
    for record in facts.records() {
        c.checkpoint(1)?;
        if !closes_local_graph(record) {
            return Ok(None);
        }
        match record {
            FunctionRecord::Instruction {
                offset, decoded, ..
            } if *offset == entry
                && matches!(
                    decoded.flow,
                    InstructionFlow::Jump { link: false, .. }
                        | InstructionFlow::Indirect { link: false, .. }
                ) =>
            {
                instructions += 1
            }
            // `link=false` only excludes x1/x5. The common engine emits the
            // architectural destination even for x0; every other write fails.
            FunctionRecord::Value {
                offset,
                register: 0,
                value: AbstractValue::Constant { value: 0 },
                ..
            } if *offset == entry => zero_writes += 1,
            FunctionRecord::Edge {
                from,
                target: Some(to),
                relation: EdgeKind::Jump | EdgeKind::Indirect,
                external: true,
            } if *from == entry && *to == target => edges += 1,
            FunctionRecord::SemanticGap {
                offset,
                reason: SemanticGapReason::OpaqueCall,
            } if *offset == entry => (),
            FunctionRecord::Transfer { offset, .. } | FunctionRecord::CallInputs { offset, .. }
                if *offset == entry => {}
            FunctionRecord::Expression { .. }
            | FunctionRecord::Block { .. }
            | FunctionRecord::Reference { .. } => (),
            _ => return Ok(None),
        }
    }
    if (instructions, edges, zero_writes) != (1, 1, 1) {
        return Ok(None);
    }
    let Some(inputs) = facts.call_inputs(entry, c)? else {
        return Ok(None);
    };
    if inputs.registers.len() != 32 {
        return Ok(None);
    }
    // Recheck the recorded pre-transfer state as well as architectural writes.
    // Expression ownership is local to this index, never inferred from IDs.
    for (register, value) in inputs.registers.iter().enumerate() {
        c.checkpoint(1)?;
        let unchanged = match (register, value) {
            (0, AbstractValue::Constant { value: 0 })
            | (2, AbstractValue::EntryStack { offset: 0 }) => true,
            (_, AbstractValue::Expression { id }) => matches!(
                facts.expression(*id, c)?,
                Some(Expression::EntryRegister { register: r }) if usize::from(*r) == register
            ),
            _ => false,
        };
        if !unchanged {
            return Ok(None);
        }
    }
    Ok(Some(TailTransfer {
        entry,
        site: entry,
        target,
    }))
}

pub(super) fn compose<'m>(
    facts: &FunctionIndex<'_, '_>,
    out: &mut FunctionWrites<'m>,
    summaries: &[&FunctionWrites<'_>],
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    let Some(tail) = transparent(facts, out.entry, c)? else {
        return Ok(());
    };
    out.tail = Some(tail);
    let summary = find_summary(Some(tail.target), summaries, c)?;
    let blocked = if tail.target == tail.entry {
        Some(WriteBarrierKind::TailCycle)
    } else if let Some(summary) = summary {
        if summary.tail.is_some_and(|t| t.target == tail.entry) {
            Some(WriteBarrierKind::TailCycle)
        } else if summary.tail.is_some() {
            Some(WriteBarrierKind::TailChain)
        } else if out.abi.is_none() || out.abi != summary.abi {
            Some(WriteBarrierKind::UnknownAbi)
        } else {
            None
        }
    } else {
        Some(WriteBarrierKind::UnknownCallee)
    };
    if let Some(kind) = blocked {
        out.barriers.push(
            WriteBarrier {
                origin: WriteOrigin::local(out.entry),
                site: tail.site,
                kind,
            },
            c.position(),
        )?;
        return Ok(());
    }
    let summary = summary.ok_or_else(|| {
        Error::new(
            ErrorCode::Integrity,
            "unblocked tail composition has no target summary",
        )
    })?;
    let arguments = std::array::from_fn(|word| WriteValue::base(WriteBase::Argument(word as u8)));
    let stack = WriteValue::base(WriteBase::Stack {
        function: out.entry,
        offset: 0,
    });
    let translate = |value| substitute(value, &arguments, summary.entry, stack);
    let origin = |source: WriteOrigin| WriteOrigin {
        tail: Some(tail),
        ..source
    };
    // Filter only the two observations of this proved tail instruction. Body
    // barriers are copied afterwards, even if they have the same numerical PC.
    let mut barriers = AdmittedVec::new(memory);
    for barrier in out.barriers.iter() {
        c.checkpoint(1)?;
        if !(barrier.site == tail.site
            && barrier.origin == WriteOrigin::local(out.entry)
            && matches!(
                barrier.kind,
                WriteBarrierKind::Call | WriteBarrierKind::ExternalTransfer
            ))
        {
            barriers.push(*barrier, c.position())?;
        }
    }
    for barrier in summary.barriers.iter() {
        c.checkpoint(1)?;
        barriers.push(
            WriteBarrier {
                origin: origin(barrier.origin),
                ..*barrier
            },
            c.position(),
        )?;
    }
    if !summary.closed_graph {
        barriers.push(
            WriteBarrier {
                origin: origin(WriteOrigin::local(summary.entry)),
                site: summary.entry,
                kind: WriteBarrierKind::OpenGraph,
            },
            c.position(),
        )?;
    }
    for write in summary.writes.iter() {
        c.checkpoint(1)?;
        out.writes.push(
            FunctionWrite {
                origin: origin(write.origin),
                address: translate(write.address),
                value: translate(write.value),
                ..*write
            },
            c.position(),
        )?;
    }
    for reload in summary.reloads.iter() {
        c.checkpoint(1)?;
        out.reloads.push(
            StackReload {
                origin: origin(reload.origin),
                value: translate(reload.value),
                ..*reload
            },
            c.position(),
        )?;
    }
    if let Some(proof) = summary.context.as_ref().filter(|proof| {
        proof.context.entry == summary.entry && Some(proof.context.abi) == summary.abi
    }) {
        let mut branches = AdmittedVec::new(memory);
        for branch in proof.branches.iter() {
            c.checkpoint(1)?;
            branches.push(*branch, c.position())?;
        }
        let mut unreachable = AdmittedVec::new(memory);
        for site in proof.unreachable.iter() {
            c.checkpoint(1)?;
            unreachable.push(*site, c.position())?;
        }
        out.context = Some(ContextProof {
            context: CallContext {
                entry: out.entry,
                ..proof.context
            },
            owner: proof.owner,
            branches,
            unreachable,
        });
    }
    out.barriers = barriers;
    out.closed_graph &= summary.closed_graph;
    if let Some(word) = summary
        .word
        .as_ref()
        .filter(|word| word.context.entry == summary.entry && Some(word.context.abi) == summary.abi)
    {
        let mut events = AdmittedVec::new(memory);
        for event in word.events.iter() {
            c.checkpoint(1)?;
            events.push(
                WordEvent {
                    address: translate(event.address),
                    ..*event
                },
                c.position(),
            )?;
        }
        out.word = Some(WordPostcondition {
            contents: WordContents {
                origin: origin(word.contents.origin),
                destination: translate(word.contents.destination),
                source: translate(word.contents.source),
                ..word.contents
            },
            context: CallContext {
                entry: out.entry,
                ..word.context
            },
            query: word.query,
            extent: word.extent,
            needs_bounds: word.needs_bounds,
            events,
        });
    }
    Ok(())
}
