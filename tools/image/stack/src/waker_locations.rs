//! Typed Waker vtable anchors plus a deliberately small local address algebra.
//! Alternatives overapproximate all incoming paths. No memory forwarding,
//! register-set equality, source names, or neighbouring scopes supply a proof.
use crate::{FunctionFacts, Load, TypeFacts};
use oer_analysis_elf::Elf;
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Value {
    Constant(u32),
    Vtable(u32),
}

struct Proof<'a, 'm> {
    facts: &'a FunctionFacts,
    types: &'a TypeFacts,
    code: &'a [u8],
    end: u32,
    memo: BTreeMap<(u32, u8), Option<BTreeSet<Value>>>,
    active: BTreeSet<(u32, u8)>,
    work: Ledger<'m>,
}

impl Proof<'_, '_> {
    fn bytes(&self, pc: u32) -> Option<&[u8]> {
        self.code
            .get(pc.checked_sub(self.facts.function.address)? as usize..)
    }

    fn operand(
        &mut self,
        pc: u32,
        operand: Operand,
        depth: usize,
        control: &mut dyn RunControl,
    ) -> Result<Option<BTreeSet<Value>>> {
        match operand {
            Operand::Register(0) => self.one(Value::Constant(0), control),
            Operand::Immediate(value) => self.one(Value::Constant(value), control),
            Operand::Register(register) => self.value(pc, register, depth, control),
        }
    }

    fn one(
        &mut self,
        value: Value,
        control: &mut dyn RunControl,
    ) -> Result<Option<BTreeSet<Value>>> {
        let mut values = BTreeSet::new();
        self.work.add(&mut values, value, control)?;
        Ok(Some(values))
    }

    fn value(
        &mut self,
        pc: u32,
        register: u8,
        depth: usize,
        control: &mut dyn RunControl,
    ) -> Result<Option<BTreeSet<Value>>> {
        control.checkpoint(1)?;
        // Bound host recursion too. A cycle or a longer proof is unknown.
        if depth >= 128 {
            return Ok(None);
        }
        if let Some(niche) = self.types.waker_register(register, pc, self.end, control)?
            && (!niche || self.nonzero(pc, register, 0, control)?)
        {
            return self.one(Value::Vtable(0), control);
        }
        if let Some(known) = self.memo.get(&(pc, register)) {
            if let Some(values) = known {
                self.work
                    .admit(values.len().saturating_mul(size_of::<Value>()), control)?;
            }
            return Ok(known.clone());
        }
        if pc == self.facts.function.address
            || !self.work.add(&mut self.active, (pc, register), control)?
        {
            return Ok(None);
        }
        let result = self.incoming(pc, register, depth, control)?;
        self.active.remove(&(pc, register));
        if let Some(values) = &result {
            self.work
                .admit(values.len().saturating_mul(size_of::<Value>()), control)?;
        }
        self.work
            .insert(&mut self.memo, (pc, register), result.clone(), control)?;
        Ok(result)
    }

    fn incoming(
        &mut self,
        pc: u32,
        register: u8,
        depth: usize,
        control: &mut dyn RunControl,
    ) -> Result<Option<BTreeSet<Value>>> {
        let Some(incoming) = self
            .facts
            .target_predecessors
            .get(&pc)
            .filter(|v| !v.is_empty())
        else {
            return Ok(None);
        };
        let mut values = BTreeSet::new();
        for &from in incoming {
            control.checkpoint(1)?;
            let Some(bytes) = self.bytes(from) else {
                return Ok(None);
            };
            let Some(decoded) = RiscvDecoder.decode(bytes) else {
                return Ok(None);
            };
            let op = RiscvDecoder.lift(bytes);
            if barrier(op, decoded.flow) {
                return Ok(None);
            }
            let next = if op.destination_register() == Some(register) {
                match op {
                    SemanticOp::Integer {
                        op: IntegerOp::Add,
                        left,
                        right,
                        ..
                    } => {
                        let Some(left) = self.operand(from, left, depth + 1, control)? else {
                            return Ok(None);
                        };
                        let Some(right) = self.operand(from, right, depth + 1, control)? else {
                            return Ok(None);
                        };
                        let mut sums = BTreeSet::new();
                        for a in &left {
                            for b in &right {
                                control.checkpoint(1)?;
                                let value = match (*a, *b) {
                                    (Value::Constant(a), Value::Constant(b)) => {
                                        Value::Constant(a.wrapping_add(b))
                                    }
                                    (Value::Vtable(a), Value::Constant(b))
                                    | (Value::Constant(b), Value::Vtable(a)) => {
                                        Value::Vtable(a.wrapping_add(b))
                                    }
                                    _ => return Ok(None),
                                };
                                self.work.add(&mut sums, value, control)?;
                            }
                        }
                        Some(sums)
                    }
                    _ => return Ok(None),
                }
            } else {
                self.value(from, register, depth + 1, control)?
            };
            let Some(next) = next else {
                return Ok(None);
            };
            for value in next {
                self.work.add(&mut values, value, control)?;
            }
        }
        Ok(Some(values))
    }

    /// Prove the null niche is excluded on every path, without assuming that
    /// merely being inside an Option's lexical scope selects Some.
    fn nonzero(
        &mut self,
        pc: u32,
        register: u8,
        depth: usize,
        control: &mut dyn RunControl,
    ) -> Result<bool> {
        control.checkpoint(1)?;
        if depth >= 128 || register == 0 || pc == self.facts.function.address {
            return Ok(false);
        }
        let Some(incoming) = self
            .facts
            .target_predecessors
            .get(&pc)
            .filter(|v| !v.is_empty())
        else {
            return Ok(false);
        };
        for &from in incoming {
            control.checkpoint(1)?;
            let Some(bytes) = self.bytes(from) else {
                return Ok(false);
            };
            let Some(decoded) = RiscvDecoder.decode(bytes) else {
                return Ok(false);
            };
            let op = RiscvDecoder.lift(bytes);
            if barrier(op, decoded.flow) {
                return Ok(false);
            }
            if let InstructionFlow::Branch { displacement } = decoded.flow
                && let Some((test, left, right)) = RiscvDecoder.branch(bytes)
                && matches!((left, right),
                    (Operand::Register(r), Operand::Register(0)) | (Operand::Register(0), Operand::Register(r)) if r == register)
            {
                let taken = pc == from.wrapping_add(displacement as u32);
                let fallthrough = pc == from.wrapping_add(u32::from(decoded.length));
                if taken != fallthrough
                    && ((test == BranchTest::Ne && taken)
                        || (test == BranchTest::Eq && fallthrough))
                {
                    continue;
                }
            }
            let register = if op.destination_register() == Some(register) {
                match op {
                    SemanticOp::Integer {
                        op: IntegerOp::Add,
                        left: Operand::Register(r),
                        right: Operand::Immediate(0) | Operand::Register(0),
                        ..
                    }
                    | SemanticOp::Integer {
                        op: IntegerOp::Add,
                        left: Operand::Register(0),
                        right: Operand::Register(r),
                        ..
                    } => r,
                    _ => return Ok(false),
                }
            } else {
                register
            };
            if !self.nonzero(from, register, depth + 1, control)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn barrier(op: SemanticOp, flow: InstructionFlow) -> bool {
    op == SemanticOp::Unsupported
        || matches!(
            flow,
            InstructionFlow::Jump { link: true, .. } | InstructionFlow::Indirect { link: true, .. }
        )
        || matches!(op, SemanticOp::Memory { kind, .. } if !matches!(kind, MemoryKind::Load | MemoryKind::LoadReserved | MemoryKind::FloatLoad))
}

pub(crate) fn slots(
    file: &Elf<'_>,
    facts: &FunctionFacts,
    types: &TypeFacts,
    load: &Load,
    site: u32,
    work: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Option<BTreeSet<usize>>> {
    // Analysis may include companion ELFs; this TypeFacts supplies locations
    // and instructions only for its own source image.
    let Ok(section) = file.section(facts.function.section) else {
        return Ok(None);
    };
    let start = u64::from(facts.function.address);
    let end = start + u64::from(facts.function.size);
    if !section.executable
        || !section.contains(start)
        || end > section.address.saturating_add(section.size)
    {
        return Ok(None);
    }
    let code = crate::image::function_bytes(file, &facts.function)?;
    let Some(decoded) = site
        .checked_sub(facts.function.address)
        .and_then(|offset| code.get(offset as usize..))
        .and_then(|bytes| RiscvDecoder.decode(bytes))
    else {
        return Ok(None);
    };
    let Some(end) = site.checked_add(u32::from(decoded.length)) else {
        return Ok(None);
    };
    let mut proof = Proof {
        facts,
        types,
        code,
        end,
        memo: BTreeMap::new(),
        active: BTreeSet::new(),
        work: Ledger::new(work.memory(), control)?,
    };
    let Some(values) = proof.operand(load.site, Operand::Register(load.base), 0, control)? else {
        return Ok(None);
    };
    let mut slots = BTreeSet::new();
    for value in values {
        control.checkpoint(1)?;
        let Value::Vtable(offset) = value else {
            return Ok(None);
        };
        let slot = match offset.wrapping_add(load.offset as u32) {
            0 => 0,
            4 => 1,
            8 => 2,
            12 => 3,
            _ => return Ok(None),
        };
        work.add(&mut slots, slot, control)?;
    }
    Ok((!slots.is_empty()).then_some(slots))
}
