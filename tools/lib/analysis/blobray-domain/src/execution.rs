//! Concrete execution contracts. Scenarios are explicit inputs, never inferred facts.
use crate::*;

/// Native concrete request format.
pub const EXECUTION_SCHEMA: u32 = 25;
/// Maximum phases in one request; a whole finite matrix fits in one request.
pub const MAX_EXECUTION_CASES: usize = 4096;
/// Maximum recorded events of one execution phase. A bounded poll loop that
/// exhausts a 100,000-operation budget must be observable without truncation.
pub const MAX_EXECUTION_EVENTS: u32 = 1 << 20;
/// Maximum explicitly supplied RV32 ABI words per invocation.
pub const MAX_EXECUTION_ARGUMENT_WORDS: usize = 256;
/// Preloads one invocation may declare.
pub const MAX_MEMORY_PRELOADS: usize = 128;

/// Maximum executables one target maps.
pub const MAX_TARGET_EXECUTABLES: usize = 65;

/// Address space of the static ELF executables named by content, loaded in
/// order; each invocation selects its own entry within the mappings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTarget {
    pub executables: Vec<ArtifactId>,
    pub abi: CallAbi,
    pub stack: MemorySeed,
}
impl ExecutionTarget {
    /// Whether `object` is one of the standalone executables this target maps.
    pub fn maps(&self, object: &ObjectId) -> bool {
        object.location == ObjectLocation::Standalone && self.executables.contains(&object.artifact)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySeed {
    pub address: u32,
    pub length: u32,
    /// None leaves unwritten bytes unknown; never silently zero-filled.
    pub fill: Option<u8>,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterCell {
    pub address: u32,
    pub width: u8,
    pub value: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    /// Exact guest PC in this target's captured address space.
    pub entry: u32,
    pub goal: ExecutionGoal,
    /// Already lowered RV32 integer ABI words: a0..a7, then ascending stack words.
    /// `None` and omitted register words remain unknown, including on a filled stack.
    /// Every other integer register except `ra`, `sp`, `gp` and `tp` starts at zero,
    /// so an isolated root can save callee-saved registers in its prologue.
    /// Clients lower multiword/variadic arguments and insert ABI padding explicitly.
    pub arguments: Vec<Option<u32>>,
    pub memory: Vec<ExecutionRegion>,
    /// Known bytes written, after the phase's memory is mapped and before its
    /// entry, into memory the phase already maps writable: image data,
    /// declared RAM or the stack. A preload is an input, not a guest effect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preload: Vec<MemoryPreload>,
    pub models: Vec<DeviceDeclaration>,
    pub calls: Vec<CallDeclaration>,
    pub observe_memory: Vec<MemorySelection>,
    pub observe_calls: Option<CallCapture>,
    pub observe_timeline: TimelineCapture,
}
/// Known bytes at `address`, preloaded into mapped writable memory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPreload {
    pub address: u32,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCase {
    pub name: String,
    pub reset: SessionReset,
    /// Fill byte of both sides' stack bytes the target's stack seed leaves
    /// unwritten in this case's phases, instead of the targets' own fill.
    /// Explicit seed bytes and argument words still take precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_fill: Option<u8>,
    pub relation: Option<ComparisonRelation>,
    pub vendor: Invocation,
    pub replacement: Option<Invocation>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionReset {
    /// Recreate captured images and discard all previous mutable state/dependencies.
    Cold,
    /// Continue the preceding successful phase with session-owned regions.
    Warm,
}
/// Lifetime of a caller-declared RAM mapping. Images/stack have fixed owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegionLifetime {
    Phase,
    Session,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRegion {
    pub seed: MemorySeed,
    pub lifetime: RegionLifetime,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ExecutionGoal {
    Return,
    /// Stop at a code symbol of an executable the target maps.
    ReachSymbol {
        target: SymbolId,
    },
    /// Stop at transfer, before executing the callee. Ordinary calls use x1/x5;
    /// optionally include x0 tail transfers, excluding canonical ABI returns.
    ObserveCall {
        target: SymbolId,
        include_tail: bool,
    },
}
/// Validated physical boundary supplied by application; backend never resolves symbols.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedExecutionGoal {
    Return,
    ReachSymbol { address: u32 },
    ObserveCall { address: u32, include_tail: bool },
}
#[derive(Clone, Copy, Debug)]
pub struct ExecutionStart {
    pub entry: u32,
    pub stack: u32,
    pub arguments: [Option<u32>; 8],
    pub goal: ResolvedExecutionGoal,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompiledBinding {
    ProductionEntry,
    SharedCore,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRequest {
    pub schema: u32,
    pub vendor: ExecutionTarget,
    pub replacement: Option<ExecutionTarget>,
    pub binding: Option<CompiledBinding>,
    pub cases: Vec<ExecutionCase>,
    /// Hard capacity; exhaustion is a resource failure, not truncated evidence.
    pub max_events: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionEvent {
    Memory {
        site: u32,
        transaction: MemoryTransaction,
    },
    Branch {
        site: u32,
        target: u32,
        fallthrough: u32,
        taken: bool,
    },
    CallTransfer {
        site: u32,
        target: u32,
        tail: bool,
        indirect: bool,
        stack: Option<u32>,
        target_kind: ObservedCallTarget,
        words: u16,
    },
    TransferArgument {
        word: u16,
        value: ObservedWord,
    },
    ModeledCall {
        site: u32,
        target: u32,
        tail: bool,
        response: u32,
        boundary: CallBoundary,
    },
    CallArgument {
        word: u16,
        value: Option<u32>,
    },
    CallReturn {
        words: [Option<u32>; 2],
    },
    CallOutput {
        address: u32,
        width: u8,
        value: u32,
        scope: CallOutputScope,
    },
    Allocation {
        address: u32,
        requested: u32,
        capacity: u32,
        lifetime: RegionLifetime,
    },
    DelayMicros {
        value: u32,
    },
    Read {
        address: u32,
        width: u8,
        value: u32,
    },
    Write {
        address: u32,
        width: u8,
        value: u32,
    },
    Fence {
        predecessor: u8,
        successor: u8,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionStop {
    Returned {
        low: Option<u32>,
        high: Option<u32>,
    },
    ReachedSymbol {
        pc: u32,
    },
    ObservedCall {
        pc: u32,
        target: u32,
        tail: bool,
    },
    /// Entry returned before reaching the requested non-return goal.
    GoalNotReached {
        low: Option<u32>,
        high: Option<u32>,
    },
    Incomplete {
        pc: u32,
        reason: ExecutionGap,
    },
    BlockedByPriorPhase,
}
impl ExecutionStop {
    /// Completion of the declared phase goal, independent of comparison verdict.
    pub fn completed(&self) -> bool {
        matches!(
            self,
            Self::Returned { .. } | Self::ReachedSymbol { .. } | Self::ObservedCall { .. }
        )
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionGap {
    CallModel { target: u32, issue: CallIssue },
    UnsupportedInstruction,
    Memory { address: u32, access: MemoryAccess },
    UnknownRegister { register: u8 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryAccess {
    Fetch,
    Read,
    Write,
    Atomic,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionObservation {
    pub stop: ExecutionStop,
    pub steps: u64,
    pub events: Vec<ExecutionEvent>,
    pub models: Vec<ModelObservation>,
    pub calls: Vec<CallObservation>,
    pub final_memory: Vec<FinalMemoryChunk>,
    /// Persistent bytes written, when the invocation selects
    /// [`TimelineCapture::written`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub written: Vec<WrittenRange>,
}
impl ExecutionObservation {
    /// Goal reached with all environment obligations due at this boundary satisfied.
    pub fn completed(&self) -> bool {
        self.stop.completed()
            && self
                .models
                .iter()
                .all(|m| m.status == m.expected_status() && m.status != ModelStatus::Incomplete)
            && self
                .calls
                .iter()
                .all(|m| m.status == m.expected_status() && m.status != ModelStatus::Incomplete)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ComparisonVerdict {
    Match,
    Diff,
    Incomplete,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseComparison {
    pub effect_claim: Option<EffectClaimCeiling>,
    pub effect_gap: Option<EffectGap>,
    pub verdict: ComparisonVerdict,
    /// First established difference in a selected observation domain.
    pub difference: Option<ComparisonDifference>,
}
/// Session-owned memory. None denotes unknown/inaccessible bytes, never zero.
pub trait ExecutionMemory {
    /// Identify the current instruction independently of progress/reporting state.
    fn instruction(&mut self, pc: u32);
    /// Identify `pc` as the current instruction, then return the halfword at
    /// `pc` and, when fetchable, the next one: exactly the results of
    /// `instruction(pc)` and two halfword `Fetch` reads. Implementations may
    /// answer both halfwords with one lookup.
    fn fetch(
        &mut self,
        pc: u32,
        control: &mut dyn RunControl,
    ) -> Result<(Option<u32>, Option<u32>)> {
        self.instruction(pc);
        let low = self.read(pc, 2, MemoryAccess::Fetch, control)?;
        let high = match (low, pc.checked_add(2)) {
            (Some(_), Some(next)) => self.read(next, 2, MemoryAccess::Fetch, control)?,
            _ => None,
        };
        Ok((low, high))
    }
    /// Observe an eligible transfer before goal completion or dispatch; never execute a model.
    fn observe_call(&mut self, input: &CallInput, control: &mut dyn RunControl) -> Result<()>;
    fn call(&mut self, input: &CallInput, control: &mut dyn RunControl) -> Result<CallDispatch>;
    fn read(
        &mut self,
        address: u32,
        width: u8,
        access: MemoryAccess,
        control: &mut dyn RunControl,
    ) -> Result<Option<u32>>;
    fn write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
        control: &mut dyn RunControl,
    ) -> Result<bool>;
    /// One data load: a known value, unknown bytes (such as a stored unknown
    /// register or uninitialized memory), or an unavailable address.
    fn load(
        &mut self,
        address: u32,
        width: u8,
        control: &mut dyn RunControl,
    ) -> Result<MemoryReadValue> {
        Ok(
            match self.read(address, width, MemoryAccess::Read, control)? {
                Some(value) => MemoryReadValue::Known { value },
                None => MemoryReadValue::Unavailable,
            },
        )
    }
    /// Store `width` bytes of an unknown register, such as uninitialized
    /// padding: the bytes become unknown. A device, which would observe the
    /// value, never accepts one; false is an invalid access.
    fn write_unknown(
        &mut self,
        address: u32,
        width: u8,
        control: &mut dyn RunControl,
    ) -> Result<bool>;
    fn event(&mut self, event: ExecutionEvent, control: &mut dyn RunControl) -> Result<()>;

    /// Read one aligned known word and replace this session's reservation.
    /// Unknown/inaccessible data returns None and leaves no reservation.
    fn load_reserved(
        &mut self,
        address: u32,
        order: ExecutionOrdering,
        control: &mut dyn RunControl,
    ) -> Result<Option<u32>>;
    /// Check store permissions even without a reservation, then conditionally
    /// write a word. Some(false) is a failed reservation, None an invalid access.
    /// Every attempt clears the reservation, including failure.
    fn store_conditional(
        &mut self,
        address: u32,
        value: u32,
        order: ExecutionOrdering,
        control: &mut dyn RunControl,
    ) -> Result<Option<bool>>;
    /// One indivisible read/update/write of a known aligned writable word.
    /// Calls `update` exactly once on success, never on invalid/unknown memory.
    /// Returns the old word and invalidates overlapping reservations. The callback
    /// supplies pure ISA arithmetic; it must not acquire resources or call memory.
    fn modify_word(
        &mut self,
        address: u32,
        order: ExecutionOrdering,
        update: &mut dyn FnMut(u32) -> u32,
        control: &mut dyn RunControl,
    ) -> Result<Option<u32>>;
}
/// Ordering requested by the guest atomic instruction. A single-hart environment
/// executes memory operations in program order; this is not a concurrency model.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionOrdering {
    pub acquire: bool,
    pub release: bool,
}
/// Concrete ISA capability; no input selection, repository, models or verdict authority.
pub trait Executor {
    fn identity(&self) -> &'static str;
    fn execute(
        &self,
        start: &ExecutionStart,
        memory: &mut dyn ExecutionMemory,
        control: &mut dyn RunControl,
    ) -> Result<(ExecutionStop, u64)>;
}
/// An invalid execution request that names the violated condition.
fn require(condition: bool, detail: impl FnOnce() -> String) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            format!("invalid execution request: {}", detail()),
        ))
    }
}
impl ExecutionRequest {
    pub fn validate(&self) -> Result<()> {
        require(self.schema == EXECUTION_SCHEMA, || {
            format!("schema {} is not {EXECUTION_SCHEMA}", self.schema)
        })?;
        require(!self.cases.is_empty(), || "no cases".into())?;
        require(self.cases.len() <= MAX_EXECUTION_CASES, || {
            format!("{} cases exceed {MAX_EXECUTION_CASES}", self.cases.len())
        })?;
        require(self.cases[0].reset == SessionReset::Cold, || {
            format!(
                "the first case `{}` is not a cold reset",
                self.cases[0].name
            )
        })?;
        require(
            self.max_events != 0 && self.max_events <= MAX_EXECUTION_EVENTS,
            || {
                format!(
                    "max_events {} is outside 1..={MAX_EXECUTION_EVENTS}",
                    self.max_events
                )
            },
        )?;
        require(self.replacement.is_some() == self.binding.is_some(), || {
            "a replacement target and its binding must be given together".into()
        })?;
        for (side, target) in [
            ("vendor", Some(&self.vendor)),
            ("replacement", self.replacement.as_ref()),
        ] {
            let Some(target) = target else { continue };
            require(
                (1..=MAX_TARGET_EXECUTABLES).contains(&target.executables.len()),
                || {
                    format!(
                        "the {side} target maps {} executables, outside 1..={MAX_TARGET_EXECUTABLES}",
                        target.executables.len()
                    )
                },
            )?;
            require(
                target
                    .executables
                    .iter()
                    .enumerate()
                    .all(|(i, id)| !target.executables[..i].contains(id)),
                || format!("the {side} target maps an executable twice"),
            )?;
            target.stack.validate()?;
            require(
                target.stack.length >= 16
                    && (u64::from(target.stack.address) + u64::from(target.stack.length)) % 16 == 0,
                || {
                    format!(
                        "the {side} stack is shorter than 16 bytes or its top is not 16-byte aligned"
                    )
                },
            )?;
        }
        for case in &self.cases {
            let name = &case.name;
            require(!name.is_empty() && name.len() <= 256, || {
                format!("case name `{name}` is empty or longer than 256 bytes")
            })?;
            require(
                case.replacement.is_some() == self.replacement.is_some(),
                || format!("case `{name}` and the request disagree on a replacement side"),
            )?;
            match (&case.relation, &case.replacement) {
                (Some(relation), Some(other)) => relation.validate(&case.vendor, other)?,
                (None, None) => {}
                (Some(_), None) => {
                    return require(false, || {
                        format!("case `{name}` has a relation but no replacement to compare")
                    });
                }
                // A paired case without a relation is setup: both sides run
                // and nothing of it is compared, observed or claimed.
                (None, Some(_)) => {}
            }
            // A vendor symbol goal may pair with a replacement return: a
            // prefix comparison of every vendor effect before the boundary
            // with the complete replacement, which compares no return or call.
            if let Some(other) = &case.replacement {
                require(
                    std::mem::discriminant(&case.vendor.goal)
                        == std::mem::discriminant(&other.goal)
                        || (matches!(
                            case.vendor.goal,
                            ExecutionGoal::ObserveCall { .. } | ExecutionGoal::ReachSymbol { .. }
                        ) && other.goal == ExecutionGoal::Return
                            && case
                                .relation
                                .as_ref()
                                .is_some_and(|r| !r.returns.low && !r.returns.high && !r.calls)),
                    || format!("case `{name}` pairs goals that cannot be compared"),
                )?;
            }
            for (side, input, target) in std::iter::once(("vendor", &case.vendor, &self.vendor))
                .chain(
                    case.replacement
                        .as_ref()
                        .zip(self.replacement.as_ref())
                        .map(|(input, target)| ("replacement", input, target)),
                )
            {
                require(input.entry & 1 == 0 && input.entry < u32::MAX - 1, || {
                    format!(
                        "case `{name}` {side} entry {:#x} is odd or out of range",
                        input.entry
                    )
                })?;
                match &input.goal {
                    ExecutionGoal::Return => {}
                    ExecutionGoal::ReachSymbol { target: symbol }
                    | ExecutionGoal::ObserveCall { target: symbol, .. } => {
                        require(target.maps(&symbol.object), || {
                            format!(
                                "case `{name}` {side} goal symbol is not in an executable of the target"
                            )
                        })?;
                    }
                }
                input.entry_stack(&target.stack)?;
                input.validate_memory_selection().map_err(|e| {
                    Error::new(e.code, format!("case `{name}` {side}: {}", e.message))
                })?;
                if let Some(capture) = &input.observe_calls {
                    capture.validate()?;
                }
                require(input.memory.len() <= 128, || {
                    format!(
                        "case `{name}` {side} declares {} memory regions, more than 128",
                        input.memory.len()
                    )
                })?;
                require(input.models.len() <= MAX_DEVICE_MODELS, || {
                    format!(
                        "case `{name}` {side} declares {} device models, more than {MAX_DEVICE_MODELS}",
                        input.models.len()
                    )
                })?;
                for (index, region) in input.memory.iter().enumerate() {
                    let seed = &region.seed;
                    seed.validate()?;
                    if let Some(other) = input.memory[..index].iter().find(|r| {
                        let s = &r.seed;
                        u64::from(s.address) < u64::from(seed.address) + u64::from(seed.length)
                            && u64::from(seed.address) < u64::from(s.address) + u64::from(s.length)
                    }) {
                        return require(false, || {
                            format!(
                                "case `{name}` {side} memory regions {:#x}+{:#x} and {:#x}+{:#x} overlap",
                                other.seed.address, other.seed.length, seed.address, seed.length
                            )
                        });
                    }
                }
                require(input.preload.len() <= MAX_MEMORY_PRELOADS, || {
                    format!(
                        "case `{name}` {side} declares {} preloads, more than {MAX_MEMORY_PRELOADS}",
                        input.preload.len()
                    )
                })?;
                for preload in &input.preload {
                    require(
                        !preload.bytes.is_empty()
                            && u64::from(preload.address) + (preload.bytes.len() as u64)
                                < u64::from(u32::MAX),
                        || {
                            format!(
                                "case `{name}` {side} preload at {:#x} of {} bytes is empty or out of range",
                                preload.address,
                                preload.bytes.len()
                            )
                        },
                    )?;
                }
                require(input.calls.len() <= MAX_CALL_MODELS, || {
                    format!("case `{name}` {side} has more than {MAX_CALL_MODELS} call models")
                })?;
                for (index, call) in input.calls.iter().enumerate() {
                    call.validate()?;
                    require(
                        !input.calls[..index]
                            .iter()
                            .any(|m| m.id == call.id || m.binding.address == call.binding.address),
                        || {
                            format!(
                                "case `{name}` {side} repeats call model `{}` or its address {:#x}",
                                call.id, call.binding.address
                            )
                        },
                    )?;
                }
                for (index, model) in input.models.iter().enumerate() {
                    model.validate()?;
                    require(
                        !input.models[..index].iter().any(|m| m.id == model.id),
                        || format!("case `{name}` {side} repeats device model `{}`", model.id),
                    )?;
                }
            }
        }
        Ok(())
    }
}
impl Invocation {
    /// Initial integer argument registers; absent words are unknown, never zero.
    pub fn register_arguments(&self) -> [Option<u32>; 8] {
        let mut registers = [None; 8];
        for (dst, src) in registers.iter_mut().zip(&self.arguments) {
            *dst = *src;
        }
        registers
    }

    /// Validate the argument/stack geometry and return the aligned entry SP.
    /// Stack words occupy an upward-growing prefix at SP of a 16-byte rounded
    /// area at the top of the declared stack. Remaining bytes below SP are the
    /// callee's stack capacity. No minimum callee frame size is inferred.
    pub fn entry_stack(&self, stack: &MemorySeed) -> Result<u32> {
        stack.validate()?;
        if self.arguments.len() > MAX_EXECUTION_ARGUMENT_WORDS {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "too many execution argument words",
            ));
        }
        let reserved = (self.arguments.len().saturating_sub(8) as u32 * 4).next_multiple_of(16);
        let top = stack.address + stack.length; // MemorySeed validates checked range.
        if stack.length < 16 || !top.is_multiple_of(16) || reserved > stack.length {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "execution arguments do not fit aligned stack",
            ));
        }
        Ok(top - reserved)
    }
}
impl MemorySeed {
    pub fn validate(&self) -> Result<()> {
        if self.length == 0
            || self.bytes.len() as u64 > u64::from(self.length)
            || u64::from(self.address) + u64::from(self.length) >= u64::from(u32::MAX)
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "invalid memory seed range",
            ));
        }
        Ok(())
    }
}

/// Streaming evidence; each record is bounded independently of trace length.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ExecutionEvidence {
    FinalMemory {
        case: u32,
        replacement: bool,
        chunk: FinalMemoryChunk,
    },
    CallModel {
        case: u32,
        replacement: bool,
        observation: CallObservation,
    },
    Model {
        case: u32,
        replacement: bool,
        observation: ModelObservation,
    },
    Event {
        case: u32,
        replacement: bool,
        event: ExecutionEvent,
    },
    /// One persistent byte range the phase wrote, after its final memory.
    Written {
        case: u32,
        replacement: bool,
        range: WrittenRange,
    },
    Outcome {
        case: u32,
        replacement: bool,
        stop: ExecutionStop,
        steps: u64,
    },
    Comparison {
        case: u32,
        result: CaseComparison,
    },
    /// After the last case, the vendor side's records and then the
    /// replacement's: code that side reached over the whole execution, split
    /// into ascending, disjoint address ranges that each fit one record.
    Coverage {
        replacement: bool,
        coverage: ExecutionCoverage,
    },
}
/// Executed instructions in one coverage record; with the branch bound, one
/// record stays within a control message.
pub const MAX_COVERAGE_RECORD_INSTRUCTIONS: usize = 1536;
/// Branches in one coverage record.
pub const MAX_COVERAGE_RECORD_BRANCHES: usize = 512;
/// Indirect transfers in one coverage record.
pub const MAX_COVERAGE_RECORD_TRANSFERS: usize = 256;

/// Code one side reached in its executable captured segments over every phase
/// of an execution. Execution from writable RAM is not captured code.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCoverage {
    /// Addresses of executed instructions, strictly ascending.
    pub instructions: Vec<u32>,
    /// Conditional branches with the directions they took, ascending by site.
    pub branches: Vec<BranchCoverage>,
    /// Distinct targets of executed indirect calls and jumps (not returns),
    /// ascending by site and target.
    pub transfers: Vec<IndirectTransfer>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndirectTransfer {
    pub site: u32,
    pub target: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchCoverage {
    pub site: u32,
    pub taken: bool,
    pub fallthrough: bool,
}
impl ExecutionCoverage {
    /// Ordered, unique and self-consistent: every observed branch direction
    /// belongs to an executed instruction.
    pub fn validate(&self, c: &mut dyn RunControl) -> Result<()> {
        let invalid = || Error::new(ErrorCode::Integrity, "invalid execution coverage");
        c.checkpoint(
            (self.instructions.len() + self.branches.len() + self.transfers.len()) as u64 / 1024
                + 1,
        )?;
        if self.instructions.len() > MAX_COVERAGE_RECORD_INSTRUCTIONS
            || self.branches.len() > MAX_COVERAGE_RECORD_BRANCHES
            || self.transfers.len() > MAX_COVERAGE_RECORD_TRANSFERS
            || self.transfers.windows(2).any(|w| w[0] >= w[1])
            || self
                .transfers
                .iter()
                .any(|t| self.instructions.binary_search(&t.site).is_err())
            || self.instructions.iter().any(|pc| pc & 1 != 0)
            || self.instructions.windows(2).any(|w| w[0] >= w[1])
            || self.branches.windows(2).any(|w| w[0].site >= w[1].site)
            || self.branches.iter().any(|b| {
                !(b.taken || b.fallthrough) || self.instructions.binary_search(&b.site).is_err()
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
    /// Split into ascending records within the record bounds. Each record
    /// holds the branches of its own instructions; empty coverage stays one
    /// empty record.
    pub fn split(self) -> Vec<ExecutionCoverage> {
        let mut records = Vec::new();
        let mut branches = self.branches.into_iter().peekable();
        let mut transfers = self.transfers.into_iter().peekable();
        let mut current = ExecutionCoverage::default();
        for pc in self.instructions {
            let branch = branches.next_if(|b| b.site == pc);
            let mut targets = Vec::new();
            while let Some(transfer) = transfers.next_if(|t| t.site == pc) {
                targets.push(transfer);
            }
            if current.instructions.len() == MAX_COVERAGE_RECORD_INSTRUCTIONS
                || (branch.is_some() && current.branches.len() == MAX_COVERAGE_RECORD_BRANCHES)
                || current.transfers.len() + targets.len() > MAX_COVERAGE_RECORD_TRANSFERS
            {
                records.push(std::mem::take(&mut current));
            }
            current.instructions.push(pc);
            current.branches.extend(branch);
            current.transfers.extend(targets);
        }
        if records.is_empty() || !current.instructions.is_empty() {
            records.push(current);
        }
        records
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    #[test]
    fn coverage_splits_into_bounded_ascending_records_that_keep_branches_with_their_sites() {
        let instructions: Vec<u32> = (0..MAX_COVERAGE_RECORD_INSTRUCTIONS as u32 + 3)
            .map(|i| 0x1000 + 4 * i)
            .collect();
        // More branches than one record holds, all before the instruction bound.
        let branches: Vec<BranchCoverage> = instructions[..MAX_COVERAGE_RECORD_BRANCHES + 1]
            .iter()
            .map(|site| BranchCoverage {
                site: *site,
                taken: true,
                fallthrough: false,
            })
            .collect();
        let whole = ExecutionCoverage {
            instructions: instructions.clone(),
            branches: branches.clone(),
            transfers: vec![],
        };
        let parts = whole.split();
        assert!(parts.len() >= 2);
        let mut last = None;
        for part in &parts {
            part.validate(&mut || Ok(())).unwrap();
            assert!(last.is_none_or(|l| part.instructions[0] > l));
            last = part.instructions.last().copied();
        }
        let rejoined: Vec<u32> = parts.iter().flat_map(|p| p.instructions.clone()).collect();
        assert_eq!(rejoined, instructions);
        let rejoined: Vec<_> = parts.iter().flat_map(|p| p.branches.clone()).collect();
        assert_eq!(rejoined, branches);
        assert_eq!(
            ExecutionCoverage::default().split(),
            vec![ExecutionCoverage::default()]
        );
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    fn invocation() -> Invocation {
        Invocation {
            observe_calls: None,
            observe_timeline: TimelineCapture::default(),
            observe_memory: vec![],
            goal: ExecutionGoal::Return,
            entry: 0x1000,
            arguments: vec![Some(0); 8],
            memory: vec![],
            preload: vec![],
            models: vec![],
            calls: vec![],
        }
    }

    fn request() -> ExecutionRequest {
        let target = ExecutionTarget {
            executables: vec![ArtifactId::of_bytes(b"validation fixture")],
            abi: CallAbi::RiscvInteger,
            stack: MemorySeed {
                address: 0x8000,
                length: 4096,
                fill: None,
                bytes: vec![],
            },
        };
        ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: target.clone(),
            replacement: Some(target),
            binding: Some(CompiledBinding::SharedCore),
            cases: vec![ExecutionCase {
                relation: Some(ComparisonRelation {
                    effects: None,
                    projection: None,
                    calls: false,
                    returns: ReturnWords {
                        low: true,
                        high: false,
                    },
                    events: EventChannels {
                        timeline: TimelineCapture::default(),
                        mmio_read: true,
                        mmio_write: true,
                        fence: true,
                        delay: true,
                    },
                    memory: vec![],
                }),
                reset: SessionReset::Cold,
                stack_fill: None,
                name: "case".into(),
                vendor: invocation(),
                replacement: Some(invocation()),
            }],
            max_events: 16,
        }
    }

    fn rejected(request: ExecutionRequest) -> String {
        let error = request.validate().expect_err("the request is invalid");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        error.message
    }

    fn region(address: u32, length: u32) -> ExecutionRegion {
        ExecutionRegion {
            seed: MemorySeed {
                address,
                length,
                fill: None,
                bytes: vec![],
            },
            lifetime: RegionLifetime::Phase,
        }
    }

    fn selection(name: &str, address: u32, length: u32) -> MemorySelection {
        MemorySelection {
            name: name.into(),
            address,
            length,
        }
    }

    #[test]
    fn the_fixture_is_valid() {
        request().validate().unwrap();
    }

    #[test]
    fn every_request_error_names_its_condition() {
        let mut r = request();
        r.cases[0].reset = SessionReset::Warm;
        assert!(rejected(r).contains("first case `case` is not a cold reset"));

        let mut r = request();
        r.max_events = 0;
        assert!(rejected(r).contains("max_events 0"));

        let mut r = request();
        r.binding = None;
        assert!(rejected(r).contains("binding must be given together"));

        // A paired case without a relation is a valid setup case.
        let mut r = request();
        r.cases[0].relation = None;
        r.validate().unwrap();

        let mut r = request();
        r.cases[0].vendor.entry = 0x1001;
        assert!(rejected(r).contains("vendor entry 0x1001 is odd"));

        let mut r = request();
        r.cases[0].replacement.as_mut().unwrap().memory =
            vec![region(0x3000, 0x20), region(0x3010, 0x20)];
        assert!(
            rejected(r).contains("replacement memory regions 0x3000+0x20 and 0x3010+0x20 overlap")
        );

        let mut r = request();
        r.cases[0].vendor.observe_memory =
            vec![selection("a", 0x3000, 4), selection("a", 0x3010, 4)];
        let message = rejected(r);
        assert!(message.contains("case `case` vendor"), "{message}");
        assert!(message.contains("selection name `a` repeats"), "{message}");

        let mut r = request();
        r.cases[0].vendor.observe_memory =
            vec![selection("a", 0x3000, 8), selection("b", 0x3004, 4)];
        assert!(rejected(r).contains("`a` at 0x3000+0x8 and `b` at 0x3004+0x4 overlap"));
    }

    #[test]
    fn every_relation_error_names_its_condition() {
        let mut r = request();
        r.cases[0].relation.as_mut().unwrap().returns.low = false;
        r.cases[0].relation.as_mut().unwrap().events = EventChannels {
            timeline: TimelineCapture::default(),
            mmio_read: false,
            mmio_write: false,
            fence: false,
            delay: false,
        };
        assert!(rejected(r).contains("it compares nothing"));

        let mut r = request();
        r.cases[0].relation.as_mut().unwrap().memory = vec![MemoryPair {
            vendor: 0,
            replacement: 0,
        }];
        assert!(rejected(r).contains("memory pair 0 names selection 0 / 0"));

        let mut r = request();
        r.cases[0].vendor.observe_memory = vec![selection("v", 0x3000, 4)];
        r.cases[0].replacement.as_mut().unwrap().observe_memory = vec![selection("p", 0x3000, 8)];
        r.cases[0].relation.as_mut().unwrap().memory = vec![MemoryPair {
            vendor: 0,
            replacement: 0,
        }];
        assert!(rejected(r).contains("pairs `v` (4 bytes) with `p` (8 bytes)"));

        let mut r = request();
        r.cases[0].vendor.goal = ExecutionGoal::ReachSymbol {
            target: SymbolId {
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(b"goal object"),
                    location: ObjectLocation::Standalone,
                },
                table: SymbolTableKind::Static,
                table_section: 1,
                index: 1,
            },
        };
        assert!(rejected(r).contains("compared returns need a return goal on both sides"));
    }
}
