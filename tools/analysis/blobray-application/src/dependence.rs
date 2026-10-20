//! Dependence of compared observations on executed replacement instructions.
//!
//! A forward pass over a session's step log gives every executed step the
//! steps it depends on: the producers of the registers and memory bytes it
//! reads, and the conditional branch it is control dependent on. A branch
//! controls the steps of its frame, and of the calls they make, until its
//! immediate post-dominator in the statically recovered CFG of the executing
//! function. A backward walk from the steps that produced compared
//! observations then marks the observed steps. Instruction semantics come
//! from the ISA-neutral `FunctionSemantics`; the log supplies addresses,
//! events and call-model results.
use crate::execution_steps::{CaseSinks, StepEntry, StepLog};
use crate::*;
use std::collections::BTreeSet;
use std::hash::{BuildHasherDefault, Hasher};

/// Multiplicative hash of guest addresses and step numbers; the analysis
/// hashes only its own `u32` keys, so no collision resistance is needed.
#[derive(Default)]
struct AddressHasher(u64);
impl Hasher for AddressHasher {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_u32(u32::from(*byte));
        }
    }
    fn write_u32(&mut self, value: u32) {
        self.0 = (self.0 ^ u64::from(value)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    fn finish(&self) -> u64 {
        // The product's high bits are well mixed; fold them into the low
        // bits that select buckets, keeping the high bits for tags.
        self.0 ^ (self.0 >> 29)
    }
}
type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<AddressHasher>>;

/// No producer: an input of the session or phase.
const INPUT: u32 = u32::MAX;
/// The calling convention dependence assumes: a call clobbers its
/// registers and returns through its links, and the argument words travel
/// in its argument registers.
const ABI: oer_analysis_riscv_model::CallAbi = oer_analysis_riscv_model::CallAbi::RiscvInteger;
/// Argument words passed in registers.
const REGISTER_ARGUMENTS: usize = ABI.arguments().len();
/// Instructions of one function's recovered CFG; a larger function gets no
/// post-dominators, so its branches control the rest of their frame.
const MAX_FUNCTION_INSTRUCTIONS: usize = 1 << 16;
/// Dependencies of one step above which deduplication sorts instead of
/// comparing each against those already kept.
const LINEAR_DEDUP: usize = 32;
/// Working memory admitted per logged entry for the dependence graph.
const ENTRY_COST: u64 = 48;
/// Bytes of one entry of the byte producer table.
const WORD_BYTES: u32 = 4;

/// The producing step of every guest byte a step of the phase defined, kept
/// by aligned word so an access hashes once per word, not once per byte.
#[derive(Default)]
struct Producers {
    words: HashMap<u32, [u32; WORD_BYTES as usize]>,
}

impl Producers {
    fn get(&self, byte: u32) -> Option<u32> {
        let step = self.words.get(&(byte / WORD_BYTES))?[(byte % WORD_BYTES) as usize];
        (step != INPUT).then_some(step)
    }
    /// Producers of the `width` bytes at `address`.
    fn read(&self, address: u32, width: u32, mut found: impl FnMut(u32)) {
        let mut byte = address;
        let mut left = width;
        while left > 0 {
            let (word, lane) = (byte / WORD_BYTES, byte % WORD_BYTES);
            let span = (WORD_BYTES - lane).min(left);
            if let Some(steps) = self.words.get(&word) {
                for step in &steps[lane as usize..(lane + span) as usize] {
                    if *step != INPUT {
                        found(*step);
                    }
                }
            }
            byte = byte.wrapping_add(span);
            left -= span;
        }
    }
    /// `step` produced the `width` bytes at `address`.
    fn write(&mut self, address: u32, width: u32, step: u32) {
        let mut byte = address;
        let mut left = width;
        while left > 0 {
            let (word, lane) = (byte / WORD_BYTES, byte % WORD_BYTES);
            let span = (WORD_BYTES - lane).min(left);
            self.words
                .entry(word)
                .or_insert([INPUT; WORD_BYTES as usize])[lane as usize..(lane + span) as usize]
                .fill(step);
            byte = byte.wrapping_add(span);
            left -= span;
        }
    }
    /// Forget the producers of the `length` bytes at `address`, up to the
    /// end of the address space.
    fn forget(&mut self, address: u32, length: u32) {
        let end = (u64::from(address) + u64::from(length)).min(1 << u32::BITS);
        let clear = |word: u32, steps: &mut [u32; WORD_BYTES as usize]| {
            for (lane, step) in steps.iter_mut().enumerate() {
                let byte = u64::from(word) * u64::from(WORD_BYTES) + lane as u64;
                if u64::from(address) <= byte && byte < end {
                    *step = INPUT;
                }
            }
            steps.iter().any(|s| *s != INPUT)
        };
        // Whichever is smaller: the range's words or the defined words.
        let first = address / WORD_BYTES;
        let last = end.div_ceil(u64::from(WORD_BYTES)) as u32;
        if (last - first) as usize > self.words.len() {
            self.words.retain(|word, steps| clear(*word, steps));
        } else {
            for word in first..last {
                if let Some(steps) = self.words.get_mut(&word)
                    && !clear(word, steps)
                {
                    self.words.remove(&word);
                }
            }
        }
    }
    /// Every defined byte with its producer.
    fn defined(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.words.iter().flat_map(|(word, steps)| {
            steps
                .iter()
                .enumerate()
                .filter(|(_, step)| **step != INPUT)
                .map(move |(lane, step)| (word * WORD_BYTES + lane as u32, *step))
        })
    }
}

/// Executed and observed replacement instruction addresses.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObservedInstructions {
    pub executed: BTreeSet<u32>,
    pub observed: BTreeSet<u32>,
    /// Instructions any emitted event depends on, compared or not.
    pub effect: BTreeSet<u32>,
    /// Instructions the memory state at a case end depends on.
    pub state: BTreeSet<u32>,
    /// Bytes of ordinary memory the executed instructions read, as
    /// `(address, width)`: the data a result depends on, which evidence
    /// digests by content.
    pub reads: BTreeSet<(u32, u8)>,
}

use oer_analysis_blobray_analysis::closure::CodeMemory as Code;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Plain,
    Load,
    Store,
    /// Reads and writes memory and defines its destination from the read.
    Atomic,
    Branch,
    Call,
    Return,
    /// A jump without link; `target` when direct.
    Jump {
        target: Option<u32>,
    },
}

/// What dependence needs of one instruction.
#[derive(Clone, Copy, Debug)]
struct Shape {
    length: u8,
    /// Registers read; zero entries are unused (x0 has no producer).
    uses: [u8; 2],
    /// Register written, or zero.
    def: u8,
    kind: Kind,
}

fn shape(code: &dyn Code, semantics: &dyn FunctionSemantics, pc: u32) -> Option<Shape> {
    let mut bytes = [0; 4];
    let available = code.code(pc, &mut bytes)?;
    let op = semantics.decode(&bytes[..available])?;
    let bytes = &bytes[..usize::from(op.length)];
    let register = |o: Operand| match o {
        Operand::Register(r) => r,
        Operand::Immediate(_) => 0,
    };
    let link = |lifted: SemanticOp| match lifted {
        SemanticOp::Link { dest } => dest,
        _ => 0,
    };
    let (uses, def, kind) = match op.flow {
        InstructionFlow::Branch { .. } => {
            let (_, left, right) = semantics.branch(bytes)?;
            ([register(left), register(right)], 0, Kind::Branch)
        }
        InstructionFlow::Jump {
            displacement,
            link: linked,
        } => (
            [0; 2],
            link(semantics.lift(bytes)),
            if linked {
                Kind::Call
            } else {
                Kind::Jump {
                    target: Some(pc.wrapping_add_signed(displacement)),
                }
            },
        ),
        InstructionFlow::Indirect {
            base, link: linked, ..
        } => (
            [base, 0],
            link(semantics.lift(bytes)),
            if linked {
                Kind::Call
            } else {
                Kind::Jump { target: None }
            },
        ),
        InstructionFlow::Return { base } => ([base, 0], link(semantics.lift(bytes)), Kind::Return),
        InstructionFlow::Trap => return None,
        InstructionFlow::Stop => ([0; 2], 0, Kind::Plain),
        InstructionFlow::Next => match semantics.lift(bytes) {
            SemanticOp::Integer {
                dest, left, right, ..
            } => ([register(left), register(right)], dest, Kind::Plain),
            SemanticOp::Upper { dest, .. } | SemanticOp::Link { dest } => {
                ([0; 2], dest, Kind::Plain)
            }
            // No executor runs floating point, so no executed step has this
            // shape; it keeps the static CFG running through FP code.
            SemanticOp::Opaque { dest } => ([0; 2], dest, Kind::Plain),
            SemanticOp::Memory {
                kind,
                base,
                dest,
                source,
                ..
            } => {
                let (uses, kind) = match kind {
                    MemoryKind::Load | MemoryKind::LoadReserved => ([base, 0], Kind::Load),
                    MemoryKind::Store => ([base, source.unwrap_or(0)], Kind::Store),
                    // A conditional store's result depends on its operands.
                    MemoryKind::StoreConditional => ([base, source.unwrap_or(0)], Kind::Store),
                    MemoryKind::Atomic => ([base, source.unwrap_or(0)], Kind::Atomic),
                    // As for `Opaque`: static CFG only.
                    MemoryKind::FloatLoad => ([base, 0], Kind::Load),
                    MemoryKind::FloatStore => ([base, 0], Kind::Store),
                };
                (uses, dest.unwrap_or(0), kind)
            }
            SemanticOp::Fence { .. } | SemanticOp::None | SemanticOp::Unsupported => {
                ([0; 2], 0, Kind::Plain)
            }
        },
    };
    Some(Shape {
        length: op.length,
        uses,
        def,
        kind,
    })
}

/// Immediate post-dominators of the conditional branches of one function,
/// by branch site; None is the function exit.
fn post_dominators(
    code: &dyn Code,
    semantics: &dyn FunctionSemantics,
    starts: &BTreeSet<u32>,
    entry: u32,
    shapes: &mut HashMap<u32, Option<Shape>>,
) -> HashMap<u32, Option<u32>> {
    // Node 0 is the exit; instruction nodes follow in discovery order.
    let mut index: HashMap<u32, usize> = HashMap::default();
    let mut addresses = vec![0u32];
    let mut successors: Vec<Vec<usize>> = vec![vec![]];
    let mut pending = vec![entry];
    index.insert(entry, 1);
    addresses.push(entry);
    successors.push(vec![]);
    while let Some(pc) = pending.pop() {
        let node = index[&pc];
        let next_of = |shape: &Shape| pc.wrapping_add(u32::from(shape.length));
        let shape = *shapes
            .entry(pc)
            .or_insert_with(|| shape(code, semantics, pc));
        let targets: Vec<Option<u32>> = match shape {
            None => vec![None],
            Some(s) => match s.kind {
                Kind::Plain | Kind::Load | Kind::Store | Kind::Atomic | Kind::Call => {
                    vec![Some(next_of(&s))]
                }
                Kind::Branch => {
                    let mut bytes = [0; 4];
                    let available = code.code(pc, &mut bytes).unwrap_or(0);
                    match semantics.decode(&bytes[..available]).map(|op| op.flow) {
                        Some(InstructionFlow::Branch { displacement }) => {
                            vec![
                                Some(pc.wrapping_add_signed(displacement)),
                                Some(next_of(&s)),
                            ]
                        }
                        _ => vec![None],
                    }
                }
                Kind::Jump { target: Some(t) } if !starts.contains(&t) || t == entry => {
                    vec![Some(t)]
                }
                Kind::Return | Kind::Jump { .. } => vec![None],
            },
        };
        for target in targets {
            let to = match target {
                None => 0,
                Some(t) => match index.get(&t) {
                    Some(&i) => i,
                    None => {
                        if addresses.len() > MAX_FUNCTION_INSTRUCTIONS {
                            return HashMap::default();
                        }
                        let i = addresses.len();
                        index.insert(t, i);
                        addresses.push(t);
                        successors.push(vec![]);
                        pending.push(t);
                        i
                    }
                },
            };
            successors[node].push(to);
        }
    }
    // Post-dominators are dominators of the reversed CFG rooted at the exit.
    let count = addresses.len();
    let mut predecessors: Vec<Vec<usize>> = vec![vec![]; count];
    for (node, targets) in successors.iter().enumerate() {
        for &t in targets {
            predecessors[t].push(node);
        }
    }
    // Postorder of the reversed CFG from the exit.
    let mut order = vec![usize::MAX; count];
    let mut postorder = Vec::with_capacity(count);
    let mut visited = vec![false; count];
    let mut stack = vec![(0usize, 0usize)];
    visited[0] = true;
    while let Some((node, next)) = stack.last_mut() {
        if let Some(&p) = predecessors[*node].get(*next) {
            *next += 1;
            if !visited[p] {
                visited[p] = true;
                stack.push((p, 0));
            }
        } else {
            order[*node] = postorder.len();
            postorder.push(*node);
            stack.pop();
        }
    }
    let mut idom = vec![usize::MAX; count];
    idom[0] = 0;
    let intersect = |idom: &[usize], mut a: usize, mut b: usize| {
        while a != b {
            while order[a] < order[b] {
                a = idom[a];
            }
            while order[b] < order[a] {
                b = idom[b];
            }
        }
        a
    };
    let mut changed = true;
    while changed {
        changed = false;
        for &node in postorder.iter().rev().skip(1) {
            let mut new = usize::MAX;
            for &s in &successors[node] {
                if idom[s] != usize::MAX {
                    new = if new == usize::MAX {
                        s
                    } else {
                        intersect(&idom, s, new)
                    };
                }
            }
            if new != idom[node] {
                idom[node] = new;
                changed = true;
            }
        }
    }
    let mut result = HashMap::default();
    for (node, &pc) in addresses.iter().enumerate().skip(1) {
        if matches!(shapes.get(&pc), Some(Some(s)) if s.kind == Kind::Branch) {
            // A branch without a path to the exit keeps controlling its frame.
            let ipdom = match idom[node] {
                usize::MAX | 0 => None,
                d => Some(addresses[d]),
            };
            result.insert(pc, ipdom);
        }
    }
    result
}

/// The step in progress while its log entries arrive.
struct Current {
    step: u32,
    pc: u32,
    shape: Shape,
    control: u32,
    call_returned: bool,
}

/// One conditional branch instance controlling later steps.
struct Controller {
    step: u32,
    until: Option<u32>,
    depth: usize,
}

struct Graph {
    pcs: Vec<u32>,
    offsets: Vec<u32>,
    deps: Vec<u32>,
    seeds: Vec<u32>,
    /// Every emitted event and call argument, compared or not.
    effect_seeds: Vec<u32>,
    /// Producers of the memory at every case end.
    state_seeds: Vec<u32>,
    /// Unconditional transfers (calls, jumps and returns), ascending.
    transfers: Vec<u32>,
    /// First step of every phase, ascending.
    phases: Vec<u32>,
}

impl Graph {
    /// Steps `seeds` depend on. An unconditional transfer offers no
    /// alternative path: every later step of its phase runs only through it,
    /// so it is marked when any later step of its phase is.
    fn walk(&self, seeds: Vec<u32>) -> Vec<bool> {
        let mut marked = vec![false; self.pcs.len()];
        let mut pending = seeds;
        loop {
            while let Some(step) = pending.pop() {
                let s = step as usize;
                if std::mem::replace(&mut marked[s], true) {
                    continue;
                }
                let (from, to) = (self.offsets[s] as usize, self.offsets[s + 1] as usize);
                pending.extend(
                    self.deps[from..to]
                        .iter()
                        .copied()
                        .filter(|d| !marked[*d as usize]),
                );
            }
            // The last marked step of each phase.
            let mut last = vec![None; self.phases.len()];
            for (phase, window) in last.iter_mut().enumerate() {
                let start = self.phases[phase] as usize;
                let end = self
                    .phases
                    .get(phase + 1)
                    .map_or(self.pcs.len(), |s| *s as usize);
                *window = (start..end).rev().find(|s| marked[*s]);
            }
            pending.extend(self.transfers.iter().copied().filter(|t| {
                let phase = self.phases.partition_point(|s| s <= t).saturating_sub(1);
                !marked[*t as usize]
                    && last
                        .get(phase)
                        .copied()
                        .flatten()
                        .is_some_and(|l| l > *t as usize)
            }));
            if pending.is_empty() {
                return marked;
            }
        }
    }
}

struct Forward<'x> {
    code: &'x dyn Code,
    semantics: &'x dyn FunctionSemantics,
    starts: &'x BTreeSet<u32>,
    shapes: &'x mut HashMap<u32, Option<Shape>>,
    dominators: &'x mut HashMap<u32, HashMap<u32, Option<u32>>>,
    registers: [u32; 32],
    memory: Producers,
    /// Accesses of the current step, reused across steps.
    reads: Vec<(u32, u8, bool)>,
    writes: Vec<(u32, u8, bool)>,
    outputs: Vec<(u32, u8)>,
    frames: Vec<u32>,
    control: Vec<Controller>,
    current: Option<Current>,
    /// The last finished step's pc, shape and whether a call model returned.
    last: Option<(u32, Shape, bool)>,
    events: Vec<u32>,
    arguments: Vec<[u32; REGISTER_ARGUMENTS]>,
    /// The last step of the current phase.
    phase_last: Option<u32>,
    /// `[address, length]` of memory that ends with the phase.
    transient: Vec<[u32; 2]>,
    graph: Graph,
}

impl Forward<'_> {
    fn finish(&mut self) {
        let Some(step) = self.current.take() else {
            return;
        };
        // The step's dependencies go straight to the end of the graph's list.
        let start = self.graph.deps.len();
        let deps = &mut self.graph.deps;
        if step.control != INPUT {
            deps.push(step.control);
        }
        for r in step.shape.uses {
            if r != 0 && self.registers[r as usize] != INPUT {
                deps.push(self.registers[r as usize]);
            }
        }
        if matches!(step.shape.kind, Kind::Load | Kind::Atomic) {
            for &(address, width, device) in &self.reads {
                if !device {
                    self.memory
                        .read(address, u32::from(width), |producer| deps.push(producer));
                }
            }
        }
        for &(address, width, device) in &self.writes {
            if !device {
                self.memory.write(address, u32::from(width), step.step);
            }
        }
        for &(address, width) in &self.outputs {
            self.memory.write(address, u32::from(width), step.step);
        }
        if step.shape.def != 0 {
            self.registers[step.shape.def as usize] = step.step;
        }
        if step.call_returned {
            for r in ABI.clobbered() {
                self.registers[r as usize] = step.step;
            }
        }
        if step.shape.kind == Kind::Branch {
            let entry = *self.frames.last().unwrap_or(&step.pc);
            let (code, semantics, starts) = (self.code, self.semantics, self.starts);
            let shapes = &mut *self.shapes;
            let until = self
                .dominators
                .entry(entry)
                .or_insert_with(|| post_dominators(code, semantics, starts, entry, shapes))
                .get(&step.pc)
                .copied()
                .flatten();
            self.control.push(Controller {
                step: step.step,
                until,
                depth: self.frames.len(),
            });
        }
        self.reads.clear();
        self.writes.clear();
        self.outputs.clear();
        // The walk only marks reachable steps, so the order of a step's
        // dependencies is free; a step has a handful of them.
        let deps = &mut self.graph.deps;
        let own = &mut deps[start..];
        if own.len() > LINEAR_DEDUP {
            own.sort_unstable();
        }
        let mut kept = 0;
        for i in 0..own.len() {
            let dependency = own[i];
            let seen = if own.len() > LINEAR_DEDUP {
                kept > 0 && own[kept - 1] == dependency
            } else {
                own[..kept].contains(&dependency)
            };
            if !seen {
                own[kept] = dependency;
                kept += 1;
            }
        }
        deps.truncate(start + kept);
        self.graph.offsets.push(deps.len() as u32);
        self.last = Some((step.pc, step.shape, step.call_returned));
    }

    fn instruction(&mut self, pc: u32) {
        self.finish();
        let (code, semantics) = (self.code, self.semantics);
        let shape = *self
            .shapes
            .entry(pc)
            .or_insert_with(|| shape(code, semantics, pc));
        let Some(shape) = shape else {
            // Not replacement code, such as the return sentinel.
            self.last = None;
            return;
        };
        if let Some((last, previous, returned)) = self.last {
            let fallthrough = last.wrapping_add(u32::from(previous.length));
            match previous.kind {
                Kind::Call if !returned && pc != fallthrough => self.frames.push(pc),
                Kind::Return if self.frames.len() > 1 => {
                    self.frames.pop();
                }
                Kind::Jump { .. } if self.starts.contains(&pc) => {
                    if let Some(frame) = self.frames.last_mut() {
                        *frame = pc;
                    }
                }
                _ => {}
            }
        }
        if self.frames.is_empty() {
            self.frames.push(pc);
        }
        let depth = self.frames.len();
        while self
            .control
            .last()
            .is_some_and(|c| c.depth > depth || (c.depth == depth && c.until == Some(pc)))
        {
            self.control.pop();
        }
        let step = self.graph.pcs.len() as u32;
        self.phase_last = Some(step);
        if matches!(shape.kind, Kind::Call | Kind::Return | Kind::Jump { .. }) {
            self.graph.transfers.push(step);
        }
        self.graph.pcs.push(pc);
        self.current = Some(Current {
            step,
            pc,
            shape,
            control: self.control.last().map_or(INPUT, |c| c.step),
            call_returned: false,
        });
    }

    fn sinks(&mut self, sinks: &CaseSinks) {
        let seeds = &mut self.graph.seeds;
        for &e in &sinks.events {
            if let Some(&step) = self.events.get(e as usize) {
                seeds.push(step);
            }
        }
        for &(e, register) in &sinks.registers {
            if let Some(word) = ABI.argument_word(register)
                && let Some(words) = self.arguments.get(e as usize)
            {
                seeds.push(words[word]);
            }
        }
        for (register, selected) in ABI.results().into_iter().zip(sinks.returns) {
            if selected {
                seeds.push(self.registers[usize::from(register)]);
            }
        }
        if sinks.goal
            && let Some(last) = self.phase_last
        {
            seeds.push(last);
        }
        for &[address, length] in &sinks.memory {
            seeds.extend((0..length).filter_map(|i| self.memory.get(address.wrapping_add(i))));
        }
        seeds.retain(|s| *s != INPUT);
        for &(e, register) in &sinks.arguments {
            if let Some(word) = ABI.argument_word(register)
                && let Some(words) = self.arguments.get(e as usize)
                && words[word] != INPUT
            {
                self.graph.effect_seeds.push(words[word]);
            }
        }
        let transient = &self.transient;
        self.graph.state_seeds.extend(
            self.memory
                .defined()
                .filter(|(b, _)| {
                    !transient
                        .iter()
                        .any(|[a, l]| *b >= *a && u64::from(*b) < u64::from(*a) + u64::from(*l))
                })
                .map(|(_, s)| s),
        );
    }
}

/// Accumulates executed and observed instructions over recorded sessions of
/// one replacement target.
pub(crate) struct Analyzer<'x> {
    code: &'x dyn Code,
    semantics: &'x dyn FunctionSemantics,
    starts: &'x BTreeSet<u32>,
    shapes: HashMap<u32, Option<Shape>>,
    dominators: HashMap<u32, HashMap<u32, Option<u32>>>,
    pub result: ObservedInstructions,
}

impl<'x> Analyzer<'x> {
    pub fn new(
        code: &'x dyn Code,
        starts: &'x BTreeSet<u32>,
        semantics: &'x dyn FunctionSemantics,
    ) -> Self {
        Self {
            code,
            semantics,
            starts,
            shapes: HashMap::default(),
            dominators: HashMap::default(),
            result: ObservedInstructions::default(),
        }
    }

    /// Add the executed and observed instructions of one session's log.
    pub fn session(
        &mut self,
        log: StepLog<'_>,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let _admitted = memory.reserve(log.entries.len() as u64 * ENTRY_COST, c.position())?;
        let result = &mut self.result;
        let mut forward = Forward {
            code: self.code,
            semantics: self.semantics,
            starts: self.starts,
            shapes: &mut self.shapes,
            dominators: &mut self.dominators,
            registers: [INPUT; 32],
            memory: Producers::default(),
            reads: vec![],
            writes: vec![],
            outputs: vec![],
            frames: vec![],
            control: vec![],
            current: None,
            last: None,
            events: vec![],
            arguments: vec![],
            phase_last: None,
            transient: vec![],
            graph: Graph {
                pcs: vec![],
                offsets: vec![0],
                deps: vec![],
                seeds: vec![],
                effect_seeds: vec![],
                state_seeds: vec![],
                transfers: vec![],
                phases: vec![],
            },
        };
        let mut cases = log.sinks.iter();
        // Steps repeat the same reads: reduce them before the ordered set.
        let mut reads = Vec::new();
        for (i, entry) in log.entries.iter().enumerate() {
            if i % WORK_BLOCK == 0 {
                c.checkpoint(WORK_BLOCK as u64)?;
            }
            match *entry {
                StepEntry::Phase => {
                    forward.finish();
                    forward.graph.phases.push(forward.graph.pcs.len() as u32);
                    forward.registers = [INPUT; 32];
                    forward.frames.clear();
                    forward.control.clear();
                    forward.last = None;
                    forward.events.clear();
                    forward.arguments.clear();
                    forward.phase_last = None;
                    forward.transient.clear();
                }
                StepEntry::Input {
                    address,
                    length,
                    transient,
                } => {
                    if transient {
                        forward.transient.push([address, length]);
                    }
                    forward.memory.forget(address, length);
                }
                StepEntry::Instruction { pc } => forward.instruction(pc),
                StepEntry::Read {
                    address,
                    width,
                    device,
                } => {
                    if forward.current.is_some() {
                        forward.reads.push((address, width, device));
                        if !device {
                            reads.push((address, width));
                        }
                    }
                }
                StepEntry::Write {
                    address,
                    width,
                    device,
                } => {
                    if forward.current.is_some() {
                        forward.writes.push((address, width, device));
                    }
                }
                StepEntry::Event { index } => {
                    let step = forward.current.as_ref().map_or(INPUT, |s| s.step);
                    let index = index as usize;
                    forward.events.resize(index + 1, INPUT);
                    forward.events[index] = step;
                    if step != INPUT {
                        forward.graph.effect_seeds.push(step);
                    }
                    let mut words = [INPUT; REGISTER_ARGUMENTS];
                    for (w, word) in words.iter_mut().enumerate() {
                        *word = forward.registers[usize::from(ABI.arguments()[w])];
                    }
                    forward
                        .arguments
                        .resize(index + 1, [INPUT; REGISTER_ARGUMENTS]);
                    forward.arguments[index] = words;
                }
                StepEntry::CallReturn => {
                    if let Some(step) = &mut forward.current {
                        step.call_returned = true;
                    }
                }
                StepEntry::Output { address, width } => {
                    if forward.current.is_some() {
                        forward.outputs.push((address, width));
                    }
                }
                StepEntry::CaseEnd => {
                    forward.finish();
                    let sinks = cases.next().ok_or_else(|| {
                        Error::new(ErrorCode::Integrity, "step log case without sinks")
                    })?;
                    forward.sinks(sinks);
                }
            }
        }
        forward.finish();
        let mut graph = forward.graph;
        if graph.phases.first() != Some(&0) {
            graph.phases.insert(0, 0);
        }
        let seeds = std::mem::take(&mut graph.seeds);
        let effect_seeds = std::mem::take(&mut graph.effect_seeds);
        let state_seeds = std::mem::take(&mut graph.state_seeds);
        // Steps far outnumber distinct instructions: number the distinct
        // pcs once, so each walk reduces its marks without sorting steps.
        let mut numbers = HashMap::<u32, u32>::default();
        let mut executed = Vec::new();
        let step_numbers: Vec<u32> = graph
            .pcs
            .iter()
            .map(|pc| {
                *numbers.entry(*pc).or_insert_with(|| {
                    executed.push(*pc);
                    executed.len() as u32 - 1
                })
            })
            .collect();
        let walk = |seeds: Vec<u32>| {
            let marked = graph.walk(seeds);
            let mut hit = vec![false; executed.len()];
            for (number, marked) in step_numbers.iter().zip(&marked) {
                if *marked {
                    hit[*number as usize] = true;
                }
            }
            executed
                .iter()
                .zip(hit)
                .filter_map(|(pc, hit)| hit.then_some(*pc))
                .collect::<Vec<u32>>()
        };
        let observed = walk(seeds);
        let effect = walk(effect_seeds);
        let state = walk(state_seeds);
        c.checkpoint(3 * graph.pcs.len() as u64)?;
        reads.sort_unstable();
        reads.dedup();
        result.reads.extend(reads);
        result.executed.extend(executed);
        result.observed.extend(observed);
        result.effect.extend(effect);
        result.state.extend(state);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn producers_track_bytes_across_word_boundaries() {
        let mut producers = Producers::default();
        producers.write(0x1002, 4, 7);
        producers.write(0x1005, 1, 8);
        let read = |producers: &Producers, address, width| {
            let mut found = vec![];
            producers.read(address, width, |step| found.push(step));
            found
        };
        assert_eq!(read(&producers, 0x1000, 8), [7, 7, 7, 8]);
        assert_eq!(producers.get(0x1001), None);
        assert_eq!(producers.get(0x1004), Some(7));
        producers.forget(0x1003, 2);
        assert_eq!(read(&producers, 0x1000, 8), [7, 8]);
        let mut defined: Vec<_> = producers.defined().collect();
        defined.sort_unstable();
        assert_eq!(defined, [(0x1002, 7), (0x1005, 8)]);
        // A range beyond the defined words forgets through the table.
        producers.forget(0, u32::MAX);
        assert_eq!(producers.defined().count(), 0);
        producers.write(u32::MAX - 1, 2, 9);
        producers.forget(u32::MAX - 1, 4);
        assert_eq!(producers.get(u32::MAX), None);
    }
}
