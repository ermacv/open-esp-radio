//! Explicit packed-command ports over a shared bounded register bank.
use crate::*;

pub const MAX_COMMAND_PORTS: usize = 64;

/// Caller-selected wire geometry; no chip address or command layout is inferred.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandBank {
    /// Nonempty disjoint contiguous fields, each at most sixteen bits.
    pub selector_mask: u32,
    pub data_mask: u32,
    /// Exact fixed bits outside selector/data fields. Busy must be clear.
    pub read_command: u32,
    pub write_command: u32,
    /// One response bit; never a valid bit of an ordinary issued command.
    pub busy_mask: u32,
    /// Optional exact reset word. It cannot also address a declared cell.
    pub reset_command: Option<u32>,
    /// Strictly increasing aligned addresses.
    pub ports: Vec<CommandPort>,
    pub cells: Vec<CommandCell>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandPort {
    pub address: u32,
    /// Idle response with busy clear. Initial busy is a separate obligation.
    pub initial: u32,
    pub initial_busy_reads: u32,
    /// Each issue returns busy this many times, then requires a ready read.
    /// Poll counts are an environmental assumption, not hardware time.
    pub busy_reads: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandCell {
    /// Unshifted selector; cells are strictly ordered by selector.
    pub selector: u32,
    pub initial: u32,
    /// None reads retained state; Some consumes one sample per read issue.
    /// Some(empty) is exhausted, never an implicit retained-value fallback.
    pub reads: Option<Vec<u32>>,
}
/// Cumulative instance accounting, separate from CPU port read/write counts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandObservation {
    pub issued: u64,
    pub completed: u64,
    pub resets: u64,
    pub aborted: u64,
    pub pending: u32,
    pub scripted_reads: u32,
}
impl CommandBank {
    pub fn validate(&self) -> Result<()> {
        let bad = || Error::new(ErrorCode::InvalidRequest, "invalid packed-command bank");
        let field = |mask: u32| {
            mask != 0 && mask.count_ones() <= 16 && {
                let n = u64::from(mask >> mask.trailing_zeros());
                n & (n + 1) == 0
            }
        };
        if !field(self.selector_mask)
            || !field(self.data_mask)
            || self.selector_mask & self.data_mask != 0
            || !self.busy_mask.is_power_of_two()
            || self.busy_mask & (self.selector_mask | self.data_mask) != 0
            || self.read_command == self.write_command
            || (self.read_command | self.write_command)
                & (self.selector_mask | self.data_mask | self.busy_mask)
                != 0
            || self.ports.is_empty()
            || self.ports.len() > MAX_COMMAND_PORTS
            || self.cells.is_empty()
            || self.cells.len() > MAX_DEVICE_VALUES
        {
            return Err(bad());
        }
        let selector_max = self.selector_mask >> self.selector_mask.trailing_zeros();
        let data_max = self.data_mask >> self.data_mask.trailing_zeros();
        for (i, p) in self.ports.iter().enumerate() {
            if p.address % 4 != 0
                || p.address > u32::MAX - 4
                || p.initial & self.busy_mask != 0
                || i != 0 && self.ports[i - 1].address >= p.address
            {
                return Err(bad());
            }
        }
        let mut samples = 0usize;
        for (i, cell) in self.cells.iter().enumerate() {
            if cell.selector > selector_max
                || cell.initial > data_max
                || i != 0 && self.cells[i - 1].selector >= cell.selector
            {
                return Err(bad());
            }
            if let Some(reads) = &cell.reads {
                samples = samples.checked_add(reads.len()).ok_or_else(bad)?;
                if samples > MAX_DEVICE_VALUES || reads.iter().any(|v| *v > data_max) {
                    return Err(bad());
                }
            }
        }
        if let Some(reset) = self.reset_command {
            if reset & self.busy_mask != 0 {
                return Err(bad());
            }
            let fixed = reset & !(self.selector_mask | self.data_mask);
            let selector = (reset & self.selector_mask) >> self.selector_mask.trailing_zeros();
            if (fixed == self.read_command || fixed == self.write_command)
                && self.cells.iter().any(|cell| cell.selector == selector)
            {
                return Err(bad());
            }
        }
        Ok(())
    }
    pub fn samples(&self) -> usize {
        self.cells
            .iter()
            .filter_map(|c| c.reads.as_ref())
            .map(Vec::len)
            .sum()
    }
    pub fn initial_pending(&self) -> u32 {
        self.ports
            .iter()
            .filter(|p| p.initial_busy_reads != 0)
            .count() as u32
    }
    /// Nested declaration allocations, excluding the inline owner and allocator bookkeeping.
    pub fn payload_bytes(&self) -> u64 {
        (self.ports.len() * std::mem::size_of::<CommandPort>()
            + self.cells.len() * std::mem::size_of::<CommandCell>()
            + self.samples() * 4) as u64
    }
    pub(crate) fn identity_words(&self, put: &mut dyn FnMut(u32) -> Result<()>) -> Result<()> {
        for n in [
            self.selector_mask,
            self.data_mask,
            self.read_command,
            self.write_command,
            self.busy_mask,
            u32::from(self.reset_command.is_some()),
            self.reset_command.unwrap_or(0),
            self.ports.len() as u32,
        ] {
            put(n)?;
        }
        for p in &self.ports {
            for n in [p.address, p.initial, p.initial_busy_reads, p.busy_reads] {
                put(n)?;
            }
        }
        put(self.cells.len() as u32)?;
        for cell in &self.cells {
            put(cell.selector)?;
            put(cell.initial)?;
            put(u32::from(cell.reads.is_some()))?;
            if let Some(reads) = &cell.reads {
                put(reads.len() as u32)?;
                for value in reads {
                    put(*value)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> DeviceDeclaration {
        DeviceDeclaration {
            id: "commands".into(),
            applicability: "synthetic wire".into(),
            lifetime: RegionLifetime::Session,
            behavior: DeviceBehavior::CommandBank(CommandBank {
                selector_mask: 0xff,
                data_mask: 0xff00,
                read_command: 0x10000,
                write_command: 0x20000,
                busy_mask: 0x40000,
                reset_command: Some(0x80000),
                ports: vec![CommandPort {
                    address: 0x1000,
                    initial: 0,
                    initial_busy_reads: 1,
                    busy_reads: 2,
                }],
                cells: vec![CommandCell {
                    selector: 1,
                    initial: 9,
                    reads: Some(vec![3, 4]),
                }],
            }),
        }
    }
    #[test]
    fn command_identity_covers_every_wire_state_and_transcript_input() {
        let d = fixture();
        let identity = d.identity(&mut || Ok(())).unwrap();
        for change in 0..14 {
            let mut changed = d.clone();
            let DeviceBehavior::CommandBank(b) = &mut changed.behavior else {
                unreachable!()
            };
            match change {
                0 => b.selector_mask = 0xf,
                1 => b.data_mask = 0xf00,
                2 => b.read_command = 0x100000,
                3 => b.write_command = 0x200000,
                4 => b.busy_mask = 0x400000,
                5 => b.reset_command = None,
                6 => b.ports[0].address += 4,
                7 => b.ports[0].initial = 1,
                8 => b.ports[0].initial_busy_reads = 0,
                9 => b.ports[0].busy_reads += 1,
                10 => b.cells[0].selector = 2,
                11 => b.cells[0].initial = 8,
                12 => b.cells[0].reads = None,
                13 => b.cells[0].reads.as_mut().unwrap().reverse(),
                _ => unreachable!(),
            }
            assert_ne!(identity, changed.identity(&mut || Ok(())).unwrap());
        }
    }
    #[test]
    fn ambiguous_commands_overlapping_fields_and_unbounded_geometry_are_rejected() {
        for change in 0..13 {
            let DeviceBehavior::CommandBank(mut b) = fixture().behavior else {
                unreachable!()
            };
            match change {
                0 => b.selector_mask = 0,
                1 => b.data_mask = 0x501,
                2 => b.data_mask = b.selector_mask,
                3 => b.busy_mask = 3,
                4 => b.read_command = b.write_command,
                5 => b.write_command |= b.data_mask,
                6 => b.reset_command = Some(b.read_command | 1),
                7 => b.ports.push(b.ports[0].clone()),
                8 => b.ports[0].initial = b.busy_mask,
                9 => b.cells[0].initial = 256,
                10 => b.cells.push(b.cells[0].clone()),
                11 => b.cells[0].reads = Some(vec![0; MAX_DEVICE_VALUES + 1]),
                12 => b.ports.clear(),
                _ => unreachable!(),
            }
            assert_eq!(
                b.validate().unwrap_err().code,
                ErrorCode::InvalidRequest,
                "{change}"
            );
        }
    }
}
