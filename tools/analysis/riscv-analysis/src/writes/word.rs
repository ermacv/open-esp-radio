//! One word after normal return, separately from conservative write events.
//! Private certificates are built by the common evaluator over its existing CFG.
use super::*;
use crate::FunctionInput;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WordQuery {
    pub destination: u8,
    pub source: u8,
    pub length: u8,
    pub offset: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WordContents {
    pub origin: WriteOrigin,
    pub destination: WriteValue,
    pub source: WriteValue,
    pub load: u64,
    pub load_order: u32,
    pub store: u64,
    pub store_order: u32,
    pub width: u8,
    /// A separate all-path proof; false does not invalidate load-time provenance.
    pub source_unchanged_before_load: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordEventKind {
    Load,
    Store,
    Return,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WordEvent {
    pub owner: u64,
    pub path: u32,
    pub order: u32,
    pub site: u64,
    pub kind: WordEventKind,
    pub width: u8,
    pub address: WriteValue,
    pub read: Option<(u64, u32, u8)>,
}
pub struct WordPostcondition<'m> {
    pub(crate) contents: WordContents,
    pub(crate) context: CallContext,
    pub(crate) query: WordQuery,
    pub(crate) extent: u32,
    pub(crate) needs_bounds: bool,
    pub(crate) events: AdmittedVec<'m, WordEvent>,
}
impl std::fmt::Debug for WordPostcondition<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WordPostcondition")
            .field("contents", &self.contents)
            .field("context", &self.context)
            .field("query", &self.query)
            .field("extent", &self.extent)
            .field("needs_bounds", &self.needs_bounds)
            .field("destination_alignment", &self.destination_alignment())
            .field("destination_bounds", &self.destination_bounds())
            .finish()
    }
}
impl WordPostcondition<'_> {
    pub fn events(&self) -> &[WordEvent] {
        &self.events
    }
    pub fn destination_alignment(&self) -> u32 {
        4
    }
    pub fn contents(&self) -> &WordContents {
        &self.contents
    }
    pub fn query(&self) -> WordQuery {
        self.query
    }
    pub fn extent(&self) -> u32 {
        self.extent
    }
    /// The unsigned destination interval required by comparisons in the proof.
    /// A relative stack coordinate by itself does NOT discharge this premise.
    pub fn destination_bounds(&self) -> Option<(u32, u32)> {
        self.needs_bounds.then_some((0, u32::MAX - self.extent))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordBarrier {
    Target,
    Abi,
    Context,
    Alignment,
    DestinationBounds,
    MissingProof,
}

#[derive(Clone, Copy, Debug)]
pub struct CallWord {
    pub caller: u64,
    pub site: u64,
    pub callee: u64,
    pub contents: WordContents,
    pub(crate) arguments: [Option<WriteValue>; 8],
}

/// Argument substitution is visible even when a premise is missing. Only
/// `into_checked` can produce the application used by memory forwarding.
#[derive(Debug)]
pub struct WordInstance {
    word: CallWord,
    barrier: Option<WordBarrier>,
}
impl WordInstance {
    pub fn caller(&self) -> u64 {
        self.word.caller
    }
    pub fn site(&self) -> u64 {
        self.word.site
    }
    pub fn contents(&self) -> &WordContents {
        &self.word.contents
    }
    pub fn barrier(&self) -> Option<WordBarrier> {
        self.barrier
    }
    pub fn into_checked(self) -> std::result::Result<CallWord, WordBarrier> {
        match self.barrier {
            Some(barrier) => Err(barrier),
            None => Ok(self.word),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn call_word(
    caller: &FunctionIndex<'_, '_>,
    entry: u64,
    site: u64,
    abi: Option<CallAbi>,
    summary: &FunctionWrites<'_>,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<std::result::Result<CallWord, WordBarrier>> {
    Ok(
        instantiate_word(caller, entry, site, abi, summary, memory, c)?
            .and_then(WordInstance::into_checked),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn instantiate_word(
    caller: &FunctionIndex<'_, '_>,
    entry: u64,
    site: u64,
    abi: Option<CallAbi>,
    summary: &FunctionWrites<'_>,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<std::result::Result<WordInstance, WordBarrier>> {
    let Some(transfer) = caller.transfer(site, c)? else {
        return Ok(Err(WordBarrier::Target));
    };
    if !transfer.call
        || !matches!(transfer.target, AbstractValue::ImageAddress { address } if u64::from(*address) == summary.entry)
    {
        return Ok(Err(WordBarrier::Target));
    }
    if abi.is_none() || abi != summary.abi {
        return Ok(Err(WordBarrier::Abi));
    }
    let Some(proof) = &summary.word else {
        return Ok(Err(WordBarrier::MissingProof));
    };
    let mut closed = summary.closed_graph;
    for record in caller.records() {
        c.checkpoint(1)?;
        closed &= closes_local_graph(record);
    }
    if !closed {
        return Ok(Err(WordBarrier::MissingProof));
    }
    let mut values = Values::new(entry, abi, true, memory);
    values.prepare(caller, c)?;
    let Some(inputs) = caller.call_inputs(site, c)? else {
        return Ok(Err(WordBarrier::Context));
    };
    let arguments = std::array::from_fn(|word| {
        inputs
            .registers
            .get(word + 10)
            .map_or(WriteValue::UNKNOWN, |v| values.value(v))
    });
    if !proof.context.applies(summary.entry, abi, &arguments) {
        return Ok(Err(WordBarrier::Context));
    }
    let destination = arguments[usize::from(proof.query.destination)];
    let aligned = match destination {
        WriteValue::Base {
            base: WriteBase::Address(address),
            offset,
        } => address.wrapping_add(offset) % 4 == 0,
        WriteValue::Base {
            base: WriteBase::Stack { offset, .. },
            offset: delta,
        } => {
            abi.is_some_and(|abi| abi.stack_alignment() % 4 == 0)
                && offset.wrapping_add(i64::from(delta as i32)) % 4 == 0
        }
        _ => false,
    };
    // The selected ABI's stack alignment is 16. No numerical bounds follow.
    let barrier = if !aligned {
        Some(WordBarrier::Alignment)
    } else if proof.needs_bounds
        && !destination
            .constant()
            .is_some_and(|v| v <= u32::MAX - proof.extent)
    {
        Some(WordBarrier::DestinationBounds)
    } else {
        None
    };
    let stack = inputs
        .registers
        .get(2)
        .map_or(WriteValue::UNKNOWN, |v| values.value(v));
    let mut required =
        std::array::from_fn(|word| proof.context.arguments[word].map(|_| arguments[word]));
    required[usize::from(proof.query.destination)] = Some(destination);
    required[usize::from(proof.query.source)] = Some(arguments[usize::from(proof.query.source)]);
    Ok(Ok(WordInstance {
        word: CallWord {
            caller: entry,
            site,
            callee: summary.entry,
            contents: WordContents {
                destination: substitute(
                    proof.contents.destination,
                    &arguments,
                    summary.entry,
                    stack,
                ),
                source: substitute(proof.contents.source, &arguments, summary.entry, stack),
                ..proof.contents
            },
            arguments: required,
        },
        barrier,
    }))
}

#[allow(clippy::too_many_arguments)]
pub fn contextual_word_function_writes<'m>(
    input: FunctionInput<'_>,
    isa: &dyn FunctionSemantics,
    context: &CallContext,
    query: WordQuery,
    summaries: &[&FunctionWrites<'_>],
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<FunctionWrites<'m>> {
    context::specialized(input, isa, context, Some(query), summaries, memory, c)
}
