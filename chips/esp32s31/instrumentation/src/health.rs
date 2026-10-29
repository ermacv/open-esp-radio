use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// One monotonic clock within a boot. The cut/measurement can change between reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Bracket {
    pub first_micros: u64,
    pub last_micros: u64,
}
impl Bracket {
    pub const fn valid(self) -> bool {
        self.first_micros <= self.last_micros
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Cut {
    Before,
    After,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CounterKind {
    Mpdu,
    FcsError,
    Abort,
    AbortFcsPass,
    PowerDrop,
    HeSigB,
    SameBm,
    SignalField,
    End,
    DataSuccess,
    OtherUnicast,
    BufferFull,
    FifoOverflow,
    Tkip,
    BtBlock,
    FrequencyHop,
    LastUnmatched,
    AckInterrupt,
    RtsInterrupt,
    CfoScaled40,
    BrxAgc,
    Brx,
    Nrx,
    NrxAbort,
    NrxAgcExit,
    NrxBasebandOff,
    NrxFdmWatchdog,
    NrxRestart,
    NrxService,
    NrxTxOver,
    NrxUnsupported,
    NrxHeFormat,
    NrxHtSig,
    NrxHeUnsupported,
    NrxHeSigACrc,
    TxRts,
    TxCts,
    TxTrack,
    TxTrcts,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CounterValue {
    Modulo10(u16),
    Modulo16(u16),
    Modulo32(u32),
    SignedScaled40(i32),
}
impl CounterValue {
    /// Modulo increment only; one cannot infer how many complete wraps occurred.
    pub const fn delta_since(self, before: Self, same_epoch: bool) -> Option<u32> {
        if !same_epoch {
            return None;
        }
        match (self, before) {
            (Self::Modulo10(a), Self::Modulo10(b)) if a < 1024 && b < 1024 => {
                Some(a.wrapping_sub(b) as u32 & 1023)
            }
            (Self::Modulo16(a), Self::Modulo16(b)) => Some(a.wrapping_sub(b) as u32),
            (Self::Modulo32(a), Self::Modulo32(b)) => Some(a.wrapping_sub(b)),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Counter {
    pub cut: Cut,
    pub epoch: u32,
    pub read: Bracket,
    pub kind: CounterKind,
    pub value: CounterValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Acquisition {
    Unobserved,
    Undated,
    Overlong,
    ClockReversed,
    Window(Bracket),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Temperature {
    pub sensor_units: i16,
    pub acquisition: Acquisition,
}
impl Temperature {
    pub const fn age_micros(self, read_micros: u64) -> Option<u64> {
        match self.acquisition {
            Acquisition::Window(b) if b.valid() && b.last_micros <= read_micros => {
                read_micros.checked_sub(b.first_micros)
            }
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum NoiseFloor {
    /// Distinct ten-bit negative code; no assumed sampling cadence.
    NegativeCode10 { raw: u16 },
    /// Native twelve-bit sixteenths-dB representation and its decoded value.
    SixteenthsDb12 { raw: u16, quarter_db: i16 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReceivedRate {
    pub family: u8,
    pub code: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum SignalValue {
    NoiseFloor(NoiseFloor),
    Temperature(Temperature),
    /// From one normalized RX descriptor, not an ambient RSSI register.
    Received {
        peer: [u8; 6],
        rssi_dbm: i8,
        rate: Option<ReceivedRate>,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Signal {
    pub read: Bracket,
    pub channel: u8,
    pub gain: Option<u16>,
    pub value: SignalValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Due {
    Unknown,
    NotDue,
    Due,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Disposition {
    Inspected,
    Started,
    Ran,
    Skipped,
    Failed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum TrackingOperation {
    Tick,
    Rfpll,
    Common,
    Transmit,
    WifiBasebandOff,
    Temperature,
    Other(u8),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ThermalCondition {
    pub current: i16,
    pub reference: i16,
    pub threshold: u8,
    pub inhibited: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Tracking {
    pub condition: Option<ThermalCondition>,
    pub operation: TrackingOperation,
    pub due: Due,
    pub disposition: Disposition,
    pub at_micros: u64,
    pub committed: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MeasurementContext {
    pub gain: u8,
    pub measurement: u8,
    pub shared_radio: bool,
    pub radio_stage: bool,
    pub fine_code: Option<u8>,
    pub high_level: bool,
    pub frequency_code: Option<u16>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DcIqRequest {
    pub context: MeasurementContext,
    pub iteration: u8,
    pub chain: u8,
    pub control: u16,
    pub mode: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum SarPhase {
    DetectorConfigured,
    ToneConfigured,
    ReferenceControl,
    ToneArmed,
    Triggered,
    Ready,
    TimedOut,
    Captured,
    ToneCleared,
    ToneStopped,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum MeasurementValue {
    SarPhase {
        phase: SarPhase,
        measurement: Option<u8>,
        sample: Option<u8>,
        value: Option<u16>,
    },
    RetainedPowerDetector {
        tx_path: u8,
        rx_path: u8,
        dco: [u16; 4],
        clear_tone_after_ready: bool,
        reference_codes: [i16; 2],
        calibrated: bool,
    },
    /// Configuration and the first false-ready observation. Engine remains owned.
    DcIqPending {
        request: DcIqRequest,
        activity: bool,
    },
    /// Ready/read/disable atom has completed. Times bracket that atom, not exact hardware edges.
    DcIqCaptured {
        request: DcIqRequest,
        sums: Option<[i32; 3]>,
    },
    /// Existing SAR/PWDET result; no extra acquisition or residual is requested.
    PowerDetector {
        measurement: u8,
        sample: u8,
        raw: u16,
    },
    SarWait {
        elapsed_micros: u64,
        observations: u16,
        ready: bool,
    },
    TxDcComparator {
        gain: u8,
        iteration: u8,
        high: [bool; 2],
    },
    /// Existing accepted correction result. No residual means none was measured.
    RxDcCorrection {
        context: MeasurementContext,
        configuration: [u16; 2],
        iterations: u8,
        converged: bool,
        residual: Option<[i32; 3]>,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Measurement {
    pub read: Bracket,
    pub channel: Option<u8>,
    pub gain: Option<u16>,
    pub value: MeasurementValue,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Record {
    Counter(Counter),
    Signal(Signal),
    Tracking(Tracking),
    Measurement(Measurement),
}

impl Record {
    /// Capture aggregate ids follow this dictionary's record categories.
    pub const fn kind(self) -> u16 {
        match self {
            Self::Counter(_) => 0,
            Self::Signal(_) => 1,
            Self::Tracking(_) => 2,
            Self::Measurement(_) => 3,
        }
    }
}
