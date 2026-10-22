//! A guard must catch the first store after a whole-frame stack adjustment
//! and still leave room for the exception that records the overflow.

use super::{HartStack, InterruptStacks};
use std::collections::BTreeSet;
use std::fmt::Write as _;

#[derive(Default)]
struct Frames {
    largest: Option<(u32, u64)>,
    unknown: Vec<u32>,
}

impl InterruptStacks {
    fn hart_frames(&self, hart: &HartStack) -> Frames {
        let reached: BTreeSet<_> = hart
            .levels
            .iter()
            .chain([&hart.exception])
            .flat_map(|level| level.bound.reached.iter().copied())
            .collect();
        let mut frames = Frames::default();
        for address in reached {
            match self.frames.get(&address).copied().flatten() {
                Some(bytes) => {
                    if frames.largest.is_none_or(|(_, largest)| bytes > largest) {
                        frames.largest = Some((address, bytes));
                    }
                }
                None => frames.unknown.push(address),
            }
        }
        frames
    }

    pub(super) fn guard_problem(&self, hart: &HartStack) -> Option<String> {
        let frames = self.hart_frames(hart);
        let guard = u64::from(self.contract.interrupts.irq_stack_guard_bytes[&hart.core]);
        let exception = hart.exception.bytes.unwrap_or(hart.exception.partial);
        let largest = frames.largest.map_or(0, |(_, bytes)| bytes);
        if exception.saturating_add(largest) > guard {
            return Some(format!(
                "hart {}'s interrupt guard needs exception {exception} + largest frame {largest} \
                 bytes, more than its {guard} bytes",
                hart.core
            ));
        }
        if !frames.unknown.is_empty() {
            return Some(format!(
                "hart {}'s interrupt guard has {} reachable functions without a known frame: {}",
                hart.core,
                frames.unknown.len(),
                frames
                    .unknown
                    .iter()
                    .map(|&address| self.name(address))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // An unknown exception also makes the hart's bound unknown. The
        // existing stack check rejects it for Product and reports its holes
        // for Diagnostic; the table keeps the partial value visibly marked.
        None
    }

    pub(super) fn render_guards(&self) -> String {
        let mut out = String::from(
            "Interrupt guards (bytes; E includes the exception entry)\nhart\tguard\tE\tlargest frame\tE + frame\tspare\tfunction\n",
        );
        for hart in &self.harts {
            let frames = self.hart_frames(hart);
            let guard = self.contract.interrupts.irq_stack_guard_bytes[&hart.core];
            let exception = hart.exception.bytes.unwrap_or(hart.exception.partial);
            let (largest, function) = frames
                .largest
                .map_or((0, "—".to_owned()), |(address, bytes)| {
                    (bytes, self.name(address))
                });
            let total = exception.saturating_add(largest);
            let partial = if hart.exception.bytes.is_none() || !frames.unknown.is_empty() {
                " + ?"
            } else {
                ""
            };
            let spare = i128::from(guard) - i128::from(total);
            let _ = writeln!(
                out,
                "{}\t{guard}\t{exception}{}\t{largest}\t{total}{partial}\t{spare}{}\t{function}",
                hart.core,
                if hart.exception.bytes.is_none() {
                    " + ?"
                } else {
                    ""
                },
                if partial.is_empty() { "" } else { " (at most)" }
            );
            for address in frames.unknown {
                let _ = writeln!(out, "  unknown frame: {}", self.name(address));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Required, tests::partial};
    use std::collections::BTreeSet;

    fn report(frame: Option<u64>) -> super::InterruptStacks {
        let mut report = partial(1000);
        report.harts[0].bytes = Some(1000);
        report.harts[0].exception.bytes = Some(432);
        report.harts[0].exception.partial = 432;
        report.harts[0].exception.bound.reached = BTreeSet::from([0x1000]);
        report.frames.insert(0x1000, frame);
        report.names.insert(0x1000, "exception_helper".to_owned());
        report
    }

    #[test]
    fn a_frame_on_the_guard_boundary_passes_both_policies() {
        let report = report(Some(3664));
        for policy in [Required::Product, Required::Diagnostic] {
            assert!(report.check(policy).unwrap().is_empty());
        }
        assert!(
            report
                .render()
                .contains("0\t4096\t432\t3664\t4096\t0\texception_helper")
        );
    }

    #[test]
    fn one_byte_over_fails_product_and_warns_diagnostic_with_the_same_table() {
        let report = report(Some(3665));
        let error = report.check(Required::Product).unwrap_err().to_string();
        let warnings = report.check(Required::Diagnostic).unwrap();
        assert_eq!(warnings.len(), 1);
        let table = "0\t4096\t432\t3665\t4097\t-1\texception_helper";
        assert!(error.contains(table), "{error}");
        assert!(warnings[0].contains(table), "{warnings:?}");
    }

    #[test]
    fn an_unknown_reachable_frame_is_not_silently_treated_as_zero() {
        let report = report(None);
        assert!(report.check(Required::Product).is_err());
        let warnings = report.check(Required::Diagnostic).unwrap();
        assert!(warnings[0].contains("without a known frame"));
        assert!(report.render().contains("unknown frame: exception_helper"));
    }

    #[test]
    fn all_reached_frames_count_including_tasks_off_the_critical_path_but_not_other_harts() {
        let mut report = report(Some(304));
        let mut level = report.harts[0].exception.clone();
        level.level = 2;
        level.bound.reached = BTreeSet::from([0x2000, 0x3000]);
        level.bound.path = vec![(0x2000, 16)];
        report.harts[0].levels.push(level);
        report.frames.extend([
            (0x2000, Some(16)),
            (0x3000, Some(3665)),
            (0x4000, Some(10000)),
        ]);
        report.names.insert(0x3000, "off_path_task".to_owned());
        assert!(
            report
                .check(Required::Product)
                .unwrap_err()
                .to_string()
                .contains("off_path_task")
        );
        report.frames.insert(0x3000, Some(3664));
        assert!(report.check(Required::Product).is_ok());
        let mut hart1 = report.harts[0].clone();
        hart1.core = 1;
        hart1.levels.clear();
        report.contract.interrupts.harts.push(1);
        report
            .contract
            .interrupts
            .irq_stack_guard_bytes
            .insert(1, 1024);
        report.harts.push(hart1);
        assert!(report.check(Required::Product).is_ok());
        report.harts[1].exception.bound.reached.insert(0x3000);
        assert!(
            report
                .check(Required::Product)
                .unwrap_err()
                .to_string()
                .contains("hart 1")
        );
    }
}
