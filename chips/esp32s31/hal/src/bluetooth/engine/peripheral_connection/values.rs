//! Public connection values whose SRAM encoding stays in the codec.

#![forbid(unsafe_code)]

/// Link Layer kind for one packet appended to the live connection TX cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionTransmitPduKind {
    /// LLID 0b01, continuing an L2CAP PDU.
    DataContinuation,
    /// LLID 0b10, starting or completing an L2CAP PDU.
    DataStartOrComplete,
    /// LLID 0b11, Link Layer control.
    Control,
}

impl PeripheralConnectionTransmitPduKind {
    pub(super) const fn llid(self) -> u8 {
        match self {
            Self::DataContinuation => 1,
            Self::DataStartOrComplete => 2,
            Self::Control => 3,
        }
    }
}

/// Air-interface identity consumed by the S31 connection link state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionIdentity {
    access_address: [u8; 4],
    crc_initialization: [u8; 3],
}

impl PeripheralConnectionIdentity {
    /// Construct the exact two fields in over-the-air little-endian order.
    pub const fn new(access_address: [u8; 4], crc_initialization: [u8; 3]) -> Self {
        Self {
            access_address,
            crc_initialization,
        }
    }

    /// Access Address octets in Link Layer wire order.
    pub const fn access_address_wire_bytes(self) -> [u8; 4] {
        self.access_address
    }

    /// CRCInit octets in Link Layer wire order.
    pub const fn crc_initialization_wire_bytes(self) -> [u8; 3] {
        self.crc_initialization
    }

    pub(super) const fn crc_initialization_word(self) -> [u8; 4] {
        [
            self.crc_initialization[0],
            self.crc_initialization[1],
            self.crc_initialization[2],
            0,
        ]
    }
}

/// One validated LE data channel projected into the S31 frequency table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionDataChannel {
    index: u8,
    frequency_image: u8,
}

impl PeripheralConnectionDataChannel {
    /// Bind one of the 37 Link Layer data-channel indices.
    pub const fn new(index: u8) -> Option<Self> {
        if index >= 37 {
            return None;
        }
        let frequency_image = if index <= 10 {
            (index + 1) * 2
        } else {
            (index + 2) * 2
        };
        Some(Self {
            index,
            frequency_image,
        })
    }

    pub const fn index(self) -> u8 {
        self.index
    }

    pub(super) const fn frequency_image(self) -> u8 {
        self.frequency_image
    }
}

/// Wrapping absolute Controller time of the latest valid connection reception.
///
/// The first event seeds this field with connection creation time. Hardware
/// subsequently updates it independently of scheduler anchor capture and RX
/// payload delivery. Zero is a valid timestamp at controller wrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionReceiveTime(u32);

impl PeripheralConnectionReceiveTime {
    pub const fn from_controller_ticks(ticks: u32) -> Self {
        Self(ticks)
    }

    pub const fn wrapping_controller_ticks(self) -> u32 {
        self.0
    }
}

/// Non-empty Controller event span installed before one connection RUN.
///
/// This is a required link-state input for both the first and recurring event.
/// A completed event publishes its independent captured receive-time
/// observation in the scheduler item; the two values never share storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionEventSpan(u32);

impl PeripheralConnectionEventSpan {
    pub const fn new(ticks: u32) -> Option<Self> {
        if ticks == 0 || ticks > i32::MAX as u32 {
            None
        } else {
            Some(Self(ticks))
        }
    }

    pub(super) const fn ticks(self) -> u32 {
        self.0
    }
}

/// Opaque Controller receive-time observation captured for a connection event.
///
/// This is neither scheduler time nor a normalized packet-start anchor. It can
/// enter only the chip-private epoch and PHY timing projection above this
/// controller-memory boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionCapturedAnchorTime(u32);

impl PeripheralConnectionCapturedAnchorTime {
    pub(super) const fn from_controller_sram_word(word: u32) -> Self {
        Self(word)
    }

    /// Borrow the wrapping tick observation for chip-private time projection.
    #[doc(hidden)]
    pub const fn wrapping_controller_ticks(self) -> u32 {
        self.0
    }
}

/// Whether a completed connection scheduler item published a receive-time capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionCapturedAnchorAvailability {
    Absent,
    Available(PeripheralConnectionCapturedAnchorTime),
}

/// Non-empty raw Controller window for one connection scheduler item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionSchedulerWindow {
    start: u32,
    end: u32,
}

impl PeripheralConnectionSchedulerWindow {
    pub const fn new(start: u32, end: u32) -> Option<Self> {
        let duration = end.wrapping_sub(start);
        if duration == 0 || duration > i32::MAX as u32 {
            None
        } else {
            Some(Self { start, end })
        }
    }

    pub(super) const fn start(self) -> u32 {
        self.start
    }

    pub(super) const fn end(self) -> u32 {
        self.end
    }
}

/// Bounded first-event receive wait expressed only in physical time.
///
/// The controller-memory codec owns the positional duration/mode encoding.
/// Callers provide the accepted transmit-window width and the symmetric timing
/// uncertainty which surrounds it; they cannot construct a descriptor word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionReceiveWait {
    transmit_window_micros: u32,
    timing_guard_micros: u32,
    total_micros: u16,
}

impl PeripheralConnectionReceiveWait {
    /// Form the complete first-event receive wait from the transmit-window
    /// width and the timing guard, in microseconds of the LE radio time: this
    /// codec is the representation boundary, and the caller keeps the
    /// domain-typed spans.
    ///
    /// The extra 61 microseconds are a fixed S31 PHY allowance recovered from
    /// the complete connection-event builder. This constructor admits only the
    /// short hardware form used by every valid legacy first transmit window.
    pub fn new(transmit_window_micros: u64, timing_guard_micros: u64) -> Option<Self> {
        let total = timing_guard_micros
            .checked_mul(2)?
            .checked_add(transmit_window_micros)?
            .checked_add(61)?;
        if transmit_window_micros == 0 || total > 65_534 {
            return None;
        }
        Some(Self {
            transmit_window_micros: u32::try_from(transmit_window_micros).ok()?,
            timing_guard_micros: u32::try_from(timing_guard_micros).ok()?,
            total_micros: u16::try_from(total).ok()?,
        })
    }

    pub const fn transmit_window_micros(self) -> u32 {
        self.transmit_window_micros
    }

    pub const fn timing_guard_micros(self) -> u32 {
        self.timing_guard_micros
    }

    pub const fn total_micros(self) -> u32 {
        self.total_micros as u32
    }
}

/// Recurring-event receive wait for the reviewed software window-widening path.
///
/// The semantic duration combines the Controller's fixed guard, accumulated
/// anchor uncertainty, twice the current window widening and the final
/// Controller boundary guard. The private SRAM codec alone selects the
/// zero-duration, short or half-resolution long descriptor representation.
/// Automatic window widening is intentionally not represented by this type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionRecurringReceiveWait {
    total_micros: u32,
}

impl PeripheralConnectionRecurringReceiveWait {
    /// Form one receive wait whose physical duration, `micros` of the LE
    /// radio time, is represented exactly.
    ///
    /// The long form stores two-microsecond units. Odd long durations are
    /// rejected instead of silently reproducing the vendor's truncating shift.
    pub fn new(micros: u64) -> Option<Self> {
        if micros > 131_070 || (micros > 65_534 && micros & 1 != 0) {
            return None;
        }
        Some(Self {
            total_micros: u32::try_from(micros).ok()?,
        })
    }

    pub const fn total_micros(self) -> u32 {
        self.total_micros
    }
}

/// Source-owned event priority shared by connection state and scheduler item.
///
/// The first event starts at 13. A normally completed recurring event resets
/// to 8. Ordinary conflict escalation is capped at 14; the distinct exhausted
/// retry-budget path may force 15. Those policy transitions remain outside the
/// private descriptor encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionSchedulerPriority(u8);

impl PeripheralConnectionSchedulerPriority {
    /// Priority selected by the reviewed ESP32-S31 first-event policy.
    pub const FIRST_EVENT: Self = Self(13);

    /// Baseline restored before an ordinary recurring event.
    pub const RECURRING_BASELINE: Self = Self(8);

    /// A priority from 0 to 15; the connection policy chooses it.
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 15 { Some(Self(value)) } else { None }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Opaque category of one non-sentinel connection scheduler status.
///
/// The raw controller word remains private to the memory codec. Zero versus
/// nonzero is diagnostic only and does not classify Link Layer completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionSchedulerItemCompletionStatus {
    Zero,
    NonZero,
    /// The item left the scheduler without executing: it was cancelled,
    /// deleted with its list or stopped.
    Aborted,
}
