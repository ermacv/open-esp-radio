#![expect(
    clippy::fn_params_excessive_bools,
    reason = "generated: a field setter's one bool parameter is the bit's value"
)]
//! The ESP32-S31 PAC: the exact hardware model generated from the chip's
//! register model (`chips/esp32s31/registers/`) by `cargo registers
//! generate`, and the root that takes every claimed part once.
//!
//! The generated module holds the typed register blocks with their field
//! accessors by class, value domains, typed DMA pointers, the interrupt
//! sources, the instances' bases, the ownership parts and partitions, the
//! memory layouts and the analog-I2C dictionary. It has no logic: every
//! sequence, poll and policy over it is the HAL's.
//!
//! A field's value is cut only inside the PAC: `Bits::cut` is not reachable
//! from outside it,
//!
//! ```compile_fail,E0624
//! let field = oer_chips_esp32s31_pac::Bits::<4>::cut(0x1f);
//! # let _ = field;
//! ```
//!
//! and `oer-portable-mmio` has no value type of its own:
//!
//! ```compile_fail,E0412
//! let field: oer_portable_mmio::Bits<4>;
//! ```
//!
//! A conversion that loses nothing refuses to build where the field does not
//! hold every value: a byte into four bits, a halfword into twelve, an index
//! of five into two bits, a scaled index past the field, two parts that are
//! not the whole value and two bytes into twelve bits.
//!
//! ```compile_fail,E0080
//! let field: oer_chips_esp32s31_pac::Bits<4> = oer_chips_esp32s31_pac::Bits::from(0xff_u8);
//! # let _ = field;
//! ```
//!
//! ```compile_fail,E0080
//! let field: oer_chips_esp32s31_pac::Bits<12> = oer_chips_esp32s31_pac::Bits::from(0xffff_u16);
//! # let _ = field;
//! ```
//!
//! ```compile_fail,E0080
//! use oer_portable_bounded::Index;
//! let field: oer_chips_esp32s31_pac::Bits<2> =
//!     oer_chips_esp32s31_pac::Bits::from(Index::<5>::constant::<4>());
//! # let _ = field;
//! ```
//!
//! ```compile_fail,E0080
//! use oer_portable_bounded::Index;
//! let field: oer_chips_esp32s31_pac::Bits<4> =
//!     oer_chips_esp32s31_pac::Bits::affine::<3, 1, 8>(Index::constant::<7>());
//! # let _ = field;
//! ```
//!
//! ```compile_fail,E0080
//! let (low, high) = oer_chips_esp32s31_pac::Bits::<11>::constant::<5>().split::<8, 2>();
//! # let _ = (low, high);
//! ```
//!
//! ```compile_fail,E0080
//! let field: oer_chips_esp32s31_pac::Bits<12> = oer_chips_esp32s31_pac::Bits::from_le_bytes([1, 2]);
//! # let _ = field;
//! ```
//!
//! T0's movable program part cannot acknowledge the permanent IRQ owner's cause:
//!
//! ```compile_fail,E0599
//! # use oer_chips_esp32s31_pac::timg::Timg0Timer0;
//! fn clear(mut timer: Timg0Timer0) {
//!     timer.trigger_int_clr_timers_t0_int_clr();
//! }
//! ```
//!
//! The timer partition cannot reset the group and re-enable its watchdog:
//!
//! ```compile_fail,E0609
//! # use oer_chips_esp32s31_pac::ownership::TimerGroup0Peripherals;
//! fn reset(mut timer: TimerGroup0Peripherals) {
//!     timer.hp_sys_clkrst_timg0.modify_timergrp0_ctrl0(|w| {
//!         w.set_reg_timergrp0_rst_en(true);
//!     });
//! }
//! ```
//!
//! The permanent IRQ part cannot change the movable program owner's enable:
//!
//! ```compile_fail,E0599
//! # use oer_chips_esp32s31_pac::timg::Timg0InterruptBank;
//! fn enable(mut irq: Timg0InterruptBank) {
//!     irq.modify_int_ena_timers(|w| { w.set_t0_int_ena(true); });
//! }
//! ```
//!
//! The split CLR word exposes only T0's strobe:
//!
//! ```compile_fail,E0599
//! # use oer_chips_esp32s31_pac::timg::Timg0InterruptBank;
//! fn clear_watchdog(mut bank: Timg0InterruptBank) {
//!     bank.trigger_int_clr_timers_wdt_int_clr();
//! }
//! ```
//!
//! ```compile_fail,E0599
//! # use oer_chips_esp32s31_pac::timg::Timg0InterruptBank;
//! fn clear_timer1(mut bank: Timg0InterruptBank) {
//!     bank.trigger_int_clr_timers_t1_int_clr();
//! }
//! ```
//!
//! PMU wake observations grant no enable writes or pending acknowledgement.
//! The radio clock/power owner has no access to these wake words.
//!
//! ```
//! fn observe(wake: &oer_chips_esp32s31_pac::pmu::PmuWakeStatus) -> bool {
//!     wake.read_hp_int_ena().soc_wakeup_int_ena()
//! }
//! ```
//!
//! ```compile_fail
//! fn change(wake: &mut oer_chips_esp32s31_pac::pmu::PmuWakeStatus) {
//!     wake.modify_hp_int_ena(|_| {});
//! }
//! ```
//!
//! ```compile_fail
//! fn change(wake: &mut oer_chips_esp32s31_pac::pmu::PmuWakeStatus) {
//!     wake.modify_lp_int_ena(|_| {});
//! }
//! ```
//!
//! ```compile_fail
//! fn acknowledge(
//!     wake: &mut oer_chips_esp32s31_pac::pmu::PmuWakeStatus,
//!     sample: oer_chips_esp32s31_pac::pmu::int_raw::Snapshot,
//! ) {
//!     wake.acknowledge_int_raw(sample);
//! }
//! ```
//!
//! ```compile_fail
//! fn acknowledge(
//!     wake: &mut oer_chips_esp32s31_pac::pmu::PmuWakeStatus,
//!     sample: oer_chips_esp32s31_pac::pmu::lp_int_raw::Snapshot,
//! ) {
//!     wake.acknowledge_lp_int_raw(sample);
//! }
//! ```
//!
//! ```compile_fail
//! fn observe(radio: &oer_chips_esp32s31_pac::pmu::PmuRadio) {
//!     let _ = radio.read_hp_int_ena();
//! }
//! ```
//!
//! Modem wake-route observation is independent of clock configuration and
//! grants no write to the inherited wake enables.
//!
//! ```
//! fn observe(wake: &oer_chips_esp32s31_pac::modem_lpcon::ModemWakeStatus) -> bool {
//!     wake.read_modem_intr_wakeup_conf().lp_timer_intr_wakeup_en()
//! }
//! ```
//!
//! ```compile_fail,E0599
//! fn change(wake: &mut oer_chips_esp32s31_pac::modem_lpcon::ModemWakeStatus) {
//!     wake.modify_modem_intr_wakeup_conf(|_| {});
//! }
//! ```
//!
//! ```compile_fail,E0599
//! fn change(wake: &mut oer_chips_esp32s31_pac::pmu::PmuWakeStatus) {
//!     wake.modify_slp_wakeup_cntl2(|_| {});
//! }
//! ```

#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

#[cfg(test)]
extern crate std;

mod generated;

pub use generated::*;

use core::sync::atomic::{AtomicBool, Ordering};

/// Whether the root's one claim was taken.
static TAKEN: AtomicBool = AtomicBool::new(false);

/// Every partition of the claimed register blocks, the first time it is
/// called; `None` after.
///
/// The PAC is the only code that constructs these blocks: no other crate
/// of an image reaches the claimed words.
pub fn take() -> Option<ownership::Partitions> {
    if TAKEN.swap(true, Ordering::AcqRel) {
        return None;
    }
    #[allow(
        unsafe_code,
        clippy::disallowed_methods,
        reason = "the PAC root takes its one claim"
    )]
    // SAFETY: `TAKEN` lets this run once per boot, so no other claim of
    // these blocks exists; the generated per-hart `disjoint_blocks` checks
    // prove no two blocks of one hart's context list one word, and no other
    // code of an image accesses a claimed word (the PAC is the only
    // constructor of a block). A hart-local word is listed once per hart:
    // element `h` of a hart-local part is accessed only on hart `h`, which
    // the HAL's per-hart grant enforces.
    let claims = unsafe { oer_portable_mmio::assume_owned::<ownership::Claims>() };
    Some(ownership::partition(claims))
}
