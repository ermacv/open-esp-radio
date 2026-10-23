//! Operation-local structural facts over one borrowed function record stream.
//! No target selection, pointer interpretation or completeness claims.
use crate::*;

/// Input contracts of the existing record consumers; never inferred from data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordPolicy {
    /// IDs are 0..N in definition order, Integer/Load operands use earlier IDs,
    /// instruction/input/transfer offsets are unique, and references agree.
    Canonical,
    /// Any u32 ID and any record order. The last expression, reference or
    /// input definition wins. Forward references and cycles are not validated.
    Dictionary,
}

pub struct MemoryObservation<'a> {
    pub record: u64,
    pub offset: u64,
    pub access: MemoryKind,
    pub width: u8,
    pub address: &'a AbstractValue,
    pub value: Option<&'a AbstractValue>,
}

pub struct CallInputObservation<'a> {
    pub record: u64,
    pub offset: u64,
    pub registers: &'a [AbstractValue],
}

pub struct TransferObservation<'a> {
    pub record: u64,
    pub offset: u64,
    pub target: &'a AbstractValue,
    pub call: bool,
}

/// The only preparation of expression, reference and callsite lookup tables.
/// Positions borrow the original records; no values or names are cloned.
/// Every index allocation is admitted before growth and released on drop/error.
pub struct FunctionIndex<'a, 'm> {
    capture_id: Option<u64>,
    extent: Option<CodeRange>,
    records: &'a [FunctionRecord],
    policy: RecordPolicy,
    expressions: AdmittedVec<'m, usize>,
    references: AdmittedVec<'m, usize>,
    inputs: AdmittedVec<'m, usize>,
    transfers: AdmittedVec<'m, usize>,
}

fn integrity(s: &str) -> Error {
    Error::new(ErrorCode::Integrity, s)
}
fn expression(records: &[FunctionRecord], record: usize) -> (u32, &Expression) {
    match &records[record] {
        FunctionRecord::Expression { id, expression, .. } => (*id, expression),
        _ => unreachable!("expression index contains only expression positions"),
    }
}
fn reference(records: &[FunctionRecord], record: usize) -> &ReferenceTarget {
    match &records[record] {
        FunctionRecord::Reference { target, .. } => target,
        _ => unreachable!("reference index contains only reference positions"),
    }
}
fn call_input(records: &[FunctionRecord], record: usize) -> CallInputObservation<'_> {
    match &records[record] {
        FunctionRecord::CallInputs { offset, registers } => CallInputObservation {
            record: record as u64,
            offset: *offset,
            registers,
        },
        _ => unreachable!("input index contains only call input positions"),
    }
}
fn transfer(records: &[FunctionRecord], record: usize) -> TransferObservation<'_> {
    match &records[record] {
        FunctionRecord::Transfer {
            offset,
            target,
            call,
        } => TransferObservation {
            record: record as u64,
            offset: *offset,
            target,
            call: *call,
        },
        _ => unreachable!("transfer index contains only transfer positions"),
    }
}
fn lookup_work(len: usize) -> u64 {
    len.max(1).ilog2() as u64 + 1
}

impl<'a, 'm> FunctionIndex<'a, 'm> {
    pub fn new(
        records: &'a [FunctionRecord],
        policy: RecordPolicy,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut out = Self {
            capture_id: None,
            extent: None,
            records,
            policy,
            expressions: AdmittedVec::new(memory),
            references: AdmittedVec::new(memory),
            inputs: AdmittedVec::new(memory),
            transfers: AdmittedVec::new(memory),
        };
        let mut instructions = AdmittedVec::new(memory);
        for (record, r) in records.iter().enumerate() {
            c.checkpoint(1)?;
            match r {
                FunctionRecord::ImageContext { capture_id, extent } => {
                    if out.capture_id.is_some() && policy == RecordPolicy::Canonical {
                        return Err(integrity("duplicate image context"));
                    }
                    out.capture_id = Some(*capture_id);
                    out.extent = Some(*extent);
                }
                FunctionRecord::Instruction { .. } if policy == RecordPolicy::Canonical => {
                    instructions.push(record, c.position())?
                }
                FunctionRecord::Expression { id, expression, .. } => {
                    if policy == RecordPolicy::Canonical {
                        if *id as usize != out.expressions.len() {
                            return Err(integrity("noncanonical navigation expression IDs"));
                        }
                        let earlier = |v: &AbstractValue| !matches!(v, AbstractValue::Expression { id: other } if other >= id);
                        if !match expression {
                            Expression::Integer { left, right, .. } => {
                                earlier(left) && earlier(right)
                            }
                            Expression::Load { address, .. }
                            | Expression::CallWord {
                                source: address, ..
                            } => earlier(address),
                            _ => true,
                        } {
                            return Err(integrity("forward or cyclic navigation expression"));
                        }
                    }
                    out.expressions.push(record, c.position())?;
                }
                FunctionRecord::Reference { .. } => out.references.push(record, c.position())?,
                FunctionRecord::CallInputs { .. } => out.inputs.push(record, c.position())?,
                FunctionRecord::Transfer { .. } => out.transfers.push(record, c.position())?,
                _ => (),
            }
        }
        c.checkpoint(records.len() as u64 * lookup_work(records.len()))?;
        if policy == RecordPolicy::Dictionary {
            out.expressions
                .sort_unstable_by_key(|&r| (expression(records, r).0, r));
        }
        // The source position breaks ties without a stable sort's unadmitted
        // scratch. Taking the rightmost match preserves last-definition wins.
        out.references.sort_unstable_by(|&a, &b| {
            reference(records, a)
                .symbol
                .cmp(&reference(records, b).symbol)
                .then(a.cmp(&b))
        });
        out.inputs
            .sort_unstable_by_key(|&r| (call_input(records, r).offset, r));
        out.transfers
            .sort_unstable_by_key(|&r| (transfer(records, r).offset, r));
        let instruction_offset = |r: usize| match records[r] {
            FunctionRecord::Instruction { offset, .. } => offset,
            _ => unreachable!("instruction index contains only instruction positions"),
        };
        instructions.sort_unstable_by_key(|&r| (instruction_offset(r), r));
        if policy == RecordPolicy::Canonical {
            let conflict = |class: &str, offset: u64, a: usize, b: usize| {
                integrity(&format!(
                    "duplicate or inconsistent navigation facts: class={class} offset={offset:#x}; \
                     first record[{a}]={:?}; second record[{b}]={:?}; function extent={:?}; position={:?}",
                    records[a],
                    records[b],
                    out.extent,
                    c.position()
                ))
            };
            for pair in instructions.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                if instruction_offset(a) == instruction_offset(b) {
                    return Err(conflict("instruction", instruction_offset(a), a, b));
                }
            }
            for pair in out.inputs.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                if call_input(records, a).offset == call_input(records, b).offset {
                    return Err(conflict("call-inputs", call_input(records, a).offset, a, b));
                }
            }
            for pair in out.transfers.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                if transfer(records, a).offset == transfer(records, b).offset {
                    return Err(conflict("transfer", transfer(records, a).offset, a, b));
                }
            }
            for pair in out.references.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let (left, right) = (reference(records, a), reference(records, b));
                if left.symbol == right.symbol && left != right {
                    let offset = match records[a] {
                        FunctionRecord::Reference { ref raw, .. } => raw.offset,
                        _ => unreachable!("reference index contains only reference positions"),
                    };
                    return Err(conflict("reference", offset, a, b));
                }
            }
        }
        Ok(out)
    }

    /// Original order and positions, including records not indexed here.
    pub fn records(&self) -> &'a [FunctionRecord] {
        self.records
    }
    pub fn capture_id(&self) -> Option<u64> {
        self.capture_id
    }

    pub fn extent(&self) -> Option<CodeRange> {
        self.extent
    }

    /// Borrow a saved expression by exact ID, charging before lookup.
    pub fn expression(&self, id: u32, c: &mut dyn RunControl) -> Result<Option<&'a Expression>> {
        c.checkpoint(match self.policy {
            RecordPolicy::Canonical => 1,
            RecordPolicy::Dictionary => lookup_work(self.expressions.len()),
        })?;
        Ok(self.get_expression(id))
    }

    fn get_expression(&self, id: u32) -> Option<&'a Expression> {
        let record = match self.policy {
            RecordPolicy::Canonical => *self.expressions.get(id as usize)?,
            RecordPolicy::Dictionary => {
                let end = self
                    .expressions
                    .partition_point(|&r| expression(self.records, r).0 <= id);
                *self.expressions.get(end.checked_sub(1)?)?
            }
        };
        let (found, value) = expression(self.records, record);
        (found == id).then_some(value)
    }

    /// Borrow an expression during a consumer-metered traversal. Blobray's
    /// canonical graph evaluators charge their own steps and use this primitive
    /// for constant-time operand access and fixed-depth shape recognition.
    pub fn value_expression(&self, value: &AbstractValue) -> Option<&'a Expression> {
        if let AbstractValue::Expression { id } = value {
            self.get_expression(*id)
        } else {
            None
        }
    }

    pub fn reference(
        &self,
        symbol: &SymbolId,
        c: &mut dyn RunControl,
    ) -> Result<Option<&'a ReferenceTarget>> {
        c.checkpoint(lookup_work(self.references.len()))?;
        let end = self
            .references
            .partition_point(|&r| &reference(self.records, r).symbol <= symbol);
        Ok(end.checked_sub(1).and_then(|i| {
            let target = reference(self.records, self.references[i]);
            (&target.symbol == symbol).then_some(target)
        }))
    }

    /// State at exactly this instruction offset. No nearest-site lookup or
    /// relocation adjustment: the consumer selects its transfer instruction.
    pub fn call_inputs(
        &self,
        offset: u64,
        c: &mut dyn RunControl,
    ) -> Result<Option<CallInputObservation<'a>>> {
        c.checkpoint(lookup_work(self.inputs.len()))?;
        let end = self
            .inputs
            .partition_point(|&r| call_input(self.records, r).offset <= offset);
        Ok(end.checked_sub(1).and_then(|i| {
            let input = call_input(self.records, self.inputs[i]);
            (input.offset == offset).then_some(input)
        }))
    }

    /// Exact recorded transfer; its target is an observation, not a selected callee.
    pub fn transfer(
        &self,
        offset: u64,
        c: &mut dyn RunControl,
    ) -> Result<Option<TransferObservation<'a>>> {
        c.checkpoint(lookup_work(self.transfers.len()))?;
        let end = self
            .transfers
            .partition_point(|&r| transfer(self.records, r).offset <= offset);
        Ok(end.checked_sub(1).and_then(|i| {
            let value = transfer(self.records, self.transfers[i]);
            (value.offset == offset).then_some(value)
        }))
    }

    pub fn accesses(
        &self,
        c: &mut dyn RunControl,
        emit: &mut dyn FnMut(MemoryObservation<'a>, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        for (record, r) in self.records.iter().enumerate() {
            c.checkpoint(1)?;
            let FunctionRecord::MemoryAccess {
                offset,
                access,
                width,
                address,
                value,
                ..
            } = r
            else {
                continue;
            };
            emit(
                MemoryObservation {
                    record: record as u64,
                    offset: *offset,
                    access: *access,
                    width: *width,
                    address,
                    value: value.as_ref(),
                },
                c,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
