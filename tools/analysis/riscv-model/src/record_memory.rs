//! Capacity ownership for retained function records. Shared targets are charged
//! per reference conservatively; this measures admitted capacity, not RSS.
use crate::*;
use std::mem::size_of;

impl AbstractValue {
    pub fn allocated_bytes(&self) -> u64 {
        match self {
            Self::Alternatives { values } => values.allocated_bytes(),
            Self::Symbol { symbol, .. } => symbol.object.artifact.allocated_bytes(),
            _ => 0,
        }
    }
}
impl Expression {
    pub fn allocated_bytes(&self) -> u64 {
        match self {
            Self::Integer { left, right, .. } => left.allocated_bytes() + right.allocated_bytes(),
            Self::Load { address, .. }
            | Self::CallWord {
                source: address, ..
            } => address.allocated_bytes(),
            _ => 0,
        }
    }
}
impl ReferenceTarget {
    fn retained_bytes(&self) -> u64 {
        (size_of::<Self>() + 2 * size_of::<usize>() + self.name.capacity()) as u64
            + self.symbol.object.artifact.allocated_bytes()
    }
}
impl FunctionRecord {
    /// Variable allocation capacities, including boxes and conservatively charged Arcs.
    /// The inline enum is accounted by the owning container.
    pub fn allocated_bytes(&self) -> u64 {
        let value =
            |v: &Option<AbstractValue>| v.as_ref().map_or(0, AbstractValue::allocated_bytes);
        match self {
            Self::AlignedValue { fact } => {
                size_of::<AlignedValue>() as u64
                    + fact.initial.allocated_bytes()
                    + fact.value.allocated_bytes()
            }
            Self::Condition { left, right, .. }
            | Self::ReturnValue {
                low: left,
                high: right,
                ..
            }
            | Self::Dispatch {
                base: left,
                index: right,
                ..
            } => left.allocated_bytes() + right.allocated_bytes(),
            Self::Expression { expression, .. } => expression.allocated_bytes(),
            Self::CallInputs { registers, .. } | Self::ReturnInputs { registers, .. } => {
                (registers.capacity() * size_of::<AbstractValue>()) as u64
                    + registers
                        .iter()
                        .map(AbstractValue::allocated_bytes)
                        .sum::<u64>()
            }
            Self::Transfer { target, .. }
            | Self::Value { value: target, .. }
            | Self::StoreAddressRange {
                initial: target, ..
            } => target.allocated_bytes(),
            Self::MemoryAccess {
                address, value: v, ..
            } => address.allocated_bytes() + value(v),
            Self::Instruction { bytes, decoded, .. } => {
                (bytes.capacity() + decoded.text.capacity()) as u64
            }
            Self::Reference { raw, target, .. } => {
                size_of::<FunctionRelocation>() as u64
                    + raw.target.retained_bytes()
                    + target.retained_bytes()
            }
            Self::ImageContext { .. }
            | Self::SemanticGap { .. }
            | Self::Block { .. }
            | Self::Edge { .. }
            | Self::Gap { .. }
            | Self::Fence { .. } => 0,
        }
    }
}
/// Each incoming record must already be covered by the producer's transient
/// workspace. Admission transfers it into retained ownership before the next record.
/// A failed push preserves existing records and releases the incoming payload's
/// reservation. Admitted vector capacity may remain available for a later push.
pub struct RecordBuffer<'a> {
    records: AdmittedVec<'a, FunctionRecord>,
    payloads: AdmittedVec<'a, MemoryReservation<'a>>,
    memory: &'a WorkingMemory,
}
impl<'a> RecordBuffer<'a> {
    pub fn new(memory: &'a WorkingMemory) -> Self {
        Self {
            records: AdmittedVec::new(memory),
            payloads: AdmittedVec::new(memory),
            memory,
        }
    }
    pub fn push(&mut self, record: FunctionRecord, position: RunPosition) -> Result<()> {
        let payload = self.memory.reserve(record.allocated_bytes(), position)?;
        self.payloads.push(payload, position)?;
        if let Err(error) = self.records.push(record, position) {
            self.payloads.pop();
            return Err(error);
        }
        Ok(())
    }
}
impl std::ops::Deref for RecordBuffer<'_> {
    type Target = [FunctionRecord];
    fn deref(&self) -> &Self::Target {
        &self.records
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn displaced_dispatch_accounts_values_and_round_trips_its_inline_displacement() {
        let memory = WorkingMemory::new(64 * 1024).unwrap();
        let base = AbstractValue::Alternatives {
            values: ValueAlternatives::new(vec![
                ValueAlternative::Constant { value: 0x5000 },
                ValueAlternative::Constant { value: 0x6000 },
            ])
            .unwrap(),
        };
        let payload = base.allocated_bytes();
        let record = FunctionRecord::Dispatch {
            offset: 0x14,
            base,
            index: AbstractValue::Unknown,
            scale: 2,
            displacement: -8,
        };
        // The displacement is inline, covered by the retained enum's size.
        assert_eq!(record.allocated_bytes(), payload);
        assert_eq!(
            serde_json::from_slice::<FunctionRecord>(&serde_json::to_vec(&record).unwrap())
                .unwrap(),
            record
        );
        let mut saved = RecordBuffer::new(&memory);
        saved.push(record, RunPosition::default()).unwrap();
        assert!(memory.used() >= payload + size_of::<FunctionRecord>() as u64);
        drop(saved);
        assert_eq!(memory.used(), 0);
    }

    #[test]
    fn aligned_value_round_trips_and_accounts_its_boxed_payload() {
        let memory = WorkingMemory::new(64 * 1024).unwrap();
        let record = FunctionRecord::AlignedValue {
            fact: Box::new(AlignedValue {
                owner: 0x1000,
                point: ValuePoint::Edge {
                    from: 0x1020,
                    target: 0x1024,
                },
                register: 15,
                initial: AbstractValue::Expression { id: 0 },
                value: AbstractValue::Expression { id: 2 },
                proof: AlignmentLoopProof {
                    header: 0x1010,
                    guard: 0x101c,
                    back_edge: 0x1020,
                    step: 0x1014,
                    entry_edge: 0x1004,
                    pointer: 15,
                    mask: 3,
                },
                origin: AlignmentValueOrigin::LoopExit,
            }),
        };
        let json = serde_json::to_vec(&record).unwrap();
        assert_eq!(
            serde_json::from_slice::<FunctionRecord>(&json).unwrap(),
            record
        );
        assert_eq!(record.allocated_bytes(), size_of::<AlignedValue>() as u64);
        let mut saved = RecordBuffer::new(&memory);
        saved.push(record, RunPosition::default()).unwrap();
        assert!(memory.used() >= size_of::<AlignedValue>() as u64);
        drop(saved);
        assert_eq!(memory.used(), 0);
    }
    #[test]
    fn failed_record_growth_rolls_back_payload_and_allows_reuse() {
        let position = RunPosition::default();
        let memory = WorkingMemory::new(64 * 1024).unwrap();
        let make = |text: String| FunctionRecord::Instruction {
            offset: 0,
            bytes: Vec::new(),
            decoded: DecodedOp {
                length: 2,
                text,
                flow: InstructionFlow::Next,
            },
        };
        let mut records = RecordBuffer::new(&memory);
        for _ in 0..8 {
            records.push(make(String::new()), position).unwrap();
        }
        let payload = "x".repeat(1024);
        // Permit the payload and reservation-array growth, but no record-array
        // growth. Existing arrays remain charged during replacement admission.
        let reservation_bytes = size_of::<MemoryReservation<'_>>() as u64;
        let allowance = payload.capacity() as u64 + 16 * reservation_bytes;
        let blocker = memory
            .reserve(
                memory.observation().limit_bytes - memory.used() - allowance,
                position,
            )
            .unwrap();
        let before = memory.used();
        let error = records.push(make(payload), position).unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        assert_eq!(
            error.memory.unwrap().requested_bytes,
            16 * size_of::<FunctionRecord>() as u64
        );
        assert_eq!(records.len(), 8);
        assert_eq!(records.payloads.len(), records.len());
        // Only the enlarged reservation array remains, not the failed payload.
        assert_eq!(memory.used(), before + 8 * reservation_bytes);
        drop(blocker);
        records.push(make("retry".into()), position).unwrap();
        assert_eq!(records.len(), 9);
        assert_eq!(records.payloads.len(), records.len());
        drop(records);
        assert_eq!(memory.used(), 0);
    }

    #[test]
    fn retained_payloads_consume_capacity_and_release_on_failure() {
        let memory = WorkingMemory::new(24 * 1024).unwrap();
        let record = FunctionRecord::Instruction {
            offset: 0,
            bytes: vec![1, 0],
            decoded: DecodedOp {
                length: 2,
                text: "x".repeat(8192),
                flow: InstructionFlow::Next,
            },
        };
        let mut records = RecordBuffer::new(&memory);
        records
            .push(record.clone(), RunPosition::default())
            .unwrap();
        let after_first = memory.used();
        assert!(after_first >= 8192);
        records
            .push(record.clone(), RunPosition::default())
            .unwrap();
        assert!(memory.used() >= after_first + 8192);
        assert_eq!(
            records
                .push(record, RunPosition::default())
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimited
        );
        assert_eq!(records.len(), 2);
        drop(records);
        assert_eq!(memory.used(), 0);
    }
}
