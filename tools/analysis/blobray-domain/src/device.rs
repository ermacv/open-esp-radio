//! Explicit device assumptions and their per-lifetime participation evidence.
use crate::*;
use sha2::{Digest, Sha256};
pub const MAX_DEVICE_MODELS: usize = 128;
pub const MAX_DEVICE_PORTS: usize = 4096;
pub const MAX_DEVICE_VALUES: usize = 4096;
/// Largest retained aperture, in bytes.
pub const MAX_APERTURE_BYTES: u32 = 1 << 24;
/// Distinct words an aperture may retain during one lifetime.
pub const MAX_APERTURE_WORDS: usize = 1 << 16;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceDeclaration {
    pub id: String,
    /// Caller-declared conditions, not an accepted hardware assertion.
    pub applicability: String,
    pub lifetime: RegionLifetime,
    pub behavior: DeviceBehavior,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DeviceBehavior {
    CommandBank(CommandBank),
    RegisterBank {
        cells: Vec<RegisterCell>,
    },
    ConstantRead {
        address: u32,
        width: u8,
        value: u32,
    },
    SequenceRead {
        address: u32,
        width: u8,
        runs: Vec<ReadRun>,
    },
    W1c {
        address: u32,
        width: u8,
        initial: u32,
        clear_mask: u32,
        read_clear_mask: u32,
    },
    ReadClear {
        address: u32,
        width: u8,
        initial: u32,
        clear_mask: u32,
    },
    SelfClearing {
        address: u32,
        width: u8,
        initial: u32,
        store_mask: u32,
        command_mask: u32,
    },
    Fifo {
        address: u32,
        width: u8,
        reads: Vec<u32>,
        writes: Vec<u32>,
    },
    IndexedBank {
        index_address: u32,
        data_address: u32,
        width: u8,
        index: Option<u32>,
        values: Vec<u32>,
    },
    /// Read-only port whose reads cycle through `values` indefinitely: a
    /// periodic measurement stream. No read count is declared or owed; every
    /// read remains an MMIO event.
    CyclicRead {
        address: u32,
        width: u8,
        values: Vec<u32>,
    },
    /// A multiplexed read port: each write to `selector_address` selects the
    /// sequence declared for that selector, and each read of
    /// `value_address` consumes the selected sequence's finite runs. Writing
    /// an undeclared selector, reading the selector, writing the value or
    /// reading without a selection or past the selected runs is an issue;
    /// the model completes when every sequence is consumed.
    SelectedSequence {
        selector_address: u32,
        value_address: u32,
        width: u8,
        sequences: Vec<SelectedRuns>,
    },
    /// Word-aligned MMIO range in which every naturally aligned 1-, 2- or
    /// 4-byte access not claimed by an exact port of another model is retained
    /// storage: a word reads `initial` until written, then its last value.
    /// Every access remains an ordinary MMIO event, so reads of never-written
    /// words stay visible evidence rather than a hidden assumption.
    RetainedAperture {
        start: u32,
        length: u32,
        initial: u32,
    },
}

/// The finite reads one selector of a [`DeviceBehavior::SelectedSequence`]
/// answers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedRuns {
    pub selector: u32,
    pub runs: Vec<ReadRun>,
}

/// An explicit finite count of identical consecutive peripheral responses.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRun {
    pub value: u32,
    pub count: u32,
}
impl ReadRun {
    pub const fn once(value: u32) -> Self {
        Self { value, count: 1 }
    }
    /// Validate bounded encoded and logical lengths without expanding responses.
    pub fn total(runs: &[Self]) -> Result<u32> {
        let bad = || Error::new(ErrorCode::InvalidRequest, "invalid finite read sequence");
        if runs.is_empty() || runs.len() > MAX_DEVICE_VALUES {
            return Err(bad());
        }
        runs.iter().try_fold(0u32, |total, run| {
            if run.count == 0 {
                return Err(bad());
            }
            total.checked_add(run.count).ok_or_else(bad)
        })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DeviceIssue {
    InvalidCommand { value: u32 },
    UnknownSelector { selector: u32 },
    PendingCommand,
    AccessWidth,
    ReadOnly,
    ExhaustedReads,
    UnexpectedWrite,
    WriteMismatch { expected: u32, actual: u32 },
    UnknownIndex,
    IndexOutOfRange { index: u32 },
    WriteOnly,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelStatus {
    Open,
    Complete,
    Incomplete,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelObservation {
    /// Present only for a packed-command bank; includes pending completion obligations.
    pub commands: Option<CommandObservation>,
    pub id: String,
    pub definition: ArtifactId,
    pub lifetime: RegionLifetime,
    /// Cumulative successful operations in this instance's lifetime.
    pub reads: u64,
    pub writes: u64,
    pub remaining_reads: u32,
    pub remaining_writes: u32,
    pub closed: bool,
    pub issue: Option<DeviceIssue>,
    pub status: ModelStatus,
}
impl ModelObservation {
    pub fn expected_status(&self) -> ModelStatus {
        if self.issue.is_some()
            || (self.closed
                && (self.remaining_reads != 0
                    || self.remaining_writes != 0
                    || self.commands.is_some_and(|c| c.pending != 0)))
        {
            ModelStatus::Incomplete
        } else if self.closed {
            ModelStatus::Complete
        } else {
            ModelStatus::Open
        }
    }
}
impl DeviceDeclaration {
    pub fn validate(&self) -> Result<()> {
        let bad = || {
            Error::new(
                ErrorCode::InvalidRequest,
                "invalid device declaration, geometry or values",
            )
        };
        if self.id.is_empty()
            || self.id.len() > 128
            || !self.id.is_ascii()
            || self.applicability.trim().is_empty()
            || self.applicability.len() > 1024
        {
            return Err(bad());
        }
        let scalar = |address: u32, width: u8, values: &[u32]| -> Result<()> {
            if !matches!(width, 1 | 2 | 4)
                || !address.is_multiple_of(u32::from(width))
                || u64::from(address) + u64::from(width) > u64::from(u32::MAX - 1)
                || values.iter().any(|n| width < 4 && *n >> (width * 8) != 0)
            {
                return Err(bad());
            }
            Ok(())
        };
        match &self.behavior {
            DeviceBehavior::CommandBank(bank) => bank.validate()?,
            DeviceBehavior::RegisterBank { cells } => {
                if cells.is_empty() || cells.len() > MAX_DEVICE_PORTS {
                    return Err(bad());
                }
                for cell in cells {
                    scalar(cell.address, cell.width, &[cell.value])?;
                }
            }
            DeviceBehavior::ConstantRead {
                address,
                width,
                value,
            } => scalar(*address, *width, &[*value])?,
            DeviceBehavior::SequenceRead {
                address,
                width,
                runs,
            } => {
                ReadRun::total(runs)?;
                for run in runs {
                    scalar(*address, *width, &[run.value])?;
                }
            }
            DeviceBehavior::W1c {
                address,
                width,
                initial,
                clear_mask,
                read_clear_mask,
            } => scalar(*address, *width, &[*initial, *clear_mask, *read_clear_mask])?,
            DeviceBehavior::ReadClear {
                address,
                width,
                initial,
                clear_mask,
            } => scalar(*address, *width, &[*initial, *clear_mask])?,
            DeviceBehavior::SelfClearing {
                address,
                width,
                initial,
                store_mask,
                command_mask,
            } => {
                scalar(*address, *width, &[*initial, *store_mask, *command_mask])?;
                if store_mask & command_mask != 0 || initial & command_mask != 0 {
                    return Err(bad());
                }
            }
            DeviceBehavior::Fifo {
                address,
                width,
                reads,
                writes,
            } => {
                if reads.len() > MAX_DEVICE_VALUES || writes.len() > MAX_DEVICE_VALUES {
                    return Err(bad());
                }
                scalar(*address, *width, reads)?;
                scalar(*address, *width, writes)?;
            }
            DeviceBehavior::CyclicRead {
                address,
                width,
                values,
            } => {
                if values.is_empty() || values.len() > MAX_DEVICE_VALUES {
                    return Err(bad());
                }
                scalar(*address, *width, values)?;
            }
            DeviceBehavior::SelectedSequence {
                selector_address,
                value_address,
                width,
                sequences,
            } => {
                if selector_address == value_address
                    || sequences.is_empty()
                    || sequences.len() > MAX_DEVICE_VALUES
                    || sequences
                        .iter()
                        .enumerate()
                        .any(|(i, s)| sequences[..i].iter().any(|t| t.selector == s.selector))
                {
                    return Err(bad());
                }
                let selectors: Vec<u32> = sequences.iter().map(|s| s.selector).collect();
                scalar(*selector_address, *width, &selectors)?;
                for sequence in sequences {
                    ReadRun::total(&sequence.runs)?;
                    for run in &sequence.runs {
                        scalar(*value_address, *width, &[run.value])?;
                    }
                }
            }
            DeviceBehavior::RetainedAperture { start, length, .. } => {
                if !start.is_multiple_of(4)
                    || !length.is_multiple_of(4)
                    || *length == 0
                    || *length > MAX_APERTURE_BYTES
                    || u64::from(*start) + u64::from(*length) > u64::from(u32::MAX - 1)
                {
                    return Err(bad());
                }
            }
            DeviceBehavior::IndexedBank {
                index_address,
                data_address,
                width,
                index,
                values,
            } => {
                if index_address == data_address
                    || values.is_empty()
                    || values.len() > MAX_DEVICE_VALUES
                    || index.is_some_and(|i| i as usize >= values.len())
                {
                    return Err(bad());
                }
                scalar(*index_address, *width, &[(values.len() - 1) as u32])?;
                scalar(*data_address, *width, values)?;
                if let Some(index) = index {
                    scalar(*index_address, *width, &[*index])?;
                }
            }
        }
        Ok(())
    }
    /// Dynamic bytes copied by cloning this declaration, excluding allocator bookkeeping.
    pub fn payload_bytes(&self) -> u64 {
        let values = match &self.behavior {
            DeviceBehavior::CommandBank(bank) => bank.payload_bytes() as usize,
            DeviceBehavior::RegisterBank { cells } => {
                cells.len() * std::mem::size_of::<RegisterCell>()
            }
            DeviceBehavior::SequenceRead { runs, .. } => {
                runs.len() * std::mem::size_of::<ReadRun>()
            }
            DeviceBehavior::IndexedBank { values, .. }
            | DeviceBehavior::CyclicRead { values, .. } => values.len() * 4,
            DeviceBehavior::Fifo { reads, writes, .. } => (reads.len() + writes.len()) * 4,
            DeviceBehavior::SelectedSequence { sequences, .. } => sequences
                .iter()
                .map(|s| 4 + s.runs.len() * std::mem::size_of::<ReadRun>())
                .sum(),
            _ => 0,
        };
        (self.id.len() + self.applicability.len() + values) as u64
    }
    /// Stable v2 content identity includes every declaration field, value and repeat count.
    /// Fields use explicit tags, little-endian u32 words and length-prefixed strings/lists.
    pub fn identity(&self, c: &mut dyn RunControl) -> Result<ArtifactId> {
        self.validate()?;
        let mut h = Sha256::new();
        h.update(b"blobray-device-v2\0");
        for s in [&self.id, &self.applicability] {
            c.bytes(s.len())?;
            h.update((s.len() as u32).to_le_bytes());
            h.update(s.as_bytes());
        }
        h.update([match self.lifetime {
            RegionLifetime::Phase => 0,
            RegionLifetime::Session => 1,
        }]);
        let mut put = |n: u32| -> Result<()> {
            c.checkpoint(1)?;
            h.update(n.to_le_bytes());
            Ok(())
        };
        match &self.behavior {
            DeviceBehavior::CommandBank(bank) => {
                put(8)?;
                bank.identity_words(&mut put)?;
            }
            DeviceBehavior::RegisterBank { cells } => {
                put(0)?;
                put(cells.len() as u32)?;
                for cell in cells {
                    put(cell.address)?;
                    put(u32::from(cell.width))?;
                    put(cell.value)?;
                }
            }
            DeviceBehavior::ConstantRead {
                address,
                width,
                value,
            } => {
                for n in [1, *address, u32::from(*width), *value] {
                    put(n)?;
                }
            }
            DeviceBehavior::SequenceRead {
                address,
                width,
                runs,
            } => {
                for n in [2, *address, u32::from(*width), runs.len() as u32] {
                    put(n)?;
                }
                for run in runs {
                    put(run.value)?;
                    put(run.count)?;
                }
            }
            DeviceBehavior::W1c {
                address,
                width,
                initial,
                clear_mask,
                read_clear_mask,
            } => {
                for n in [
                    3,
                    *address,
                    u32::from(*width),
                    *initial,
                    *clear_mask,
                    *read_clear_mask,
                ] {
                    put(n)?;
                }
            }
            DeviceBehavior::ReadClear {
                address,
                width,
                initial,
                clear_mask,
            } => {
                for n in [4, *address, u32::from(*width), *initial, *clear_mask] {
                    put(n)?;
                }
            }
            DeviceBehavior::SelfClearing {
                address,
                width,
                initial,
                store_mask,
                command_mask,
            } => {
                for n in [
                    5,
                    *address,
                    u32::from(*width),
                    *initial,
                    *store_mask,
                    *command_mask,
                ] {
                    put(n)?;
                }
            }
            DeviceBehavior::Fifo {
                address,
                width,
                reads,
                writes,
            } => {
                for n in [6, *address, u32::from(*width), reads.len() as u32] {
                    put(n)?;
                }
                for n in reads {
                    put(*n)?;
                }
                put(writes.len() as u32)?;
                for n in writes {
                    put(*n)?;
                }
            }
            DeviceBehavior::IndexedBank {
                index_address,
                data_address,
                width,
                index,
                values,
            } => {
                for n in [
                    7,
                    *index_address,
                    *data_address,
                    u32::from(*width),
                    u32::from(index.is_some()),
                    index.unwrap_or(0),
                    values.len() as u32,
                ] {
                    put(n)?;
                }
                for n in values {
                    put(*n)?;
                }
            }
            DeviceBehavior::CyclicRead {
                address,
                width,
                values,
            } => {
                for n in [10, *address, u32::from(*width), values.len() as u32] {
                    put(n)?;
                }
                for n in values {
                    put(*n)?;
                }
            }
            DeviceBehavior::RetainedAperture {
                start,
                length,
                initial,
            } => {
                for n in [9, *start, *length, *initial] {
                    put(n)?;
                }
            }
            DeviceBehavior::SelectedSequence {
                selector_address,
                value_address,
                width,
                sequences,
            } => {
                for n in [
                    11,
                    *selector_address,
                    *value_address,
                    u32::from(*width),
                    sequences.len() as u32,
                ] {
                    put(n)?;
                }
                for sequence in sequences {
                    put(sequence.selector)?;
                    put(sequence.runs.len() as u32)?;
                    for run in &sequence.runs {
                        put(run.value)?;
                        put(run.count)?;
                    }
                }
            }
        }
        format!("{:x}", h.finalize()).parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_read_runs_validate_counts_width_and_identity_without_expansion() {
        let declaration = |runs| DeviceDeclaration {
            id: "sequence".into(),
            applicability: "bounded synthetic polling".into(),
            lifetime: RegionLifetime::Session,
            behavior: DeviceBehavior::SequenceRead {
                address: 0x1000,
                width: 1,
                runs,
            },
        };
        let huge = declaration(vec![ReadRun {
            value: 7,
            count: u32::MAX,
        }]);
        huge.validate().unwrap();
        let one = declaration(vec![ReadRun::once(7)]);
        assert_eq!(huge.payload_bytes(), one.payload_bytes());
        assert_ne!(
            huge.identity(&mut || Ok(())).unwrap(),
            one.identity(&mut || Ok(())).unwrap()
        );
        for runs in [
            vec![],
            vec![ReadRun { value: 0, count: 0 }],
            vec![
                ReadRun {
                    value: 0,
                    count: u32::MAX,
                },
                ReadRun::once(0),
            ],
            vec![ReadRun::once(256)],
            vec![ReadRun::once(0); MAX_DEVICE_VALUES + 1],
        ] {
            assert_eq!(
                declaration(runs).validate().unwrap_err().code,
                ErrorCode::InvalidRequest
            );
        }
    }
    #[test]
    fn identity_binds_assumptions_lifetime_and_order() {
        let d = DeviceDeclaration {
            id: "fifo".into(),
            applicability: "fixture input domain".into(),
            lifetime: RegionLifetime::Session,
            behavior: DeviceBehavior::Fifo {
                address: 0x1000,
                width: 4,
                reads: vec![1, 2],
                writes: vec![3, 4],
            },
        };
        let identity = d.identity(&mut || Ok(())).unwrap();
        for variant in 0..7 {
            let mut changed = d.clone();
            match variant {
                0 => changed.id.push('2'),
                1 => changed.applicability.push('2'),
                2 => changed.lifetime = RegionLifetime::Phase,
                _ => {
                    if let DeviceBehavior::Fifo {
                        address,
                        width,
                        reads,
                        writes,
                    } = &mut changed.behavior
                    {
                        match variant {
                            3 => *address += 4,
                            4 => *width = 2,
                            5 => reads.reverse(),
                            6 => writes.reverse(),
                            _ => unreachable!(),
                        }
                    }
                }
            }
            assert_ne!(identity, changed.identity(&mut || Ok(())).unwrap());
        }
        assert_eq!(
            d.identity(&mut || Err(Error::new(ErrorCode::Cancelled, "fixture")))
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
    }
}
