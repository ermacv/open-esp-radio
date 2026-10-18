//! Replacement step log for observation dependence.
//!
//! A session records, on request, what dependence analysis needs beyond the
//! instruction semantics: each fetched instruction, the memory each step
//! accessed, the events it emitted, call-model results and memory the
//! environment defined. The executor is unchanged; the log is taken from the
//! memory port it already uses.
use crate::*;

/// One entry of a step log. Entries after an `Instruction` belong to that step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StepEntry {
    /// A phase starts: every register holds an input.
    Phase,
    /// The environment defined `length` bytes at `address` from inputs:
    /// stack, seeded or allocated memory. `transient` memory ends with the
    /// phase and is no state of the session.
    Input {
        address: u32,
        length: u32,
        transient: bool,
    },
    /// A step starts by fetching the instruction at `pc`.
    Instruction { pc: u32 },
    /// The step read guest memory. Device reads return model values.
    Read {
        address: u32,
        width: u8,
        device: bool,
    },
    /// The step wrote guest memory.
    Write {
        address: u32,
        width: u8,
        device: bool,
    },
    /// The step emitted the event at `index` of its phase's events.
    Event { index: u32 },
    /// A call model returned at the step: caller-saved registers are its results.
    CallReturn,
    /// A call model wrote `width` bytes at `address` at the step.
    Output { address: u32, width: u8 },
    /// A case ended; the n-th case end has the log's n-th sinks.
    CaseEnd,
}

/// Compared observations of one case, as dependence sinks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CaseSinks {
    /// Events whose emitting step is observed.
    pub events: Vec<u32>,
    /// Events that observe a register at their emitting step, such as call
    /// argument words.
    pub registers: Vec<(u32, u8)>,
    /// Compared return words, low then high.
    pub returns: [bool; 2],
    /// Compared final memory as `[address, length]`.
    pub memory: Vec<[u32; 2]>,
    /// Whether the case returned: reaching the goal is compared, so the
    /// phase's last step is observed.
    pub goal: bool,
    /// Every call argument event with its register, compared or not.
    pub arguments: Vec<(u32, u8)>,
}

impl CaseSinks {
    /// Sinks of the observations of `right` that `compared` lists. Call
    /// argument words beyond the registers are not followed.
    pub fn of(
        compared: &blobray_verification::ComparedObservations,
        right: &ExecutionObservation,
    ) -> Self {
        let mut sinks = Self::default();
        for (index, event) in right.events.iter().enumerate() {
            if let ExecutionEvent::TransferArgument { word, .. } = event
                && let Ok(word) = u8::try_from(*word)
                && word < 8
            {
                sinks.arguments.push((index as u32, 10 + word));
            }
        }
        for &index in &compared.events {
            match right.events.get(index as usize) {
                Some(ExecutionEvent::TransferArgument { word, .. }) => {
                    if let Ok(word) = u8::try_from(*word)
                        && word < 8
                    {
                        sinks.registers.push((index, 10 + word));
                    }
                }
                Some(_) => sinks.events.push(index),
                None => {}
            }
        }
        if matches!(right.stop, ExecutionStop::Returned { .. }) {
            sinks.returns = compared.returns;
            sinks.goal = true;
        }
        sinks.memory = compared.memory.clone();
        sinks
    }
}

/// Entries admitted by the first growth of a log.
const INITIAL_ENTRIES: usize = 1 << 16;

/// The step log of one session and the sinks of its cases.
pub(crate) struct StepLog<'a> {
    pub entries: Vec<StepEntry>,
    pub sinks: Vec<CaseSinks>,
    memory: &'a WorkingMemory,
    _capacity: Option<MemoryReservation<'a>>,
}

impl<'a> StepLog<'a> {
    pub fn new(memory: &'a WorkingMemory) -> Self {
        Self {
            entries: Vec::new(),
            sinks: Vec::new(),
            memory,
            _capacity: None,
        }
    }

    /// Append `entry`, admitting doubled capacity before allocating it.
    #[inline]
    pub fn push(&mut self, entry: StepEntry, c: &mut dyn RunControl) -> Result<()> {
        if self.entries.len() == self.entries.capacity() {
            self.grow(c)?;
        }
        self.entries.push(entry);
        Ok(())
    }

    #[cold]
    fn grow(&mut self, c: &mut dyn RunControl) -> Result<()> {
        let count = (self.entries.capacity() * 2).max(INITIAL_ENTRIES);
        let reservation = self.memory.reserve(
            (count * std::mem::size_of::<StepEntry>()) as u64,
            c.position(),
        )?;
        self.entries
            .try_reserve_exact(count - self.entries.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "step log allocation refused"))?;
        self._capacity = Some(reservation);
        Ok(())
    }

    /// End the current case with its sinks.
    pub fn end_case(&mut self, sinks: CaseSinks, c: &mut dyn RunControl) -> Result<()> {
        self.push(StepEntry::CaseEnd, c)?;
        self.sinks.push(sinks);
        Ok(())
    }
}
