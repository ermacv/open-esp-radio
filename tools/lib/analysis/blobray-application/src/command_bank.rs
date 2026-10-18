//! Admitted mutable command-port state; geometry/assumptions are domain values.
use crate::*;
#[derive(Clone, Copy)]
enum Pending {
    Initial,
    Read,
    Write { cell: usize, value: u32 },
    Reset,
}
struct PortState {
    response: u32,
    busy: u32,
    pending: Option<Pending>,
}
struct CellState {
    value: u32,
    cursor: usize,
}
pub(crate) struct CommandState<'m> {
    ports: AdmittedVec<'m, PortState>,
    cells: AdmittedVec<'m, CellState>,
    observation: CommandObservation,
    samples: u32,
}
impl<'m> CommandState<'m> {
    pub fn new(
        bank: &CommandBank,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        c.checkpoint((bank.cells.len() + bank.ports.len()) as u64 + 1)?;
        let mut result = Self {
            ports: AdmittedVec::new(memory),
            cells: AdmittedVec::new(memory),
            observation: CommandObservation {
                pending: bank.initial_pending(),
                ..Default::default()
            },
            samples: bank.samples() as u32,
        };
        for p in &bank.ports {
            c.checkpoint(1)?;
            result.ports.push(
                PortState {
                    response: p.initial,
                    busy: p.initial_busy_reads,
                    pending: (p.initial_busy_reads != 0).then_some(Pending::Initial),
                },
                c.position(),
            )?;
        }
        for cell in &bank.cells {
            c.checkpoint(1)?;
            result.cells.push(
                CellState {
                    value: cell.initial,
                    cursor: 0,
                },
                c.position(),
            )?;
        }
        Ok(result)
    }
    pub fn read(&mut self, bank: &CommandBank, slot: usize) -> Result<u32> {
        let port = &mut self.ports[slot];
        if port.pending.is_some() {
            if port.busy != 0 {
                port.busy -= 1;
                return Ok(port.response | bank.busy_mask);
            }
            let completed = increment(self.observation.completed)?;
            if let Some(Pending::Write { cell, value }) = port.pending {
                self.cells[cell].value = value;
            }
            port.pending = None;
            self.observation.completed = completed;
            self.observation.pending -= 1;
        }
        Ok(port.response)
    }
    pub fn write(
        &mut self,
        bank: &CommandBank,
        slot: usize,
        word: u32,
        c: &mut dyn RunControl,
    ) -> Result<std::result::Result<(), DeviceIssue>> {
        // Admission/checkpoint precedes every cursor or bank mutation.
        c.checkpoint(bank.cells.len().max(1).ilog2() as u64 + 1)?;
        let issued = increment(self.observation.issued)?;
        let port = &mut self.ports[slot];
        if bank.reset_command == Some(word) {
            let resets = increment(self.observation.resets)?;
            let aborted = if port.pending.is_some() {
                increment(self.observation.aborted)?
            } else {
                self.observation.aborted
            };
            if port.pending.is_none() {
                self.observation.pending += 1;
            }
            self.observation.resets = resets;
            self.observation.aborted = aborted;
            self.observation.issued = issued;
            port.pending = Some(Pending::Reset);
            port.response = word;
            port.busy = bank.ports[slot].busy_reads;
            return Ok(Ok(()));
        }
        if port.pending.is_some() {
            return Ok(Err(DeviceIssue::PendingCommand));
        }
        let fixed = word & !(bank.selector_mask | bank.data_mask);
        if fixed != bank.read_command && fixed != bank.write_command {
            return Ok(Err(DeviceIssue::InvalidCommand { value: word }));
        }
        // Read commands carry no input data. No ignored command bits.
        if fixed == bank.read_command && word & bank.data_mask != 0 {
            return Ok(Err(DeviceIssue::InvalidCommand { value: word }));
        }
        let selector = (word & bank.selector_mask) >> bank.selector_mask.trailing_zeros();
        let Ok(index) = bank.cells.binary_search_by_key(&selector, |c| c.selector) else {
            return Ok(Err(DeviceIssue::UnknownSelector { selector }));
        };
        let state = &mut self.cells[index];
        let (response, pending) = if fixed == bank.write_command {
            let value = (word & bank.data_mask) >> bank.data_mask.trailing_zeros();
            (word, Pending::Write { cell: index, value })
        } else {
            let value = if let Some(reads) = &bank.cells[index].reads {
                let Some(value) = reads.get(state.cursor) else {
                    return Ok(Err(DeviceIssue::ExhaustedReads));
                };
                state.cursor += 1;
                self.observation.scripted_reads += 1;
                *value
            } else {
                state.value
            };
            (
                word | (value << bank.data_mask.trailing_zeros()),
                Pending::Read,
            )
        };
        port.response = response;
        port.pending = Some(pending);
        port.busy = bank.ports[slot].busy_reads;
        self.observation.issued = issued;
        self.observation.pending += 1;
        Ok(Ok(()))
    }
    pub fn observation(&self) -> CommandObservation {
        self.observation
    }
    pub fn remaining(&self) -> u32 {
        self.samples - self.observation.scripted_reads
    }
}
fn increment(value: u64) -> Result<u64> {
    value
        .checked_add(1)
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "command counter exhausted"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bank() -> CommandBank {
        CommandBank {
            selector_mask: 0xff,
            data_mask: 0xff00,
            read_command: 0x10000,
            write_command: 0x20000,
            busy_mask: 0x40000,
            reset_command: Some(0x80000),
            ports: vec![
                CommandPort {
                    address: 0x1000,
                    initial: 0,
                    initial_busy_reads: 0,
                    busy_reads: 1,
                },
                CommandPort {
                    address: 0x1004,
                    initial: 0,
                    initial_busy_reads: 0,
                    busy_reads: 0,
                },
            ],
            cells: vec![
                CommandCell {
                    selector: 1,
                    initial: 42,
                    reads: None,
                },
                CommandCell {
                    selector: 2,
                    initial: 0,
                    reads: Some(vec![7, 8]),
                },
            ],
        }
    }
    fn issue(s: &mut CommandState<'_>, b: &CommandBank, slot: usize, word: u32) {
        assert_eq!(s.write(b, slot, word, &mut || Ok(())).unwrap(), Ok(()));
    }
    #[test]
    fn both_ports_share_committed_values_and_reset_discards_only_pending_write() {
        let b = bank();
        let m = WorkingMemory::new(8192).unwrap();
        let mut s = CommandState::new(&b, &m, &mut || Ok(())).unwrap();
        issue(&mut s, &b, 0, 0x26301);
        assert_eq!(s.read(&b, 0).unwrap(), 0x66301); // Busy; staged value is not visible.
        issue(&mut s, &b, 1, 0x10001);
        assert_eq!(s.read(&b, 1).unwrap(), 0x12a01);
        assert_eq!(s.observation().pending, 1);
        assert_eq!(s.read(&b, 0).unwrap(), 0x26301); // Ready commits 99.
        issue(&mut s, &b, 1, 0x10001);
        assert_eq!(s.read(&b, 1).unwrap(), 0x16301);
        issue(&mut s, &b, 0, 0x20101); // Pending write of 1.
        issue(&mut s, &b, 0, 0x80000); // Reset aborts it, but itself needs completion.
        assert_eq!(s.observation().pending, 1);
        assert_eq!(s.observation().aborted, 1);
        assert_eq!(s.read(&b, 0).unwrap(), 0xc0000);
        assert_eq!(s.read(&b, 0).unwrap(), 0x80000);
        issue(&mut s, &b, 1, 0x10001);
        assert_eq!(s.read(&b, 1).unwrap(), 0x16301);
        assert_eq!(s.observation().issued, 6);
        assert_eq!(s.observation().completed, 5);
        assert_eq!(s.observation().pending, 0);
        drop(s);
        assert_eq!(m.observation().reserved_bytes, 0);
    }
    #[test]
    fn scripted_samples_are_consumed_once_at_issue_without_retained_fallback() {
        let b = bank();
        let m = WorkingMemory::new(8192).unwrap();
        let mut s = CommandState::new(&b, &m, &mut || Ok(())).unwrap();
        for value in [7, 8] {
            issue(&mut s, &b, 0, 0x10002);
            assert_eq!(s.read(&b, 0).unwrap(), 0x50002 | value << 8);
            assert_eq!(s.read(&b, 0).unwrap(), 0x10002 | value << 8);
            assert_eq!(s.read(&b, 0).unwrap(), 0x10002 | value << 8);
        }
        let before = s.observation();
        assert_eq!(
            s.write(&b, 0, 0x10002, &mut || Ok(())).unwrap(),
            Err(DeviceIssue::ExhaustedReads)
        );
        assert_eq!(s.observation(), before);
        assert_eq!(s.remaining(), 0);
    }
    #[test]
    fn rejected_or_cancelled_issue_keeps_bank_cursor_and_pending_state_unchanged() {
        let b = bank();
        let m = WorkingMemory::new(8192).unwrap();
        let mut s = CommandState::new(&b, &m, &mut || Ok(())).unwrap();
        for (word, issue) in [
            (0x10003, DeviceIssue::UnknownSelector { selector: 3 }),
            (0x10102, DeviceIssue::InvalidCommand { value: 0x10102 }),
            (0x50002, DeviceIssue::InvalidCommand { value: 0x50002 }),
        ] {
            assert_eq!(s.write(&b, 0, word, &mut || Ok(())).unwrap(), Err(issue));
            assert_eq!(s.observation(), CommandObservation::default());
        }
        let error = s
            .write(&b, 0, 0x10002, &mut || {
                Err(Error::new(ErrorCode::Cancelled, "cancel"))
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(s.remaining(), 2);
        issue(&mut s, &b, 0, 0x10002);
        let before = s.observation();
        assert_eq!(
            s.write(&b, 0, 0x10002, &mut || Ok(())).unwrap(),
            Err(DeviceIssue::PendingCommand)
        );
        assert_eq!(s.observation(), before);
    }
    #[test]
    fn failed_state_admission_releases_partial_allocations() {
        let mut b = bank();
        b.cells = (0..200)
            .map(|selector| CommandCell {
                selector,
                initial: 0,
                reads: None,
            })
            .collect();
        let m = WorkingMemory::new(512).unwrap();
        let error = CommandState::new(&b, &m, &mut || Ok(())).err().unwrap();
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        assert_eq!(m.observation().reserved_bytes, 0);
    }
    #[test]
    fn initial_busy_is_an_obligation_and_reset_does_not_rewind_shared_samples() {
        let mut b = bank();
        b.ports[0].initial_busy_reads = 1;
        let m = WorkingMemory::new(8192).unwrap();
        let mut s = CommandState::new(&b, &m, &mut || Ok(())).unwrap();
        assert_eq!(s.observation().pending, 1);
        assert_eq!(
            s.write(&b, 0, 0x10002, &mut || Ok(())).unwrap(),
            Err(DeviceIssue::PendingCommand)
        );
        assert_eq!(s.read(&b, 0).unwrap(), 0x40000);
        assert_eq!(s.read(&b, 0).unwrap(), 0);
        issue(&mut s, &b, 0, 0x10002);
        issue(&mut s, &b, 0, 0x80000);
        assert_eq!(s.observation().aborted, 1);
        assert_eq!(s.observation().scripted_reads, 1);
        assert_eq!(s.remaining(), 1);
        // The other port still consumes the next shared sample.
        issue(&mut s, &b, 1, 0x10002);
        assert_eq!(s.read(&b, 1).unwrap(), 0x10802);
        assert_eq!(s.remaining(), 0);
        assert_eq!(s.observation().pending, 1); // Reset itself is not yet acknowledged.
    }
}
