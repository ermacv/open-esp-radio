//! Task-side MAC operations: one public LL accessor each, over the generated
//! field accessors of the MAC's task side.
//!
//! SOURCE: esp-idf-7b9cc1ac-ieee802154-common-ll (`ieee802154_common_ll.h` of
//! ESP-IDF `4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d`), unless a step names
//! another source.

use oer_esp32s31_pac::ieee802154::{
    command::Opcode,
    ed_config::{CcaMode, EdSampleMode},
    event_enable, rx_abort_enable, security_key, tx_abort_enable,
};
use oer_espressif_coex::CoexPti;
use oer_espressif_ieee802154_mac_values::COEX_DISABLED_PTI;
use oer_espressif_ieee802154_mac_values::{
    Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154Channel, Ieee802154DebugCounter,
    Ieee802154EdSampleMode, Ieee802154Event, Ieee802154MacControl, Ieee802154MultipanEnableState,
    Ieee802154MultipanIndex, Ieee802154PanIdentity, Ieee802154RxAbortEnableSet, Ieee802154RxStatus,
    Ieee802154TxAbortEnableSet, Ieee802154TxStateCode, Ieee802154TxStatus,
};
use oer_mmio::{Bits, Index, Placement};

use super::{Ieee802154TaskOwner, codes};
use crate::{ieee802154::policy::Ieee802154MacPolicySnapshot, layouts::DmaWindowAddress};

/// The transmit-on delay `ieee802154_txon_delay_set` writes.
///
/// SOURCE: BLOB_LIBBTBB_IEEE802154_TXON_DELAY_SET.
const TX_ON_DELAY: Bits<10> = Bits::constant::<45>();

/// The receive-on delay `ieee802154_txon_delay_set` writes.
///
/// SOURCE: BLOB_LIBBTBB_IEEE802154_TXON_DELAY_SET.
const RX_ON_DELAY: Bits<11> = Bits::constant::<50>();

/// The transmit/receive turnaround delay `ieee802154_txon_delay_set` writes.
///
/// SOURCE: BLOB_LIBBTBB_IEEE802154_TXON_DELAY_SET.
const TXRX_SWITCH_DELAY: Bits<10> = Bits::constant::<117>();

/// The transmit-off delay `ieee802154_txon_delay_set` writes.
///
/// SOURCE: BLOB_LIBBTBB_IEEE802154_TXON_DELAY_SET.
const TX_OFF_DELAY: Bits<6> = Bits::constant::<5>();

/// One five-bit coexistence priority of the MAC's PTI fields.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct Ieee802154Pti(Bits<5>);

impl Ieee802154Pti {
    /// The priority both fields take while coexistence is disabled
    /// (`ieee802154_ll_disable_coex`).
    pub(crate) const COEX_DISABLED: Self = Self(Bits::constant::<{ COEX_DISABLED_PTI as u32 }>());

    /// A four-bit shared-table priority, unchanged in the five-bit field, as
    /// libcoexist writes it.
    pub(crate) const fn from_coex(pti: CoexPti) -> Self {
        // Four bits always fit the five-bit field.
        Self(Bits::truncate(pti.value() as u32))
    }

    /// The priority `value`, if it fits five bits.
    pub(crate) const fn new(value: u8) -> Option<Self> {
        match Bits::new(value as u32) {
            Ok(bits) => Some(Self(bits)),
            Err(_) => None,
        }
    }

    /// The priority.
    pub(crate) const fn value(self) -> u8 {
        byte(self.0)
    }
}

/// Readback of the interrupt-masked MAC foundation.
///
/// It excludes `EVENT_STATUS`: the foundation neither owns nor acknowledges
/// pending events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ieee802154FoundationSnapshot {
    events_masked: bool,
    rx_aborts_masked: bool,
    tx_aborts_masked: bool,
    ed_uses_average: bool,
    txrx_pti: Ieee802154Pti,
    ack_pti: Ieee802154Pti,
    txon_delay_applied: bool,
}

impl Ieee802154FoundationSnapshot {
    /// One readback, as a test reports it.
    #[cfg(test)]
    pub(crate) const fn new(
        events_masked: bool,
        rx_aborts_masked: bool,
        tx_aborts_masked: bool,
        ed_uses_average: bool,
        txrx_pti: Ieee802154Pti,
        ack_pti: Ieee802154Pti,
        txon_delay_applied: bool,
    ) -> Self {
        Self {
            events_masked,
            rx_aborts_masked,
            tx_aborts_masked,
            ed_uses_average,
            txrx_pti,
            ack_pti,
            txon_delay_applied,
        }
    }

    pub(crate) const fn events_masked(self) -> bool {
        self.events_masked
    }

    pub(crate) const fn rx_aborts_masked(self) -> bool {
        self.rx_aborts_masked
    }

    pub(crate) const fn tx_aborts_masked(self) -> bool {
        self.tx_aborts_masked
    }

    pub(crate) const fn ed_uses_average(self) -> bool {
        self.ed_uses_average
    }

    pub(crate) const fn txrx_pti(self) -> Ieee802154Pti {
        self.txrx_pti
    }

    pub(crate) const fn ack_pti(self) -> Ieee802154Pti {
        self.ack_pti
    }

    /// Whether `TXON_DELAY`, `TXOFF_DELAY`, `RXON_DELAY` and
    /// `TXRX_SWITCH_DELAY` hold the vendor MAC-initialization values.
    pub(crate) const fn txon_delay_applied(self) -> bool {
        self.txon_delay_applied
    }
}

/// The MAC power-sequencing words from `PAON_DELAY` to `DCDC_CTRL`: the
/// power-amplifier-on, transmit-on, transmit-enable stop, transmit-off,
/// receive-on, transmit/receive switch and continuous-receive delays and the
/// MAC's control of the DC-DC converter around transmission. Reserved and
/// absent bits are not exposed.
///
/// The public driver writes only the transmit-on, transmit-off, receive-on
/// and switch delays (`ieee802154_txon_delay_set`); no pinned vendor code
/// writes the others. Values are field values without physical units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PowerSequence {
    pa_on_delay: u16,
    tx_on_delay: u16,
    tx_enable_stop_delay: u8,
    tx_off_delay: u8,
    rx_on_delay: u16,
    txrx_switch_delay: u16,
    continuous_rx_delay: u8,
    dcdc_pre_raise_delay: u8,
    dcdc_drop_delay: u8,
    dcdc_control_enabled: bool,
    dcdc_raise_for_tx: bool,
}

impl Ieee802154PowerSequence {
    /// `PAON_DELAY`: ten-bit power-amplifier-on delay.
    pub const fn pa_on_delay(self) -> u16 {
        self.pa_on_delay
    }

    /// `TXON_DELAY`: ten-bit transmit-on delay.
    pub const fn tx_on_delay(self) -> u16 {
        self.tx_on_delay
    }

    /// `TXEN_STOP_DELAY`: six-bit transmit-enable stop delay.
    pub const fn tx_enable_stop_delay(self) -> u8 {
        self.tx_enable_stop_delay
    }

    /// `TXOFF_DELAY`: six-bit transmit-off delay.
    pub const fn tx_off_delay(self) -> u8 {
        self.tx_off_delay
    }

    /// `RXON_DELAY`: eleven-bit receive-on delay.
    pub const fn rx_on_delay(self) -> u16 {
        self.rx_on_delay
    }

    /// `TXRX_SWITCH_DELAY`: ten-bit transmit/receive switch delay.
    pub const fn txrx_switch_delay(self) -> u16 {
        self.txrx_switch_delay
    }

    /// `CONT_RX_DELAY`: six-bit continuous-receive delay.
    pub const fn continuous_rx_delay(self) -> u8 {
        self.continuous_rx_delay
    }

    /// `DCDC_CTRL.DCDC_PRE_UP_DELAY`: delay before the DC-DC raise.
    pub const fn dcdc_pre_raise_delay(self) -> u8 {
        self.dcdc_pre_raise_delay
    }

    /// `DCDC_CTRL.DCDC_DOWN_DELAY`: delay before the DC-DC drop.
    pub const fn dcdc_drop_delay(self) -> u8 {
        self.dcdc_drop_delay
    }

    /// `DCDC_CTRL.DCDC_CTRL_EN`: the MAC controls the DC-DC converter.
    pub const fn dcdc_control_enabled(self) -> bool {
        self.dcdc_control_enabled
    }

    /// `DCDC_CTRL.TX_DCDC_UP`: raise the DC-DC converter for transmission.
    pub const fn dcdc_raise_for_tx(self) -> bool {
        self.dcdc_raise_for_tx
    }
}

/// A field of at most eight bits, as a byte.
pub(super) const fn byte<const WIDTH: u32>(bits: Bits<WIDTH>) -> u8 {
    const { assert!(WIDTH <= 8, "the field fits a byte") };
    bits.get().to_le_bytes()[0]
}

/// A field of at most sixteen bits, as a halfword.
const fn halfword<const WIDTH: u32>(bits: Bits<WIDTH>) -> u16 {
    const { assert!(WIDTH <= 16, "the field fits a halfword") };
    let [low, high, _, _] = bits.get().to_le_bytes();
    u16::from_le_bytes([low, high])
}

/// The register index of one of the four PAN contexts.
const fn context(index: Ieee802154MultipanIndex) -> Index<4> {
    match index.value() {
        0 => Index::constant::<0>(),
        1 => Index::constant::<1>(),
        2 => Index::constant::<2>(),
        // A context index is below four by construction: the fourth one.
        _ => Index::constant::<3>(),
    }
}

/// The transmit-security key words, in write order, checked against the PAC.
const KEY_WORDS: [Index<{ security_key::COUNT }>; security_key::COUNT] = [
    Index::constant::<0>(),
    Index::constant::<1>(),
    Index::constant::<2>(),
    Index::constant::<3>(),
];

/// The `CCA_MODE` value of a CCA mode.
const fn cca_mode(mode: Ieee802154CcaMode) -> CcaMode {
    match mode {
        Ieee802154CcaMode::Carrier => CcaMode::Carrier,
        Ieee802154CcaMode::EnergyDetection => CcaMode::EnergyDetection,
        Ieee802154CcaMode::CarrierOrEnergyDetection => CcaMode::CarrierOrEnergyDetection,
        Ieee802154CcaMode::CarrierAndEnergyDetection => CcaMode::CarrierAndEnergyDetection,
    }
}

/// The CCA mode of a `CCA_MODE` value.
const fn cca_mode_of(mode: CcaMode) -> Ieee802154CcaMode {
    match mode {
        CcaMode::Carrier => Ieee802154CcaMode::Carrier,
        CcaMode::EnergyDetection => Ieee802154CcaMode::EnergyDetection,
        CcaMode::CarrierOrEnergyDetection => Ieee802154CcaMode::CarrierOrEnergyDetection,
        CcaMode::CarrierAndEnergyDetection => Ieee802154CcaMode::CarrierAndEnergyDetection,
    }
}

/// Set every `EVENT_ENABLE` field, the two unnamed events included, to
/// `enable`: `IEEE802154_EVENT_MASK`.
fn set_all_events(writer: &mut event_enable::W, enable: bool) {
    writer
        .set_tx_done(enable)
        .set_rx_done(enable)
        .set_ack_tx_done(enable)
        .set_ack_rx_done(enable)
        .set_rx_abort(enable)
        .set_tx_abort(enable)
        .set_ed_done(enable)
        .set_events_7(enable)
        .set_timer0_overflow(enable)
        .set_timer1_overflow(enable)
        .set_clock_count_match(enable)
        .set_tx_sfd_done(enable)
        .set_rx_sfd_done(enable)
        .set_events_13(enable);
}

/// Set one named event's `EVENT_ENABLE` field.
fn set_event(writer: &mut event_enable::W, event: Ieee802154Event, enable: bool) {
    match event {
        Ieee802154Event::TxDone => writer.set_tx_done(enable),
        Ieee802154Event::RxDone => writer.set_rx_done(enable),
        Ieee802154Event::AckTxDone => writer.set_ack_tx_done(enable),
        Ieee802154Event::AckRxDone => writer.set_ack_rx_done(enable),
        Ieee802154Event::RxAbort => writer.set_rx_abort(enable),
        Ieee802154Event::TxAbort => writer.set_tx_abort(enable),
        Ieee802154Event::EdDone => writer.set_ed_done(enable),
        Ieee802154Event::Timer0Overflow => writer.set_timer0_overflow(enable),
        Ieee802154Event::Timer1Overflow => writer.set_timer1_overflow(enable),
        Ieee802154Event::ClockCountMatch => writer.set_clock_count_match(enable),
        Ieee802154Event::TxSfdDone => writer.set_tx_sfd_done(enable),
        Ieee802154Event::RxSfdDone => writer.set_rx_sfd_done(enable),
    };
}

/// The events the interrupt handler serves: every named event but TIMER0
/// overflow and the clock-count match, which the driver enables only while
/// it uses them.
const RUNTIME_EVENTS: [Ieee802154Event; 10] = [
    Ieee802154Event::TxDone,
    Ieee802154Event::RxDone,
    Ieee802154Event::AckTxDone,
    Ieee802154Event::AckRxDone,
    Ieee802154Event::RxAbort,
    Ieee802154Event::TxAbort,
    Ieee802154Event::EdDone,
    Ieee802154Event::Timer1Overflow,
    Ieee802154Event::TxSfdDone,
    Ieee802154Event::RxSfdDone,
];

/// Set `IEEE802154_RX_ABORT_BASELINE`'s fields to `enable`: the ACK
/// transmit timeout and its coexistence break.
fn set_rx_abort_baseline(writer: &mut rx_abort_enable::W, enable: bool) {
    writer
        .set_tx_ack_timeout(enable)
        .set_tx_ack_coexistence_break(enable);
}

/// Set every `RX_ABORT_ENABLE` field to `enable`:
/// `IEEE802154_RX_ABORT_ALL`, the unnamed reason bits included.
fn set_rx_abort_all(writer: &mut rx_abort_enable::W, enable: bool) {
    let (bits_9_14, bits_19_22, bits_26_30) = if enable {
        (
            Bits::constant::<0x3f>(),
            Bits::constant::<0xf>(),
            Bits::constant::<0x1f>(),
        )
    } else {
        (
            Bits::constant::<0>(),
            Bits::constant::<0>(),
            Bits::constant::<0>(),
        )
    };
    writer
        .set_rx_stop(enable)
        .set_sfd_timeout(enable)
        .set_crc_error(enable)
        .set_invalid_length(enable)
        .set_filter_fail(enable)
        .set_no_rss(enable)
        .set_coexistence_break(enable)
        .set_unexpected_ack(enable)
        .set_rx_restart(enable)
        .set_rx_abort_en_9_14(bits_9_14)
        .set_tx_ack_timeout(enable)
        .set_tx_ack_stop(enable)
        .set_tx_ack_coexistence_break(enable)
        .set_enhanced_ack_security_error(enable)
        .set_rx_abort_en_19_22(bits_19_22)
        .set_ed_abort(enable)
        .set_ed_stop(enable)
        .set_ed_coexistence_reject(enable)
        .set_rx_abort_en_26_30(bits_26_30);
}

/// Set `IEEE802154_TX_ABORT_BASELINE`'s fields to `enable`: the ACK receive
/// timeout, the transmit coexistence break, the transmit security error and
/// both CCA failures.
fn set_tx_abort_baseline(writer: &mut tx_abort_enable::W, enable: bool) {
    writer
        .set_rx_ack_timeout(enable)
        .set_tx_coexistence_break(enable)
        .set_tx_security_error(enable)
        .set_cca_failed(enable)
        .set_cca_busy(enable);
}

/// Set every `TX_ABORT_ENABLE` field to `enable`:
/// `IEEE802154_TX_ABORT_ALL`, the unnamed reason bits included.
fn set_tx_abort_all(writer: &mut tx_abort_enable::W, enable: bool) {
    let (bits_9_14, bits_19_22, bits_25_30) = if enable {
        (
            Bits::constant::<0x3f>(),
            Bits::constant::<0xf>(),
            Bits::constant::<0x3f>(),
        )
    } else {
        (
            Bits::constant::<0>(),
            Bits::constant::<0>(),
            Bits::constant::<0>(),
        )
    };
    writer
        .set_rx_ack_stop(enable)
        .set_rx_ack_sfd_timeout(enable)
        .set_rx_ack_crc_error(enable)
        .set_rx_ack_invalid_length(enable)
        .set_rx_ack_filter_fail(enable)
        .set_rx_ack_no_rss(enable)
        .set_rx_ack_coexistence_break(enable)
        .set_rx_ack_type_not_ack(enable)
        .set_rx_ack_restart(enable)
        .set_tx_abort_en_9_14(bits_9_14)
        .set_rx_ack_timeout(enable)
        .set_tx_stop(enable)
        .set_tx_coexistence_break(enable)
        .set_tx_security_error(enable)
        .set_tx_abort_en_19_22(bits_19_22)
        .set_cca_failed(enable)
        .set_cca_busy(enable)
        .set_tx_abort_en_25_30(bits_25_30);
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154TaskOwner<P, E, B> {
    /// `ieee802154_ll_set_cmd`: write one command image from zero.
    pub(crate) fn issue(&mut self, opcode: Opcode) {
        self.mac.write_command(opcode, |_| {});
    }

    /// Mask every MAC event, the two unnamed ones included.
    pub(crate) fn mask_all_events(&mut self) {
        self.mac
            .modify_event_enable(|writer| set_all_events(writer, false));
    }

    /// Enable exactly the events the interrupt handler serves.
    pub(crate) fn enable_runtime_events(&mut self) {
        self.mac.modify_event_enable(|writer| {
            set_all_events(writer, false);
            for event in RUNTIME_EVENTS {
                set_event(writer, event, true);
            }
        });
    }

    /// `ieee802154_ll_enable_events(IEEE802154_EVENT_MASK)`: set all
    /// fourteen enable fields, the two unnamed ones the public mask covers
    /// included.
    pub(crate) fn enable_all_events(&mut self) {
        self.mac
            .modify_event_enable(|writer| set_all_events(writer, true));
    }

    /// `ieee802154_ll_enable_events` or `ieee802154_ll_disable_events` of
    /// one event.
    pub(crate) fn set_event_enabled(&mut self, event: Ieee802154Event, enable: bool) {
        self.mac
            .modify_event_enable(|writer| set_event(writer, event, enable));
    }

    /// Mask every receive-abort reason.
    pub(crate) fn mask_all_rx_aborts(&mut self) {
        self.mac
            .modify_rx_abort_enable(|writer| set_rx_abort_all(writer, false));
    }

    /// Enable exactly the runtime receive-abort baseline.
    pub(crate) fn enable_only_runtime_rx_aborts(&mut self) {
        self.mac.modify_rx_abort_enable(|writer| {
            set_rx_abort_all(writer, false);
            set_rx_abort_baseline(writer, true);
        });
    }

    /// `ieee802154_ll_enable_rx_abort_events` (`enable`) or
    /// `ieee802154_ll_disable_rx_abort_events` of one set: the set's fields
    /// change, every other keeps its value.
    pub(crate) fn set_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet, enable: bool) {
        self.mac.modify_rx_abort_enable(|writer| match set {
            Ieee802154RxAbortEnableSet::RuntimeBaseline => set_rx_abort_baseline(writer, enable),
            Ieee802154RxAbortEnableSet::All => set_rx_abort_all(writer, enable),
        });
    }

    /// Mask every transmit-abort reason.
    pub(crate) fn mask_all_tx_aborts(&mut self) {
        self.mac
            .modify_tx_abort_enable(|writer| set_tx_abort_all(writer, false));
    }

    /// Enable exactly the runtime transmit-abort baseline.
    pub(crate) fn enable_only_runtime_tx_aborts(&mut self) {
        self.mac.modify_tx_abort_enable(|writer| {
            set_tx_abort_all(writer, false);
            set_tx_abort_baseline(writer, true);
        });
    }

    /// `ieee802154_ll_enable_tx_abort_events` (`enable`) or
    /// `ieee802154_ll_disable_tx_abort_events` of one set.
    pub(crate) fn set_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet, enable: bool) {
        self.mac.modify_tx_abort_enable(|writer| match set {
            Ieee802154TxAbortEnableSet::RuntimeBaseline => set_tx_abort_baseline(writer, enable),
            Ieee802154TxAbortEnableSet::All => set_tx_abort_all(writer, enable),
        });
    }

    /// `ieee802154_ll_set_ed_sample_mode`.
    pub(crate) fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode) {
        let mode = match mode {
            Ieee802154EdSampleMode::Maximum => EdSampleMode::Maximum,
            Ieee802154EdSampleMode::Average => EdSampleMode::Average,
        };
        self.mac.modify_ed_config(|writer| {
            writer.set_ed_sample_mode(mode);
        });
    }

    /// `ieee802154_txon_delay_set`: the MAC transmit-on, receive-on,
    /// turnaround and transmit-off delays, in the vendor order.
    pub(crate) fn apply_txon_delay(&mut self) {
        self.mac.modify_txon_delay(|writer| {
            writer.set_txon_delay(TX_ON_DELAY);
        });
        self.mac.modify_rxon_delay(|writer| {
            writer.set_rxon_delay(RX_ON_DELAY);
        });
        self.mac.modify_txrx_switch_delay(|writer| {
            writer.set_txrx_switch_delay(TXRX_SWITCH_DELAY);
        });
        self.mac.modify_txoff_delay(|writer| {
            writer.set_txoff_delay(TX_OFF_DELAY);
        });
    }

    /// `ieee802154_ll_set_freq` of the channel's frequency code.
    pub(crate) fn set_channel(&mut self, channel: Ieee802154Channel) {
        let code = channel.frequency_code();
        self.mac.modify_channel(|writer| {
            writer.set_frequency_code(code);
        });
    }

    /// `ieee802154_ll_get_freq`.
    pub(crate) fn frequency_code(&self) -> u8 {
        self.mac.read_channel().frequency_code()
    }

    /// `ieee802154_ll_set_power` of a power-table index; the field carries
    /// no dBm or calibration meaning.
    pub(crate) fn set_tx_power_code(&mut self, code: u8) {
        self.mac.modify_tx_power(|writer| {
            writer.set_power_code(code);
        });
    }

    /// `ieee802154_ll_set_cca_mode`.
    pub(crate) fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.mac.modify_ed_config(|writer| {
            writer.set_cca_mode(cca_mode(mode));
        });
    }

    /// `ieee802154_ll_set_cca_threshold`: the signed register code.
    pub(crate) fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.mac.modify_ed_config(|writer| {
            writer.set_cca_threshold_code(threshold.cast_unsigned());
        });
    }

    /// `ieee802154_ll_set_ack_timeout` in 16-microsecond units.
    pub(crate) fn set_ack_timeout(&mut self, units: u16) {
        self.mac.modify_ack_timeout(|writer| {
            writer.set_timeout(units);
        });
    }

    /// `ieee802154_ll_get_ack_timeout` in 16-microsecond units.
    pub(crate) fn ack_timeout(&self) -> u16 {
        self.mac.read_ack_timeout().timeout()
    }

    /// `ieee802154_ll_set_tx_auto_ack`.
    pub(crate) fn set_tx_auto_ack(&mut self, enable: bool) {
        self.mac.modify_control(|writer| {
            writer.set_auto_ack_tx(enable);
        });
    }

    /// `ieee802154_ll_get_tx_auto_ack`.
    pub(crate) fn tx_auto_ack(&self) -> bool {
        self.mac.read_control().auto_ack_tx()
    }

    /// `ieee802154_ll_set_rx_auto_ack`.
    pub(crate) fn set_rx_auto_ack(&mut self, enable: bool) {
        self.mac.modify_control(|writer| {
            writer.set_auto_ack_rx(enable);
        });
    }

    /// `ieee802154_ll_get_rx_auto_ack`.
    pub(crate) fn rx_auto_ack(&self) -> bool {
        self.mac.read_control().auto_ack_rx()
    }

    /// `ieee802154_ll_set_tx_enhance_ack`.
    pub(crate) fn set_tx_enhanced_ack(&mut self, enable: bool) {
        self.mac.modify_control(|writer| {
            writer.set_enhanced_ack_tx(enable);
        });
    }

    /// `ieee802154_ll_get_tx_enhance_ack`.
    pub(crate) fn tx_enhanced_ack(&self) -> bool {
        self.mac.read_control().enhanced_ack_tx()
    }

    /// `ieee802154_ll_set_coordinator`.
    pub(crate) fn set_coordinator(&mut self, enable: bool) {
        self.mac.modify_control(|writer| {
            writer.set_coordinator(enable);
        });
    }

    /// `ieee802154_ll_set_promiscuous`.
    pub(crate) fn set_promiscuous(&mut self, enable: bool) {
        self.mac.modify_control(|writer| {
            writer.set_promiscuous(enable);
        });
    }

    /// `ieee802154_ll_set_pending_mode`: the one-bit enhanced/Zigbee
    /// selector.
    pub(crate) fn set_pending_mode(&mut self, enhanced: bool) {
        self.mac.modify_control(|writer| {
            writer.set_pending_enhanced(enhanced);
        });
    }

    /// `ieee802154_ll_get_pending_mode`.
    pub(crate) fn pending_mode(&self) -> bool {
        self.mac.read_control().pending_enhanced()
    }

    /// The six PIB control fields, one public LL setter each, in the vendor
    /// update order.
    pub(crate) fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.set_tx_auto_ack(control.tx_auto_ack());
        self.set_rx_auto_ack(control.rx_auto_ack());
        self.set_tx_enhanced_ack(control.enhanced_ack_tx());
        self.set_coordinator(control.coordinator());
        self.set_promiscuous(control.promiscuous());
        self.set_pending_mode(control.enhanced_pending());
    }

    /// Enable one PAN context, keeping the other three.
    fn enable_context(&mut self, index: Ieee802154MultipanIndex) {
        self.mac.modify_control(|writer| {
            match index.value() {
                0 => writer.set_multipan0_enabled(true),
                1 => writer.set_multipan1_enabled(true),
                2 => writer.set_multipan2_enabled(true),
                _ => writer.set_multipan3_enabled(true),
            };
        });
    }

    /// `ieee802154_ll_set_multipan_panid`: enable the context, then write.
    pub(crate) fn set_multipan_pan_id(&mut self, index: Ieee802154MultipanIndex, pan_id: u16) {
        self.enable_context(index);
        self.mac.modify_multipan_pan_id(context(index), |writer| {
            writer.set_pan_id(pan_id);
        });
    }

    /// `ieee802154_ll_get_multipan_panid`.
    pub(crate) fn multipan_pan_id(&self, index: Ieee802154MultipanIndex) -> u16 {
        self.mac.read_multipan_pan_id(context(index)).pan_id()
    }

    /// `ieee802154_ll_set_multipan_short_addr`: enable the context, then
    /// write.
    pub(crate) fn set_multipan_short_address(
        &mut self,
        index: Ieee802154MultipanIndex,
        address: u16,
    ) {
        self.enable_context(index);
        self.mac
            .modify_multipan_short_address(context(index), |writer| {
                writer.set_address(address);
            });
    }

    /// `ieee802154_ll_get_multipan_short_addr`.
    pub(crate) fn multipan_short_address(&self, index: Ieee802154MultipanIndex) -> u16 {
        self.mac
            .read_multipan_short_address(context(index))
            .address()
    }

    /// `ieee802154_ll_set_multipan_ext_addr`: enable the context, then write
    /// the low and the high little-endian word.
    pub(crate) fn set_multipan_extended_address(
        &mut self,
        index: Ieee802154MultipanIndex,
        address: [u8; 8],
    ) {
        self.enable_context(index);
        let [a0, a1, a2, a3, a4, a5, a6, a7] = address;
        self.mac
            .modify_multipan_extended_address_low(context(index), |writer| {
                writer.set_address_word(u32::from_le_bytes([a0, a1, a2, a3]));
            });
        self.mac
            .modify_multipan_extended_address_high(context(index), |writer| {
                writer.set_address_word(u32::from_le_bytes([a4, a5, a6, a7]));
            });
    }

    /// `ieee802154_ll_get_multipan_ext_addr`.
    pub(crate) fn multipan_extended_address(&self, index: Ieee802154MultipanIndex) -> [u8; 8] {
        let [a0, a1, a2, a3] = self
            .mac
            .read_multipan_extended_address_low(context(index))
            .address_word()
            .to_le_bytes();
        let [a4, a5, a6, a7] = self
            .mac
            .read_multipan_extended_address_high(context(index))
            .address_word()
            .to_le_bytes();
        [a0, a1, a2, a3, a4, a5, a6, a7]
    }

    /// The primary PAN identity, as the public driver programs it: the
    /// PAN identifier, the short and the extended address of context zero,
    /// each enabling the context first.
    pub(crate) fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        let primary = Ieee802154MultipanIndex::CONTEXT0;
        self.set_multipan_pan_id(primary, identity.pan_id());
        self.set_multipan_short_address(primary, identity.short_address());
        self.set_multipan_extended_address(primary, identity.extended_address());
    }

    /// `ieee802154_ll_set_multipan_enable_mask`: all four context enables.
    pub(crate) fn set_multipan_enable(&mut self, state: Ieee802154MultipanEnableState) {
        let [first, second, third, fourth] = state.enabled();
        self.mac.modify_control(|writer| {
            writer
                .set_multipan0_enabled(first)
                .set_multipan1_enabled(second)
                .set_multipan2_enabled(third)
                .set_multipan3_enabled(fourth);
        });
    }

    /// `ieee802154_ll_get_multipan_enable_mask`.
    pub(crate) fn multipan_enable(&self) -> Ieee802154MultipanEnableState {
        let control = self.mac.read_control();
        Ieee802154MultipanEnableState::new(
            control.multipan0_enabled(),
            control.multipan1_enabled(),
            control.multipan2_enabled(),
            control.multipan3_enabled(),
        )
    }

    /// `ieee802154_ll_set_pending_bit`: the outgoing ACK's frame-pending bit.
    pub(crate) fn set_frame_pending(&mut self, pending: bool) {
        self.mac.modify_pending_config(|writer| {
            writer.set_frame_pending(pending);
        });
    }

    /// `ieee802154_ll_enhack_generate_done_notify`: write its one image.
    pub(crate) fn notify_enhanced_ack_generated(&mut self) {
        self.mac.write_enhanced_ack_notify(|writer| {
            writer.set_notify_image(
                oer_esp32s31_pac::ieee802154::enhanced_ack_notify::NotifyImage::GenerateDone.bits(),
            );
        });
    }

    /// `ieee802154_ll_set_transmit_security`.
    pub(crate) fn set_transmit_security(&mut self, enable: bool) {
        self.mac.modify_security_control(|writer| {
            writer.set_tx_enable(enable);
        });
    }

    /// `ieee802154_ll_set_security_offset`: the seven-bit field keeps the
    /// low seven bits, as the vendor bitfield assignment does.
    pub(crate) fn set_security_offset(&mut self, offset: u8) {
        let offset = Bits::truncate(u32::from(offset));
        self.mac.modify_security_control(|writer| {
            writer.set_payload_offset(offset);
        });
    }

    /// `ieee802154_ll_set_security_addr`: the low, then the high
    /// little-endian word.
    pub(crate) fn set_security_address(&mut self, address: &[u8; 8]) {
        let [a0, a1, a2, a3, a4, a5, a6, a7] = *address;
        self.mac.write_security_address_low(|writer| {
            writer.set_address_word(u32::from_le_bytes([a0, a1, a2, a3]));
        });
        self.mac.write_security_address_high(|writer| {
            writer.set_address_word(u32::from_le_bytes([a4, a5, a6, a7]));
        });
    }

    /// `ieee802154_ll_set_security_key`: four little-endian words in order.
    pub(crate) fn set_security_key(&mut self, key: &[u8; 16]) {
        let (words, _) = key.as_chunks::<4>();
        for (index, word) in KEY_WORDS.into_iter().zip(words) {
            let word = u32::from_le_bytes(*word);
            self.mac.write_security_key(index, |writer| {
                writer.set_key_word(word);
            });
        }
    }

    /// Replace only the five-bit TX/RX coexistence priority field.
    pub(crate) fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.mac.modify_coex_pti(|writer| {
            writer.set_txrx_pti(pti.0);
        });
    }

    /// Replace only the five-bit ACK coexistence priority field.
    pub(crate) fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.mac.modify_coex_pti(|writer| {
            writer.set_ack_pti(pti.0);
        });
    }

    /// `ieee802154_ll_set_tx_addr`: the placed frame the transmitter reads.
    pub(crate) fn set_tx_address(&mut self, address: DmaWindowAddress) {
        self.mac.write_tx_dma_address(address, |_| {});
    }

    /// `ieee802154_ll_set_rx_addr`: the placed frame the receiver writes.
    pub(crate) fn set_rx_address(&mut self, address: DmaWindowAddress) {
        self.mac.write_rx_dma_address(address, |_| {});
    }

    /// `ieee802154_ll_set_ed_duration`: the public LL's sixteen-bit
    /// duration into the twenty-four-bit field, keeping the delay field.
    pub(crate) fn set_ed_duration(&mut self, symbols: u16) {
        // Sixteen bits always fit the twenty-four-bit field.
        let duration = Bits::truncate(u32::from(symbols));
        self.mac.modify_ed_duration(|writer| {
            writer.set_duration(duration);
        });
    }

    /// `ieee802154_ll_timer0_set_threshold`, in microseconds.
    pub(crate) fn set_timer0_threshold(&mut self, microseconds: u32) {
        self.mac.write_timer0_threshold(|writer| {
            writer.set_threshold(microseconds);
        });
    }

    /// `ieee802154_ll_timer1_set_threshold`, in microseconds.
    pub(crate) fn set_timer1_threshold(&mut self, microseconds: u32) {
        self.mac.write_timer1_threshold(|writer| {
            writer.set_threshold(microseconds);
        });
    }

    /// `ieee802154_ll_get_rx_status`, field by field.
    pub(crate) fn rx_status(&self) -> Ieee802154RxStatus {
        let status = self.mac.read_rx_status();
        Ieee802154RxStatus::new(
            byte(status.filter_fail_reason()),
            codes::rx_abort_reason(status.abort_reason_variant()),
            codes::rx_state(status.state()),
            status.preamble_match(),
            status.sfd_match(),
        )
    }

    /// `ieee802154_ll_get_tx_status`, field by field.
    pub fn tx_status(&self) -> Ieee802154TxStatus {
        let status = self.mac.read_tx_status();
        Ieee802154TxStatus::new(
            Ieee802154TxStateCode::from_field(byte(status.state())),
            codes::tx_abort_reason(status.abort_reason_variant()),
            codes::tx_security_error(
                status.security_error_variant(),
                status.security_error().get() == 0,
            ),
        )
    }

    /// `ieee802154_ll_get_*_cnt`: one counter as the register model's
    /// sixteen-bit field.
    pub(crate) fn debug_counter(&self, counter: Ieee802154DebugCounter) -> u16 {
        let mac = &self.mac;
        match counter {
            Ieee802154DebugCounter::SfdTimeout => {
                mac.read_sfd_timeout_counter().sfd_timeout_count()
            }
            Ieee802154DebugCounter::CrcError => mac.read_crc_error_counter().crc_error_count(),
            Ieee802154DebugCounter::EdAbort => mac.read_ed_abort_counter().count(),
            Ieee802154DebugCounter::CcaFail => mac.read_cca_fail_counter().count(),
            Ieee802154DebugCounter::RxFilterFail => mac.read_rx_filter_fail_counter().count(),
            Ieee802154DebugCounter::NoRssDetect => mac.read_no_rss_detect_counter().count(),
            Ieee802154DebugCounter::RxAbortCoex => mac.read_rx_abort_coex_counter().count(),
            Ieee802154DebugCounter::RxRestart => mac.read_rx_restart_counter().count(),
            Ieee802154DebugCounter::TxAckAbortCoex => mac.read_tx_ack_abort_coex_counter().count(),
            Ieee802154DebugCounter::EdScanBreakCoex => {
                mac.read_ed_scan_break_coex_counter().count()
            }
            Ieee802154DebugCounter::RxAckAbortCoex => mac.read_rx_ack_abort_coex_counter().count(),
            Ieee802154DebugCounter::RxAckTimeout => mac.read_rx_ack_timeout_counter().count(),
            Ieee802154DebugCounter::TxBreakCoex => mac.read_tx_break_coex_counter().count(),
            Ieee802154DebugCounter::TxSecurityError => mac.read_tx_security_error_counter().count(),
            Ieee802154DebugCounter::CcaBusy => mac.read_cca_busy_counter().count(),
        }
    }

    /// `ieee802154_ll_clear_debug_cnt` of one counter: the clear register
    /// written from zero with only that counter's field set.
    pub(crate) fn clear_debug_counter(&mut self, counter: Ieee802154DebugCounter) {
        self.mac.write_diagnostic_counter_clear(|writer| {
            match counter {
                Ieee802154DebugCounter::SfdTimeout => writer.set_sfd_timeout(true),
                Ieee802154DebugCounter::CrcError => writer.set_crc_error(true),
                Ieee802154DebugCounter::EdAbort => writer.set_ed_abort(true),
                Ieee802154DebugCounter::CcaFail => writer.set_cca_fail(true),
                Ieee802154DebugCounter::RxFilterFail => writer.set_rx_filter_fail(true),
                Ieee802154DebugCounter::NoRssDetect => writer.set_no_rss_detect(true),
                Ieee802154DebugCounter::RxAbortCoex => writer.set_rx_abort_coex(true),
                Ieee802154DebugCounter::RxRestart => writer.set_rx_restart(true),
                Ieee802154DebugCounter::TxAckAbortCoex => writer.set_tx_ack_abort_coex(true),
                Ieee802154DebugCounter::EdScanBreakCoex => writer.set_ed_scan_break_coex(true),
                Ieee802154DebugCounter::RxAckAbortCoex => writer.set_rx_ack_abort_coex(true),
                Ieee802154DebugCounter::RxAckTimeout => writer.set_rx_ack_timeout(true),
                Ieee802154DebugCounter::TxBreakCoex => writer.set_tx_break_coex(true),
                Ieee802154DebugCounter::TxSecurityError => writer.set_tx_security_error(true),
                Ieee802154DebugCounter::CcaBusy => writer.set_cca_busy(true),
            };
        });
    }

    /// Read the fields the interrupt-masked foundation writes, once per
    /// word.
    pub(crate) fn foundation_snapshot(&self) -> Ieee802154FoundationSnapshot {
        let mac = &self.mac;
        let events = mac.read_event_enable();
        let rx_aborts = mac.read_rx_abort_enable();
        let tx_aborts = mac.read_tx_abort_enable();
        let ed_config = mac.read_ed_config();
        let coex_pti = mac.read_coex_pti();
        let tx_on = mac.read_txon_delay().txon_delay();
        let tx_off = mac.read_txoff_delay().txoff_delay();
        let rx_on = mac.read_rxon_delay().rxon_delay();
        let switch = mac.read_txrx_switch_delay().txrx_switch_delay();
        let events_masked = !(events.tx_done()
            || events.rx_done()
            || events.ack_tx_done()
            || events.ack_rx_done()
            || events.rx_abort()
            || events.tx_abort()
            || events.ed_done()
            || events.events_7()
            || events.timer0_overflow()
            || events.timer1_overflow()
            || events.clock_count_match()
            || events.tx_sfd_done()
            || events.rx_sfd_done()
            || events.events_13());
        let rx_aborts_masked = !(rx_aborts.rx_stop()
            || rx_aborts.sfd_timeout()
            || rx_aborts.crc_error()
            || rx_aborts.invalid_length()
            || rx_aborts.filter_fail()
            || rx_aborts.no_rss()
            || rx_aborts.coexistence_break()
            || rx_aborts.unexpected_ack()
            || rx_aborts.rx_restart()
            || rx_aborts.rx_abort_en_9_14().get() != 0
            || rx_aborts.tx_ack_timeout()
            || rx_aborts.tx_ack_stop()
            || rx_aborts.tx_ack_coexistence_break()
            || rx_aborts.enhanced_ack_security_error()
            || rx_aborts.rx_abort_en_19_22().get() != 0
            || rx_aborts.ed_abort()
            || rx_aborts.ed_stop()
            || rx_aborts.ed_coexistence_reject()
            || rx_aborts.rx_abort_en_26_30().get() != 0);
        let tx_aborts_masked = !(tx_aborts.rx_ack_stop()
            || tx_aborts.rx_ack_sfd_timeout()
            || tx_aborts.rx_ack_crc_error()
            || tx_aborts.rx_ack_invalid_length()
            || tx_aborts.rx_ack_filter_fail()
            || tx_aborts.rx_ack_no_rss()
            || tx_aborts.rx_ack_coexistence_break()
            || tx_aborts.rx_ack_type_not_ack()
            || tx_aborts.rx_ack_restart()
            || tx_aborts.tx_abort_en_9_14().get() != 0
            || tx_aborts.rx_ack_timeout()
            || tx_aborts.tx_stop()
            || tx_aborts.tx_coexistence_break()
            || tx_aborts.tx_security_error()
            || tx_aborts.tx_abort_en_19_22().get() != 0
            || tx_aborts.cca_failed()
            || tx_aborts.cca_busy()
            || tx_aborts.tx_abort_en_25_30().get() != 0);
        Ieee802154FoundationSnapshot {
            events_masked,
            rx_aborts_masked,
            tx_aborts_masked,
            ed_uses_average: ed_config.ed_sample_mode_variant() == EdSampleMode::Average,
            txrx_pti: Ieee802154Pti(coex_pti.txrx_pti()),
            ack_pti: Ieee802154Pti(coex_pti.ack_pti()),
            txon_delay_applied: tx_on == TX_ON_DELAY
                && tx_off == TX_OFF_DELAY
                && rx_on == RX_ON_DELAY
                && switch == TXRX_SWITCH_DELAY,
        }
    }

    /// Read the static MAC policy the foundation owner writes, once per
    /// word: channel, CCA, ACK timeout, control, the context enables and
    /// the primary PAN identity.
    pub(crate) fn mac_policy_snapshot(&self) -> Ieee802154MacPolicySnapshot {
        let mac = &self.mac;
        let ed_config = mac.read_ed_config();
        let control = mac.read_control();
        let primary = Ieee802154MultipanIndex::CONTEXT0;
        Ieee802154MacPolicySnapshot::new(
            mac.read_channel().frequency_code(),
            cca_mode_of(ed_config.cca_mode_variant()),
            ed_config.cca_threshold_code().cast_signed(),
            Ieee802154AckTimeout::from_units(mac.read_ack_timeout().timeout()),
            Ieee802154MacControl::new(
                control.auto_ack_tx(),
                control.auto_ack_rx(),
                control.enhanced_ack_tx(),
                control.coordinator(),
                control.promiscuous(),
                control.pending_enhanced(),
            ),
            Ieee802154MultipanEnableState::new(
                control.multipan0_enabled(),
                control.multipan1_enabled(),
                control.multipan2_enabled(),
                control.multipan3_enabled(),
            ),
            Ieee802154PanIdentity::new(
                self.multipan_pan_id(primary),
                self.multipan_short_address(primary),
                self.multipan_extended_address(primary),
            ),
        )
    }

    /// Read the eight power-sequencing words from `PAON_DELAY` to
    /// `DCDC_CTRL`, each once.
    pub(crate) fn power_sequence(&self) -> Ieee802154PowerSequence {
        let mac = &self.mac;
        let dcdc = mac.read_dcdc_ctrl();
        Ieee802154PowerSequence {
            pa_on_delay: halfword(mac.read_paon_delay().paon_delay()),
            tx_on_delay: halfword(mac.read_txon_delay().txon_delay()),
            tx_enable_stop_delay: byte(mac.read_txen_stop_delay().txen_stop_dly()),
            tx_off_delay: byte(mac.read_txoff_delay().txoff_delay()),
            rx_on_delay: halfword(mac.read_rxon_delay().rxon_delay()),
            txrx_switch_delay: halfword(mac.read_txrx_switch_delay().txrx_switch_delay()),
            continuous_rx_delay: byte(mac.read_cont_rx_delay().cont_rx_delay()),
            dcdc_pre_raise_delay: dcdc.dcdc_pre_up_delay(),
            dcdc_drop_delay: dcdc.dcdc_down_delay(),
            dcdc_control_enabled: dcdc.dcdc_ctrl_en(),
            dcdc_raise_for_tx: dcdc.tx_dcdc_up(),
        }
    }
}
