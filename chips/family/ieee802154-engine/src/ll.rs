//! Hardware-side helpers of the public ESP-IDF IEEE 802.15.4 driver, ported
//! over a backend whose methods are the driver's `ieee802154_ll_*` accessors.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/ieee802154/driver/esp_ieee802154_util.c`,
//! `esp_ieee802154_timer.c` and `esp_ieee802154_dev.c` (Copyright Espressif
//! Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!
//! The pinned driver (`components/ieee802154/driver`) mixes hardware steps
//! with its callbacks and state. This module owns only the hardware steps:
//! the modem ETM channel helpers of `esp_ieee802154_util.c`, the timer
//! helpers of `esp_ieee802154_timer.c`, `event_end_process` and the register
//! part of `ieee802154_mac_init` from `esp_ieee802154_dev.c`, and
//! `ieee802154_sec_clear`. Timer callbacks, operation state and interrupt
//! allocation stay with the driver engine.
//!
//! Every helper is generic over [`Ieee802154LowLevel`], which chip adapters
//! implement over HAL transactions, so the engine and its tests issue exactly the
//! accessor sequence the vendor driver issues and a recording backend can
//! compare it with the compiled vendor driver.

use crate::coex::{CoexPti, Ieee802154Coexistence};
use oer_espressif_ieee802154_mac_values::{
    Ieee802154CcaMode, Ieee802154Channel, Ieee802154DebugCounter, Ieee802154EdSampleMode,
    Ieee802154EtmChannel, Ieee802154EtmRoute, Ieee802154Event, Ieee802154EventMask,
    Ieee802154EventObservation, Ieee802154MacCommand, Ieee802154MultipanEnableState,
    Ieee802154MultipanIndex, Ieee802154ResolvedTxPower, Ieee802154RxAbortEnableSet,
    Ieee802154RxAbortReasonObservation, Ieee802154RxStatus, Ieee802154Timer,
    Ieee802154TxAbortEnableSet, Ieee802154TxAbortReasonObservation,
};
use oer_ieee802154_port::RadioOperation;

/// The `ieee802154_ll_*` accessors, the modem ETM register steps and the
/// timer commands used by the driver's hardware helpers.
///
/// Each method is one accessor of the pinned public LL; implementations must
/// not add, merge or reorder register transactions.
pub trait Ieee802154LowLevel {
    /// A placed frame-buffer address, opaque to the shared engine.
    type Address: Copy + Eq;
    /// `ieee802154_ll_set_cmd` of an operation command.
    fn set_command(&mut self, command: Ieee802154MacCommand);
    /// `ieee802154_ll_get_events`.
    fn events(&mut self) -> Ieee802154EventObservation;
    /// Sample and acknowledge the asserted events of `mask` once.
    /// Stop timer producers and clear while their events are enabled, before
    /// masking: masked timer status hides retained pending.
    fn clear_events(&mut self, mask: Ieee802154EventMask);
    /// `ieee802154_ll_get_rx_abort_reason`.
    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation;
    /// `ieee802154_ll_get_tx_abort_reason`.
    fn tx_abort_reason(&mut self) -> Ieee802154TxAbortReasonObservation;
    /// `ieee802154_ll_get_rx_status`.
    fn rx_status(&mut self) -> Ieee802154RxStatus;
    /// `ieee802154_ll_is_current_rx_frame`.
    fn is_current_rx_frame(&mut self) -> bool;
    /// `ieee802154_ll_set_tx_addr`.
    fn set_tx_address(&mut self, address: Self::Address);
    /// `ieee802154_ll_set_rx_addr`.
    fn set_rx_address(&mut self, address: Self::Address);
    /// `ieee802154_ll_get_tx_auto_ack`.
    fn tx_auto_ack(&mut self) -> bool;
    /// `ieee802154_ll_get_rx_auto_ack`.
    fn rx_auto_ack(&mut self) -> bool;
    /// `ieee802154_ll_get_tx_enhance_ack`.
    fn tx_enhanced_ack(&mut self) -> bool;
    /// `ieee802154_ll_get_pending_mode`.
    fn pending_mode(&mut self) -> bool;
    /// `ieee802154_ll_set_pending_bit`.
    fn set_pending_bit(&mut self, pending: bool);
    /// `ieee802154_ll_get_freq`.
    fn frequency_code(&mut self) -> u8;
    /// `ieee802154_ll_get_ed_rss`.
    fn ed_rss(&mut self) -> i8;
    /// The chip's `IEEE802154_RSSI_COMPENSATION_VALUE` in dBm, added to every
    /// measured RSSI and energy-detection value the engine reports.
    fn rssi_compensation_dbm(&self) -> i8;
    /// `ieee802154_ll_is_cca_busy`.
    fn cca_busy(&mut self) -> bool;
    /// `ieee802154_ll_get_*_cnt`: one MAC diagnostic counter.
    fn debug_counter(&mut self, counter: Ieee802154DebugCounter) -> u16;
    /// `ieee802154_ll_clear_debug_cnt` of one counter.
    fn clear_debug_counter(&mut self, counter: Ieee802154DebugCounter);
    /// `ieee802154_ll_set_ed_duration` in 16-microsecond symbols.
    fn set_ed_duration(&mut self, symbols: u16);
    /// `ieee802154_ll_enhack_generate_done_notify`.
    fn notify_enhanced_ack_generated(&mut self);
    /// `ieee802154_ll_disable_rx_abort_events`.
    fn disable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet);
    /// `ieee802154_ll_set_multipan_panid`, which also enables the context.
    fn set_multipan_panid(&mut self, index: Ieee802154MultipanIndex, panid: u16);
    /// `ieee802154_ll_get_multipan_panid`.
    fn multipan_panid(&mut self, index: Ieee802154MultipanIndex) -> u16;
    /// `ieee802154_ll_set_multipan_short_addr`, which also enables the context.
    fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16);
    /// `ieee802154_ll_get_multipan_short_addr`.
    fn multipan_short_address(&mut self, index: Ieee802154MultipanIndex) -> u16;
    /// `ieee802154_ll_set_multipan_ext_addr`, which also enables the context.
    fn set_multipan_extended_address(&mut self, index: Ieee802154MultipanIndex, address: [u8; 8]);
    /// `ieee802154_ll_get_multipan_ext_addr`.
    fn multipan_extended_address(&mut self, index: Ieee802154MultipanIndex) -> [u8; 8];
    /// `ieee802154_ll_set_multipan_enable_mask`.
    fn set_multipan_enable(&mut self, state: Ieee802154MultipanEnableState);
    /// `ieee802154_ll_get_multipan_enable_mask`.
    fn multipan_enable(&mut self) -> Ieee802154MultipanEnableState;
    /// `ieee802154_ll_set_ack_timeout` in 16-microsecond units.
    fn set_ack_timeout(&mut self, units: u16);
    /// `ieee802154_ll_get_ack_timeout` in 16-microsecond units.
    fn ack_timeout(&mut self) -> u16;
    /// `ieee802154_ll_set_security_addr`.
    fn set_security_address(&mut self, address: &[u8; 8]);
    /// `ieee802154_ll_set_security_key`.
    fn set_security_key(&mut self, key: &[u8; 16]);
    /// `ieee802154_ll_set_security_offset`: the seven-bit field keeps the low
    /// seven bits, as the vendor bitfield assignment does.
    fn set_security_offset(&mut self, offset: u8);
    /// `ieee802154_ll_enable_events(IEEE802154_EVENT_MASK)`.
    fn enable_all_events(&mut self);
    /// `ieee802154_ll_enable_events` of one event.
    fn enable_event(&mut self, event: Ieee802154Event);
    /// `ieee802154_ll_disable_events` of one event.
    fn disable_event(&mut self, event: Ieee802154Event);
    /// `ieee802154_ll_enable_tx_abort_events`.
    fn enable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet);
    /// `ieee802154_ll_enable_rx_abort_events`.
    fn enable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet);
    /// `ieee802154_ll_set_ed_sample_mode`.
    fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode);
    /// `ieee802154_ll_disable_coex`.
    fn disable_coex(&mut self);
    /// libcoexist `hal_set_IEEE802154_TXRX_pti`.
    fn set_txrx_pti(&mut self, pti: CoexPti);
    /// libcoexist `hal_set_IEEE802154_ACK_pti`.
    fn set_ack_pti(&mut self, pti: CoexPti);
    /// `ieee802154_ll_set_freq` of the channel's frequency code.
    fn set_channel(&mut self, channel: Ieee802154Channel);
    /// `ieee802154_ll_set_power` of the resolved power index.
    fn set_tx_power(&mut self, power: &Ieee802154ResolvedTxPower<'_>);
    /// `ieee802154_ll_set_cca_mode`.
    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode);
    /// `ieee802154_ll_set_cca_threshold`.
    fn set_cca_threshold(&mut self, threshold_dbm: i8);
    /// `ieee802154_ll_set_tx_auto_ack`.
    fn set_tx_auto_ack(&mut self, enable: bool);
    /// `ieee802154_ll_set_rx_auto_ack`.
    fn set_rx_auto_ack(&mut self, enable: bool);
    /// `ieee802154_ll_set_tx_enhance_ack`.
    fn set_tx_enhanced_ack(&mut self, enable: bool);
    /// `ieee802154_ll_set_coordinator`.
    fn set_coordinator(&mut self, enable: bool);
    /// `ieee802154_ll_set_promiscuous`.
    fn set_promiscuous(&mut self, enable: bool);
    /// `ieee802154_ll_set_pending_mode`: the one-bit enhanced/Zigbee selector.
    fn set_pending_mode(&mut self, enhanced: bool);
    /// `ieee802154_ll_set_transmit_security`.
    fn set_transmit_security(&mut self, enable: bool);
    /// `ieee802154_ll_timer0_set_threshold` or `ieee802154_ll_timer1_set_threshold`
    /// in microseconds.
    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32);
    /// `ieee802154_ll_set_cmd` of the timer's start command.
    fn start_timer(&mut self, timer: Ieee802154Timer);
    /// `ieee802154_ll_set_cmd` of the timer's stop command.
    fn stop_timer(&mut self, timer: Ieee802154Timer);
    /// `REG_READ(ETM_CHEN_AD0_REG)` tested for the channel bit.
    fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool;
    /// `ETM_CHENCLR_AD0_REG` written back with the channel bit added.
    fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel);
    /// `ETM_CHENSET_AD0_REG` written back with the channel bit added.
    fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel);
    /// The channel's complete event word, then its complete task word.
    fn set_etm_route(&mut self, route: Ieee802154EtmRoute);
}

/// The live receive signal strength of chips whose owner reads it, as the
/// driver's `ieee802154_get_recent_rssi` does (on the ESP32-S31 the low byte
/// of the shared baseband's current receive information).
///
/// It is separate from [`Ieee802154LowLevel`]: it reads no MAC register, and
/// a chip provides it only once its owner models the read.
pub trait Ieee802154RecentRssi {
    /// The signed RSSI in dBm of the most recent reception.
    fn recent_rssi(&mut self) -> i8;
}

/// `ieee802154_etm_channel_clear`: disable `channel` only if it is enabled.
pub fn etm_channel_clear<Ll: Ieee802154LowLevel + ?Sized>(
    ll: &mut Ll,
    channel: Ieee802154EtmChannel,
) {
    if ll.etm_channel_enabled(channel) {
        ll.disable_etm_channel(channel);
    }
}

/// `ieee802154_etm_set_event_task`: clear the route's channel, program its
/// event and task words, then enable it.
pub fn etm_set_event_task<Ll: Ieee802154LowLevel + ?Sized>(ll: &mut Ll, route: Ieee802154EtmRoute) {
    let channel = route.channel();
    etm_channel_clear(ll, channel);
    ll.set_etm_route(route);
    ll.enable_etm_channel(channel);
}

/// `is_target_time_expired`: the wrapping difference `now - target` has a
/// clear sign bit, so a target equal to `now` has expired.
pub const fn target_time_expired(target: u32, now: u32) -> bool {
    now.wrapping_sub(target) & (1 << 31) == 0
}

/// Threshold `ieee802154_timer*_fire_at` programs for `fire_time` sampled at
/// `now`: zero once expired, otherwise the remaining microseconds.
pub const fn timer_threshold(fire_time: u32, now: u32) -> u32 {
    if target_time_expired(fire_time, now) {
        0
    } else {
        fire_time.wrapping_sub(now)
    }
}

/// `ieee802154_timer0_fire_at` / `ieee802154_timer1_fire_at`: program the
/// remaining interval, then start the timer. `now` is the caller's truncated
/// `esp_timer_get_time` sample.
pub fn timer_fire_at<Ll: Ieee802154LowLevel + ?Sized>(
    ll: &mut Ll,
    timer: Ieee802154Timer,
    fire_time: u32,
    now: u32,
) {
    ll.set_timer_threshold(timer, timer_threshold(fire_time, now));
    ll.start_timer(timer);
}

/// Register steps of `event_end_process`: clear both ETM channels, then stop
/// both timers. The driver engine also drops the timer callbacks.
pub fn event_end_process<Ll: Ieee802154LowLevel + ?Sized>(ll: &mut Ll) {
    etm_channel_clear(ll, Ieee802154EtmChannel::Channel0);
    etm_channel_clear(ll, Ieee802154EtmChannel::Channel1);
    ll.stop_timer(Ieee802154Timer::Timer0);
    ll.stop_timer(Ieee802154Timer::Timer1);
}

/// `ieee802154_sec_clear`.
pub fn sec_clear<Ll: Ieee802154LowLevel + ?Sized>(ll: &mut Ll) {
    ll.set_transmit_security(false);
}

/// Register steps of `ieee802154_mac_init` between the MAC reset and
/// `ieee802154_txon_delay_set`.
///
/// All events are enabled except TIMER0, which the ACK watchdog enables per
/// transmission; the runtime abort baselines are enabled; ED reports the
/// average sample. With software coexistence the ACK PTI takes its level and
/// the TX/RX PTI the idle scene; without it both PTIs are disabled. The
/// caller owns the preceding MAC reset and PIB initialization and the
/// following TX-on delay, interrupt allocation and modem initialization.
pub fn mac_init_registers<Ll: Ieee802154LowLevel + ?Sized>(
    ll: &mut Ll,
    coexistence: &Ieee802154Coexistence,
) {
    ll.enable_all_events();
    ll.disable_event(Ieee802154Event::Timer0Overflow);
    ll.enable_tx_aborts(Ieee802154TxAbortEnableSet::RuntimeBaseline);
    ll.enable_rx_aborts(Ieee802154RxAbortEnableSet::RuntimeBaseline);
    ll.set_ed_sample_mode(Ieee802154EdSampleMode::Average);
    match coexistence {
        Ieee802154Coexistence::Software(priorities) => {
            ll.set_ack_pti(priorities.operation(RadioOperation::Ack));
            ll.set_txrx_pti(priorities.operation(RadioOperation::Idle));
        }
        Ieee802154Coexistence::Disabled => ll.disable_coex(),
    }
}

/// `IEEE802154_SET_TXRX_PTI`: publish the TX/RX PTI of `operation`. A build
/// without software coexistence compiles it to nothing.
pub fn set_txrx_pti<Ll: Ieee802154LowLevel + ?Sized>(
    ll: &mut Ll,
    coexistence: &Ieee802154Coexistence,
    operation: RadioOperation,
) {
    if let Ieee802154Coexistence::Software(priorities) = coexistence {
        ll.set_txrx_pti(priorities.operation(operation));
    }
}

#[cfg(any(test, feature = "test-support"))]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "explicit test-support register model"
)]
pub mod model;

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    clippy::arithmetic_side_effects,
    clippy::unreachable,
    reason = "test assertions and reference fixtures"
)]
mod tests;
