#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
//! The HIL radio pair's services: two boards of one chip, a sender and a
//! receiver, driven step by step by the host's program.
//!
//! Both images serve [`Radio`]'s endpoints: the bring-up, the stop, the
//! boot's two snapshots and the receive path's counters. The sender
//! (`esp32s31-case-c-sender`) adds its own (target, round); the
//! receiver (`esp32s31-rx-ring`) adds its own (expect, window,
//! stall, turn). No endpoint waits for the host: each answers what its
//! hardware did, a wait inside a handler bounded by its hardware's source
//! or by the duration the request names.
//!
//! The messages are shared by the images, which answer them, and the host
//! program, which sends them and decides.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// The most readings a snapshot page holds.
pub const PAGE_READINGS: usize = 40;
/// The most descriptors a ring's state reports: the receiver's 16 and R.
pub const RING_DESCRIPTORS: usize = 17;
/// Case C's cases (`case_c::CASES`).
pub const CASES: usize = 6;
/// A case's ends by the frame before it: each case or none.
pub const BEFORE: usize = CASES + 1;
/// The most distinct continuation words a chain window reports a position.
pub const WORDS: usize = 8;

oer_protocols_hil_protocol::messages! {
    endpoint SnapshotCold = "radio-pair/snapshot-cold" => SnapshotPage;
    endpoint SnapshotUp = "radio-pair/snapshot-up" => SnapshotPage;
    topic SnapshotPage = "radio-pair/snapshot-page";
    endpoint BringUp = "radio-pair/bring-up" => BroughtUp;
    topic BroughtUp = "radio-pair/brought-up";
    endpoint ReceiveCounters = "radio-pair/receive-counters" => Counters;
    topic Counters = "radio-pair/counters";
    endpoint Stop = "radio-pair/stop" => Stopped;
    topic Stopped = "radio-pair/stopped";
    endpoint Target = "radio-pair/target" => Targeted;
    topic Targeted = "radio-pair/targeted";
    endpoint Round = "radio-pair/round" => Sent;
    topic Sent = "radio-pair/sent";
    endpoint Expect = "radio-pair/expect" => Expecting;
    topic Expecting = "radio-pair/expecting";
    endpoint Window = "radio-pair/window" => Windowed;
    topic Windowed = "radio-pair/windowed";
    endpoint Stall = "radio-pair/stall" => Stood;
    topic Stood = "radio-pair/stood";
    endpoint Turn = "radio-pair/turn" => Turned;
    topic Turned = "radio-pair/turned";
}

/// Why an endpoint answered without acting: the step it needs is not the
/// one the image is at.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Refusal {
    /// The radio is not brought up yet.
    NotUp,
    /// The radio is already up: the bring-up and the cold snapshot are done.
    AlreadyUp,
    /// The radio was stopped: nothing runs until the next boot.
    Down,
    /// A step failed: until the next boot only the counters, the snapshots
    /// and the stop answer.
    StepFailed,
    /// The receiver was not told its sender ([`Expect`]).
    NoSender,
    /// The sender was not told its receiver ([`Target`]).
    NoReceiver,
    /// A snapshot page past the last.
    NoSuchPage,
}

/// A failure inside a step: the probe's phase and its index there, the
/// same pair its journal kept.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Failure {
    pub phase: u32,
    pub index: u32,
}

/// A page of the snapshot taken before the bring-up.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SnapshotCold {
    pub page: u8,
}

/// A page of the snapshot taken after the bring-up.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SnapshotUp {
    pub page: u8,
}

/// The answer to a snapshot request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a message: the image builds its answer once and publishes it"
)]
pub enum SnapshotPage {
    Readings {
        page: u8,
        pages: u8,
        readings: heapless::Vec<Reading, PAGE_READINGS>,
    },
    Refused(Refusal),
}

/// One named reading: what it is, its index in an array (0 otherwise) and
/// its value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Reading {
    pub item: Item,
    pub index: u8,
    pub value: Value,
}

/// A reading's value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Value {
    /// A register's word or an unsigned quantity.
    Word(u32),
    /// A signed quantity.
    Signed(i32),
    /// A register of a block whose bus clock is off or whose reset is held:
    /// it was not read.
    Unreadable,
}

/// What a snapshot reads. The registers are the bring-up's own: each one
/// the HAL's radio power, modem, domain or Wi-Fi client modules read or
/// write on the bring-up path (the module named on each), in the register
/// model's names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Item {
    /// Hart 0's reset cause, its raw code.
    ResetCause,
    /// `PMU.HP_ACTIVE_ICG_MODEM` (`radio/modem/platform.rs`).
    PmuHpActiveIcgModem,
    /// `PMU.HP_ACTIVE_HP_CK_POWER` (`radio/modem/ops.rs`).
    PmuHpActiveHpCkPower,
    /// `PMU.RF_PWC` (`radio/modem/ops.rs`).
    PmuRfPwc,
    /// `PMU.ANA_PERI_PWR_CTRL` (`radio/modem/ops.rs`, `platform.rs`).
    PmuAnaPeriPwrCtrl,
    /// `HP_SYS_CLKRST.MODEM_CTRL0` (`radio/modem/ops.rs`, `platform.rs`).
    HpSysClkrstModemCtrl0,
    /// `HP_SYS_CLKRST.MODEM_CONF` (`radio/modem/clock_device.rs`, `platform.rs`).
    HpSysClkrstModemConf,
    /// `MODEM_SYSCON.CLK_CONF` (`radio/modem/ops.rs`, `syscon.rs`).
    ModemSysconClkConf,
    /// `MODEM_SYSCON.CLK_CONF1` (`radio/modem/ops.rs`, `syscon.rs`).
    ModemSysconClkConf1,
    /// `MODEM_SYSCON.CLK_CONF_POWER_ST` (`radio/modem/ops.rs`, `shared_clock.rs`, `syscon.rs`).
    ModemSysconClkConfPowerSt,
    /// `MODEM_SYSCON.MODEM_RST_CONF` (`radio/modem/ops.rs`, `syscon.rs`).
    ModemSysconModemRstConf,
    /// `MODEM_SYSCON.WIFI_BB_CFG` (`radio/modem/ops.rs`).
    ModemSysconWifiBbCfg,
    /// `MODEM_LPCON.LP_TIMER_CONF` (`radio/modem/ops.rs`, `shared_clock.rs`).
    ModemLpconLpTimerConf,
    /// `MODEM_LPCON.COEX_LP_CLK_CONF` (`radio/modem/ops.rs`, `shared_clock.rs`).
    ModemLpconCoexLpClkConf,
    /// `MODEM_LPCON.WIFI_LP_CLK_CONF` (`radio/modem/ops.rs`).
    ModemLpconWifiLpClkConf,
    /// `MODEM_LPCON.MODEM_32K_CLK_CONF` (`radio/modem/ops.rs`).
    ModemLpconModem32kClkConf,
    /// `MODEM_LPCON.CLK_CONF_POWER_ST` (`radio/modem/ops.rs`, `shared_clock.rs`, `syscon.rs`).
    ModemLpconClkConfPowerSt,
    /// `MODEM_LPCON.TICK_CONF` (`radio/modem/low_power_phy.rs`).
    ModemLpconTickConf,
    /// The temperature sensor's code, read after the bring-up.
    TemperatureCode,
    /// The RF PLL's frequency block: its control image, parameter image,
    /// result and ready flag, observed after the bring-up.
    RfpllControl,
    RfpllParameter,
    RfpllResult,
    RfpllReady,
    /// The PHY's common calibration results (`PhyCommonCalibration`).
    CalibrationTemperature,
    CalibrationRfpllReferenceTemperature,
    CalibrationRxReferenceTemperature,
    CalibrationTxReferenceTemperature,
    CalibrationPowerReferenceTemperature,
    CalibrationSensorIndex,
    CalibrationCrystalSelector,
    CalibrationRcResult,
    CalibrationFilterDcap,
    CalibrationRcCalibrated,
    CalibrationDcode,
    CalibrationI2cFrequencyParameter,
    CalibrationXtalDuty,
    /// The PHY's Wi-Fi calibration results (`PhyWifiCalibration`).
    WifiTxGainAdjustment,
    WifiCalibratedFlags,
    WifiTxDco,
    WifiTxReferenceCodes,
    WifiTxCapacitance,
    WifiTxPowerCurve,
    WifiTxPowerCorrections,
    WifiTxPowerAdjustment,
    WifiCalibratedAttenuation,
    WifiTxIqConfig,
    WifiTxIqCoefficient,
    WifiRxIqCoefficients,
    WifiExternalDcode,
    WifiCalibrationTemperature,
    WifiCalibrationChannel,
    WifiRxTableLastIndex,
    WifiSharedRxTableLastIndex,
    WifiIndexDc,
    WifiFineDc,
    WifiSharedIndexDc,
}

/// Bring the radio up: the PHY's registration, coexistence, power, clocks,
/// the PHY client, the tracking due, the MAC's cold start on the probe's
/// ring, the channel and the receive.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct BringUp;

/// The answer to [`BringUp`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum BroughtUp {
    /// Up and receiving: the station's and the access point's addresses.
    Up {
        station: [u8; 6],
        ap: [u8; 6],
    },
    Failed(Failure),
    Refused(Refusal),
}

/// The receive path's counters now.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReceiveCounters;

/// The answer to [`ReceiveCounters`]: the MAC's counters as the hardware
/// holds them (cumulative, wrapping) and the ring's state, absent once a
/// failed turn's stop took the ring down.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a message: the image builds its answer once and publishes it"
)]
pub enum Counters {
    Counted {
        mac: MacCounters,
        decode: DecodeCounters,
        hang: HangCounters,
        ring: Option<RingState>,
    },
    Refused(Refusal),
}

/// The MAC's ordinary receive counters (`MacRxPrimaryStatistics`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MacCounters {
    pub mpdu: u16,
    pub fcs_error: u16,
    pub abort: u16,
    pub abort_fcs_pass: u16,
    pub power_drop_error: u16,
    pub signal_field: u16,
    pub end: u16,
    pub data_success: u16,
    pub other_unicast: u16,
    pub buffer_full: u16,
    pub fifo_overflow: u16,
    pub last_unmatched_error: u16,
    pub ack_interrupt: u16,
}

/// The baseband's and the normal receive's decoder errors
/// (`MacRxDecodeErrorStatistics`), ten-bit counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DecodeCounters {
    pub brx_agc: u16,
    pub brx: u16,
    pub nrx: u16,
    pub nrx_abort: u16,
    pub nrx_agc_exit: u16,
    pub nrx_baseband_off: u16,
    pub nrx_fdm_watchdog: u16,
    pub nrx_restart: u16,
    pub nrx_service: u16,
    pub nrx_tx_over: u16,
    pub nrx_unsupported: u16,
}

/// The MAC's and the baseband's hang counters (`MacRxHangStatistics`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HangCounters {
    pub rx: u8,
    pub tx: u8,
    pub rx_tx_hang: u32,
    pub rx_tx_panic: u32,
}

/// Where the ring's walker stands and each descriptor's control word.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RingState {
    pub standing: Standing,
    /// The ring's descriptors in its order, then R's.
    pub controls: heapless::Vec<u32, RING_DESCRIPTORS>,
}

/// What the ring's walker read (`RxRing::standing`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Standing {
    /// LAST's ring index; `None` outside the ring.
    pub last: Option<u8>,
    pub next: NextAt,
    pub tail: u8,
    /// The accepted tail's descriptor, read between the two cursor reads.
    pub tail_word: TailWord,
    /// The cursor read before and after the tail's word.
    pub cursor: Cursor,
}

/// The accepted tail's descriptor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum TailWord {
    #[default]
    Armed,
    Completed,
}

/// Whether the walker's cursor moved while the tail's word was read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Cursor {
    #[default]
    Steady,
    Moved,
}

/// Where NEXT points.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum NextAt {
    #[default]
    Zero,
    Ring(u8),
    Elsewhere,
}

/// Stop the radio: the walker's edge, the stop request and its hold, the
/// reset after a stop, the release. Nothing runs after it until the next
/// boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Stop;

/// The answer to [`Stop`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Stopped {
    Stopped { micros: u32 },
    Failed(Failure),
    Refused(Refusal),
}

/// Send to `receiver`: the receiver's station address the host names, as
/// [`Expect`] names the receiver its sender.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Target {
    pub receiver: [u8; 6],
}

/// The answer to [`Target`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Targeted {
    Targeted,
    Refused(Refusal),
}

/// Send `rounds` of case C to the receiver: one frame of each case a round,
/// each awaited, in the round's drawn order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Round {
    pub rounds: u32,
}

/// The answer to [`Round`]: per case, by the frame before it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a message: the image builds its answer once and publishes it"
)]
pub enum Sent {
    Sent {
        cases: heapless::Vec<CaseEnds, CASES>,
        micros: u32,
    },
    Failed(RoundFailure),
    Refused(Refusal),
}

/// Where the rounds stopped: the failed step, the frame it was at, the time
/// since the rounds began, the completion wait's bound, and the transmit
/// queue as the MAC held it when the step failed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RoundFailure {
    pub failure: Failure,
    /// The round, from zero.
    pub round: u32,
    /// The frame's case (`case_c::Case::tag`).
    pub tag: u32,
    pub micros: u32,
    pub bound_millis: u32,
    pub queue: QueueState,
}

/// The ordinary transmit queue as the MAC holds it
/// (`MacOrdinaryTxQueueSnapshot`): a sequence of reads, not one capture.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct QueueState {
    pub ownership: QueueOwnership,
    /// The queue's raised result events.
    pub raised: heapless::Vec<QueueEvent, 3>,
    /// The MAC's shared activity code.
    pub active_state: u8,
    pub hang: HangCounters,
}

/// A transmit queue's result event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum QueueEvent {
    Completion,
    Timeout,
    Collision,
}

/// The queue's `ENABLE` and `VALID` bits: who holds it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum QueueOwnership {
    Idle,
    Enabled,
    Valid,
    Published,
}

/// One case's ends by the frame before it: acknowledged (status 0), ACK
/// timeouts (status 5), and every other end in all.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CaseEnds {
    pub tag: u32,
    pub acknowledged: [u32; BEFORE],
    pub timeouts: [u32; BEFORE],
    pub other: u32,
}

/// Count the frames from `sender`: the sender's station address the host
/// names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Expect {
    pub sender: [u8; 6],
}

/// The answer to [`Expect`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Expecting {
    Expecting,
    Refused(Refusal),
}

/// Serve the ring for `millis`: every single lent for a spare (copied in
/// place when none is held), every chain copied in place, each checked;
/// with `chains`, each chain's descriptors and continuation words
/// recorded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Window {
    pub millis: u32,
    pub record: Record,
}

/// What a window records besides its counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Record {
    Counts,
    /// Each chain's descriptors and continuation words too.
    Chains,
}

/// The answer to [`Window`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[allow(
    clippy::large_enum_variant,
    reason = "a message: the image builds its answer once and publishes it"
)]
pub enum Windowed {
    Counted {
        counts: Counts,
        spares: u8,
        chains: heapless::Vec<ChainCase, CASES>,
        words: heapless::Vec<WordSeen, WORDS>,
        words_overflow: u32,
    },
    Failed(Failure),
    Refused(Refusal),
}

/// A window's units by kind.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Counts {
    pub sender: u32,
    pub singles: u32,
    pub chains: u32,
    pub mismatched: u32,
    pub lengths: u32,
    pub foreign: u32,
    pub length_anomalies: u32,
    pub doorbell_anomalies: u32,
}

/// One case's chains in a window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ChainCase {
    pub tag: u32,
    pub chains: u32,
    pub least: u32,
    pub most: u32,
    pub expected: u32,
    pub mismatched: u32,
}

/// A continuation's control word at a position (0 first, 1 middle), and
/// how many times it was seen.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WordSeen {
    pub position: u8,
    pub word: u32,
    pub seen: u32,
}

/// Leave the ring unserved for `millis`, then read where its walker
/// stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Stall {
    pub millis: u32,
}

/// The answer to [`Stall`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Stood {
    Standing(Standing),
    Refused(Refusal),
}

/// One turn: a window of `millis`, the stop, the cold start, a window of
/// `millis`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Turn {
    pub millis: u32,
}

/// The answer to [`Turn`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Turned {
    Turned {
        before: Counts,
        after: Counts,
        handles: u8,
        stop_micros: u32,
        start_micros: u32,
    },
    Failed(Failure),
    Refused(Refusal),
}

oer_protocols_hil_agent::requests! {
    /// The sender's requests: the radio's and its own.
    pub enum SenderRequest (sessions = false) {
        SnapshotCold(SnapshotCold),
        SnapshotUp(SnapshotUp),
        BringUp(BringUp),
        ReceiveCounters(ReceiveCounters),
        Stop(Stop),
        Target(Target),
        Round(Round),
    }
}

oer_protocols_hil_agent::requests! {
    /// The receiver's requests: the radio's and its own.
    pub enum ReceiverRequest (sessions = false) {
        SnapshotCold(SnapshotCold),
        SnapshotUp(SnapshotUp),
        BringUp(BringUp),
        ReceiveCounters(ReceiveCounters),
        Stop(Stop),
        Expect(Expect),
        Window(Window),
        Stall(Stall),
        Turn(Turn),
    }
}

/// The largest request payload either image takes.
pub const MAXIMUM_PAYLOAD_BYTES: u16 = 16;

#[cfg(test)]
mod tests;
