//! Explicit physical internal observations, separate from final-state equality.
use crate::*;
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineCapture {
    pub reads: bool,
    pub writes: bool,
    pub atomics: bool,
    pub branches: bool,
    /// Report the persistent bytes the phase writes as coalesced
    /// [`WrittenRange`]s: writable image segments, session RAM and session
    /// allocations, without values or order. Not an event channel.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub written: bool,
}
/// Persistent guest bytes one phase wrote, `address..address + length`.
/// A phase reports ascending ranges separated by at least one unwritten byte.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WrittenRange {
    pub address: u32,
    pub length: u32,
}
impl WrittenRange {
    /// One past the last byte.
    pub fn end(self) -> u64 {
        u64::from(self.address) + u64::from(self.length)
    }
}
/// Written ranges one phase may report.
pub const MAX_WRITTEN_RANGES: usize = 4096;
impl TimelineCapture {
    pub fn any(self) -> bool {
        self.reads || self.writes || self.atomics || self.branches
    }
    pub fn contains(self, selected: Self) -> bool {
        (!selected.reads || self.reads)
            && (!selected.writes || self.writes)
            && (!selected.atomics || self.atomics)
            && (!selected.branches || self.branches)
    }
    pub fn selects(self, transaction: &MemoryTransaction) -> bool {
        match transaction {
            MemoryTransaction::Read { .. } => self.reads,
            MemoryTransaction::Write { .. } | MemoryTransaction::InitializeZeroed { .. } => {
                self.writes
            }
            _ => self.atomics,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MemoryReadValue {
    Known { value: u32 },
    Unknown,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MemoryTransaction {
    /// One declared allocation effect; capacity outside this accessible prefix is provenance.
    InitializeZeroed {
        address: u32,
        length: u32,
    },
    Read {
        address: u32,
        width: u8,
        value: MemoryReadValue,
    },
    Write {
        address: u32,
        width: u8,
        value: u32,
    },
    LoadReserved {
        address: u32,
        order: ExecutionOrdering,
        value: u32,
    },
    StoreConditional {
        address: u32,
        order: ExecutionOrdering,
        value: u32,
        stored: bool,
    },
    ReadModifyWrite {
        address: u32,
        order: ExecutionOrdering,
        old: u32,
        value: u32,
    },
}
impl MemoryTransaction {
    pub fn range(self) -> (u32, u32) {
        match self {
            Self::InitializeZeroed { address, length } => (address, length),
            Self::Read { address, width, .. } | Self::Write { address, width, .. } => {
                (address, u32::from(width))
            }
            Self::LoadReserved { address, .. }
            | Self::StoreConditional { address, .. }
            | Self::ReadModifyWrite { address, .. } => (address, 4),
        }
    }
    /// Comparison-only rebasing after a selected reviewed field mapping; raw evidence is unchanged.
    pub fn at_address(mut self, projected: u32) -> Self {
        match &mut self {
            Self::InitializeZeroed { address, .. }
            | Self::Read { address, .. }
            | Self::Write { address, .. }
            | Self::LoadReserved { address, .. }
            | Self::StoreConditional { address, .. }
            | Self::ReadModifyWrite { address, .. } => *address = projected,
        }
        self
    }

    pub fn known(self) -> bool {
        !matches!(
            self,
            Self::Read {
                value: MemoryReadValue::Unknown | MemoryReadValue::Unavailable,
                ..
            }
        )
    }
    pub fn validate(self) -> Result<()> {
        let (address, width, value) = match self {
            Self::InitializeZeroed { address, length } => {
                if length == 0 || u64::from(address) + u64::from(length) >= u64::from(u32::MAX - 1)
                {
                    return Err(Error::new(
                        ErrorCode::Integrity,
                        "invalid initialized memory span",
                    ));
                }
                return Ok(());
            }
            Self::Read {
                address,
                width,
                value,
            } => (
                address,
                width,
                match value {
                    MemoryReadValue::Known { value } => Some(value),
                    _ => None,
                },
            ),
            Self::Write {
                address,
                width,
                value,
            } => (address, width, Some(value)),
            Self::LoadReserved { address, .. }
            | Self::StoreConditional { address, .. }
            | Self::ReadModifyWrite { address, .. } => (address, 4, None),
        };
        if !matches!(width, 1 | 2 | 4)
            || (matches!(
                self,
                Self::LoadReserved { .. }
                    | Self::StoreConditional { .. }
                    | Self::ReadModifyWrite { .. }
            ) && !address.is_multiple_of(4))
            || u64::from(address) + u64::from(width) >= u64::from(u32::MAX - 1)
            || (width < 4 && value.is_some_and(|v| v >> (width * 8) != 0))
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "invalid normal-memory transaction",
            ));
        }
        Ok(())
    }
    /// None means unknown equality, never a proven difference of values.
    pub fn equal(self, other: Self) -> Option<bool> {
        if let (
            Self::Read {
                address: a,
                width: aw,
                value: av,
            },
            Self::Read {
                address: b,
                width: bw,
                value: bv,
            },
        ) = (self, other)
        {
            if a != b || aw != bw {
                return Some(false);
            }
            return match (av, bv) {
                (MemoryReadValue::Known { value: a }, MemoryReadValue::Known { value: b }) => {
                    Some(a == b)
                }
                _ => None,
            };
        }
        Some(self == other)
    }
}
impl ExecutionEvent {
    /// Declared modeled effects share the normal-memory relation without cloning their evidence.
    pub fn normal_memory(&self) -> Option<MemoryTransaction> {
        match self {
            Self::Memory { transaction, .. } => Some(*transaction),
            Self::Allocation {
                address, requested, ..
            } if *requested != 0 => Some(MemoryTransaction::InitializeZeroed {
                address: *address,
                length: *requested,
            }),
            Self::CallOutput {
                address,
                width,
                value,
                ..
            } => Some(MemoryTransaction::Write {
                address: *address,
                width: *width,
                value: *value,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bulk_initialization_has_nonempty_bounded_geometry() {
        for (address, length) in [(0x4000, 0), (u32::MAX - 8, 16), (0, u32::MAX)] {
            assert!(
                MemoryTransaction::InitializeZeroed { address, length }
                    .validate()
                    .is_err()
            );
        }
        MemoryTransaction::InitializeZeroed {
            address: 0x4000,
            length: 7,
        }
        .validate()
        .unwrap();
    }
}
