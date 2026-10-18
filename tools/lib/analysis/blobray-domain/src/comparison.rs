//! Explicit selected observation relations; no inferred ABI or layout equivalence.
use crate::*;
pub const MAX_MEMORY_SELECTIONS: usize = 128;
pub const MAX_OBSERVED_MEMORY_BYTES: u32 = 1024 * 1024;
pub const MEMORY_CHUNK_BYTES: usize = 16;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySelection {
    pub name: String,
    pub address: u32,
    pub length: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnWords {
    pub low: bool,
    pub high: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventChannels {
    pub timeline: TimelineCapture,
    pub mmio_read: bool,
    pub mmio_write: bool,
    pub fence: bool,
    pub delay: bool,
}
impl EventChannels {
    pub fn any(self) -> bool {
        self.mmio_read || self.mmio_write || self.fence || self.delay || self.timeline.any()
    }
    pub fn selects(self, event: &ExecutionEvent) -> bool {
        if let Some(transaction) = event.normal_memory() {
            return self.timeline.selects(&transaction);
        }
        match event {
            ExecutionEvent::Branch { .. } => self.timeline.branches,
            ExecutionEvent::Read { .. } => self.mmio_read,
            ExecutionEvent::Write { .. } => self.mmio_write,
            ExecutionEvent::Fence { .. } => self.fence,
            ExecutionEvent::DelayMicros { .. } => self.delay,
            _ => false,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPair {
    pub vendor: u16,
    pub replacement: u16,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonRelation {
    /// The digest of the canonical encoding of the selected effect contract,
    /// reviewed outside Blobray and supplied with the comparison.
    pub effects: Option<ArtifactId>,
    /// The digest of the canonical encoding of the selected layout projection.
    pub projection: Option<ArtifactId>,
    pub returns: ReturnWords,
    pub events: EventChannels,
    pub memory: Vec<MemoryPair>,
    pub calls: bool,
}
impl ComparisonRelation {
    pub fn validate(&self, vendor: &Invocation, replacement: &Invocation) -> Result<()> {
        let require = |condition: bool, detail: &str| {
            if condition {
                Ok(())
            } else {
                Err(Error::new(
                    ErrorCode::InvalidRequest,
                    format!("invalid comparison relation: {detail}"),
                ))
            }
        };
        require(
            self.memory.len() <= MAX_MEMORY_SELECTIONS,
            &format!("more than {MAX_MEMORY_SELECTIONS} memory pairs"),
        )?;
        require(
            self.returns.low
                || self.returns.high
                || self.events.mmio_read
                || self.events.mmio_write
                || self.events.fence
                || self.events.delay
                || self.events.timeline.any()
                || !self.memory.is_empty()
                || self.calls
                || self.projection.is_some(),
            "it compares nothing: select returns, an event channel, memory, calls or a projection",
        )?;
        require(
            self.effects.is_none()
                || (self.events.mmio_read
                    && self.events.mmio_write
                    && self.events.fence
                    && self.events.delay),
            "an effect contract needs every concrete effect channel: MMIO reads and writes, fences and delays",
        )?;
        require(
            vendor.observe_timeline.contains(self.events.timeline)
                && replacement.observe_timeline.contains(self.events.timeline),
            "a compared timeline channel is not observed by both invocations",
        )?;
        require(
            !(self.returns.low || self.returns.high)
                || (matches!(vendor.goal, ExecutionGoal::Return)
                    && matches!(replacement.goal, ExecutionGoal::Return)),
            "compared returns need a return goal on both sides",
        )?;
        require(
            !self.calls
                || (vendor.observe_calls.is_some()
                    && vendor.observe_calls == replacement.observe_calls),
            "compared calls need the same call capture on both sides",
        )?;
        for (i, pair) in self.memory.iter().enumerate() {
            let (Some(a), Some(b)) = (
                vendor.observe_memory.get(pair.vendor as usize),
                replacement.observe_memory.get(pair.replacement as usize),
            ) else {
                return require(
                    false,
                    &format!(
                        "memory pair {i} names selection {} / {} that the invocations do not observe",
                        pair.vendor, pair.replacement
                    ),
                );
            };
            require(
                a.length == b.length,
                &format!(
                    "memory pair {i} pairs `{}` ({} bytes) with `{}` ({} bytes)",
                    a.name, a.length, b.name, b.length
                ),
            )?;
            require(
                !self.memory[..i]
                    .iter()
                    .any(|p| p.vendor == pair.vendor || p.replacement == pair.replacement),
                &format!("memory pair {i} reuses a selection of an earlier pair"),
            )?;
        }
        Ok(())
    }
}
impl Invocation {
    pub fn validate_memory_selection(&self) -> Result<()> {
        let bad = |detail: String| {
            Err(Error::new(
                ErrorCode::InvalidRequest,
                format!("invalid final-memory selection: {detail}"),
            ))
        };
        if self.observe_memory.len() > MAX_MEMORY_SELECTIONS {
            return bad(format!(
                "{} selections exceed {MAX_MEMORY_SELECTIONS}",
                self.observe_memory.len()
            ));
        }
        let mut total = 0u64;
        for (i, s) in self.observe_memory.iter().enumerate() {
            total += u64::from(s.length);
            if s.name.trim().is_empty() || s.name.len() > 128 || !s.name.is_ascii() {
                return bad(format!(
                    "selection {i} name `{}` is not a short ASCII name",
                    s.name
                ));
            }
            if s.length == 0
                || u64::from(s.address) + u64::from(s.length) >= u64::from(u32::MAX - 1)
            {
                return bad(format!(
                    "selection `{}` at {:#x}+{:#x} is empty or leaves the address space",
                    s.name, s.address, s.length
                ));
            }
            if total > u64::from(MAX_OBSERVED_MEMORY_BYTES) {
                return bad(format!(
                    "selections exceed {MAX_OBSERVED_MEMORY_BYTES} observed bytes at `{}`",
                    s.name
                ));
            }
            if let Some(p) = self.observe_memory[..i].iter().find(|p| p.name == s.name) {
                return bad(format!("selection name `{}` repeats", p.name));
            }
            if let Some(p) = self.observe_memory[..i].iter().find(|p| {
                u64::from(s.address) < u64::from(p.address) + u64::from(p.length)
                    && u64::from(p.address) < u64::from(s.address) + u64::from(s.length)
            }) {
                return bad(format!(
                    "selections `{}` at {:#x}+{:#x} and `{}` at {:#x}+{:#x} overlap",
                    p.name, p.address, p.length, s.name, s.address, s.length
                ));
            }
        }
        Ok(())
    }
    pub fn memory_chunks(&self) -> usize {
        self.observe_memory
            .iter()
            .map(|s| s.length.div_ceil(MEMORY_CHUNK_BYTES as u32) as usize)
            .sum()
    }
}
/// Every selected byte is represented, even unchanged, unknown or unavailable bytes.
/// Bits outside `length` and bytes whose known bit is clear are canonical zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalMemoryChunk {
    pub selection: u16,
    pub offset: u32,
    pub length: u8,
    pub bytes: [u8; MEMORY_CHUNK_BYTES],
    pub available: u16,
    pub known: u16,
}
impl FinalMemoryChunk {
    pub fn mask(&self) -> Option<u16> {
        match self.length {
            1..=15 => Some((1u16 << self.length) - 1),
            16 => Some(u16::MAX),
            _ => None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        let invalid = || Error::new(ErrorCode::Integrity, "invalid final-memory chunk");
        let mask = self.mask().ok_or_else(invalid)?;
        if self.available & !mask != 0
            || self.known & !self.available != 0
            || self
                .bytes
                .iter()
                .enumerate()
                .any(|(i, b)| (self.known & (1 << i)) == 0 && *b != 0)
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn complete(&self) -> bool {
        self.mask() == Some(self.known)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ComparisonDifference {
    EffectViolation {
        violation: EffectViolation,
    },
    ProjectedMemory {
        field: u16,
        offset: u32,
        vendor: u8,
        replacement: u8,
    },
    CallTarget {
        index: u32,
        vendor: u32,
        replacement: u32,
    },
    CallArgument {
        index: u32,
        word: u16,
        vendor: u32,
        replacement: u32,
    },
    Event {
        index: u32,
    },
    Return {
        word: u8,
        vendor: u32,
        replacement: u32,
    },
    Memory {
        pair: u16,
        offset: u32,
        vendor: u8,
        replacement: u8,
    },
}
