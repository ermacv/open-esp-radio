//! Raw-tick timing policy of one initialized scheduler epoch.

#![forbid(unsafe_code)]

use crate::scheduler::SchedulerSoftwareConfig;

use oer_esp32s31_hal::bluetooth::BluetoothControllerTimeScale;

/// Raw-tick insertion timing policy derived from one common scheduler epoch.
///
/// Overlap admission converts the scheduler environment's first policy delta
/// through the live Controller time scale for its late-start guard.
/// `r_btdm_sched_calc_seq_time` converts the second delta before adding it to
/// every item. The policy contains no role, descriptor or hardware-list
/// identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerTimingPolicy {
    late_start_guard_raw_delta: u32,
    sequence_lead_raw_delta: u32,
}

impl SchedulerTimingPolicy {
    /// Derive both raw timing deltas for one initialized scheduler epoch.
    pub const fn from_scheduler_config(
        config: SchedulerSoftwareConfig,
        scale: BluetoothControllerTimeScale,
    ) -> Self {
        Self {
            late_start_guard_raw_delta: scale
                .raw_ticks_from_micros(config.late_start_guard_micros())
                .whole_ticks,
            sequence_lead_raw_delta: scale
                .raw_ticks_from_micros(config.sequence_lead_micros())
                .whole_ticks,
        }
    }

    /// Whether one fresh sample still precedes the guarded item start.
    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn initial_deadline_is_open(
        self,
        sample: &crate::ControllerTimeSample,
        raw_item_start: u32,
    ) -> bool {
        (sample
            .raw_ticks()
            .wrapping_add(self.late_start_guard_raw_delta)
            .wrapping_sub(raw_item_start) as i32)
            < 0
    }

    /// Raw lead added to every item's sequence time.
    pub const fn sequence_lead_raw_delta(self) -> u32 {
        self.sequence_lead_raw_delta
    }
}

#[cfg(test)]
mod tests {
    use oer_esp32s31_hal::bluetooth::BluetoothControllerHalInitConfig;

    use super::SchedulerTimingPolicy;
    use crate::{ControllerTimeSample, scheduler::SchedulerSoftwareConfig};

    #[test]
    fn insertion_policy_uses_one_initialized_scheduler_epoch_scale() {
        let policy = SchedulerTimingPolicy::from_scheduler_config(
            SchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
        );

        assert_eq!(policy.sequence_lead_raw_delta(), 92);
        assert!(policy.initial_deadline_is_open(&ControllerTimeSample::for_validation(22), 103));
        assert!(!policy.initial_deadline_is_open(&ControllerTimeSample::for_validation(23), 103));
    }
}
