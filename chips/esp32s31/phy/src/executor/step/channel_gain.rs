//! Retained publication of the existing channel gain image, by whole entries.
use super::{Failure, io::Io};
use crate::{channel::PhyWifiTxGainImage, tracking::observation::EngineTiming};

// The measured 32-entry atom costs 63 us. Eight entries leave margin for
// model/dispatcher entry and the first publication's base-index read.
const ENTRIES_PER_POLL: u8 = 8;

#[derive(Default)]
pub(super) struct Cursor {
    next: u8,
    base: u8,
    maximum_entries: [u8; 2],
    maximum_micros: [u32; 2],
}

impl Cursor {
    #[inline(never)]
    pub(super) fn advance(
        &mut self,
        io: &mut Io<'_, '_>,
        image: &PhyWifiTxGainImage,
    ) -> Result<bool, Failure> {
        if self.next == 0 {
            self.base = io.channel_gain_base()?;
        }
        let end = (self.next + ENTRIES_PER_POLL).min(32);
        while self.next < end {
            let entry = crate::hardware::tx_gain_memory_entry(image, self.base, self.next);
            io.gain_entry(entry)?;
            self.next += 1;
        }
        Ok(self.next == 32)
    }

    pub(super) fn observe(&mut self, timing: &EngineTiming, terminal: bool) {
        for (part, micros) in [timing.maximum_micros, timing.maximum_ready_micros]
            .into_iter()
            .enumerate()
        {
            if micros > self.maximum_micros[part] {
                self.maximum_micros[part] = micros;
                // Zero: non-publication action. 255: the terminal driver poll.
                self.maximum_entries[part] = if terminal { 255 } else { self.next };
            }
        }
    }

    pub(super) fn maximum_entries(&self) -> [u8; 2] {
        self.maximum_entries
    }
}
