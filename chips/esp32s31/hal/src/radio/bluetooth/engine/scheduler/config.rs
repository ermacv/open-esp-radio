//! Source-owned common scheduler timing policy.

#![forbid(unsafe_code)]

/// Source-owned scheduler timing policy copied by the reviewed Controller init.
///
/// Complete scheduler consumers establish the first value as the late-start
/// guard and the second as the per-item sequence lead. All three values are
/// microsecond deltas: the common scheduler consumes the first two through
/// its named usec-to-tick conversion, while DTM and advertising use the third
/// beside their independently recovered microsecond durations. Keeping them
/// in a software type prevents the vendor's private layout from becoming part
/// of the open ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerSoftwareConfig {
    late_start_guard_micros: u32,
    sequence_lead_micros: u32,
    preparation_lead_micros: u32,
}

impl SchedulerSoftwareConfig {
    /// Configuration constructed by the complete ESP32-S31 standalone task.
    pub const fn reviewed_standalone() -> Self {
        Self {
            late_start_guard_micros: 40,
            sequence_lead_micros: 46,
            preparation_lead_micros: 137,
        }
    }

    /// Common microsecond lead between item start and its phase anchor.
    ///
    /// SOURCE: pinned `libble_app.a[sched_3.o]::r_sym_sched_modWXEVwpjpaAKhsFBrP`
    /// stores the low byte of `options[0x10] + tick_unit - 1 + config[1]` in
    /// the scheduler environment word that `r_sym_sched_GEBmwfVsspx61ARDIjlz`
    /// returns. `r_ble_controller_init` writes 91 to the private option,
    /// the tick unit is one and the common configuration's second word is 46,
    /// so the standalone lead is 137. DTM and advertising consume the same
    /// scheduler policy; it is not part of a role-specific command or
    /// descriptor ABI.
    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn preparation_lead_micros(self) -> u32 {
        self.preparation_lead_micros
    }

    /// Microsecond guard used by both insertion deadline checks.
    pub const fn late_start_guard_micros(self) -> u32 {
        self.late_start_guard_micros
    }

    /// Microsecond lead converted and added to every raw sequence start.
    pub const fn sequence_lead_micros(self) -> u32 {
        self.sequence_lead_micros
    }
}
