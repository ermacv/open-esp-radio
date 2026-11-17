//! The run's typed verdict over its observations, decided after the walker
//! stopped and the evidence was printed. Host-tested: it depends on no
//! hardware type.
//!
//! A violation fails the run. Without one, the run passes only when every
//! arm exercised what it claims; otherwise it is an insufficient experiment.

/// The segments of one run and what each is: fixed control A, rebind B,
/// rebind B, then A′, fixed binding on already changed bindings, an
/// observation of late writes and not a restored control.
pub const SEGMENTS: usize = 4;
pub const REBIND_SEGMENT: [bool; SEGMENTS] = [false, true, true, false];
pub const LATE_SEGMENT: usize = 3;

/// Frames a fixed segment checks at least.
pub const MIN_FRAMES: u32 = 500;
/// Detaches with their window check a fixed control segment runs at least.
pub const MIN_FIXED_DETACHES: u32 = 16;
/// Rebinds a rebind segment performs at least, each covered: the BUFFER
/// word changed and a later frame landed in the new spare with every frame
/// check run.
pub const MIN_COVERED_REBINDS: u32 = 16;

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
        }
    }
}

/// The run's observations after the walker stopped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observations {
    pub counts: [Counts; SEGMENTS],
    pub violations: u32,
    pub first_violation: Option<Kind>,
    /// Retired spares, each CRC-checked after the walker stopped.
    pub retired: u32,
    pub retired_final_checks: u32,
    /// Retired spares that stayed retired while at least one ring lap of
    /// descriptors completed past them.
    pub retired_with_lap: u32,
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
    LateFrames,
    LateRetiredChecks,
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
            Self::LateFrames => "late-frames",
            Self::LateRetiredChecks => "late-retired-checks",
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
    let mut retired_rebinds = 0u32;
    for (segment, counts) in observations.counts.iter().enumerate() {
        let rebind = REBIND_SEGMENT.get(segment).copied().unwrap_or(false);
        if rebind {
            retired_rebinds = retired_rebinds.saturating_add(counts.rebinds);
            if counts.covered_rebinds < MIN_COVERED_REBINDS {
                return Some(Shortfall::RebindCoverage);
            }
        } else if segment == LATE_SEGMENT {
            if counts.frames < MIN_FRAMES {
                return Some(Shortfall::LateFrames);
            }
            if counts.retired_checks < observations.retired {
                return Some(Shortfall::LateRetiredChecks);
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
    if observations.retired != retired_rebinds
        || observations.retired_final_checks != observations.retired
    {
        return Some(Shortfall::RetiredUnchecked);
    }
    if observations.retired_with_lap != observations.retired {
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
            rebinds: 20,
            covered_rebinds: 20,
            ..Counts::default()
        };
        let late = Counts {
            frames: MIN_FRAMES,
            retired_checks: 40,
            ..Counts::default()
        };
        Observations {
            counts: [fixed, rebind, rebind, late],
            violations: 0,
            first_violation: None,
            retired: 40,
            retired_final_checks: 40,
            retired_with_lap: 40,
            unbound: 24,
            unbound_final_checks: 24,
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
        observations.first_violation = Some(Kind::Retired);
        assert_eq!(decide(&observations), Verdict::Fail(Kind::Retired));
    }

    #[test]
    fn a_violation_fails_a_run_without_coverage() {
        let observations = Observations {
            violations: 2,
            first_violation: Some(Kind::Binding),
            ..Observations::default()
        };
        assert_eq!(decide(&observations), Verdict::Fail(Kind::Binding));
    }

    #[test]
    fn a_run_without_traffic_or_rebinds_is_insufficient() {
        assert_eq!(
            decide(&Observations::default()),
            Verdict::Insufficient(Shortfall::FixedFrames)
        );
    }

    #[test]
    fn rebinds_without_a_landed_frame_are_insufficient() {
        let mut observations = covered();
        if let Some(counts) = observations.counts.get_mut(2) {
            counts.covered_rebinds = MIN_COVERED_REBINDS - 1;
        }
        assert_eq!(
            decide(&observations),
            Verdict::Insufficient(Shortfall::RebindCoverage)
        );
    }

    #[test]
    fn a_retired_spare_without_a_final_check_is_insufficient() {
        let mut observations = covered();
        observations.retired_final_checks = 39;
        assert_eq!(
            decide(&observations),
            Verdict::Insufficient(Shortfall::RetiredUnchecked)
        );
    }

    #[test]
    fn a_retired_spare_held_less_than_a_lap_is_insufficient() {
        let mut observations = covered();
        observations.retired_with_lap = 39;
        assert_eq!(
            decide(&observations),
            Verdict::Insufficient(Shortfall::RetiredWithoutLap)
        );
    }

    #[test]
    fn a_late_segment_without_frames_is_insufficient() {
        let mut observations = covered();
        if let Some(counts) = observations.counts.get_mut(LATE_SEGMENT) {
            counts.frames = 0;
        }
        assert_eq!(
            decide(&observations),
            Verdict::Insufficient(Shortfall::LateFrames)
        );
    }
}
