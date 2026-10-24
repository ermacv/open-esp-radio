//! Affine task/interrupt ownership split for the shared IEEE 802.15.4 MAC
//! register block.
//!
//! The hardware places task-side command, policy and DMA registers beside the
//! interrupt event/status registers in one SVD peripheral. The generated
//! register split divides the block by word: the interrupt owner owns
//! `EVENT_STATUS` and observes the RX/TX status words and `ED_CONFIG`; the
//! task owner owns every other register. This module wraps the two owners
//! and reunites them before the complete block can be recovered.
//!
//! The interrupt handle's surface is limited to the source-confirmed ISR
//! snapshot fields and the generated affine W1C acknowledge.

use crate::generated::ieee802154_mac::{
    self as mac, Ieee802154Mac, Ieee802154MacInterrupt, Ieee802154MacTask,
};

/// A multipan context's register index.
fn multipan_index(index: usize) -> oer_mmio::Index<4> {
    oer_mmio::Index::new(index).expect("multipan index exceeds four contexts")
}

trait TransmitSecurityProgrammingPort {
    fn write_address_low(&mut self, word: u32);
    fn write_address_high(&mut self, word: u32);
    fn write_key_word(&mut self, index: usize, word: u32);
    fn write_payload_offset(&mut self, offset: u8);
    fn set_enabled(&mut self, enabled: bool);
}

trait MultipanIdentityProgrammingPort {
    fn enable_context(&mut self, index: usize);
    fn write_pan_id(&mut self, index: usize, pan_id: u16);
    fn write_short_address(&mut self, index: usize, short_address: u16);
    fn write_extended_address_low(&mut self, index: usize, word: u32);
    fn write_extended_address_high(&mut self, index: usize, word: u32);
}

fn execute_multipan_identity_configuration<Port: MultipanIdentityProgrammingPort>(
    port: &mut Port,
    index: usize,
    pan_id: u16,
    short_address: u16,
    extended_address: [u8; 8],
) {
    assert!(index < 4, "multipan index exceeds four contexts");
    port.enable_context(index);
    port.write_pan_id(index, pan_id);
    port.enable_context(index);
    port.write_short_address(index, short_address);
    port.enable_context(index);
    port.write_extended_address_low(
        index,
        u32::from_le_bytes([
            extended_address[0],
            extended_address[1],
            extended_address[2],
            extended_address[3],
        ]),
    );
    port.write_extended_address_high(
        index,
        u32::from_le_bytes([
            extended_address[4],
            extended_address[5],
            extended_address[6],
            extended_address[7],
        ]),
    );
}

fn execute_transmit_security_configuration<Port: TransmitSecurityProgrammingPort>(
    port: &mut Port,
    address: &[u8; 8],
    key: &[u8; 16],
    payload_offset: u8,
) {
    assert!(
        payload_offset <= 0x7f,
        "transmit-security payload offset exceeds seven bits"
    );
    port.write_address_low(u32::from_le_bytes([
        address[0], address[1], address[2], address[3],
    ]));
    port.write_address_high(u32::from_le_bytes([
        address[4], address[5], address[6], address[7],
    ]));
    for index in 0..4 {
        let start = index * 4;
        port.write_key_word(
            index,
            u32::from_le_bytes([key[start], key[start + 1], key[start + 2], key[start + 3]]),
        );
    }
    port.write_payload_offset(payload_offset);
    port.set_enabled(true);
}

fn execute_transmit_security_disable<Port: TransmitSecurityProgrammingPort>(port: &mut Port) {
    port.set_enabled(false);
}

/// Execute the closed source-order transmit-security transaction for the
/// restricted parent PAC.
#[doc(hidden)]
fn configure_transmit_security(
    registers: &mut Ieee802154MacTask,
    address: &[u8; 8],
    key: &[u8; 16],
    payload_offset: u8,
) {
    execute_transmit_security_configuration(registers, address, key, payload_offset);
}

/// Disable transmit security without claiming key/address zeroization.
#[doc(hidden)]
fn disable_transmit_security(registers: &mut Ieee802154MacTask) {
    execute_transmit_security_disable(registers);
}

/// Publish the one source-confirmed enhanced-ACK notification image.
#[doc(hidden)]
fn notify_enhanced_ack_generated(registers: &mut Ieee802154MacTask) {
    registers.notify_ieee802154_enhanced_ack_generated();
}

impl TransmitSecurityProgrammingPort for Ieee802154MacTask {
    fn write_address_low(&mut self, word: u32) {
        self.publish_ieee802154_security_address_low(word);
    }

    fn write_address_high(&mut self, word: u32) {
        self.publish_ieee802154_security_address_high(word);
    }

    fn write_key_word(&mut self, index: usize, word: u32) {
        let index = oer_mmio::Index::new(index).expect("security key index exceeds four words");
        self.publish_ieee802154_security_key_word(index, word);
    }

    fn write_payload_offset(&mut self, offset: u8) {
        let offset = oer_mmio::Bits::new(u32::from(offset))
            .expect("transmit-security payload offset exceeds seven bits");
        self.modify_security_control(|writer| {
            writer.set_payload_offset(offset);
        });
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.modify_security_control(|writer| {
            writer.set_tx_enable(enabled);
        });
    }
}

impl MultipanIdentityProgrammingPort for Ieee802154MacTask {
    fn enable_context(&mut self, index: usize) {
        let index = multipan_index(index);
        self.modify_control(|writer| {
            match index.get() {
                0 => writer.set_multipan0_enabled(true),
                1 => writer.set_multipan1_enabled(true),
                2 => writer.set_multipan2_enabled(true),
                _ => writer.set_multipan3_enabled(true),
            };
        });
    }

    fn write_pan_id(&mut self, index: usize, pan_id: u16) {
        self.modify_multipan_pan_id(multipan_index(index), |writer| {
            writer.set_pan_id(pan_id);
        });
    }

    fn write_short_address(&mut self, index: usize, short_address: u16) {
        self.modify_multipan_short_address(multipan_index(index), |writer| {
            writer.set_address(short_address);
        });
    }

    fn write_extended_address_low(&mut self, index: usize, word: u32) {
        self.modify_multipan_extended_address_low(multipan_index(index), |writer| {
            writer.set_address_word(word);
        });
    }

    fn write_extended_address_high(&mut self, index: usize, word: u32) {
        self.modify_multipan_extended_address_high(multipan_index(index), |writer| {
            writer.set_address_word(word);
        });
    }
}

/// Task-side IEEE 802.15.4 MAC register owner.
///
/// The restricted PAC above this raw crate exposes only reviewed command,
/// policy and DMA operations through this handle.
///
#[must_use = "the task owner must be reunited with its interrupt owner"]
pub struct TaskRegisters {
    registers: Ieee802154MacTask,
}

/// Primitive readback of one source-confirmed PAN context.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultipanIdentityReadback {
    pan_id: u16,
    short_address: u16,
    extended_address: [u8; 8],
}

impl MultipanIdentityReadback {
    pub const fn pan_id(self) -> u16 {
        self.pan_id
    }

    pub const fn short_address(self) -> u16 {
        self.short_address
    }

    pub const fn extended_address(self) -> [u8; 8] {
        self.extended_address
    }
}

/// Primitive task-side MAC policy readback.
///
/// This value contains field images only and cannot recover the raw register
/// block. It deliberately excludes interrupt status and abort sidebands.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaticMacPolicyReadback {
    frequency_code: u8,
    cca_mode: u8,
    cca_threshold_code: u8,
    ack_timeout: u16,
    auto_ack_tx: bool,
    auto_ack_rx: bool,
    enhanced_ack_tx: bool,
    coordinator: bool,
    promiscuous: bool,
    pending_enhanced: bool,
    multipan_enabled: [bool; 4],
    identity: MultipanIdentityReadback,
}

impl StaticMacPolicyReadback {
    pub const fn frequency_code(self) -> u8 {
        self.frequency_code
    }

    pub const fn cca_mode(self) -> u8 {
        self.cca_mode
    }

    pub const fn cca_threshold_code(self) -> u8 {
        self.cca_threshold_code
    }

    pub const fn ack_timeout(self) -> u16 {
        self.ack_timeout
    }

    pub const fn auto_ack_tx(self) -> bool {
        self.auto_ack_tx
    }

    pub const fn auto_ack_rx(self) -> bool {
        self.auto_ack_rx
    }

    pub const fn enhanced_ack_tx(self) -> bool {
        self.enhanced_ack_tx
    }

    pub const fn coordinator(self) -> bool {
        self.coordinator
    }

    pub const fn promiscuous(self) -> bool {
        self.promiscuous
    }

    pub const fn pending_enhanced(self) -> bool {
        self.pending_enhanced
    }

    pub const fn multipan_enabled(self) -> [bool; 4] {
        self.multipan_enabled
    }

    pub const fn identity(self) -> MultipanIdentityReadback {
        self.identity
    }
}

/// Primitive readback of every currently modeled task-side MAC configuration
/// field, including dynamic values that are not part of runtime policy.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacConfigurationReadback {
    frequency_code: u8,
    tx_power_code: u8,
    cca_mode: u8,
    cca_threshold_code: u8,
    ed_sample_rate: u8,
    ack_timeout: u16,
    auto_ack_tx: bool,
    auto_ack_rx: bool,
    enhanced_ack_tx: bool,
    coordinator: bool,
    promiscuous: bool,
    pending_enhanced: bool,
    multipan_enabled: [bool; 4],
    identities: [MultipanIdentityReadback; 4],
    frame_pending: bool,
}

impl MacConfigurationReadback {
    pub const fn frequency_code(self) -> u8 {
        self.frequency_code
    }

    pub const fn tx_power_code(self) -> u8 {
        self.tx_power_code
    }

    pub const fn cca_mode(self) -> u8 {
        self.cca_mode
    }

    pub const fn cca_threshold_code(self) -> u8 {
        self.cca_threshold_code
    }

    pub const fn ed_sample_rate(self) -> u8 {
        self.ed_sample_rate
    }

    pub const fn ack_timeout(self) -> u16 {
        self.ack_timeout
    }

    pub const fn auto_ack_tx(self) -> bool {
        self.auto_ack_tx
    }

    pub const fn auto_ack_rx(self) -> bool {
        self.auto_ack_rx
    }

    pub const fn enhanced_ack_tx(self) -> bool {
        self.enhanced_ack_tx
    }

    pub const fn coordinator(self) -> bool {
        self.coordinator
    }

    pub const fn promiscuous(self) -> bool {
        self.promiscuous
    }

    pub const fn pending_enhanced(self) -> bool {
        self.pending_enhanced
    }

    pub const fn multipan_enabled(self) -> [bool; 4] {
        self.multipan_enabled
    }

    pub const fn identity(self, index: usize) -> MultipanIdentityReadback {
        assert!(
            index < self.identities.len(),
            "multipan index exceeds four contexts"
        );
        self.identities[index]
    }

    pub const fn frame_pending(self) -> bool {
        self.frame_pending
    }
}

/// Primitive readback of the interrupt-masked task-side foundation.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FoundationReadback {
    events_masked: bool,
    rx_aborts_masked: bool,
    tx_aborts_masked: bool,
    ed_uses_average: bool,
    txrx_pti: u8,
    ack_pti: u8,
    txon_delay_applied: bool,
}

impl FoundationReadback {
    pub const fn txon_delay_applied(self) -> bool {
        self.txon_delay_applied
    }

    pub const fn events_masked(self) -> bool {
        self.events_masked
    }

    pub const fn rx_aborts_masked(self) -> bool {
        self.rx_aborts_masked
    }

    pub const fn tx_aborts_masked(self) -> bool {
        self.tx_aborts_masked
    }

    pub const fn ed_uses_average(self) -> bool {
        self.ed_uses_average
    }

    pub const fn txrx_pti(self) -> u8 {
        self.txrx_pti
    }

    pub const fn ack_pti(self) -> u8 {
        self.ack_pti
    }
}

/// Closed classification of `EVENT_ENABLE` for one finite ED/CCA operation.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationEventEnableReadback {
    AllMasked,
    EdOperation,
    Unexpected,
}

/// Whether no receive-abort reason is enabled.
fn rx_aborts_all_masked(r: &mac::rx_abort_enable::R) -> bool {
    !(r.rx_stop()
        || r.sfd_timeout()
        || r.crc_error()
        || r.invalid_length()
        || r.filter_fail()
        || r.no_rss()
        || r.coexistence_break()
        || r.unexpected_ack()
        || r.rx_restart()
        || r.rx_abort_en_9_14() != 0
        || r.tx_ack_timeout()
        || r.tx_ack_stop()
        || r.tx_ack_coexistence_break()
        || r.enhanced_ack_security_error()
        || r.rx_abort_en_19_22() != 0
        || r.ed_abort()
        || r.ed_stop()
        || r.ed_coexistence_reject()
        || r.rx_abort_en_26_30() != 0)
}

/// Whether exactly the finite ED/CCA receive-abort reasons are enabled.
fn rx_aborts_ed_operation_only(r: &mac::rx_abort_enable::R) -> bool {
    r.ed_abort()
        && r.ed_stop()
        && r.ed_coexistence_reject()
        && !(r.rx_stop()
            || r.sfd_timeout()
            || r.crc_error()
            || r.invalid_length()
            || r.filter_fail()
            || r.no_rss()
            || r.coexistence_break()
            || r.unexpected_ack()
            || r.rx_restart()
            || r.rx_abort_en_9_14() != 0
            || r.tx_ack_timeout()
            || r.tx_ack_stop()
            || r.tx_ack_coexistence_break()
            || r.enhanced_ack_security_error()
            || r.rx_abort_en_19_22() != 0
            || r.rx_abort_en_26_30() != 0)
}

/// Whether no transmit-abort reason is enabled.
fn tx_aborts_all_masked(r: &mac::tx_abort_enable::R) -> bool {
    !(r.rx_ack_stop()
        || r.rx_ack_sfd_timeout()
        || r.rx_ack_crc_error()
        || r.rx_ack_invalid_length()
        || r.rx_ack_filter_fail()
        || r.rx_ack_no_rss()
        || r.rx_ack_coexistence_break()
        || r.rx_ack_type_not_ack()
        || r.rx_ack_restart()
        || r.tx_abort_en_9_14() != 0
        || r.rx_ack_timeout()
        || r.tx_stop()
        || r.tx_coexistence_break()
        || r.tx_security_error()
        || r.tx_abort_en_19_22() != 0
        || r.cca_failed()
        || r.cca_busy()
        || r.tx_abort_en_25_30() != 0)
}

/// Closed classification of `RX_ABORT_ENABLE` for one finite ED/CCA operation.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationRxAbortEnableReadback {
    AllMasked,
    EdOperationReasons,
    Unexpected,
}

/// Semantic observation shared by `EVENT_ENABLE`, `EVENT_STATUS`, and the
/// generated affine W1C snapshot.
///
/// Register positions never cross this type: every value is obtained through
/// a generated field reader, and only named event predicates are public.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventReadback {
    tx_done: bool,
    rx_done: bool,
    ack_tx_done: bool,
    ack_rx_done: bool,
    rx_abort: bool,
    tx_abort: bool,
    ed_done: bool,
    unclassified_7: bool,
    timer0_overflow: bool,
    timer1_overflow: bool,
    clock_count_match: bool,
    tx_sfd_done: bool,
    rx_sfd_done: bool,
}

impl Ieee802154EventReadback {
    pub(crate) fn from_event_enable(reader: &mac::event_enable::R) -> Self {
        Self {
            tx_done: reader.tx_done(),
            rx_done: reader.rx_done(),
            ack_tx_done: reader.ack_tx_done(),
            ack_rx_done: reader.ack_rx_done(),
            rx_abort: reader.rx_abort(),
            tx_abort: reader.tx_abort(),
            ed_done: reader.ed_done(),
            unclassified_7: reader.events_7(),
            timer0_overflow: reader.timer0_overflow(),
            timer1_overflow: reader.timer1_overflow(),
            clock_count_match: reader.clock_count_match(),
            tx_sfd_done: reader.tx_sfd_done(),
            rx_sfd_done: reader.rx_sfd_done(),
        }
    }

    pub(crate) fn from_event_status(reader: &mac::event_status::R) -> Self {
        Self {
            tx_done: reader.tx_done(),
            rx_done: reader.rx_done(),
            ack_tx_done: reader.ack_tx_done(),
            ack_rx_done: reader.ack_rx_done(),
            rx_abort: reader.rx_abort(),
            tx_abort: reader.tx_abort(),
            ed_done: reader.ed_done(),
            unclassified_7: reader.events_7(),
            timer0_overflow: reader.timer0_overflow(),
            timer1_overflow: reader.timer1_overflow(),
            clock_count_match: reader.clock_count_match(),
            tx_sfd_done: reader.tx_sfd_done(),
            rx_sfd_done: reader.rx_sfd_done(),
        }
    }

    pub fn from_event_status_snapshot(snapshot: &mac::event_status::Snapshot) -> Self {
        Self {
            tx_done: snapshot.tx_done(),
            rx_done: snapshot.rx_done(),
            ack_tx_done: snapshot.ack_tx_done(),
            ack_rx_done: snapshot.ack_rx_done(),
            rx_abort: snapshot.rx_abort(),
            tx_abort: snapshot.tx_abort(),
            ed_done: snapshot.ed_done(),
            unclassified_7: snapshot.events_7(),
            timer0_overflow: snapshot.timer0_overflow(),
            timer1_overflow: snapshot.timer1_overflow(),
            clock_count_match: snapshot.clock_count_match(),
            tx_sfd_done: snapshot.tx_sfd_done(),
            rx_sfd_done: snapshot.rx_sfd_done(),
        }
    }

    pub const fn tx_done(self) -> bool {
        self.tx_done
    }

    pub const fn rx_done(self) -> bool {
        self.rx_done
    }

    pub const fn ack_tx_done(self) -> bool {
        self.ack_tx_done
    }

    pub const fn ack_rx_done(self) -> bool {
        self.ack_rx_done
    }

    pub const fn rx_abort(self) -> bool {
        self.rx_abort
    }

    pub const fn tx_abort(self) -> bool {
        self.tx_abort
    }

    pub const fn ed_done(self) -> bool {
        self.ed_done
    }

    pub const fn timer0_overflow(self) -> bool {
        self.timer0_overflow
    }

    pub const fn timer1_overflow(self) -> bool {
        self.timer1_overflow
    }

    pub const fn clock_count_match(self) -> bool {
        self.clock_count_match
    }

    pub const fn tx_sfd_done(self) -> bool {
        self.tx_sfd_done
    }

    pub const fn rx_sfd_done(self) -> bool {
        self.rx_sfd_done
    }

    pub const fn has_unclassified(self) -> bool {
        self.unclassified_7
    }

    pub const fn is_clear(self) -> bool {
        !self.tx_done
            && !self.rx_done
            && !self.ack_tx_done
            && !self.ack_rx_done
            && !self.rx_abort
            && !self.tx_abort
            && !self.ed_done
            && !self.unclassified_7
            && !self.timer0_overflow
            && !self.timer1_overflow
            && !self.clock_count_match
            && !self.tx_sfd_done
            && !self.rx_sfd_done
    }

    const fn is_ed_operation(self) -> bool {
        self.rx_abort
            && self.ed_done
            && !self.tx_done
            && !self.rx_done
            && !self.ack_tx_done
            && !self.ack_rx_done
            && !self.tx_abort
            && !self.unclassified_7
            && !self.timer0_overflow
            && !self.timer1_overflow
            && !self.clock_count_match
            && !self.tx_sfd_done
            && !self.rx_sfd_done
    }
}

/// Non-secret readable transmit-security control fields.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransmitSecurityControlReadback {
    enabled: bool,
    payload_offset: u8,
}

impl TransmitSecurityControlReadback {
    pub const fn enabled(self) -> bool {
        self.enabled
    }

    pub const fn payload_offset(self) -> u8 {
        self.payload_offset
    }
}

impl TaskRegisters {
    /// Mask every MAC event through the generated closed field variant.
    #[doc(hidden)]
    pub fn mask_all_events(&mut self) {
        self.registers.modify_event_enable(|writer| {
            writer.set_tx_done(false);
            writer.set_rx_done(false);
            writer.set_ack_tx_done(false);
            writer.set_ack_rx_done(false);
            writer.set_rx_abort(false);
            writer.set_tx_abort(false);
            writer.set_ed_done(false);
            writer.set_events_7(false);
            writer.set_timer0_overflow(false);
            writer.set_timer1_overflow(false);
            writer.set_clock_count_match(false);
            writer.set_tx_sfd_done(false);
            writer.set_rx_sfd_done(false);
        });
    }

    /// Enable the finite ED/CCA operation event set.
    #[doc(hidden)]
    pub fn enable_ed_operation_events(&mut self) {
        self.registers.modify_event_enable(|writer| {
            writer.set_tx_done(false);
            writer.set_rx_done(false);
            writer.set_ack_tx_done(false);
            writer.set_ack_rx_done(false);
            writer.set_rx_abort(true);
            writer.set_tx_abort(false);
            writer.set_ed_done(true);
            writer.set_events_7(false);
            writer.set_timer0_overflow(false);
            writer.set_timer1_overflow(false);
            writer.set_clock_count_match(false);
            writer.set_tx_sfd_done(false);
            writer.set_rx_sfd_done(false);
        });
    }

    /// Enable the reviewed runtime event baseline without TIMER0.
    #[doc(hidden)]
    pub fn enable_runtime_events_without_timer0(&mut self) {
        self.registers.modify_event_enable(|writer| {
            writer.set_tx_done(true);
            writer.set_rx_done(true);
            writer.set_ack_tx_done(true);
            writer.set_ack_rx_done(true);
            writer.set_rx_abort(true);
            writer.set_tx_abort(true);
            writer.set_ed_done(true);
            writer.set_events_7(false);
            writer.set_timer0_overflow(false);
            writer.set_timer1_overflow(true);
            writer.set_clock_count_match(false);
            writer.set_tx_sfd_done(true);
            writer.set_rx_sfd_done(true);
        });
    }

    /// Mask every receive-abort reason through the generated field variant.
    #[doc(hidden)]
    pub fn mask_all_rx_aborts(&mut self) {
        self.registers.disable_ieee802154_rx_abort_all();
    }

    /// Enable exactly the receive-abort reasons owned by finite ED/CCA.
    #[doc(hidden)]
    pub fn enable_ed_operation_rx_abort_reasons(&mut self) {
        self.registers.replace_ieee802154_rx_abort_ed_operation();
    }

    /// Enable the reviewed runtime receive-abort baseline.
    #[doc(hidden)]
    pub fn enable_runtime_rx_aborts(&mut self) {
        self.registers.replace_ieee802154_rx_abort_baseline();
    }

    /// Mask every transmit-abort reason through the generated field variant.
    #[doc(hidden)]
    pub fn mask_all_tx_aborts(&mut self) {
        self.registers.disable_ieee802154_tx_abort_all();
    }

    /// Enable the reviewed runtime transmit-abort baseline.
    #[doc(hidden)]
    pub fn enable_runtime_tx_aborts(&mut self) {
        self.registers.replace_ieee802154_tx_abort_baseline();
    }

    /// Select the source-confirmed average ED sample mode.
    #[doc(hidden)]
    pub fn select_average_ed_sampling(&mut self) {
        self.registers.modify_ed_config(|writer| {
            writer.set_ed_sample_mode(mac::ed_config::EdSampleMode::Average);
        });
    }

    /// Read the complete event-delivery state through generated field readers.
    #[doc(hidden)]
    pub fn event_enable_readback(&self) -> Ieee802154EventReadback {
        Ieee802154EventReadback::from_event_enable(&self.registers.read_event_enable())
    }

    /// Classify `EVENT_ENABLE` without exporting its physical field image.
    #[doc(hidden)]
    pub fn operation_event_enable_readback(&self) -> OperationEventEnableReadback {
        let events = self.event_enable_readback();
        if events.is_clear() {
            OperationEventEnableReadback::AllMasked
        } else if events.is_ed_operation() {
            OperationEventEnableReadback::EdOperation
        } else {
            OperationEventEnableReadback::Unexpected
        }
    }

    /// Classify `RX_ABORT_ENABLE` without exporting its physical field image.
    #[doc(hidden)]
    pub fn operation_rx_abort_enable_readback(&self) -> OperationRxAbortEnableReadback {
        let events = self.registers.read_rx_abort_enable();
        if rx_aborts_all_masked(&events) {
            OperationRxAbortEnableReadback::AllMasked
        } else if rx_aborts_ed_operation_only(&events) {
            OperationRxAbortEnableReadback::EdOperationReasons
        } else {
            OperationRxAbortEnableReadback::Unexpected
        }
    }

    /// Program the public LL's sixteen-bit subset of the wider ED-duration
    /// register while preserving the unowned high byte.
    #[doc(hidden)]
    pub fn set_ed_duration(&mut self, duration: crate::generated::Ieee802154EdDurationUnits) {
        self.registers.set_ieee802154_ed_duration(duration);
    }

    /// Read the recovered low twenty-four-bit ED duration.
    #[doc(hidden)]
    pub fn ed_duration(&self) -> u32 {
        self.registers.read_ed_duration().duration()
    }

    /// Publish one complete TX DMA address word.
    #[doc(hidden)]
    pub fn publish_transmit_dma_address(&mut self, address: crate::layouts::DmaWindowAddress) {
        self.registers.publish_ieee802154_tx_dma_address(address);
    }

    /// Publish one complete RX DMA address word.
    #[doc(hidden)]
    pub fn publish_receive_dma_address(&mut self, address: crate::layouts::DmaWindowAddress) {
        self.registers.publish_ieee802154_rx_dma_address(address);
    }

    /// Issue the fixed TX-start image.
    #[doc(hidden)]
    pub fn issue_transmit(&mut self) {
        self.registers.issue_ieee802154_tx_start();
    }

    /// Issue the fixed RX-start image.
    #[doc(hidden)]
    pub fn issue_receive(&mut self) {
        self.registers.issue_ieee802154_rx_start();
    }

    /// Issue the fixed CCA-then-TX image.
    #[doc(hidden)]
    pub fn issue_clear_channel_then_transmit(&mut self) {
        self.registers.issue_ieee802154_cca_tx_start();
    }

    /// Issue the fixed ED-start image.
    #[doc(hidden)]
    pub fn issue_energy_detection(&mut self) {
        self.registers.issue_ieee802154_ed_start();
    }

    /// Issue the fixed state-specific STOP image.
    #[doc(hidden)]
    pub fn issue_stop(&mut self) {
        self.registers.issue_ieee802154_stop();
    }

    /// Publish one complete TIMER0 threshold word.
    #[doc(hidden)]
    pub fn publish_timer0_threshold(
        &mut self,
        threshold: crate::generated::Ieee802154Timer0ThresholdWord,
    ) {
        self.registers
            .publish_ieee802154_timer0_threshold(threshold);
    }

    /// Observe one complete TIMER0 counter word.
    #[doc(hidden)]
    pub fn observe_timer0_value(&self) -> crate::generated::Ieee802154Timer0ValueWord {
        self.registers.observe_ieee802154_timer0_value()
    }

    /// Issue the fixed TIMER0-start image.
    #[doc(hidden)]
    pub fn issue_timer0_start(&mut self) {
        self.registers.issue_ieee802154_timer0_start();
    }

    /// Issue the fixed TIMER0-stop image.
    #[doc(hidden)]
    pub fn issue_timer0_stop(&mut self) {
        self.registers.issue_ieee802154_timer0_stop();
    }

    /// Publish one complete TIMER1 threshold word.
    #[doc(hidden)]
    pub fn publish_timer1_threshold(
        &mut self,
        threshold: crate::generated::Ieee802154Timer1ThresholdWord,
    ) {
        self.registers
            .publish_ieee802154_timer1_threshold(threshold);
    }

    /// Observe one complete TIMER1 counter word.
    #[doc(hidden)]
    pub fn observe_timer1_value(&self) -> crate::generated::Ieee802154Timer1ValueWord {
        self.registers.observe_ieee802154_timer1_value()
    }

    /// Issue the fixed TIMER1-start image.
    #[doc(hidden)]
    pub fn issue_timer1_start(&mut self) {
        self.registers.issue_ieee802154_timer1_start();
    }

    /// Issue the fixed TIMER1-stop image.
    #[doc(hidden)]
    pub fn issue_timer1_stop(&mut self) {
        self.registers.issue_ieee802154_timer1_stop();
    }

    /// Replace only the seven-bit channel/frequency field.
    #[doc(hidden)]
    pub fn set_frequency_code(&mut self, code: u8) {
        let code = oer_mmio::Bits::new(u32::from(code)).expect("frequency code exceeds seven bits");
        self.registers.modify_channel(|writer| {
            writer.set_frequency_code(code);
        });
    }

    /// Replace only the five-bit raw TX-power field.
    #[doc(hidden)]
    pub fn set_tx_power_code(&mut self, code: crate::generated::Ieee802154TxPowerCode) {
        self.registers.set_ieee802154_tx_power_code(code);
    }

    /// Replace the two-bit CCA-mode field.
    #[doc(hidden)]
    pub fn set_cca_mode(&mut self, mode: u8) {
        use mac::ed_config::CcaMode;
        let mode = match mode {
            0 => CcaMode::Carrier,
            1 => CcaMode::EnergyDetection,
            2 => CcaMode::CarrierOrEnergyDetection,
            3 => CcaMode::CarrierAndEnergyDetection,
            _ => panic!("CCA mode exceeds two bits"),
        };
        self.registers.modify_ed_config(|writer| {
            writer.set_cca_mode(mode);
        });
    }

    /// Replace the signed eight-bit CCA threshold field.
    #[doc(hidden)]
    pub fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.registers.modify_ed_config(|writer| {
            writer.set_cca_threshold_code(threshold as u8);
        });
    }

    /// Replace the two-bit ED sample-rate field.
    #[doc(hidden)]
    pub fn set_ed_sample_rate(&mut self, rate: u8) {
        use mac::ed_config::EdSampleRate;
        let rate = match rate {
            0 => EdSampleRate::OnePerUs,
            1 => EdSampleRate::TwoPerUs,
            2 => EdSampleRate::FourPerUs,
            3 => EdSampleRate::EightPerUs,
            _ => panic!("ED sample rate exceeds two bits"),
        };
        self.registers.modify_ed_config(|writer| {
            writer.set_ed_sample_rate(rate);
        });
    }

    /// Replace the complete sixteen-bit ACK-timeout field.
    #[doc(hidden)]
    pub fn set_ack_timeout(&mut self, timeout: u16) {
        self.registers.modify_ack_timeout(|writer| {
            writer.set_timeout(timeout);
        });
    }

    /// Apply all six public PIB control bits in source order.
    #[doc(hidden)]
    pub fn set_mac_control(
        &mut self,
        auto_ack_tx: bool,
        auto_ack_rx: bool,
        enhanced_ack_tx: bool,
        coordinator: bool,
        promiscuous: bool,
        pending_enhanced: bool,
    ) {
        self.set_auto_ack_tx(auto_ack_tx);
        self.set_auto_ack_rx(auto_ack_rx);
        self.set_enhanced_ack_tx(enhanced_ack_tx);
        self.set_coordinator(coordinator);
        self.set_promiscuous(promiscuous);
        self.set_pending_enhanced(pending_enhanced);
    }

    /// Replace all four named multipan enable fields.
    #[doc(hidden)]
    pub fn set_multipan_enabled(&mut self, enabled: [bool; 4]) {
        let [first, second, third, fourth] = enabled;
        self.registers.modify_control(|writer| {
            writer
                .set_multipan0_enabled(first)
                .set_multipan1_enabled(second)
                .set_multipan2_enabled(third)
                .set_multipan3_enabled(fourth);
        });
    }

    /// Program one complete source-confirmed multipan identity transaction.
    #[doc(hidden)]
    pub fn set_multipan_identity(
        &mut self,
        index: usize,
        pan_id: u16,
        short_address: u16,
        extended_address: [u8; 8],
    ) {
        execute_multipan_identity_configuration(
            &mut self.registers,
            index,
            pan_id,
            short_address,
            extended_address,
        );
    }

    #[doc(hidden)]
    pub fn multipan_identity(&self, index: usize) -> MultipanIdentityReadback {
        let index = multipan_index(index);
        let pan_id = self.registers.read_multipan_pan_id(index);
        let short_address = self.registers.read_multipan_short_address(index);
        let extended_low = self.registers.read_multipan_extended_address_low(index);
        let extended_high = self.registers.read_multipan_extended_address_high(index);
        let low = extended_low.address_word().to_le_bytes();
        let high = extended_high.address_word().to_le_bytes();
        MultipanIdentityReadback {
            pan_id: pan_id.pan_id(),
            short_address: short_address.address(),
            extended_address: [
                low[0], low[1], low[2], low[3], high[0], high[1], high[2], high[3],
            ],
        }
    }

    /// Read all four named multipan enable fields.
    #[doc(hidden)]
    pub fn multipan_enabled(&self) -> [bool; 4] {
        let control = self.registers.read_control();
        [
            control.multipan0_enabled(),
            control.multipan1_enabled(),
            control.multipan2_enabled(),
            control.multipan3_enabled(),
        ]
    }

    /// Set the outgoing ACK frame-pending bit through a preserving update.
    #[doc(hidden)]
    pub fn set_frame_pending(&mut self, pending: bool) {
        self.registers.modify_pending_config(|writer| {
            writer.set_frame_pending(pending);
        });
    }

    /// Read the outgoing ACK frame-pending bit.
    #[doc(hidden)]
    pub fn frame_pending(&self) -> bool {
        self.registers.read_pending_config().frame_pending()
    }

    /// Publish the sole source-confirmed enhanced-ACK notification image.
    #[doc(hidden)]
    pub fn notify_enhanced_ack_generated(&mut self) {
        notify_enhanced_ack_generated(&mut self.registers);
    }

    /// Configure transmit security in exact source order.
    #[doc(hidden)]
    pub fn configure_transmit_security(
        &mut self,
        address: &[u8; 8],
        key: &[u8; 16],
        payload_offset: u8,
    ) {
        configure_transmit_security(&mut self.registers, address, key, payload_offset);
    }

    /// Disable transmit security without claiming register zeroization.
    #[doc(hidden)]
    pub fn disable_transmit_security(&mut self) {
        disable_transmit_security(&mut self.registers);
    }

    /// Read only the non-secret transmit-security control fields.
    #[doc(hidden)]
    pub fn transmit_security_control(&self) -> TransmitSecurityControlReadback {
        let control = self.registers.read_security_control();
        TransmitSecurityControlReadback {
            enabled: control.tx_enable(),
            payload_offset: control.payload_offset(),
        }
    }

    /// Replace only the generated four-bit TX/RX coexistence PTI field.
    #[doc(hidden)]
    pub fn set_txrx_pti(&mut self, pti: u8) {
        let pti = oer_mmio::Bits::new(u32::from(pti)).expect("TX/RX PTI exceeds four bits");
        self.registers.modify_coex_pti(|writer| {
            writer.set_txrx_pti(pti);
        });
    }

    /// Replace only the generated four-bit ACK coexistence PTI field.
    #[doc(hidden)]
    pub fn set_ack_pti(&mut self, pti: u8) {
        let pti = oer_mmio::Bits::new(u32::from(pti)).expect("ACK PTI exceeds four bits");
        self.registers.modify_coex_pti(|writer| {
            writer.set_ack_pti(pti);
        });
    }

    /// Read the complete interrupt-masked task-side foundation.
    #[doc(hidden)]
    pub fn foundation_readback(&self) -> FoundationReadback {
        let event_enable = self.registers.read_event_enable();
        let rx_abort_enable = self.registers.read_rx_abort_enable();
        let tx_abort_enable = self.registers.read_tx_abort_enable();
        let ed_config = self.registers.read_ed_config();
        let coex_pti = self.registers.read_coex_pti();
        let tx_on_delay = self.registers.read_txon_delay();
        let tx_off_delay = self.registers.read_txoff_delay();
        let rx_on_delay = self.registers.read_rxon_delay();
        let txrx_switch_delay = self.registers.read_txrx_switch_delay();
        FoundationReadback {
            events_masked: Ieee802154EventReadback::from_event_enable(&event_enable).is_clear(),
            rx_aborts_masked: rx_aborts_all_masked(&rx_abort_enable),
            tx_aborts_masked: tx_aborts_all_masked(&tx_abort_enable),
            ed_uses_average: ed_config.ed_sample_mode_variant()
                == mac::ed_config::EdSampleMode::Average,
            txrx_pti: coex_pti.txrx_pti(),
            ack_pti: coex_pti.ack_pti(),
            txon_delay_applied: u32::from(tx_on_delay.txon_delay())
                == crate::generated::Ieee802154TxOnDelay::Delay40.bits()
                && u32::from(tx_off_delay.txoff_delay())
                    == crate::generated::Ieee802154TxOffDelay::Delay0.bits()
                && u32::from(rx_on_delay.rxon_delay())
                    == crate::generated::Ieee802154RxOnDelay::Delay50.bits()
                && u32::from(txrx_switch_delay.txrx_switch_delay())
                    == crate::generated::Ieee802154TxRxSwitchDelay::Delay122.bits(),
        }
    }

    /// Read the complete task-owned static policy subset once per word.
    #[doc(hidden)]
    pub fn static_mac_policy_readback(&self) -> StaticMacPolicyReadback {
        let channel = self.registers.read_channel();
        let ed_config = self.registers.read_ed_config();
        let ack_timeout = self.registers.read_ack_timeout();
        let control = self.registers.read_control();
        StaticMacPolicyReadback {
            frequency_code: channel.frequency_code(),
            cca_mode: ed_config.cca_mode(),
            cca_threshold_code: ed_config.cca_threshold_code(),
            ack_timeout: ack_timeout.timeout(),
            auto_ack_tx: control.auto_ack_tx(),
            auto_ack_rx: control.auto_ack_rx(),
            enhanced_ack_tx: control.enhanced_ack_tx(),
            coordinator: control.coordinator(),
            promiscuous: control.promiscuous(),
            pending_enhanced: control.pending_enhanced(),
            multipan_enabled: [
                control.multipan0_enabled(),
                control.multipan1_enabled(),
                control.multipan2_enabled(),
                control.multipan3_enabled(),
            ],
            identity: self.multipan_identity(0),
        }
    }

    /// Read every currently modeled task-side configuration field once per
    /// backing word. This includes dynamic values absent from static policy.
    #[doc(hidden)]
    pub fn mac_configuration_readback(&self) -> MacConfigurationReadback {
        let channel = self.registers.read_channel();
        let tx_power = self.registers.read_tx_power();
        let ed_config = self.registers.read_ed_config();
        let ack_timeout = self.registers.read_ack_timeout();
        let control = self.registers.read_control();
        let pending_config = self.registers.read_pending_config();
        MacConfigurationReadback {
            frequency_code: channel.frequency_code(),
            tx_power_code: tx_power.power_code(),
            cca_mode: ed_config.cca_mode(),
            cca_threshold_code: ed_config.cca_threshold_code(),
            ed_sample_rate: ed_config.ed_sample_rate(),
            ack_timeout: ack_timeout.timeout(),
            auto_ack_tx: control.auto_ack_tx(),
            auto_ack_rx: control.auto_ack_rx(),
            enhanced_ack_tx: control.enhanced_ack_tx(),
            coordinator: control.coordinator(),
            promiscuous: control.promiscuous(),
            pending_enhanced: control.pending_enhanced(),
            multipan_enabled: [
                control.multipan0_enabled(),
                control.multipan1_enabled(),
                control.multipan2_enabled(),
                control.multipan3_enabled(),
            ],
            identities: [
                self.multipan_identity(0),
                self.multipan_identity(1),
                self.multipan_identity(2),
                self.multipan_identity(3),
            ],
            frame_pending: pending_config.frame_pending(),
        }
    }

    /// `ieee802154_txon_delay_set` of the ESP32-C5 libbtbb: the MAC
    /// transmit-on, turnaround, receive-on and transmit-off delays, in the
    /// vendor order.
    #[doc(hidden)]
    pub fn set_txon_delay(&mut self) {
        self.registers
            .set_ieee802154_tx_on_delay(crate::generated::Ieee802154TxOnDelay::Delay40);
        self.registers.set_ieee802154_txrx_switch_delay(
            crate::generated::Ieee802154TxRxSwitchDelay::Delay122,
        );
        self.registers
            .set_ieee802154_rx_on_delay(crate::generated::Ieee802154RxOnDelay::Delay50);
        self.registers
            .set_ieee802154_tx_off_delay(crate::generated::Ieee802154TxOffDelay::Delay0);
    }
}

/// Inactive or ISR-owned IEEE 802.15.4 event/status capability.
///
/// There is no conversion to the task owner and no raw register accessor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptRxStatusReadback {
    abort_reason_code: u8,
}

impl InterruptRxStatusReadback {
    pub const fn abort_reason_code(&self) -> u8 {
        self.abort_reason_code
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptTxStatusReadback {
    abort_reason_code: u8,
}

impl InterruptTxStatusReadback {
    pub const fn abort_reason_code(&self) -> u8 {
        self.abort_reason_code
    }
}

#[must_use = "the interrupt owner must be deactivated and reunited"]
pub struct InterruptRegisters {
    registers: Ieee802154MacInterrupt,
}

impl InterruptRegisters {
    /// Sample the complete thirteen-bit event field exactly once.
    #[inline]
    pub fn sample_event_status(&self) -> mac::event_status::Snapshot {
        self.registers.sample_event_status()
    }

    /// Acknowledge exactly one previously sampled event field and consume it.
    #[inline]
    pub fn acknowledge_event_status(&mut self, snapshot: mac::event_status::Snapshot) {
        self.registers.acknowledge_event_status(snapshot);
    }

    /// `ieee802154_ll_get_events`: one read of the event field without
    /// acknowledging it.
    #[inline]
    pub fn event_readback(&self) -> Ieee802154EventReadback {
        Ieee802154EventReadback::from_event_status(&self.registers.read_event_status())
    }

    /// `ieee802154_ll_clear_events`: `EVENT_STATUS &= mask`. The
    /// write-one-to-clear word is sampled once and only the asserted events
    /// that `in_mask` selects are acknowledged, so events outside the mask
    /// and events raised after the sample stay latched.
    #[inline]
    pub fn clear_events(&mut self, in_mask: impl Fn(RawEvent) -> bool) {
        let snapshot = self
            .registers
            .sample_event_status()
            .retain_tx_done(in_mask(RawEvent::TxDone))
            .retain_rx_done(in_mask(RawEvent::RxDone))
            .retain_ack_tx_done(in_mask(RawEvent::AckTxDone))
            .retain_ack_rx_done(in_mask(RawEvent::AckRxDone))
            .retain_rx_abort(in_mask(RawEvent::RxAbort))
            .retain_tx_abort(in_mask(RawEvent::TxAbort))
            .retain_ed_done(in_mask(RawEvent::EdDone))
            .retain_events_7(false)
            .retain_timer0_overflow(in_mask(RawEvent::Timer0Overflow))
            .retain_timer1_overflow(in_mask(RawEvent::Timer1Overflow))
            .retain_clock_count_match(in_mask(RawEvent::ClockCountMatch))
            .retain_tx_sfd_done(in_mask(RawEvent::TxSfdDone))
            .retain_rx_sfd_done(in_mask(RawEvent::RxSfdDone));
        self.registers.acknowledge_event_status(snapshot);
    }

    /// Observe the complete RX status word captured for an RX-abort event.
    #[inline]
    pub fn rx_status_readback(&self) -> InterruptRxStatusReadback {
        let status = self.registers.read_rx_status();
        InterruptRxStatusReadback {
            abort_reason_code: status.abort_reason(),
        }
    }

    /// Observe the complete TX status word captured for a TX-abort event.
    #[inline]
    pub fn tx_status_readback(&self) -> InterruptTxStatusReadback {
        let status = self.registers.read_tx_status();
        InterruptTxStatusReadback {
            abort_reason_code: status.abort_reason(),
        }
    }

    /// Observe the signed energy-detection result captured for ED-DONE.
    #[inline]
    pub fn ed_rss_code(&self) -> i8 {
        self.registers.read_ed_config().ed_rss_code() as i8
    }

    /// Observe the CCA result captured for ED-DONE.
    #[inline]
    pub fn cca_busy(&self) -> bool {
        self.registers.read_ed_config().cca_busy()
    }

    /// Observe only the generated RX/TX state subfields from the IRQ-owned
    /// sideband words.
    #[doc(hidden)]
    pub fn state_codes(&self) -> (u8, u8) {
        (
            self.registers.read_rx_status().state(),
            self.registers.read_tx_status().state(),
        )
    }
}

/// Split one unique MAC owner into its generated task and interrupt owners,
/// which own disjoint words.
#[inline]
pub fn split(registers: Ieee802154Mac) -> (TaskRegisters, InterruptRegisters) {
    let (task, interrupt) = registers.split();
    (
        TaskRegisters { registers: task },
        InterruptRegisters {
            registers: interrupt,
        },
    )
}

/// Consume both roles and recover the unique complete MAC owner.
#[inline]
pub fn reunite(task: TaskRegisters, interrupt: InterruptRegisters) -> Ieee802154Mac {
    Ieee802154Mac::join(task.registers, interrupt.registers)
}

mod single_field;

pub(crate) use single_field::{RawDebugCounter, RawEvent};

#[cfg(test)]
mod tests;
