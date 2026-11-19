//! The run's typed verdict over its observations, decided after the walker
//! stopped and the evidence was printed. Host-tested: it depends on no
//! hardware type.
//!
//! A violation fails the run. Without one, the run passes only when every
//! arm exercised what it claims; otherwise it is an insufficient experiment.

/// What a segment does with a unit it takes (`tasks/200`: an exhaust
/// segment keeps every unit until the ring is out, then restores them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Fixed,
    Rebind,
    Exhaust,
    /// `tasks/219` c1: the ring keeps one hole; when the walker stands the
    /// hole is armed with its spare and the oldest unit taken as the next.
    Exit,
}

/// One segment's arm: what it does, and the hold-back `hold`, the
/// completed descriptors a taken unit keeps between itself and the frozen
/// LAST (`tasks/165`: 8 comfortably, 1 the port's own condition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arm {
    pub act: Act,
    pub hold: usize,
}

impl Arm {
    /// Whether the segment rebinds.
    pub const fn rebind(self) -> bool {
        matches!(self.act, Act::Rebind | Act::Exit)
    }

    /// Whether the segment keeps one hole and exits the ring out of it.
    pub const fn exit(self) -> bool {
        matches!(self.act, Act::Exit)
    }

    /// Whether the segment exhausts the ring.
    pub const fn exhaust(self) -> bool {
        matches!(self.act, Act::Exhaust)
    }
}

/// The segments of one run (`tasks/193`): fixed and rebind alternated,
/// every one under hold 0, so a unit is taken, and in a rebind segment
/// rebound, as soon as it completed, at LAST included; a closing fixed
/// segment gives the last retirements their laps. With the feature
/// `exhaust` (`tasks/200`) the rebind segments exhaust the ring instead.
pub const SEGMENTS: usize = 15;
const fn arm(act: Act, hold: usize) -> Arm {
    Arm { act, hold }
}
#[cfg(not(any(feature = "exhaust", feature = "exit")))]
const ODD: Act = Act::Rebind;
#[cfg(feature = "exhaust")]
const ODD: Act = Act::Exhaust;
#[cfg(not(feature = "exit"))]
pub const ARMS: [Arm; SEGMENTS] = [
    arm(Act::Fixed, 0),
    arm(ODD, 0),
    arm(Act::Fixed, 0),
    arm(ODD, 0),
    arm(Act::Fixed, 0),
    arm(ODD, 0),
    arm(Act::Fixed, 0),
    arm(ODD, 0),
    arm(Act::Fixed, 0),
    arm(ODD, 0),
    arm(Act::Fixed, 0),
    arm(ODD, 0),
    arm(Act::Fixed, 0),
    arm(ODD, 0),
    arm(Act::Fixed, 0),
];
/// `tasks/219` c1 and c2 (`exit`): exits out of a ring with its hole in
/// five segments; in two, the ring exhausted without a hole (run 200's
/// hold), for what NEXT holds there.
#[cfg(feature = "exit")]
pub const ARMS: [Arm; SEGMENTS] = [
    arm(Act::Fixed, 0),
    arm(Act::Exit, 0),
    arm(Act::Fixed, 0),
    arm(Act::Exit, 0),
    arm(Act::Fixed, 0),
    arm(Act::Exhaust, 0),
    arm(Act::Fixed, 0),
    arm(Act::Exit, 0),
    arm(Act::Fixed, 0),
    arm(Act::Exit, 0),
    arm(Act::Fixed, 0),
    arm(Act::Exhaust, 0),
    arm(Act::Fixed, 0),
    arm(Act::Exit, 0),
    arm(Act::Fixed, 0),
];
/// The holds the verdict asks its rebinds of; the others are reported.
pub const HOLDS: [usize; 1] = [0];

/// Frames a fixed segment checks at least.
pub const MIN_FRAMES: u32 = 500;
/// Detaches with their window check a fixed control segment runs at least.
pub const MIN_FIXED_DETACHES: u32 = 16;
/// Covered rebinds each hold performs at least over its segments: the
/// BUFFER word changed and a later frame landed in the new spare with every
/// frame check run.
pub const MIN_COVERED_REBINDS: u32 = 1_000;
/// The buckets of the armed reserve at a rebind: one for each of 0 to 30,
/// then 31 or more.
pub const RESERVE_BUCKETS: usize = 32;

/// One segment's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(target_arch = "riscv32", derive(bytemuck::Zeroable))]
#[repr(C)]
pub struct Counts {
    pub services: u32,
    pub units: u32,
    pub descriptors: u32,
    pub chained: u32,
    pub frames: u32,
    pub prefix_ok: u32,
    pub prefix_err: u32,
    pub fcs_ok: u32,
    pub fcs_bad: u32,
    pub fcs_absent: u32,
    pub frames_on_rebound: u32,
    pub detached: u32,
    pub window_checks: u32,
    pub rebinds: u32,
    /// Of `rebinds`, those of a unit at the frozen LAST (`tasks/193`).
    pub rebinds_at_last: u32,
    /// Takes left unrebound because no armed descriptor lay ahead of the
    /// walker (its reserve 0): excluded by `tasks/193`.
    pub exhausted: u32,
    /// Rebinds made in this segment whose new spare then received a frame
    /// that passed the binding, copy and canary checks with a decoded prefix.
    pub covered_rebinds: u32,
    pub spares_exhausted: u32,
    pub not_before_last: u32,
    pub retired_checks: u32,
    pub spare_checks: u32,
    pub reload_deferred: u32,
    pub violations: u32,
    pub hw_mpdu: u32,
    pub hw_fcs_error: u32,
    pub hw_buffer_full: u32,
    pub hw_data_success: u32,
    /// Takes left for later: the unit was within the hold of LAST.
    pub held_back: u32,
    /// Frames whose counters, every 64 octets, named more than one frame
    /// or whose bytes differed from the sender's.
    pub content_bad: u32,
    /// Frames checked by content (the sender's counted frames).
    pub content_ok: u32,
    /// The armed descriptors between NEXT and the tail at each rebind.
    pub reserve: [u32; RESERVE_BUCKETS],
    /// The least of them; `u32::MAX` before the first rebind.
    pub reserve_min: u32,
    /// Retired spares returned to the bank after two laps and their CRC.
    pub returned: u32,
    /// The sender's counters this segment saw first and last, and how many
    /// it completed: the frames between that it did not are the air's
    /// losses while the ring was out.
    pub counter_first: u32,
    pub counter_last: u32,
}

/// What a violation found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Kind {
    /// The descriptor's BUFFER word or the HAL accounting named another
    /// buffer than the binding the probe made.
    Binding = 1,
    /// The HAL's copy of a frame differed from the bound spare.
    Copy = 2,
    /// A frame read from a newly bound spare still held its canary.
    Landing = 3,
    /// A detached buffer changed before its fixed re-arm.
    Window = 4,
    /// A retired spare's CRC changed.
    Retired = 5,
    /// An unbound spare's canary changed.
    Spare = 6,
    /// The HAL did not account a retired spare as retired.
    Accounting = 7,
    /// A committed frame's counters named two frames, or its bytes were not
    /// the sender's.
    Content = 8,
    /// A frame's copy differed from its spare outside the receive header's
    /// late word, bytes 48 to 51 (`tasks/194`).
    CopyOther = 9,
}

impl Kind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::Copy => "copy",
            Self::Landing => "landing",
            Self::Window => "window",
            Self::Retired => "retired",
            Self::Spare => "spare",
            Self::Accounting => "accounting",
            Self::Content => "content",
            Self::CopyOther => "copy-other",
        }
    }
}

/// The run's observations after the walker stopped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observations {
    pub counts: [Counts; SEGMENTS],
    pub violations: u32,
    pub first_violation: Option<Kind>,
    /// Retirements: every rebind retires the spare it replaces.
    pub retired: u32,
    /// Of them, returned to the bank after two laps and an unchanged CRC.
    pub returned: u32,
    /// Of them, kept to the run's end and CRC-checked after the walker
    /// stopped.
    pub kept: u32,
    pub kept_final_checks: u32,
    /// Kept spares that stayed retired while at least one ring lap of
    /// descriptors completed past them.
    pub kept_with_lap: u32,
    /// Never bound spares, each canary-checked after the walker stopped.
    pub unbound: u32,
    pub unbound_final_checks: u32,
}

/// Why a run proves too little.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortfall {
    FixedFrames,
    FixedDetaches,
    RebindCoverage,
    RetiredUnchecked,
    RetiredWithoutLap,
    UnboundUnchecked,
}

impl Shortfall {
    pub const fn name(self) -> &'static str {
        match self {
            Self::FixedFrames => "fixed-frames",
            Self::FixedDetaches => "fixed-detaches",
            Self::RebindCoverage => "rebind-coverage",
            Self::RetiredUnchecked => "retired-unchecked",
            Self::RetiredWithoutLap => "retired-without-lap",
            Self::UnboundUnchecked => "unbound-unchecked",
        }
    }
}

/// The run's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No violation, every arm covered.
    Pass,
    /// At least one violation; the first one's kind.
    Fail(Kind),
    /// No violation, but an arm did not exercise what it claims.
    Insufficient(Shortfall),
}

/// Decide the run.
pub fn decide(observations: &Observations) -> Verdict {
    if observations.violations != 0 {
        return Verdict::Fail(observations.first_violation.unwrap_or(Kind::Binding));
    }
    match shortfall(observations) {
        Some(shortfall) => Verdict::Insufficient(shortfall),
        None => Verdict::Pass,
    }
}

fn shortfall(observations: &Observations) -> Option<Shortfall> {
    let mut rebinds = 0u32;
    let mut covered = [0u32; HOLDS.len()];
    for (counts, arm) in observations.counts.iter().zip(ARMS) {
        if arm.exhaust() {
            // `tasks/200`: an exhaust segment is judged by its records.
            continue;
        }
        if arm.rebind() {
            rebinds = rebinds.saturating_add(counts.rebinds);
            if let Some(at) = HOLDS.iter().position(|hold| *hold == arm.hold)
                && let Some(sum) = covered.get_mut(at)
            {
                *sum = sum.saturating_add(counts.covered_rebinds);
            }
        } else {
            if counts.frames < MIN_FRAMES {
                return Some(Shortfall::FixedFrames);
            }
            if counts.window_checks < MIN_FIXED_DETACHES {
                return Some(Shortfall::FixedDetaches);
            }
        }
    }
    if ARMS.iter().any(|arm| arm.rebind()) && covered.iter().any(|sum| *sum < MIN_COVERED_REBINDS) {
        return Some(Shortfall::RebindCoverage);
    }
    if observations.retired != rebinds
        || observations.returned.saturating_add(observations.kept) != observations.retired
        || observations.kept_final_checks != observations.kept
    {
        return Some(Shortfall::RetiredUnchecked);
    }
    if observations.kept_with_lap != observations.kept {
        return Some(Shortfall::RetiredWithoutLap);
    }
    if observations.unbound_final_checks != observations.unbound {
        return Some(Shortfall::UnboundUnchecked);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered() -> Observations {
        let fixed = Counts {
            frames: MIN_FRAMES,
            window_checks: MIN_FIXED_DETACHES,
            ..Counts::default()
        };
        let rebind = Counts {
            frames: MIN_FRAMES,
            rebinds: 600,
            covered_rebinds: 600,
            ..Counts::default()
        };
        let counts = ARMS.map(|arm| if arm.rebind() { rebind } else { fixed });
        Observations {
            counts,
            violations: 0,
            first_violation: None,
            retired: 4_200,
            returned: 4_100,
            kept: 100,
            kept_final_checks: 100,
            kept_with_lap: 100,
            unbound: 24,
            unbound_final_checks: 24,
        }
    }

    #[test]
    fn both_holds_alternate_with_their_controls() {
        assert_eq!(ARMS.iter().filter(|arm| arm.rebind()).count(), SEGMENTS / 2);
        for hold in HOLDS {
            assert!(ARMS.iter().any(|arm| arm.rebind() && arm.hold == hold));
            assert!(ARMS.iter().any(|arm| !arm.rebind() && arm.hold == hold));
        }
    }

    #[test]
    fn full_coverage_without_a_violation_passes() {
        assert_eq!(decide(&covered()), Verdict::Pass);
    }

    #[test]
    fn any_violation_fails_even_with_full_coverage() {
        let mut observations = covered();
        observations.violations = 1;
        observations.first_violation = Some(Kind::Content);
        assert_eq!(decide(&observations), Verdict::Fail(Kind::Content));
    }

    #[test]
    fn a_run_without_traffic_or_rebinds_is_insufficient() {
        assert_eq!(
            decide(&Observations::default()),
            Verdict::Insufficient(Shortfall::FixedFrames)
        );
    }

    #[test]
    fn one_hold_short_of_its_rebinds_is_insufficient() {
        let mut observations = covered();
        for (counts, arm) in observations.counts.iter_mut().zip(ARMS) {
            if arm.rebind() && arm.hold == 0 {
                counts.covered_rebinds = 100;
            }
        }
        assert_eq!(
            decide(&observations),
            Verdict::Insufficient(Shortfall::RebindCoverage)
        );
    }

    #[test]
    fn a_retirement_neither_returned_nor_kept_is_insufficient() {
        let mut observations = covered();
        observations.returned -= 1;
        assert_eq!(
            decide(&observations),
            Verdict::Insufficient(Shortfall::RetiredUnchecked)
        );
    }

    #[test]
    fn a_kept_spare_held_less_than_a_lap_is_insufficient() {
        let mut observations = covered();
        observations.kept_with_lap = 99;
        assert_eq!(
            decide(&observations),
            Verdict::Insufficient(Shortfall::RetiredWithoutLap)
        );
    }
}
