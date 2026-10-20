//! Statically reachable code of root entries in executable captured code.
//!
//! Each function is explored by recursive descent from its entry through
//! direct branches and jumps. Direct calls, and `auipc`/`lui` + `jalr` pairs
//! whose target is known from the immediately preceding upper-immediate
//! instruction, add callees; so do the observed targets of other executed
//! indirect transfers. A jump to another function's defined start is a tail
//! transfer. Transfers to declared boundaries stop the closure; indirect
//! transfers with neither a static nor an observed target stay unresolved,
//! and those followed only to observed targets are reported as followed.
use oer_analysis_blobray_domain::*;
use oer_analysis_riscv_model::*;
use std::collections::{BTreeMap, BTreeSet};

/// Executable captured code the walker may decode.
pub trait CodeMemory {
    /// Copy up to four code bytes at `address` and return how many exist, or
    /// None when `address` is not executable captured code.
    fn code(&self, address: u32, bytes: &mut [u8; 4]) -> Option<usize>;
}

pub struct ClosureInput<'a> {
    pub roots: &'a [u32],
    /// Addresses the closure does not enter: call models, FIFO service
    /// bindings and execution goals.
    pub boundaries: &'a BTreeSet<u32>,
    /// Defined function starts, which make a plain jump a tail transfer.
    pub function_starts: &'a BTreeSet<u32>,
    /// Observed targets of executed indirect transfers, by site.
    pub observed: &'a BTreeMap<u32, BTreeSet<u32>>,
    pub code: &'a dyn CodeMemory,
    pub semantics: &'a dyn FunctionSemantics,
}

/// Working memory admitted per decoded instruction for the closure's sets.
const INSTRUCTION_COST: u64 = 96;
/// Instructions admitted by one reservation.
const ADMISSION: usize = 1024;

struct Walker<'a, 'm> {
    input: &'a ClosureInput<'a>,
    memory: &'m WorkingMemory,
    reservations: Vec<MemoryReservation<'m>>,
    decoded: usize,
}

#[derive(Default)]
struct Function {
    /// Start and end of each explored straight-line run.
    runs: Vec<[u32; 2]>,
    blocks: BTreeSet<u32>,
    branches: BTreeSet<u32>,
    callees: BTreeSet<u32>,
    modeled: BTreeSet<u32>,
    unresolved: BTreeSet<u32>,
    followed: BTreeSet<u32>,
    gaps: BTreeSet<u32>,
    pending: Vec<u32>,
}

impl Walker<'_, '_> {
    fn admit(&mut self, c: &mut dyn RunControl) -> Result<()> {
        if self.decoded == MAX_CLOSURE_INSTRUCTIONS {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "code closure instruction capacity exhausted",
            ));
        }
        if self.decoded.is_multiple_of(ADMISSION) {
            let reservation = self
                .memory
                .reserve(ADMISSION as u64 * INSTRUCTION_COST, c.position())?;
            self.reservations.try_reserve(1).map_err(|_| {
                Error::new(ErrorCode::ResourceLimited, "closure allocation refused")
            })?;
            self.reservations.push(reservation);
        }
        self.decoded += 1;
        c.checkpoint(1)
    }

    fn executable(&self, address: u32) -> bool {
        self.input.code.code(address, &mut [0; 4]).is_some()
    }

    /// A branch or intra-function jump from `site` to `target`.
    fn local(&self, f: &mut Function, site: u32, target: u32) {
        if self.input.boundaries.contains(&target) {
            f.modeled.insert(site);
        } else if !self.executable(target) {
            f.unresolved.insert(site);
        } else if f.blocks.insert(target) {
            f.pending.push(target);
        }
    }

    /// A call, or a jump that may leave the function, from `site` to `target`.
    fn transfer(&self, f: &mut Function, entry: u32, site: u32, target: u32, call: bool) {
        if self.input.boundaries.contains(&target) {
            f.modeled.insert(site);
        } else if !self.executable(target) {
            f.unresolved.insert(site);
        } else if call || (target != entry && self.input.function_starts.contains(&target)) {
            f.callees.insert(target);
        } else {
            self.local(f, site, target);
        }
    }

    fn function(&mut self, entry: u32, c: &mut dyn RunControl) -> Result<ClosureFunction> {
        let mut f = Function::default();
        let mut visited = BTreeSet::new();
        f.blocks.insert(entry);
        f.pending.push(entry);
        while let Some(start) = f.pending.pop() {
            let mut pc = start;
            // Register and value of an immediately preceding upper immediate.
            let mut upper: Option<(u8, u32)> = None;
            loop {
                if pc != entry && pc != start && self.input.boundaries.contains(&pc) {
                    f.modeled.insert(pc);
                    break;
                }
                if !visited.insert(pc) {
                    // Straight-line code entering an explored run starts a block.
                    if pc != start {
                        f.blocks.insert(pc);
                    }
                    break;
                }
                self.admit(c)?;
                let mut bytes = [0; 4];
                let Some(available) = self.input.code.code(pc, &mut bytes) else {
                    f.unresolved.insert(pc);
                    break;
                };
                let Some(op) = self.input.semantics.decode(&bytes[..available]) else {
                    f.gaps.insert(pc);
                    break;
                };
                let next = pc.wrapping_add(u32::from(op.length));
                match f.runs.last_mut() {
                    Some(run) if run[1] == pc => run[1] = next,
                    _ => f.runs.push([pc, next]),
                }
                let previous = upper.take();
                match op.flow {
                    InstructionFlow::Next => {
                        if let SemanticOp::Upper {
                            dest,
                            value,
                            pc_relative,
                        } = self.input.semantics.lift(&bytes[..usize::from(op.length)])
                            && dest != 0
                        {
                            upper = Some((
                                dest,
                                if pc_relative {
                                    pc.wrapping_add(value)
                                } else {
                                    value
                                },
                            ));
                        }
                        pc = next;
                        continue;
                    }
                    InstructionFlow::Branch { displacement } => {
                        f.branches.insert(pc);
                        self.local(&mut f, pc, pc.wrapping_add_signed(displacement));
                        self.local(&mut f, pc, next);
                    }
                    InstructionFlow::Jump { displacement, link } => {
                        self.transfer(
                            &mut f,
                            entry,
                            pc,
                            pc.wrapping_add_signed(displacement),
                            link,
                        );
                        if link {
                            pc = next;
                            continue;
                        }
                    }
                    flow @ (InstructionFlow::Indirect { .. } | InstructionFlow::Return { .. }) => {
                        let (base, offset, link) = flow.through_register().unwrap_or_default();
                        match previous.filter(|(register, _)| *register == base) {
                            Some((_, value)) => self.transfer(
                                &mut f,
                                entry,
                                pc,
                                value.wrapping_add_signed(offset),
                                link,
                            ),
                            None if matches!(flow, InstructionFlow::Return { .. }) => {}
                            None => match self.input.observed.get(&pc) {
                                Some(targets) => {
                                    f.followed.insert(pc);
                                    for target in targets {
                                        self.transfer(&mut f, entry, pc, *target, link);
                                    }
                                }
                                None => {
                                    f.unresolved.insert(pc);
                                }
                            },
                        }
                        if link {
                            pc = next;
                            continue;
                        }
                    }
                    InstructionFlow::Trap => {
                        f.gaps.insert(pc);
                    }
                    InstructionFlow::Stop => {}
                }
                break;
            }
        }
        f.callees.remove(&entry);
        f.runs.sort_unstable();
        let mut code: Vec<[u32; 2]> = Vec::new();
        for run in f.runs {
            match code.last_mut() {
                Some(last) if last[1] >= run[0] => last[1] = last[1].max(run[1]),
                _ => code.push(run),
            }
        }
        Ok(ClosureFunction {
            entry,
            code,
            blocks: f.blocks.into_iter().collect(),
            branches: f.branches.into_iter().collect(),
            callees: f.callees.into_iter().collect(),
            modeled: f.modeled.into_iter().collect(),
            unresolved: f.unresolved.into_iter().collect(),
            followed: f.followed.into_iter().collect(),
            gaps: f.gaps.into_iter().collect(),
        })
    }
}

/// Every function reachable from `roots`, ascending by entry.
pub fn code_closure(
    input: &ClosureInput<'_>,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<ClosureFunction>> {
    let mut walker = Walker {
        input,
        memory,
        reservations: Vec::new(),
        decoded: 0,
    };
    let mut functions = BTreeMap::new();
    let mut pending = input.roots.to_vec();
    while let Some(entry) = pending.pop() {
        if functions.contains_key(&entry) {
            continue;
        }
        if functions.len() == MAX_CLOSURE_FUNCTIONS {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "code closure function capacity exhausted",
            ));
        }
        let function = walker.function(entry, c)?;
        pending.extend(function.callees.iter().copied());
        functions.insert(entry, function);
    }
    Ok(functions.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two-byte instructions: `[1, r]` returns through `r`, `[2, r]` jumps
    /// through `r`, `[3, r]` loads `r` with 0x1008, anything else stops.
    struct Isa;
    impl FunctionDecoder for Isa {
        fn identity(&self) -> &'static str {
            "test"
        }
        fn decode(&self, bytes: &[u8]) -> Option<DecodedOp> {
            let base = bytes[1];
            Some(DecodedOp {
                length: 2,
                text: String::new(),
                flow: match bytes[0] {
                    1 => InstructionFlow::Return { base },
                    2 => InstructionFlow::Indirect {
                        base,
                        offset: 0,
                        link: false,
                    },
                    3 => InstructionFlow::Next,
                    _ => InstructionFlow::Stop,
                },
            })
        }
        fn reference(
            &self,
            _: &FunctionRelocation,
            _: &[FunctionRelocation],
            _: u32,
            _: &mut dyn RunControl,
        ) -> Result<NormalizedReference> {
            unreachable!()
        }
    }
    impl FunctionSemantics for Isa {
        fn semantic_identity(&self) -> &'static str {
            "test"
        }
        fn lift(&self, bytes: &[u8]) -> SemanticOp {
            match bytes[0] {
                3 => SemanticOp::Upper {
                    dest: bytes[1],
                    value: 0x1008,
                    pc_relative: false,
                },
                _ => SemanticOp::None,
            }
        }
        fn value_relocation(&self, _: &FunctionRelocation, _: SemanticOp) -> ValueRelocation {
            unreachable!()
        }
    }

    /// Code at 0x1000.
    struct Code(Vec<u8>);
    impl CodeMemory for Code {
        fn code(&self, address: u32, bytes: &mut [u8; 4]) -> Option<usize> {
            let at = usize::try_from(address.checked_sub(0x1000)?).ok()?;
            let available = self.0.get(at..)?.len().min(4);
            (available > 0).then(|| {
                bytes[..available].copy_from_slice(&self.0[at..at + available]);
                available
            })
        }
    }

    /// The closure of the one function at 0x1000, with `observed` targets
    /// at its first instruction.
    fn closure(code: &[u8], observed: &[u32]) -> ClosureFunction {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let observed: BTreeMap<_, _> = (!observed.is_empty())
            .then(|| (0x1000, observed.iter().copied().collect()))
            .into_iter()
            .collect();
        let starts = BTreeSet::from([0x1000, 0x1008]);
        let input = ClosureInput {
            roots: &[0x1000],
            boundaries: &BTreeSet::new(),
            function_starts: &starts,
            observed: &observed,
            code: &Code(code.to_vec()),
            semantics: &Isa,
        };
        code_closure(&input, &memory, &mut || Ok(()))
            .unwrap()
            .remove(0)
    }

    #[test]
    fn a_return_through_either_link_register_is_neither_unresolved_nor_followed() {
        for link in [1, 5] {
            let function = closure(&[1, link], &[0x1008]);
            assert!(function.unresolved.is_empty(), "x{link}");
            assert!(function.followed.is_empty(), "x{link}");
            assert!(function.callees.is_empty(), "x{link}");
        }
        // A jump through t0 that the decoder does not mark a return stays
        // unresolved, or follows what execution observed.
        assert_eq!(closure(&[2, 5], &[]).unresolved, [0x1000]);
        let followed = closure(&[2, 5, 0, 0, 0, 0, 0, 0, 0, 0], &[0x1008]);
        assert_eq!(
            (followed.followed, followed.callees),
            (vec![0x1000], vec![0x1008])
        );
    }

    #[test]
    fn a_return_through_a_register_just_loaded_is_a_transfer() {
        // `lui t0, …; jr t0` goes where the upper immediate says.
        let function = closure(&[3, 5, 1, 5, 0, 0, 0, 0, 0, 0], &[]);
        assert_eq!(function.callees, [0x1008]);
    }
}
