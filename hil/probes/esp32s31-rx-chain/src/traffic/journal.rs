//! Bounded retained values; contains no hardware access.
pub(super) const WIDTH: usize = 32;
pub(super) const CAPACITY: usize = 256;
pub(super) const UNIT_CAPACITY: usize = 12;
pub(super) const PAYLOAD_CAPACITY: usize = 4096;

#[cfg_attr(target_arch = "riscv32", derive(bytemuck::Zeroable))]
pub(crate) struct Unit {
    pub(super) header: [u32; 23],
    pub(super) after_irq: [u8; PAYLOAD_CAPACITY],
    pub(super) previous: [u8; PAYLOAD_CAPACITY],
    pub(super) early: [u8; PAYLOAD_CAPACITY],
    pub(super) taken: [u8; PAYLOAD_CAPACITY],
}

#[cfg_attr(target_arch = "riscv32", derive(bytemuck::Zeroable))]
pub(crate) struct Journal<const N: usize, const W: usize> {
    pub(super) pre: [[u32; W]; N],
    pub(super) previous_pre: [[u32; W]; N],
    pub(super) post: [[u32; W]; N],
    pub(super) previous_begin: u32,
    pub(super) previous_end: u32,
    pub(super) bindings: [u32; N],
    pub(super) rows: [[u32; WIDTH]; CAPACITY],
    pub(super) used: usize,
    pub(super) scans: u32,
    pub(super) units: [Unit; UNIT_CAPACITY],
    pub(super) unit_used: usize,
    previous: [u32; WIDTH],
}

impl<const N: usize, const W: usize> Journal<N, W> {
    pub(super) fn record(
        &mut self,
        mut row: [u32; WIDTH],
        phase: u32,
        force: bool,
    ) -> Result<(), oer_diagnostics::Name<32>> {
        let changed = self.previous.get(1).copied() != Some(phase)
            || [
                4, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 25, 26, 27, 29,
            ]
            .iter()
            .any(|i| self.previous.get(*i) != row.get(*i));
        *row.get_mut(0)
            .ok_or(oer_diagnostics::name!("traffic-ordinal"; 32))? = self.scans;
        *row.get_mut(1)
            .ok_or(oer_diagnostics::name!("traffic-phase"; 32))? = phase;
        self.scans = self.scans.saturating_add(1);
        self.previous = row;
        if force || changed {
            *self
                .rows
                .get_mut(self.used)
                .ok_or(oer_diagnostics::name!("traffic-journal-full"; 32))? = row;
            self.used = self.used.saturating_add(1);
        }
        Ok(())
    }

    pub(super) fn admit_live(&self) -> Result<(), oer_diagnostics::Name<32>> {
        if self.used >= CAPACITY.saturating_sub(16) {
            Err(oer_diagnostics::name!("traffic-live-full"; 32))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_coalesce_but_enable_counters_payload_and_phase_changes_survive() {
        let mut j = Journal::<1, 1> {
            pre: [[0]],
            previous_pre: [[0]],
            post: [[0]],
            previous_begin: 0,
            previous_end: 0,
            bindings: [0],
            rows: [[0; WIDTH]; CAPACITY],
            used: 0,
            scans: 0,
            units: core::array::from_fn(|_| Unit {
                header: [0; 23],
                after_irq: [0; PAYLOAD_CAPACITY],
                previous: [0; PAYLOAD_CAPACITY],
                early: [0; PAYLOAD_CAPACITY],
                taken: [0; PAYLOAD_CAPACITY],
            }),
            unit_used: 0,
            previous: [0; WIDTH],
        };
        let mut r = [0; WIDTH];
        j.record(r, 200, false).unwrap();
        r[2] = 123;
        j.record(r, 200, false).unwrap();
        assert_eq!(j.used, 1);
        for column in [13, 16, 23, 26] {
            r[column] = 1;
            j.record(r, 200, false).unwrap();
        }
        j.record(r, 201, false).unwrap();
        assert_eq!(j.used, 6);
        while j.admit_live().is_ok() {
            j.record(r, 203, true).unwrap();
        }
        for phase in 205..210 {
            j.record(r, phase, true).unwrap();
        }
        assert_eq!(j.used, CAPACITY - 11);
    }
}
