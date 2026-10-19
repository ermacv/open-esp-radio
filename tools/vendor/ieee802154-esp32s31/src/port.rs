//! Runs a [`Scenario`] against the production engine
//! (`oer_espressif_ieee802154_engine::engine`) and records the same boundary
//! vocabulary as the compiled vendor driver.
//!
//! The adapter below is the only place that knows the vendor encodings: it
//! renders each HAL low-level call as the `ieee802154_ll_*` accessor and
//! argument the vendor passes, and answers getters from the shared
//! [`LlModel`], so both sides see the same inputs. Modem ETM accesses are
//! rendered as the vendor's direct register reads and writes.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

#[cfg(feature = "sw-coex")]
use oer_esp32s31_knowledge::ieee802154_coex::{Ieee802154CoexConfig, resolve_priorities};
#[cfg(feature = "sw-coex")]
use oer_espressif_coex::{CoexPtiTable, Ieee802154CoexLevel};
#[cfg(feature = "sw-coex")]
use oer_espressif_ieee802154_engine::coex::Ieee802154Coexistence;
use oer_espressif_ieee802154_engine::engine::{
    FRAME_SIZE, Ieee802154Engine, Ieee802154EngineBuffers, Ieee802154EnhancedAck,
    Ieee802154Environment, Ieee802154FrameInfo, Ieee802154ReceivedAck, Ieee802154RxSlot,
    Ieee802154TxError,
};
use oer_espressif_ieee802154_engine::pib::{
    AutoPendingMode, Ieee802154MultipanIndex, Ieee802154PibDefaults,
};
use oer_espressif_ieee802154_engine::{
    channel::Ieee802154Channel,
    coex::CoexPti,
    ll::{Ieee802154LlCommand, Ieee802154LowLevel, Ieee802154Timer},
    tx_power::Ieee802154ResolvedTxPower,
    types::{
        Ieee802154CcaMode, Ieee802154DebugCounter, Ieee802154EdSampleMode, Ieee802154EtmChannel,
        Ieee802154EtmRoute, Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservation,
        Ieee802154MultipanEnableState, Ieee802154RxAbortEnableSet, Ieee802154RxAbortReason,
        Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode, Ieee802154RxStatus,
        Ieee802154TxAbortEnableSet, Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation,
    },
};
use oer_ieee802154_mac::FrameAddress;

use crate::{
    record::{Argument, LlModel, Record},
    scenario::{Scenario, Step},
};

/// `IEEE802154_EVENT_*` bit of each event (`ieee802154_common_ll.h`).
const EVENT_BITS: [(Ieee802154Event, u16); 12] = [
    (Ieee802154Event::TxDone, 1 << 0),
    (Ieee802154Event::RxDone, 1 << 1),
    (Ieee802154Event::AckTxDone, 1 << 2),
    (Ieee802154Event::AckRxDone, 1 << 3),
    (Ieee802154Event::RxAbort, 1 << 4),
    (Ieee802154Event::TxAbort, 1 << 5),
    (Ieee802154Event::EdDone, 1 << 6),
    (Ieee802154Event::Timer0Overflow, 1 << 8),
    (Ieee802154Event::Timer1Overflow, 1 << 9),
    (Ieee802154Event::ClockCountMatch, 1 << 10),
    (Ieee802154Event::TxSfdDone, 1 << 11),
    (Ieee802154Event::RxSfdDone, 1 << 12),
];
/// `IEEE802154_EVENT_MASK`.
const ALL_EVENTS: u64 = 0x3fff;

/// `IEEE802154_RX_ABORT_BY_*` codes.
const RX_ABORT_CODES: [(Ieee802154RxAbortReason, u64); 16] = [
    (Ieee802154RxAbortReason::RxStop, 1),
    (Ieee802154RxAbortReason::SfdTimeout, 2),
    (Ieee802154RxAbortReason::CrcError, 3),
    (Ieee802154RxAbortReason::InvalidLength, 4),
    (Ieee802154RxAbortReason::FilterFail, 5),
    (Ieee802154RxAbortReason::NoRss, 6),
    (Ieee802154RxAbortReason::CoexistenceBreak, 7),
    (Ieee802154RxAbortReason::UnexpectedAck, 8),
    (Ieee802154RxAbortReason::RxRestart, 9),
    (Ieee802154RxAbortReason::TxAckTimeout, 16),
    (Ieee802154RxAbortReason::TxAckStop, 17),
    (Ieee802154RxAbortReason::TxAckCoexistenceBreak, 18),
    (Ieee802154RxAbortReason::EnhancedAckSecurityError, 19),
    (Ieee802154RxAbortReason::EdAbort, 24),
    (Ieee802154RxAbortReason::EdStop, 25),
    (Ieee802154RxAbortReason::EdCoexistenceReject, 26),
];

/// `IEEE802154_TX_ABORT_BY_*` codes.
const TX_ABORT_CODES: [(Ieee802154TxAbortReason, u64); 15] = [
    (Ieee802154TxAbortReason::RxAckStop, 1),
    (Ieee802154TxAbortReason::RxAckSfdTimeout, 2),
    (Ieee802154TxAbortReason::RxAckCrcError, 3),
    (Ieee802154TxAbortReason::RxAckInvalidLength, 4),
    (Ieee802154TxAbortReason::RxAckFilterFail, 5),
    (Ieee802154TxAbortReason::RxAckNoRss, 6),
    (Ieee802154TxAbortReason::RxAckCoexistenceBreak, 7),
    (Ieee802154TxAbortReason::RxAckTypeNotAck, 8),
    (Ieee802154TxAbortReason::RxAckRestart, 9),
    (Ieee802154TxAbortReason::RxAckTimeout, 16),
    (Ieee802154TxAbortReason::TxStop, 17),
    (Ieee802154TxAbortReason::TxCoexistenceBreak, 18),
    (Ieee802154TxAbortReason::TxSecurityError, 19),
    (Ieee802154TxAbortReason::CcaFailed, 24),
    (Ieee802154TxAbortReason::CcaBusy, 25),
];

/// Modem ETM registers (`ieee802154_reg.h`).
const ETM_CHEN: u32 = 0x2010_8800;
const ETM_CHENSET: u32 = 0x2010_8804;
const ETM_CHENCLR: u32 = 0x2010_8808;
const ETM_CH0_EVT_ID: u32 = 0x2010_880c;
const ETM_CH0_TASK_ID: u32 = 0x2010_8810;
const ETM_CH_OFFSET: u32 = 8;

const fn bit(value: bool) -> u64 {
    value as u64
}

const fn signed(value: i8) -> u64 {
    value as i64 as u64
}

fn event_bits(mask: Ieee802154EventMask) -> u64 {
    EVENT_BITS
        .iter()
        .filter(|(event, _)| mask.contains(*event))
        .fold(0, |bits, (_, bit)| bits | u64::from(*bit))
}

const fn rx_abort_mask(set: Ieee802154RxAbortEnableSet) -> u64 {
    match set {
        Ieee802154RxAbortEnableSet::RuntimeBaseline => (1 << (16 - 1)) | (1 << (18 - 1)),
        Ieee802154RxAbortEnableSet::All => 0x7fff_ffff,
    }
}

const fn tx_abort_mask(set: Ieee802154TxAbortEnableSet) -> u64 {
    match set {
        Ieee802154TxAbortEnableSet::RuntimeBaseline => {
            (1 << (16 - 1)) | (1 << (18 - 1)) | (1 << (19 - 1)) | (1 << (24 - 1)) | (1 << (25 - 1))
        }
        Ieee802154TxAbortEnableSet::All => 0x7fff_ffff,
    }
}

const fn command_code(command: Ieee802154LlCommand) -> u64 {
    match command {
        Ieee802154LlCommand::TxStart => 0x41,
        Ieee802154LlCommand::RxStart => 0x42,
        Ieee802154LlCommand::CcaTxStart => 0x43,
        Ieee802154LlCommand::EdStart => 0x44,
        Ieee802154LlCommand::Stop => 0x45,
    }
}

const fn timer_command(timer: Ieee802154Timer, start: bool) -> u64 {
    match (timer, start) {
        (Ieee802154Timer::Timer0, true) => 0x4c,
        (Ieee802154Timer::Timer0, false) => 0x4d,
        (Ieee802154Timer::Timer1, true) => 0x4e,
        (Ieee802154Timer::Timer1, false) => 0x4f,
    }
}

const fn etm_channel(channel: Ieee802154EtmChannel) -> u32 {
    match channel {
        Ieee802154EtmChannel::Channel0 => 0,
        Ieee802154EtmChannel::Channel1 => 1,
    }
}

const fn cca_mode(mode: Ieee802154CcaMode) -> u64 {
    match mode {
        Ieee802154CcaMode::Carrier => 0,
        Ieee802154CcaMode::EnergyDetection => 1,
        Ieee802154CcaMode::CarrierOrEnergyDetection => 2,
        Ieee802154CcaMode::CarrierAndEnergyDetection => 3,
    }
}

const fn tx_error(error: Ieee802154TxError) -> u64 {
    match error {
        Ieee802154TxError::CcaBusy => 1,
        Ieee802154TxError::Abort => 2,
        Ieee802154TxError::NoAck => 3,
        Ieee802154TxError::InvalidAck => 4,
        Ieee802154TxError::Coexist => 5,
        Ieee802154TxError::Security => 6,
    }
}

/// `frame_bytes` of the vendor host glue: the PHR plus its length.
fn frame_bytes(frame: &[u8; FRAME_SIZE]) -> Vec<u8> {
    frame[..(usize::from(frame[0]) + 1).min(FRAME_SIZE)].to_vec()
}

/// The vendor host glue's seven frame-info arguments.
fn info_arguments(info: Option<&Ieee802154FrameInfo>) -> Vec<u64> {
    match info {
        Some(info) => vec![
            bit(info.pending),
            bit(info.process),
            u64::from(info.channel),
            signed(info.rssi),
            u64::from(info.lqi),
            info.timestamp,
            1,
            // `ESP_IEEE802154_MULTIPAN_MAX` marks an unmatched frame.
            info.mpf_index
                .map_or(u64::from(Ieee802154MultipanIndex::COUNT), |index| {
                    u64::from(index.value())
                }),
        ],
        None => vec![0; 8],
    }
}

enum Arg {
    Value(u64),
    Address(u32),
}

/// State shared by the low-level adapter and the environment, so records
/// interleave in call order.
struct Shared {
    model: LlModel,
    records: Vec<Record>,
    registers: BTreeMap<u32, u32>,
    driver_buffers: Vec<u32>,
    transmit_address: u32,
    transmits: usize,
    rx_address: Option<u32>,
    rx_status: u64,
    incomplete: Option<String>,
}

impl Shared {
    fn label(&mut self, address: u32) -> Argument {
        if address == self.transmit_address && self.transmits > 0 {
            return Argument::TransmitFrame(self.transmits - 1);
        }
        let index = match self
            .driver_buffers
            .iter()
            .position(|&buffer| buffer == address)
        {
            Some(index) => index,
            None => {
                self.driver_buffers.push(address);
                self.driver_buffers.len() - 1
            }
        };
        Argument::DriverBuffer(index)
    }

    fn ll(&mut self, name: &str, arguments: &[Arg]) -> u64 {
        let raw: Vec<u64> = arguments
            .iter()
            .map(|argument| match argument {
                Arg::Value(value) => *value,
                Arg::Address(address) => u64::from(*address),
            })
            .collect();
        let value = self.model.call(name, &raw);
        let arguments = arguments
            .iter()
            .map(|argument| match argument {
                Arg::Value(value) => Argument::Value(*value),
                Arg::Address(address) => self.label(*address),
            })
            .collect();
        self.records.push(Record::Ll {
            name: name.to_owned(),
            arguments,
        });
        value
    }

    /// A call whose last argument points at `buffer`, recorded by content
    /// after the model answered, as the vendor recorder does.
    fn ll_buffer(&mut self, name: &str, leading: &[u64], buffer: &mut [u8]) -> u64 {
        let mut raw = leading.to_vec();
        raw.push(buffer.as_mut_ptr() as u64);
        let value = self.model.call(name, &raw);
        let mut arguments: Vec<Argument> = leading.iter().copied().map(Argument::Value).collect();
        arguments.push(Argument::Bytes(buffer.to_vec()));
        self.records.push(Record::Ll {
            name: name.to_owned(),
            arguments,
        });
        value
    }

    fn read(&mut self, address: u32) -> u32 {
        let value = self.registers.get(&address).copied().unwrap_or(0);
        self.records.push(Record::RegisterRead { address, value });
        value
    }

    fn write(&mut self, address: u32, value: u32) {
        self.registers.insert(address, value);
        self.records.push(Record::RegisterWrite { address, value });
    }

    fn event(&mut self, name: &str, arguments: Vec<u64>, first: Vec<u8>, second: Vec<u8>) {
        self.records.push(Record::Event {
            name: name.to_owned(),
            arguments,
            first,
            second,
        });
    }
}

struct PortLl(Rc<RefCell<Shared>>);

impl PortLl {
    /// A priority published against [`level_table`], rendered as the level
    /// the vendor driver passes to libcoexist.
    fn coex(&mut self, name: &str, pti: CoexPti) {
        self.0.borrow_mut().records.push(Record::Coex {
            name: name.to_owned(),
            level: u64::from(pti.value()),
        });
    }

    fn ll(&self, name: &str, arguments: &[Arg]) -> u64 {
        self.0.borrow_mut().ll(name, arguments)
    }

    fn value(&self, name: &str, value: u64) {
        self.ll(name, &[Arg::Value(value)]);
    }
}

impl Ieee802154LowLevel for PortLl {
    fn set_command(&mut self, command: Ieee802154LlCommand) {
        self.value("ieee802154_ll_set_cmd", command_code(command));
    }
    fn events(&mut self) -> Ieee802154EventObservation {
        let bits = self.ll("ieee802154_ll_get_events", &[]) as u16;
        let named = EVENT_BITS
            .iter()
            .filter(|(_, bit)| bits & bit != 0)
            .fold(Ieee802154EventMask::NONE, |mask, (event, _)| {
                mask.union(event.mask())
            });
        if u64::from(bits) != event_bits(named) {
            self.0.borrow_mut().incomplete = Some(format!("unclassified event bits in {bits:#x}"));
        }
        Ieee802154EventObservation::from_named(named)
    }
    fn clear_events(&mut self, mask: Ieee802154EventMask) {
        self.value("ieee802154_ll_clear_events", event_bits(mask));
    }
    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        let code = self.ll("ieee802154_ll_get_rx_abort_reason", &[]);
        RX_ABORT_CODES
            .iter()
            .find(|(_, value)| *value == code)
            .map_or(
                Ieee802154RxAbortReasonObservation::Unclassified,
                |(reason, _)| Ieee802154RxAbortReasonObservation::Named(*reason),
            )
    }
    fn tx_abort_reason(&mut self) -> Ieee802154TxAbortReasonObservation {
        let code = self.ll("ieee802154_ll_get_tx_abort_reason", &[]);
        TX_ABORT_CODES
            .iter()
            .find(|(_, value)| *value == code)
            .map_or(
                Ieee802154TxAbortReasonObservation::Unclassified,
                |(reason, _)| Ieee802154TxAbortReasonObservation::Named(*reason),
            )
    }
    fn rx_status(&mut self) -> Ieee802154RxStatus {
        let status = self.ll("ieee802154_ll_get_rx_status", &[]);
        self.0.borrow_mut().rx_status = status;
        Ieee802154RxStatus::new(
            0,
            Ieee802154RxAbortReasonObservation::Unclassified,
            Ieee802154RxStateCode::new(0).expect("zero is a state code"),
            false,
            false,
        )
    }
    fn is_current_rx_frame(&mut self) -> bool {
        self.ll("ieee802154_ll_is_current_rx_frame", &[]) != 0
    }
    fn set_tx_address(&mut self, address: u32) {
        self.ll("ieee802154_ll_set_tx_addr", &[Arg::Address(address)]);
    }
    fn set_rx_address(&mut self, address: u32) {
        self.0.borrow_mut().rx_address = Some(address);
        self.ll("ieee802154_ll_set_rx_addr", &[Arg::Address(address)]);
    }
    fn tx_auto_ack(&mut self) -> bool {
        self.ll("ieee802154_ll_get_tx_auto_ack", &[]) != 0
    }
    fn rx_auto_ack(&mut self) -> bool {
        self.ll("ieee802154_ll_get_rx_auto_ack", &[]) != 0
    }
    fn tx_enhanced_ack(&mut self) -> bool {
        self.ll("ieee802154_ll_get_tx_enhance_ack", &[]) != 0
    }
    fn pending_mode(&mut self) -> bool {
        self.ll("ieee802154_ll_get_pending_mode", &[]) != 0
    }
    fn set_pending_bit(&mut self, pending: bool) {
        self.value("ieee802154_ll_set_pending_bit", bit(pending));
    }
    fn frequency_code(&mut self) -> u8 {
        self.ll("ieee802154_ll_get_freq", &[]) as u8
    }
    fn ed_rss(&mut self) -> i8 {
        self.ll("ieee802154_ll_get_ed_rss", &[]) as i8
    }
    fn cca_busy(&mut self) -> bool {
        self.ll("ieee802154_ll_is_cca_busy", &[]) != 0
    }
    // The pinned vendor build here has no `CONFIG_IEEE802154_TXRX_STATISTIC`,
    // so its driver never reads the diagnostic counters, and the stand never
    // asks the engine to collect statistics.
    fn debug_counter(&mut self, counter: Ieee802154DebugCounter) -> u16 {
        unreachable!("diagnostic counter {counter:?} read without TX/RX statistics")
    }
    fn clear_debug_counter(&mut self, counter: Ieee802154DebugCounter) {
        unreachable!("diagnostic counter {counter:?} cleared without TX/RX statistics")
    }
    fn set_ed_duration(&mut self, symbols: u16) {
        self.value("ieee802154_ll_set_ed_duration", u64::from(symbols));
    }
    fn notify_enhanced_ack_generated(&mut self) {
        self.ll("ieee802154_ll_enhack_generate_done_notify", &[]);
    }
    fn disable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.value("ieee802154_ll_disable_rx_abort_events", rx_abort_mask(set));
    }
    fn enable_all_events(&mut self) {
        self.value("ieee802154_ll_enable_events", ALL_EVENTS);
    }
    fn enable_event(&mut self, event: Ieee802154Event) {
        self.value("ieee802154_ll_enable_events", event_bits(event.mask()));
    }
    fn disable_event(&mut self, event: Ieee802154Event) {
        self.value("ieee802154_ll_disable_events", event_bits(event.mask()));
    }
    fn enable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet) {
        self.value("ieee802154_ll_enable_tx_abort_events", tx_abort_mask(set));
    }
    fn enable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.value("ieee802154_ll_enable_rx_abort_events", rx_abort_mask(set));
    }
    fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode) {
        let value = match mode {
            Ieee802154EdSampleMode::Maximum => 0,
            Ieee802154EdSampleMode::Average => 1,
        };
        self.value("ieee802154_ll_set_ed_sample_mode", value);
    }
    fn disable_coex(&mut self) {
        self.ll("ieee802154_ll_disable_coex", &[]);
    }
    fn set_txrx_pti(&mut self, pti: CoexPti) {
        self.coex("esp_coex_ieee802154_txrx_pti_set", pti);
    }
    fn set_ack_pti(&mut self, pti: CoexPti) {
        self.coex("esp_coex_ieee802154_ack_pti_set", pti);
    }
    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.value(
            "ieee802154_ll_set_freq",
            u64::from(channel.frequency_code()),
        );
    }
    fn set_tx_power(&mut self, power: &Ieee802154ResolvedTxPower<'_>) {
        self.value(
            "ieee802154_ll_set_power",
            u64::from(power.selected_provider_index()),
        );
    }
    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.value("ieee802154_ll_set_cca_mode", cca_mode(mode));
    }
    fn set_cca_threshold(&mut self, threshold_dbm: i8) {
        self.value("ieee802154_ll_set_cca_threshold", signed(threshold_dbm));
    }
    fn set_tx_auto_ack(&mut self, enable: bool) {
        self.value("ieee802154_ll_set_tx_auto_ack", bit(enable));
    }
    fn set_rx_auto_ack(&mut self, enable: bool) {
        self.value("ieee802154_ll_set_rx_auto_ack", bit(enable));
    }
    fn set_tx_enhanced_ack(&mut self, enable: bool) {
        self.value("ieee802154_ll_set_tx_enhance_ack", bit(enable));
    }
    fn set_coordinator(&mut self, enable: bool) {
        self.value("ieee802154_ll_set_coordinator", bit(enable));
    }
    fn set_promiscuous(&mut self, enable: bool) {
        self.value("ieee802154_ll_set_promiscuous", bit(enable));
    }
    fn set_pending_mode(&mut self, enhanced: bool) {
        self.value("ieee802154_ll_set_pending_mode", bit(enhanced));
    }
    fn set_transmit_security(&mut self, enable: bool) {
        self.value("ieee802154_ll_set_transmit_security", bit(enable));
    }
    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32) {
        let name = match timer {
            Ieee802154Timer::Timer0 => "ieee802154_ll_timer0_set_threshold",
            Ieee802154Timer::Timer1 => "ieee802154_ll_timer1_set_threshold",
        };
        self.value(name, u64::from(microseconds));
    }
    fn start_timer(&mut self, timer: Ieee802154Timer) {
        self.value("ieee802154_ll_set_cmd", timer_command(timer, true));
    }
    fn stop_timer(&mut self, timer: Ieee802154Timer) {
        self.value("ieee802154_ll_set_cmd", timer_command(timer, false));
    }
    fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool {
        self.0.borrow_mut().read(ETM_CHEN) & (1 << etm_channel(channel)) != 0
    }
    // The production PAC writes only the channel bit to the write-trigger
    // set and clear words; the vendor reads the word first. Both write the
    // same image while a trigger word reads as zero, which is the model's
    // value for an unwritten word.
    fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        let mut shared = self.0.borrow_mut();
        let value = shared.read(ETM_CHENCLR) | 1 << etm_channel(channel);
        shared.write(ETM_CHENCLR, value);
    }
    fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        let mut shared = self.0.borrow_mut();
        let value = shared.read(ETM_CHENSET) | 1 << etm_channel(channel);
        shared.write(ETM_CHENSET, value);
    }
    fn set_etm_route(&mut self, route: Ieee802154EtmRoute) {
        let (event, task) = match route {
            Ieee802154EtmRoute::Timer0ToTxStart => (80, 88),
            Ieee802154EtmRoute::Timer0ToCcaTx => (80, 84),
            Ieee802154EtmRoute::Timer1ToRxStart => (79, 85),
        };
        let offset = ETM_CH_OFFSET * etm_channel(route.channel());
        let mut shared = self.0.borrow_mut();
        shared.write(ETM_CH0_EVT_ID + offset, event);
        shared.write(ETM_CH0_TASK_ID + offset, task);
    }
    fn set_multipan_panid(&mut self, index: Ieee802154MultipanIndex, panid: u16) {
        self.ll(
            "ieee802154_ll_set_multipan_panid",
            &[
                Arg::Value(u64::from(index.value())),
                Arg::Value(u64::from(panid)),
            ],
        );
    }
    fn multipan_panid(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.ll(
            "ieee802154_ll_get_multipan_panid",
            &[Arg::Value(u64::from(index.value()))],
        ) as u16
    }
    fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16) {
        self.ll(
            "ieee802154_ll_set_multipan_short_addr",
            &[
                Arg::Value(u64::from(index.value())),
                Arg::Value(u64::from(address)),
            ],
        );
    }
    fn multipan_short_address(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.ll(
            "ieee802154_ll_get_multipan_short_addr",
            &[Arg::Value(u64::from(index.value()))],
        ) as u16
    }
    fn set_multipan_extended_address(&mut self, index: Ieee802154MultipanIndex, address: [u8; 8]) {
        let mut address = address;
        self.0.borrow_mut().ll_buffer(
            "ieee802154_ll_set_multipan_ext_addr",
            &[u64::from(index.value())],
            &mut address,
        );
    }
    fn multipan_extended_address(&mut self, index: Ieee802154MultipanIndex) -> [u8; 8] {
        let mut address = [0; 8];
        self.0.borrow_mut().ll_buffer(
            "ieee802154_ll_get_multipan_ext_addr",
            &[u64::from(index.value())],
            &mut address,
        );
        address
    }
    fn set_multipan_enable(&mut self, state: Ieee802154MultipanEnableState) {
        let mask = (0..Ieee802154MultipanIndex::COUNT)
            .filter_map(Ieee802154MultipanIndex::new)
            .filter(|index| state.contains(*index))
            .fold(0, |mask, index| mask | 1 << index.value());
        self.value("ieee802154_ll_set_multipan_enable_mask", mask);
    }
    fn multipan_enable(&mut self) -> Ieee802154MultipanEnableState {
        let mask = self.ll("ieee802154_ll_get_multipan_enable_mask", &[]);
        enable_state(mask as u8)
    }
    fn set_ack_timeout(&mut self, units: u16) {
        self.value("ieee802154_ll_set_ack_timeout", u64::from(units));
    }
    fn ack_timeout(&mut self) -> u16 {
        self.ll("ieee802154_ll_get_ack_timeout", &[]) as u16
    }
    fn set_security_address(&mut self, address: &[u8; 8]) {
        let mut address = *address;
        self.0
            .borrow_mut()
            .ll_buffer("ieee802154_ll_set_security_addr", &[], &mut address);
    }
    fn set_security_key(&mut self, key: &[u8; 16]) {
        let mut key = *key;
        self.0
            .borrow_mut()
            .ll_buffer("ieee802154_ll_set_security_key", &[], &mut key);
    }
    fn set_security_offset(&mut self, offset: u8) {
        self.value("ieee802154_ll_set_security_offset", u64::from(offset));
    }
}

struct PortEnv(Rc<RefCell<Shared>>);

impl Ieee802154Environment for PortEnv {
    fn now_micros(&mut self) -> u64 {
        let shared = self.0.borrow();
        shared
            .model
            .inputs
            .values
            .get("esp_timer_get_time")
            .copied()
            .unwrap_or(0)
    }
    fn receive_done(
        &mut self,
        _slot: Ieee802154RxSlot,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
    ) {
        self.0.borrow_mut().event(
            "receive_done",
            info_arguments(Some(info)),
            frame_bytes(frame),
            Vec::new(),
        );
    }
    fn receive_sfd_done(&mut self) {
        self.0
            .borrow_mut()
            .event("receive_sfd_done", Vec::new(), Vec::new(), Vec::new());
    }
    fn transmit_done(&mut self, frame: &[u8; FRAME_SIZE], ack: Option<Ieee802154ReceivedAck<'_>>) {
        self.0.borrow_mut().event(
            "transmit_done",
            info_arguments(ack.map(|ack| ack.info)),
            frame_bytes(frame),
            ack.map(|ack| frame_bytes(ack.frame)).unwrap_or_default(),
        );
    }
    fn transmit_failed(&mut self, frame: &[u8; FRAME_SIZE], error: Ieee802154TxError) {
        self.0.borrow_mut().event(
            "transmit_failed",
            vec![tx_error(error)],
            frame_bytes(frame),
            Vec::new(),
        );
    }
    fn transmit_sfd_done(&mut self, frame: &mut [u8; FRAME_SIZE]) {
        self.0.borrow_mut().event(
            "transmit_sfd_done",
            Vec::new(),
            frame_bytes(frame),
            Vec::new(),
        );
    }
    fn energy_detect_done(&mut self, power: i8) {
        self.0.borrow_mut().event(
            "energy_detect_done",
            vec![signed(power)],
            Vec::new(),
            Vec::new(),
        );
    }
    fn cca_done(&mut self, busy: bool) {
        self.0
            .borrow_mut()
            .event("cca_done", vec![bit(busy)], Vec::new(), Vec::new());
    }
    fn ed_failed(&mut self, _status: Ieee802154RxStatus) {
        let mut shared = self.0.borrow_mut();
        // The vendor passes the 32-bit status to a `uint16_t` parameter.
        let status = shared.rx_status & 0xffff;
        shared.event("ed_failed", vec![status], Vec::new(), Vec::new());
    }
    fn receive_at_done(&mut self) {
        self.0
            .borrow_mut()
            .event("receive_at_done", Vec::new(), Vec::new(), Vec::new());
    }
    fn generate_enhanced_ack(
        &mut self,
        frame: &[u8; FRAME_SIZE],
        _info: &Ieee802154FrameInfo,
        ack: &mut [u8; FRAME_SIZE],
    ) -> Ieee802154EnhancedAck {
        let mut shared = self.0.borrow_mut();
        let generated = shared.model.inputs.enhanced_ack.clone();
        let key = shared.model.inputs.enhanced_ack_key;
        shared.event(
            "enh_ack_generator",
            Vec::new(),
            frame_bytes(frame),
            generated.clone().unwrap_or_default(),
        );
        match generated {
            Some(image) => {
                let length = image.len().min(FRAME_SIZE);
                ack[..length].copy_from_slice(&image[..length]);
                key.map_or(Ieee802154EnhancedAck::Generated, |key| {
                    Ieee802154EnhancedAck::Secured { key }
                })
            }
            None => Ieee802154EnhancedAck::Refused,
        }
    }
}

/// The enable state of a vendor interface mask.
fn enable_state(mask: u8) -> Ieee802154MultipanEnableState {
    (0..Ieee802154MultipanIndex::COUNT)
        .filter_map(Ieee802154MultipanIndex::new)
        .filter(|index| mask & 1 << index.value() != 0)
        .fold(Ieee802154MultipanEnableState::NONE, |state, index| {
            state.with(index)
        })
}

/// `esp_ieee802154_set_cca_mode` argument.
fn cca_mode_of(mode: u32) -> Option<Ieee802154CcaMode> {
    Some(match mode {
        0 => Ieee802154CcaMode::Carrier,
        1 => Ieee802154CcaMode::EnergyDetection,
        2 => Ieee802154CcaMode::CarrierOrEnergyDetection,
        3 => Ieee802154CcaMode::CarrierAndEnergyDetection,
        _ => return None,
    })
}

/// `esp_ieee802154_set_pending_mode` argument.
fn pending_mode_of(mode: u32) -> Option<AutoPendingMode> {
    Some(match mode {
        0 => AutoPendingMode::Disable,
        1 => AutoPendingMode::Enable,
        2 => AutoPendingMode::Enhanced,
        3 => AutoPendingMode::Zigbee,
        _ => return None,
    })
}

#[cfg(feature = "multipan")]
fn interface(index: u8) -> Result<Ieee802154MultipanIndex, &'static str> {
    Ieee802154MultipanIndex::new(index).ok_or("interface outside the four contexts")
}

/// A coexistence table in which level `n` (`ieee802154_coex_event_t`) has
/// priority `n`, so every priority the engine publishes names the level the
/// vendor driver chose. How libcoexist maps a level to a priority is not
/// part of the compiled driver; the HAL owns and tests that resolution.
#[cfg(feature = "sw-coex")]
fn level_table() -> CoexPtiTable {
    let mut table = CoexPtiTable::VENDOR;
    for level in [
        Ieee802154CoexLevel::High,
        Ieee802154CoexLevel::Middle,
        Ieee802154CoexLevel::Low,
        Ieee802154CoexLevel::Idle,
    ] {
        table.set(
            level.event(),
            CoexPti::new(level as u8).expect("a level is a priority"),
        );
    }
    table
}

/// Channel `number` under the open domain: the comparison does not model a
/// country, and the ceiling is the open domain's EIRP.
fn permit(
    number: u8,
    levels: oer_espressif_ieee802154_engine::tx_power::Ieee802154TxPowerLevels<'_>,
) -> Option<oer_espressif_ieee802154_engine::pib::Ieee802154Permit> {
    let channel = Ieee802154Channel::new(number).ok()?;
    let permitted = oer_radio_regulatory::TEST_DOMAIN.permit(channel).ok()?;
    let ceiling = permitted.tx_ceiling(oer_radio_regulatory::PowerLimits {
        antenna_gain_mbm: 0,
        chip_maximum_mbm: i32::MAX,
        owner_cap_mbm: None,
    });
    oer_espressif_ieee802154_engine::pib::Ieee802154Permit::new(permitted, ceiling, levels).ok()
}

/// Run `scenario` against the production engine.
///
/// # Errors
///
/// Returns why the scenario cannot be compared: a step the engine does not
/// own yet, or a model input the engine vocabulary cannot represent.
pub fn run(scenario: &Scenario) -> Result<Vec<Record>, String> {
    let buffers = Box::leak(Box::new(Ieee802154EngineBuffers::new()));
    // The recovered ESP32-S31 BTBB level set, which the vendor side reads
    // through `bt_bb_get_tx_pwr_table` (`crate::record`).
    let levels = oer_esp32s31_knowledge::tx_power::ESP32S31_TX_POWER_LEVELS;
    let defaults = Ieee802154PibDefaults::default();
    let start = permit(11, levels).expect("channel 11 under the open domain");
    #[cfg(not(feature = "multipan"))]
    let mut engine = Ieee802154Engine::new(buffers, levels, defaults, start);
    // The stand's multi-PAN build uses the Kconfig default of two interfaces.
    #[cfg(feature = "multipan")]
    let mut engine = Ieee802154Engine::new_multipan(
        buffers,
        levels,
        defaults,
        oer_espressif_ieee802154_engine::engine::Ieee802154Interfaces::new(2)
            .expect("two interfaces"),
        start,
    );
    // The stand's software-coexistence build uses the driver's default
    // scene levels.
    #[cfg(feature = "sw-coex")]
    engine.set_coexistence(Ieee802154Coexistence::Software(resolve_priorities(
        Ieee802154CoexConfig::VENDOR,
        &level_table(),
    )));
    let shared = Rc::new(RefCell::new(Shared {
        model: LlModel::new((scenario.inputs)()),
        records: Vec::new(),
        registers: BTreeMap::new(),
        driver_buffers: Vec::new(),
        transmit_address: engine.model_transmit_address(),
        transmits: 0,
        rx_address: None,
        rx_status: 0,
        incomplete: None,
    }));
    let mut ll = PortLl(Rc::clone(&shared));
    let mut env = PortEnv(Rc::clone(&shared));
    let mut last_rx = None;

    for step in (scenario.steps)() {
        match step {
            Step::Enable => {
                engine.enable();
                engine.mac_init(&mut ll, defaults);
            }
            Step::Disable => engine.disable(),
            Step::SetChannel(channel) => {
                if let Some(permit) = permit(channel, levels) {
                    engine.pib().set_channel(permit);
                }
            }
            Step::SetPromiscuous(enable) => engine.pib().set_promiscuous(enable),
            Step::SetCoordinator(enable) => engine.pib().set_coordinator(enable),
            Step::SetRxWhenIdle(enable) => engine.pib().set_rx_when_idle(enable),
            Step::SetCcaMode(mode) => {
                let mode = cca_mode_of(mode).ok_or("CCA mode outside the vendor enum")?;
                engine.pib().set_cca_mode(mode);
            }
            Step::SetCcaThreshold(threshold) => engine.pib().set_cca_threshold(threshold),
            Step::SetTxPower(power) => engine.pib().set_power(power),
            Step::SetPowerWithChannel { channel, power } => {
                if let Ok(channel) = Ieee802154Channel::new(channel) {
                    engine.pib().set_power_for_channel(channel, power);
                }
            }
            Step::SetPendingMode(mode) => {
                let mode = pending_mode_of(mode).ok_or("pending mode outside the vendor enum")?;
                engine
                    .pib()
                    .set_pending_mode(Ieee802154MultipanIndex::CONTEXT0, mode);
            }
            Step::AddPendingAddress { address, short } => {
                let address = if short {
                    FrameAddress::Short(address[..2].try_into().map_err(|_| "short address")?)
                } else {
                    FrameAddress::Extended(address[..8].try_into().map_err(|_| "extended address")?)
                };
                let _ = engine.pending_table().add(address);
            }
            Step::SetPanId(panid) => engine.set_panid(&mut ll, panid),
            Step::SetShortAddress(address) => engine.set_short_address(&mut ll, address),
            Step::SetExtendedAddress(address) => engine.set_extended_address(&mut ll, address),
            Step::SetAckTimeout(timeout) => engine.set_ack_timeout(&mut ll, timeout),
            Step::GetIdentity => {
                engine.panid(&mut ll);
                engine.short_address(&mut ll);
                engine.extended_address(&mut ll);
                engine.ack_timeout(&mut ll);
            }
            // The production read (`Ieee802154RecentRssi`) is the runtime's
            // over the PAC's baseband capability and passes no engine or MAC
            // accessor; the vendor call must leave none either.
            Step::GetRecentRssi => {}
            Step::SetTransmitSecurity {
                frame,
                key,
                address,
            } => engine.set_transmit_security(&mut ll, &frame, &key, &address),
            Step::Transmit { frame, cca } => {
                shared.borrow_mut().transmits += 1;
                engine
                    .transmit(&mut ll, &mut env, &frame, cca)
                    .map_err(|_| "transmit frame image does not fit one DMA frame")?;
            }
            Step::TransmitAt { frame, cca, time } => {
                shared.borrow_mut().transmits += 1;
                engine
                    .transmit_at(&mut ll, &mut env, &frame, cca, time)
                    .map_err(|_| "transmit frame image does not fit one DMA frame")?;
            }
            Step::Receive => engine.receive(&mut ll, &mut env),
            Step::ReceiveAt { time, duration } => {
                engine.receive_at(&mut ll, &mut env, time, duration);
            }
            Step::Sleep => engine.sleep(&mut ll, &mut env),
            Step::EnergyDetect(duration) => {
                engine.energy_detect(&mut ll, &mut env, duration as u16);
            }
            Step::Cca => engine.cca(&mut ll, &mut env),
            Step::ReceiveHandleDone => {
                let slot = last_rx
                    .and_then(|address| engine.model_rx_slot(address))
                    .ok_or("released frame is not in the receive ring")?;
                engine
                    .receive_handle_done(slot)
                    .map_err(|_| "receive slot out of range")?;
            }
            #[cfg(feature = "multipan")]
            Step::SetMultipanPanId { index, panid } => {
                engine.set_multipan_panid(&mut ll, interface(index)?, panid);
            }
            #[cfg(feature = "multipan")]
            Step::SetMultipanShortAddress { index, address } => {
                engine.set_multipan_short_address(&mut ll, interface(index)?, address);
            }
            #[cfg(feature = "multipan")]
            Step::SetMultipanExtendedAddress { index, address } => {
                engine.set_multipan_extended_address(&mut ll, interface(index)?, address);
            }
            #[cfg(feature = "multipan")]
            Step::SetMultipanEnable(mask) => {
                engine.set_multipan_enable(&mut ll, enable_state(mask))
            }
            #[cfg(feature = "multipan")]
            Step::MultipanReceive(index) => {
                engine.multipan_receive(&mut ll, &mut env, interface(index)?);
            }
            #[cfg(feature = "multipan")]
            Step::MultipanSleep(index) => {
                engine.multipan_sleep(&mut ll, &mut env, interface(index)?);
            }
            #[cfg(feature = "multipan")]
            Step::MultipanRxWhenIdle { index, enable } => {
                engine.multipan_set_rx_when_idle(interface(index)?, enable);
            }
            #[cfg(feature = "multipan")]
            Step::MultipanSetPendingMode { index, mode } => {
                let mode = pending_mode_of(mode).ok_or("pending mode outside the vendor enum")?;
                engine.pib().set_pending_mode(interface(index)?, mode);
            }
            #[cfg(feature = "multipan")]
            Step::MultipanAddPendingAddress {
                index,
                address,
                short,
            } => {
                let address = if short {
                    FrameAddress::Short(address[..2].try_into().map_err(|_| "short address")?)
                } else {
                    FrameAddress::Extended(address[..8].try_into().map_err(|_| "extended address")?)
                };
                let _ = engine.pending_table_for(interface(index)?).add(address);
            }
            Step::Input(name, value) => {
                shared
                    .borrow_mut()
                    .model
                    .inputs
                    .values
                    .insert(name.to_owned(), value);
            }
            Step::DeliverFrame(frame) => {
                let address = shared
                    .borrow()
                    .rx_address
                    .ok_or("a frame arrived before an RX buffer was published")?;
                engine.model_dma_write(address, &frame);
                last_rx = Some(address);
            }
            Step::Interrupt {
                events,
                rx_abort,
                tx_abort,
            } => {
                {
                    let mut shared = shared.borrow_mut();
                    let values = &mut shared.model.inputs.values;
                    values.insert("ieee802154_ll_get_events".to_owned(), u64::from(events));
                    values.insert(
                        "ieee802154_ll_get_rx_abort_reason".to_owned(),
                        u64::from(rx_abort),
                    );
                    values.insert(
                        "ieee802154_ll_get_tx_abort_reason".to_owned(),
                        u64::from(tx_abort),
                    );
                }
                engine.isr(&mut ll, &mut env);
            }
        }
        if let Some(reason) = shared.borrow_mut().incomplete.take() {
            return Err(reason);
        }
    }
    let records = std::mem::take(&mut shared.borrow_mut().records);
    Ok(records)
}
