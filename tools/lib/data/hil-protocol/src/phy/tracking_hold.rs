//! One shared PHY tracking tick timed on the device, for the RF hold bound.

use serde::{Deserialize, Serialize};

use postcard_schema::Schema;

/// Force tracking branches with the vendor's temperature-tracking debug
/// policy and run one due tick, timing its operations. The periodic timer
/// must be suspended first.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PhyTrackingHoldRequest {
    /// Force the RFPLL-cap branch.
    pub rfpll: bool,
    /// Force the calibration branch (common RX calibration and TXDC/PWDET).
    pub calibration: bool,
    /// The forced threshold; zero runs the forced branches on every due tick.
    pub threshold: u8,
}

/// How the timed tick ended.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum PhyTrackingHoldTick {
    /// Tracking was due and ran to completion.
    Tracked,
    /// No tracking was due.
    NotDue,
    /// The domain could not serve the tick.
    Skipped,
    /// Tracking failed; the domain is poisoned.
    #[default]
    Failed,
}

/// The timing of one observed operation in microseconds, waits and nested
/// operations included.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PhyTrackingHoldTiming {
    /// Completed intervals.
    pub completed: u16,
    /// Failed intervals.
    pub failed: u16,
    /// The longest interval.
    pub maximum_micros: u32,
    /// The sum of every interval.
    pub elapsed_micros: u32,
}

/// The radio clients the shared PHY domain served when the tick ran: they
/// select which branches the vendor tracking runs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PhyTrackingHoldClients {
    pub wifi: bool,
    pub bluetooth: bool,
    pub ieee802154: bool,
}

/// The operations of one timed tick.
///
/// The RF hold is the three grant-protect intervals and
/// `wifi_baseband_off`. A grant-protect interval is the time the PHY's
/// coexistence request was programmed, not a proof that the arbiter kept
/// every other radio off the air. `tracking` is the whole tick, and the
/// other timings are branches nested inside it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PhyTrackingHoldEvidence {
    pub tick: PhyTrackingHoldTick,
    /// The clients the domain served; `None` when it was not registered.
    pub clients: Option<PhyTrackingHoldClients>,
    /// A clock reversal, overflow or malformed sequence invalidated timing.
    pub invalid: bool,
    pub tracking: PhyTrackingHoldTiming,
    pub rfpll: PhyTrackingHoldTiming,
    pub calibration: PhyTrackingHoldTiming,
    pub rx_gain: PhyTrackingHoldTiming,
    pub tx_dc_pwdet: PhyTrackingHoldTiming,
    /// Grant-protect held around the RFPLL capacitor correction.
    pub grant_protect_rfpll: PhyTrackingHoldTiming,
    /// Grant-protect held around the common branch (DCODE, RX gain).
    pub grant_protect_common: PhyTrackingHoldTiming,
    /// Grant-protect held around the transmit branch (TXDC/PWDET).
    pub grant_protect_transmit: PhyTrackingHoldTiming,
    pub wifi_baseband_off: PhyTrackingHoldTiming,
}
