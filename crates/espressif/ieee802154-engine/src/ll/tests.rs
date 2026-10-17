//! Accessor sequences are independent expectations read from the pinned
//! ESP-IDF driver sources, not snapshots of this module's output.

use std::{vec, vec::Vec};

use crate::{channel::*, coex::*, ll::*, tx_power::*, types::*};

/// One recorded accessor call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Call {
    EnableAllEvents,
    EnableEvent(Ieee802154Event),
    DisableEvent(Ieee802154Event),
    EnableTxAborts(Ieee802154TxAbortEnableSet),
    EnableRxAborts(Ieee802154RxAbortEnableSet),
    SetEdSampleMode(Ieee802154EdSampleMode),
    DisableCoex,
    SetTxrxPti(u8),
    SetAckPti(u8),
    SetChannel(u8),
    SetTxPower { channel: u8, index: u8 },
    SetCcaMode(Ieee802154CcaMode),
    SetCcaThreshold(i8),
    SetTxAutoAck(bool),
    SetRxAutoAck(bool),
    SetTxEnhancedAck(bool),
    SetCoordinator(bool),
    SetPromiscuous(bool),
    SetPendingMode(bool),
    SetTransmitSecurity(bool),
    SetTimerThreshold(Ieee802154Timer, u32),
    StartTimer(Ieee802154Timer),
    StopTimer(Ieee802154Timer),
    EtmChannelEnabled(Ieee802154EtmChannel),
    DisableEtmChannel(Ieee802154EtmChannel),
    EnableEtmChannel(Ieee802154EtmChannel),
    SetEtmRoute(Ieee802154EtmRoute),
}

/// Recording backend; `enabled` lists the ETM channels reported enabled.
#[derive(Default)]
pub(crate) struct Recorder {
    pub(crate) calls: Vec<Call>,
    pub(crate) enabled: Vec<Ieee802154EtmChannel>,
}

/// The HAL helpers never reach the interrupt-side or operation accessors;
/// the driver engine tests cover them.
macro_rules! unused {
    ($($name:ident($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        $(fn $name(&mut self, $(_: $arg),*) $(-> $ret)? {
            unreachable!(concat!(stringify!($name), " is not a HAL helper accessor"))
        })*
    };
}

impl Ieee802154LowLevel for Recorder {
    unused! {
        set_command(Ieee802154LlCommand);
        events() -> Ieee802154EventObservation;
        clear_events(Ieee802154EventMask);
        rx_abort_reason() -> Ieee802154RxAbortReasonObservation;
        tx_abort_reason() -> Ieee802154TxAbortReasonObservation;
        rx_status() -> Ieee802154RxStatus;
        is_current_rx_frame() -> bool;
        set_tx_address(u32);
        set_rx_address(u32);
        tx_auto_ack() -> bool;
        rx_auto_ack() -> bool;
        tx_enhanced_ack() -> bool;
        pending_mode() -> bool;
        set_pending_bit(bool);
        frequency_code() -> u8;
        ed_rss() -> i8;
        cca_busy() -> bool;
        debug_counter(Ieee802154DebugCounter) -> u16;
        clear_debug_counter(Ieee802154DebugCounter);
        set_ed_duration(u16);
        notify_enhanced_ack_generated();
        disable_rx_aborts(Ieee802154RxAbortEnableSet);
        set_multipan_panid(Ieee802154MultipanIndex, u16);
        multipan_panid(Ieee802154MultipanIndex) -> u16;
        set_multipan_short_address(Ieee802154MultipanIndex, u16);
        multipan_short_address(Ieee802154MultipanIndex) -> u16;
        set_multipan_extended_address(Ieee802154MultipanIndex, [u8; 8]);
        multipan_extended_address(Ieee802154MultipanIndex) -> [u8; 8];
        set_multipan_enable(Ieee802154MultipanEnableState);
        multipan_enable() -> Ieee802154MultipanEnableState;
        set_ack_timeout(u16);
        ack_timeout() -> u16;
        set_security_offset(u8);
    }

    fn set_security_address(&mut self, _: &[u8; 8]) {
        unreachable!("set_security_address is not a HAL helper accessor")
    }

    fn set_security_key(&mut self, _: &[u8; 16]) {
        unreachable!("set_security_key is not a HAL helper accessor")
    }

    fn enable_all_events(&mut self) {
        self.calls.push(Call::EnableAllEvents);
    }
    fn enable_event(&mut self, event: Ieee802154Event) {
        self.calls.push(Call::EnableEvent(event));
    }
    fn disable_event(&mut self, event: Ieee802154Event) {
        self.calls.push(Call::DisableEvent(event));
    }
    fn enable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet) {
        self.calls.push(Call::EnableTxAborts(set));
    }
    fn enable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.calls.push(Call::EnableRxAborts(set));
    }
    fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode) {
        self.calls.push(Call::SetEdSampleMode(mode));
    }
    fn disable_coex(&mut self) {
        self.calls.push(Call::DisableCoex);
    }
    fn set_txrx_pti(&mut self, pti: CoexPti) {
        self.calls.push(Call::SetTxrxPti(pti.value()));
    }
    fn set_ack_pti(&mut self, pti: CoexPti) {
        self.calls.push(Call::SetAckPti(pti.value()));
    }
    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.calls.push(Call::SetChannel(channel.number()));
    }
    fn set_tx_power(&mut self, power: &Ieee802154ResolvedTxPower<'_>) {
        self.calls.push(Call::SetTxPower {
            channel: power.channel().number(),
            index: power.selected_provider_index(),
        });
    }
    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.calls.push(Call::SetCcaMode(mode));
    }
    fn set_cca_threshold(&mut self, threshold_dbm: i8) {
        self.calls.push(Call::SetCcaThreshold(threshold_dbm));
    }
    fn set_tx_auto_ack(&mut self, enable: bool) {
        self.calls.push(Call::SetTxAutoAck(enable));
    }
    fn set_rx_auto_ack(&mut self, enable: bool) {
        self.calls.push(Call::SetRxAutoAck(enable));
    }
    fn set_tx_enhanced_ack(&mut self, enable: bool) {
        self.calls.push(Call::SetTxEnhancedAck(enable));
    }
    fn set_coordinator(&mut self, enable: bool) {
        self.calls.push(Call::SetCoordinator(enable));
    }
    fn set_promiscuous(&mut self, enable: bool) {
        self.calls.push(Call::SetPromiscuous(enable));
    }
    fn set_pending_mode(&mut self, enhanced: bool) {
        self.calls.push(Call::SetPendingMode(enhanced));
    }
    fn set_transmit_security(&mut self, enable: bool) {
        self.calls.push(Call::SetTransmitSecurity(enable));
    }
    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32) {
        self.calls
            .push(Call::SetTimerThreshold(timer, microseconds));
    }
    fn start_timer(&mut self, timer: Ieee802154Timer) {
        self.calls.push(Call::StartTimer(timer));
    }
    fn stop_timer(&mut self, timer: Ieee802154Timer) {
        self.calls.push(Call::StopTimer(timer));
    }
    fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool {
        self.calls.push(Call::EtmChannelEnabled(channel));
        self.enabled.contains(&channel)
    }
    fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.calls.push(Call::DisableEtmChannel(channel));
        self.enabled.retain(|enabled| *enabled != channel);
    }
    fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.calls.push(Call::EnableEtmChannel(channel));
        self.enabled.push(channel);
    }
    fn set_etm_route(&mut self, route: Ieee802154EtmRoute) {
        self.calls.push(Call::SetEtmRoute(route));
    }
}

/// `ieee802154_mac_init` without software coexistence
/// (esp_ieee802154_dev.c L909-L930).
#[test]
fn mac_init_enables_the_runtime_baseline_in_vendor_order() {
    let mut ll = Recorder::default();
    mac_init_registers(&mut ll, &Ieee802154Coexistence::Disabled);
    assert_eq!(
        ll.calls,
        [
            Call::EnableAllEvents,
            Call::DisableEvent(Ieee802154Event::Timer0Overflow),
            Call::EnableTxAborts(Ieee802154TxAbortEnableSet::RuntimeBaseline),
            Call::EnableRxAborts(Ieee802154RxAbortEnableSet::RuntimeBaseline),
            Call::SetEdSampleMode(Ieee802154EdSampleMode::Average),
            Call::DisableCoex,
        ]
    );
}

/// `ieee802154_mac_init` with software coexistence publishes the middle
/// ACK level, then the idle scene (esp_ieee802154_dev.c L924-L926), and
/// each scene switch publishes only the TX/RX PTI.
#[test]
fn software_coexistence_publishes_the_vendor_priorities() {
    let coexistence = Ieee802154Coexistence::Software(vendor_priorities());
    let mut ll = Recorder::default();
    mac_init_registers(&mut ll, &coexistence);
    assert_eq!(ll.calls[5..], [Call::SetAckPti(8), Call::SetTxrxPti(1)]);

    let mut ll = Recorder::default();
    set_txrx_pti(&mut ll, &coexistence, Ieee802154CoexScene::RxAt);
    set_txrx_pti(
        &mut ll,
        &Ieee802154Coexistence::Disabled,
        Ieee802154CoexScene::Tx,
    );
    assert_eq!(ll.calls, [Call::SetTxrxPti(8)]);
}

/// `ieee802154_etm_channel_clear` writes the clear word only for an enabled
/// channel (esp_ieee802154_util.c L70-L76).
#[test]
fn etm_channel_clear_disables_only_an_enabled_channel() {
    let mut ll = Recorder::default();
    etm_channel_clear(&mut ll, Ieee802154EtmChannel::Channel1);
    assert_eq!(
        ll.calls,
        [Call::EtmChannelEnabled(Ieee802154EtmChannel::Channel1)]
    );

    let mut ll = Recorder {
        enabled: vec![Ieee802154EtmChannel::Channel1],
        ..Recorder::default()
    };
    etm_channel_clear(&mut ll, Ieee802154EtmChannel::Channel1);
    assert_eq!(
        ll.calls,
        [
            Call::EtmChannelEnabled(Ieee802154EtmChannel::Channel1),
            Call::DisableEtmChannel(Ieee802154EtmChannel::Channel1),
        ]
    );
}

/// `ieee802154_etm_set_event_task` clears, programs, then enables the
/// channel (esp_ieee802154_util.c L78-L86).
#[test]
fn etm_set_event_task_reprograms_a_cleared_channel() {
    for (route, channel) in [
        (
            Ieee802154EtmRoute::Timer0ToTxStart,
            Ieee802154EtmChannel::Channel0,
        ),
        (
            Ieee802154EtmRoute::Timer0ToCcaTx,
            Ieee802154EtmChannel::Channel0,
        ),
        (
            Ieee802154EtmRoute::Timer1ToRxStart,
            Ieee802154EtmChannel::Channel1,
        ),
    ] {
        let mut ll = Recorder {
            enabled: vec![channel],
            ..Recorder::default()
        };
        etm_set_event_task(&mut ll, route);
        assert_eq!(
            ll.calls,
            [
                Call::EtmChannelEnabled(channel),
                Call::DisableEtmChannel(channel),
                Call::SetEtmRoute(route),
                Call::EnableEtmChannel(channel),
            ]
        );
    }
}

/// `event_end_process` clears both ETM channels, then stops both timers
/// (esp_ieee802154_dev.c L142-L148).
#[test]
fn event_end_clears_both_etm_channels_before_stopping_both_timers() {
    let mut ll = Recorder {
        enabled: vec![Ieee802154EtmChannel::Channel0],
        ..Recorder::default()
    };
    event_end_process(&mut ll);
    assert_eq!(
        ll.calls,
        [
            Call::EtmChannelEnabled(Ieee802154EtmChannel::Channel0),
            Call::DisableEtmChannel(Ieee802154EtmChannel::Channel0),
            Call::EtmChannelEnabled(Ieee802154EtmChannel::Channel1),
            Call::StopTimer(Ieee802154Timer::Timer0),
            Call::StopTimer(Ieee802154Timer::Timer1),
        ]
    );
}

/// `is_target_time_expired` tests the sign bit of `now - target`
/// (esp_ieee802154_timer.h L33-L36).
#[test]
fn a_target_expires_when_the_wrapping_difference_is_non_negative() {
    assert!(target_time_expired(100, 100));
    assert!(target_time_expired(100, 101));
    assert!(!target_time_expired(101, 100));
    assert!(target_time_expired(u32::MAX, 3));
    assert!(!target_time_expired(3, u32::MAX));
    assert!(!target_time_expired(1 << 31, 0));
    assert!(target_time_expired((1 << 31) + 1, 0));
}

/// `ieee802154_timer*_fire_at` programs zero once expired, otherwise the
/// wrapping remainder, then starts the timer (esp_ieee802154_timer.c).
#[test]
fn fire_at_programs_the_remaining_interval_then_starts() {
    assert_eq!(timer_threshold(1_000, 1_000), 0);
    assert_eq!(timer_threshold(1_000, 1_200), 0);
    assert_eq!(timer_threshold(1_200, 1_000), 200);
    assert_eq!(timer_threshold(4, u32::MAX - 5), 10);
    assert_eq!(timer_threshold(1 << 31, 0), 1 << 31);

    for timer in [Ieee802154Timer::Timer0, Ieee802154Timer::Timer1] {
        let mut ll = Recorder::default();
        timer_fire_at(&mut ll, timer, 5_000, 2_000);
        assert_eq!(
            ll.calls,
            [
                Call::SetTimerThreshold(timer, 3_000),
                Call::StartTimer(timer),
            ]
        );
    }
}

/// `ieee802154_sec_clear` only disables transmit security
/// (esp_ieee802154_sec.c L27-L30).
#[test]
fn sec_clear_disables_transmit_security() {
    let mut ll = Recorder::default();
    sec_clear(&mut ll);
    assert_eq!(ll.calls, [Call::SetTransmitSecurity(false)]);
}

/// The priorities the ESP32-S31 vendor table resolves for the driver's
/// default scene levels (idle 1, TX/RX 3, timed TX/RX 8, ACK 8), as an
/// example chip's coexistence.
fn vendor_priorities() -> Ieee802154CoexPriorities {
    let pti = |value| CoexPti::new(value).expect("four-bit priority");
    Ieee802154CoexPriorities::new(pti(1), pti(3), pti(8), pti(8))
}
