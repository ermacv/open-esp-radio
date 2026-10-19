//! What a backend serves, stated before any request.

use crate::{LeRadio, RadioDuration, RadioTiming, TestPhy};

/// One LE PHY.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LePhy {
    /// LE 1M.
    Le1M,
    /// LE 2M.
    Le2M,
    /// LE Coded, with either coding.
    LeCoded,
}

impl LePhy {
    const fn bit(self) -> u8 {
        match self {
            Self::Le1M => 1,
            Self::Le2M => 1 << 1,
            Self::LeCoded => 1 << 2,
        }
    }
}

impl TestPhy {
    /// The PHY the test runs on, without its coding.
    pub const fn phy(self) -> LePhy {
        match self {
            Self::Le1M => LePhy::Le1M,
            Self::Le2M => LePhy::Le2M,
            Self::LeCodedS8 | Self::LeCodedS2 => LePhy::LeCoded,
        }
    }
}

/// A set of LE PHYs.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct LePhys(u8);

impl LePhys {
    /// No PHY.
    pub const NONE: Self = Self(0);
    /// LE 1M alone.
    pub const LE_1M: Self = Self::NONE.with(LePhy::Le1M);
    /// LE 1M, LE 2M and LE Coded.
    pub const ALL: Self = Self::LE_1M.with(LePhy::Le2M).with(LePhy::LeCoded);

    /// This set and `phy`.
    #[must_use]
    pub const fn with(self, phy: LePhy) -> Self {
        Self(self.0 | phy.bit())
    }

    /// Whether the set holds `phy`.
    pub const fn contains(self, phy: LePhy) -> bool {
        self.0 & phy.bit() != 0
    }
}

/// Who runs the Link Layer acknowledgement and flow control of a connection
/// (Core Specification Vol 6, Part B, 4.5.9).
///
/// The owner keeps the transmit sequence number (SN) and the next expected
/// sequence number (NESN), retransmits an unacknowledged PDU, suppresses
/// duplicates, answers with empty PDUs when nothing is queued and sets the
/// More Data (MD) bit.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkAcknowledgement {
    /// The radio hardware. The backend receives only the LLID and payload of
    /// each queued PDU, reports when the peer acknowledged it and reports
    /// only new, non-empty receptions.
    Hardware,
    /// Backend software in its time-critical context, for example with
    /// `oer-bluetooth-le-ll`'s `connection::acknowledgement` state machine. The
    /// port contract is the same as for [`Self::Hardware`].
    Software,
}

/// How a backend carries a connection's data.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LeConnectionCapabilities {
    /// Longest data PDU payload the backend transmits and receives, in
    /// octets, including an encrypted PDU's MIC. The protocol maximum is 255:
    /// 251 octets of data and the 4-octet MIC.
    pub max_data_payload: u8,
    /// Who acknowledges data PDUs.
    pub link_acknowledgement: LinkAcknowledgement,
    /// The longest first-event wait for the central: the transmit window
    /// widened by the timing guard on both sides.
    pub max_first_receive_wait: RadioDuration<LeRadio>,
    /// The longest receive wait of a later event.
    pub max_receive_wait: RadioDuration<LeRadio>,
}

/// The legacy advertising a backend sends, by what the set listens for.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LegacyAdvertisingTypes {
    /// `ADV_NONCONN_IND`, which listens for nothing.
    pub nonconnectable: bool,
    /// The types that listen after each PDU: `ADV_IND` and `ADV_SCAN_IND`,
    /// which answer `SCAN_REQ` or report every reception, and
    /// `ADV_DIRECT_IND`, which reports every reception.
    pub responsive: bool,
}

impl LegacyAdvertisingTypes {
    /// No advertising.
    pub const NONE: Self = Self {
        nonconnectable: false,
        responsive: false,
    };

    /// Whether any type is served.
    pub const fn any(self) -> bool {
        self.nonconnectable || self.responsive
    }
}

/// What a backend serves.
///
/// Every value a Controller submits is checked against these capabilities
/// when it is built ([`Supported`](crate::Supported)), so a backend refuses
/// nothing they exclude; a value they include may still meet the world
/// (time, the epoch, the clock).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LeRadioCapabilities {
    /// Legacy advertising PDU types.
    pub legacy_advertising: LegacyAdvertisingTypes,
    /// Passive scanners.
    pub passive_scanning: bool,
    /// Active scanners, which send `SCAN_REQ`.
    pub active_scanning: bool,
    /// A filter accept list and scanners that filter against it.
    pub filter_accept_list: bool,
    /// Peripheral connections, and how they carry data.
    pub peripheral_connection: Option<LeConnectionCapabilities>,
    /// Direct Test Mode.
    pub direct_test_mode: bool,
    /// PHYs of advertising sets, scanners and connections.
    pub phys: LePhys,
    /// PHYs of Direct Test Mode events.
    pub test_phys: LePhys,
    /// The lowest transmit power the backend selects, in dBm; it rounds a
    /// higher request down to a level it supports.
    pub min_tx_power_dbm: i8,
    /// How far past the current time a reservation may end.
    pub max_ahead: RadioDuration<LeRadio>,
    /// How the backend keeps reservations apart and admits events, which
    /// the planner uses.
    pub timing: RadioTiming,
}

impl LeRadioCapabilities {
    /// Nothing is served.
    pub const NONE: Self = Self {
        legacy_advertising: LegacyAdvertisingTypes::NONE,
        passive_scanning: false,
        active_scanning: false,
        filter_accept_list: false,
        peripheral_connection: None,
        direct_test_mode: false,
        phys: LePhys::NONE,
        test_phys: LePhys::NONE,
        min_tx_power_dbm: i8::MAX,
        max_ahead: RadioDuration::from_micros(0),
        timing: RadioTiming::MINIMAL,
    };

    /// Whether any scanner is served.
    pub const fn scanning(&self) -> bool {
        self.passive_scanning || self.active_scanning
    }
}

/// A backend's capabilities, known when the image is built: the marker a
/// Controller is generic over and every [`Supported`](crate::Supported)
/// value is branded by. The integrating firmware names one per backend
/// configuration; the Controller never names the backend.
pub trait LeCapabilities: 'static {
    /// What the backend serves.
    const CAPS: LeRadioCapabilities;
}

/// The capabilities of a host without a radio: nothing is served, so no
/// value is ever supported.
pub enum NoCapabilities {}

impl LeCapabilities for NoCapabilities {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities::NONE;
}

#[cfg(test)]
mod tests;
