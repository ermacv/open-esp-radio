//! Host-testable journal operations shared with the hardware collector.

pub(super) const RECORDS: usize = 256;
pub(super) const WIDTH: usize = 48;
const STOPPED_RECORDS: usize = 10;

#[cfg_attr(target_arch = "riscv32", derive(bytemuck::Zeroable))]
pub(crate) struct Journal {
    pub(super) rows: [[u32; WIDTH]; RECORDS],
    pub(super) used: usize,
    previous: [u32; WIDTH],
    pub(super) scans: u32,
    pub(super) max_gap: u32,
    last_end: u32,
}

impl Journal {
    /// Keep the stopped checkpoints available when live collection fills up.
    pub(super) fn check_live_budget(&self) -> Result<(), &'static str> {
        if self.used >= RECORDS.saturating_sub(STOPPED_RECORDS) {
            Err("irq120-journal-live-full")
        } else {
            Ok(())
        }
    }

    pub(super) fn record(
        &mut self,
        mut row: [u32; WIDTH],
        phase: u32,
        force: bool,
    ) -> Result<(), &'static str> {
        // Ignore timestamps and ordinal when comparing observations.
        const VALUES: [usize; 35] = [
            3, 4, 5, 6, 7, 9, 11, 13, 14, 15, 16, 18, 20, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
            32, 33, 34, 35, 36, 37, 38, 39, 41, 42, 44, 45,
        ];
        let changed = self.previous.get(1).copied() != Some(phase)
            || VALUES.iter().any(|i| self.previous.get(*i) != row.get(*i));
        *row.get_mut(0).ok_or("row-scan")? = self.scans;
        *row.get_mut(1).ok_or("row-phase")? = phase;
        let begin = *row.get(2).ok_or("row-begin")?;
        if self.scans != 0 {
            self.max_gap = self.max_gap.max(begin.wrapping_sub(self.last_end));
        }
        self.last_end = *row.get(47).ok_or("row-end")?;
        self.previous = row;
        self.scans = self.scans.saturating_add(1);
        if force || changed {
            *self.rows.get_mut(self.used).ok_or("irq120-journal-full")? = row;
            self.used = self.used.saturating_add(1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal() -> Journal {
        Journal {
            rows: [[0; WIDTH]; RECORDS],
            used: 0,
            previous: [0; WIDTH],
            scans: 0,
            max_gap: 0,
            last_end: 0,
        }
    }

    #[test]
    fn changes_and_forced_checkpoints_are_kept_but_timestamp_only_scans_are_coalesced() {
        let mut trace = journal();
        let mut row = [0; WIDTH];
        row[2] = 10;
        row[47] = 20;
        assert_eq!(trace.record(row, 160, false), Ok(()));
        assert_eq!(trace.used, 1);

        row[0] = 999; // incoming ordinal is replaced by the journal's scan count
        row[2] = 30;
        row[43] = 35; // before-ack cycle is also not an event
        row[47] = 40;
        assert_eq!(trace.record(row, 160, false), Ok(()));
        assert_eq!(trace.used, 1);
        assert_eq!(trace.scans, 2);
        assert_eq!(trace.max_gap, 10);

        row[9] = 0x20; // new RAW observation
        assert_eq!(trace.record(row, 160, false), Ok(()));
        row[15] = 2; // new terminal DONE bitmap
        assert_eq!(trace.record(row, 160, false), Ok(()));
        row[26] = 1; // receive-counter change without a new IRQ
        assert_eq!(trace.record(row, 160, false), Ok(()));
        row[44] = 0x40; // changed post-ack RAW must also be retained
        assert_eq!(trace.record(row, 160, false), Ok(()));
        assert_eq!(trace.used, 5);

        assert_eq!(trace.record(row, 161, false), Ok(()));
        assert_eq!(trace.used, 6, "phase changes are observations");
        assert_eq!(trace.record(row, 161, true), Ok(()));
        assert_eq!(trace.used, 7, "forced identical checkpoints are retained");
        assert_eq!(trace.rows[6][0], 7);
        assert_eq!(trace.rows[6][1], 161);
    }

    #[test]
    fn live_budget_reserves_stopped_rows_and_full_journal_refuses_an_extra_row() {
        let mut trace = journal();
        let mut row = [0; WIDTH];
        for _ in 0..RECORDS - STOPPED_RECORDS {
            assert_eq!(trace.check_live_budget(), Ok(()));
            assert_eq!(trace.record(row, 160, true), Ok(()));
        }
        assert_eq!(trace.check_live_budget(), Err("irq120-journal-live-full"));

        for phase in 162..170 {
            assert_eq!(trace.record(row, phase, true), Ok(()));
        }
        assert_eq!(trace.used, RECORDS - 2);
        assert_eq!(trace.rows[RECORDS - 3][1], 169);
        for _ in 0..2 {
            assert_eq!(trace.record(row, 169, true), Ok(()));
        }
        let last = trace.rows[RECORDS - 1];
        row[9] = 0x20;
        assert_eq!(trace.record(row, 169, true), Err("irq120-journal-full"));
        assert_eq!(trace.used, RECORDS);
        assert_eq!(
            trace.rows[RECORDS - 1],
            last,
            "capacity refusal keeps prior evidence"
        );
    }
}
