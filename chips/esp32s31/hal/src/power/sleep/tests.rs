//! The sleep owner over host memory, driven through the family sequence:
//! what a sleep leaves in the words, read back through the same owner.

use std::{vec, vec::Vec};

use oer_chips_esp32s31_pac::instances;
use oer_chips_family_drivers_pmu::sleep::{
    Outcome, RtcTicks, SleepClocks, SleepRequest, now, sleep_until,
};

use super::SleepRegisters;

type HostSleep<'a> = SleepRegisters<
    oer_portable_mmio::Host<'a>,
    oer_portable_mmio::Host<'a>,
    oer_portable_mmio::Host<'a>,
>;

/// Host memory of `bytes` bytes, each word `fill`.
fn memory(bytes: usize, fill: u32) -> Vec<u32> {
    vec![fill; bytes.div_ceil(4)]
}

/// Run `check` on a sleep owner whose sleep-state words start at zero,
/// whose control words all read `control_fill` and whose LP timer words all
/// read `timer_fill`.
fn with_owner(control_fill: u32, timer_fill: u32, check: impl FnOnce(&mut HostSleep<'_>)) {
    let mut state = memory(instances::pmu::SIZE, 0);
    let mut control = memory(instances::pmu::SIZE, control_fill);
    let mut timer = memory(instances::rtc_timer::SIZE, timer_fill);
    let mut owner = SleepRegisters::host(&mut state, &mut control, &mut timer)
        .expect("host memory holds the blocks");
    check(&mut owner);
}

fn request(clocks: SleepClocks) -> SleepRequest {
    SleepRequest {
        wake_at: RtcTicks::new(0x0001_2345_6789).expect("a 48-bit count"),
        give_up_after: RtcTicks::new(0x0001_2345_9999).expect("a 48-bit count"),
        clocks,
    }
}

#[test]
fn a_counter_reading_joins_the_high_and_low_words() {
    // Every timer word reads 0x0102: the high word's 16-bit field and the
    // low word's 32-bit field both hold it.
    with_owner(0, 0x0102, |owner| {
        assert_eq!(now(owner).map(RtcTicks::get), Ok(0x0102_0000_0102));
    });
}

#[test]
fn a_sleep_keeps_every_domain_and_oscillator_on_and_stalls_the_cpu() {
    // Every control word reads ones: the raw status reports a reject at
    // once, so the sequence runs to its end.
    with_owner(u32::MAX, 0, |owner| {
        let slept = sleep_until(owner, request(SleepClocks::Running)).expect("an outcome");
        assert!(matches!(slept.outcome, Outcome::Rejected { .. }));
        let state = &owner.state;
        let power = state.read_hp_sleep_dig_power();
        assert!(!power.hp_sleep_pd_top_pd_en());
        assert!(!power.hp_sleep_pd_hp_cpu_pd_en());
        assert!(!power.hp_sleep_pd_modem_pwr_pd_en());
        assert!(state.read_hp_sleep_hp_sys_cntl().hp_sleep_dig_cpu_stall());
        assert!(state.read_hp_sleep_xtal().hp_sleep_xpd_xtal());
        assert!(state.read_lp_sleep_xtal().lp_sleep_xpd_xtal());
        let pll = state.read_hp_sleep_hp_ck_power();
        assert!(pll.hp_sleep_xpd_cpll() && pll.hp_sleep_xpd_mpll() && pll.hp_sleep_xpd_bbpll());
        assert!(state.read_lp_sleep_lp_ck_power().lp_sleep_xpd_fosc_clk());
        assert_eq!(
            state
                .read_hp_sleep_hp_regulator0()
                .hp_sleep_hp_regulator_dbias()
                .get(),
            26
        );
        assert_eq!(
            state.read_hp_sleep_icg_hp_func().hp_sleep_dig_icg_func_en(),
            u32::MAX
        );
        assert!(state.read_hp_sleep_sysclk().hp_sleep_icg_sys_clock_en());
        assert!(
            state.read_hp_active_sysclk().hp_active_icg_sys_clock_en(),
            "the wake returns to an open system clock gate"
        );
        assert!(!state.read_hp_active_hp_sys_cntl().hp_active_dig_cpu_stall());
        assert!(state.read_hp_active_bias().hp_active_xpd_bias());
    });
}

#[test]
fn a_record_reads_what_the_last_request_left_without_acknowledging() {
    // Every control and timer word reads ones.
    with_owner(u32::MAX, u32::MAX, |owner| {
        let record = owner.record();
        assert!(record.woke && record.rejected && record.target_fired);
        assert_eq!(
            record.wake_causes,
            owner.control.read_slp_wakeup_status0().wakeup_cause()
        );
        assert_eq!(record.rtc, 0xffff_ffff_ffff);
        assert!(
            owner.control.read_int_raw().soc_wakeup_int_raw(),
            "nothing acknowledged"
        );
    });
}

#[test]
fn gated_clocks_close_every_gate() {
    with_owner(u32::MAX, 0, |owner| {
        sleep_until(owner, request(SleepClocks::Gated)).expect("an outcome");
        let state = &owner.state;
        assert_eq!(
            state.read_hp_sleep_icg_hp_func().hp_sleep_dig_icg_func_en(),
            0
        );
        assert_eq!(
            state.read_hp_sleep_icg_hp_apb().hp_sleep_dig_icg_apb_en(),
            0
        );
        assert!(!state.read_hp_sleep_sysclk().hp_sleep_icg_sys_clock_en());
    });
}

#[test]
fn the_target_splits_into_the_timer_words_and_the_wake_ends_disabled() {
    with_owner(u32::MAX, 0, |owner| {
        sleep_until(owner, request(SleepClocks::Running)).expect("an outcome");
        assert_eq!(owner.timer.read_tar0_high().main_timer_tar_high0(), 0x0001);
        assert_eq!(
            owner.timer.read_tar0_low().main_timer_tar_low0(),
            0x2345_6789
        );
        assert_eq!(owner.control.read_slp_wakeup_cntl2().wakeup_ena(), 0);
        assert!(!owner.control.read_slp_wakeup_cntl0().slp_reject_en());
    });
}
