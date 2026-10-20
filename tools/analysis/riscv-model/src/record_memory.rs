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
            Self::Load { address, .. } => address.allocated_bytes(),
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
            Self::CallInputs { registers, .. } => {
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
            Self::SemanticGap { .. }
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
