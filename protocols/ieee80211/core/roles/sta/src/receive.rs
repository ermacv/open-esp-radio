//! The station's receive classifier, shared with anything that reads the
//! air the way the station does.
//!
//! [`classify`] decodes one MPDU's MAC header and sorts it into a management
//! or a data frame; [`observe_scan`] records a management frame in a scan
//! table with the channel and signal the receiver reported. A
//! [`Management`] frame is made only by [`classify`], so it always holds a
//! decoded management header.
//!
//! Received bytes reach this module; nothing here panics on them.

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{RxEvidence, RxMetadata};
use {
    crate::scan::{ScanObservation, ScanTable},
    oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason,
    oer_protocols_ieee80211_wire::frame::DecodedHeader,
};

/// A received management frame: its decoded MAC header.
///
/// Only [`classify`] makes one.
#[derive(Clone, Copy, Debug)]
pub struct Management<'frame>(DecodedHeader<'frame>);

impl<'frame> Management<'frame> {
    /// The complete MPDU.
    pub const fn frame(&self) -> &'frame [u8] {
        self.0.frame()
    }

    /// The decoded MAC header.
    pub const fn header(&self) -> &DecodedHeader<'frame> {
        &self.0
    }

    /// The Frame Control subtype: 8 for a beacon, 5 for a probe response.
    pub const fn subtype(&self) -> u16 {
        self.0.control().subtype()
    }
}

/// One received MPDU, sorted by its frame kind.
#[derive(Clone, Copy, Debug)]
pub enum Received<'frame> {
    /// A management frame.
    Management(Management<'frame>),
    /// A data frame, before admission.
    Data(DecodedHeader<'frame>),
}

/// Decode `frame`'s MAC header and sort it by kind.
///
/// A control frame, a frame of another protocol version and a frame shorter
/// than its header are refused with [`RxDropReason::Header`].
pub fn classify(frame: &[u8]) -> Result<Received<'_>, RxDropReason> {
    let header = DecodedHeader::decode(frame).ok_or(RxDropReason::Header)?;
    Ok(if header.control().is_data() {
        Received::Data(header)
    } else {
        Received::Management(Management(header))
    })
}

/// Record `management`, received at `now`, in `scan`; with `network`, a
/// BSS the table does not hold yet enters it only when it names that SSID
/// ([`ScanTable::observe_network`]).
///
/// A record without a DS Parameter Set element takes the channel the
/// receiver was configured to. A frame without a received signal level is
/// recorded without one.
pub fn observe_scan<const N: usize>(
    scan: &mut ScanTable<N>,
    now: Instant,
    management: Management<'_>,
    meta: &impl RxMetadata,
    network: Option<&[u8]>,
) -> ScanObservation {
    let common = meta.common();
    let rssi = match common.rssi_dbm {
        RxEvidence::HardwareObserved(rssi) | RxEvidence::ProtocolValidated(rssi) => Some(rssi),
        RxEvidence::Unavailable => None,
    };
    let channel = common.channel.number();
    match network {
        Some(ssid) => scan.observe_network(management.frame(), channel, rssi, Some(now), ssid),
        None => scan.observe_management(management.frame(), channel, rssi, Some(now)),
    }
}

/// What a scan reports of `meta`'s frame of Frame Control `subtype`, which
/// [`observe_scan`] recorded as `observation`.
pub(crate) fn scan_heard(
    subtype: u16,
    to_us: bool,
    meta: &impl RxMetadata,
    observation: ScanObservation,
) -> crate::events::ScanHeard {
    crate::events::ScanHeard {
        own: !matches!(observation, ScanObservation::Ignored),
        probe_response: subtype == PROBE_RESPONSE,
        to_us,
        rssi: match meta.common().rssi_dbm {
            RxEvidence::HardwareObserved(rssi) | RxEvidence::ProtocolValidated(rssi) => Some(rssi),
            RxEvidence::Unavailable => None,
        },
    }
}

/// The Frame Control subtype of a probe response (IEEE 802.11-2020 9.2.4.1.3).
const PROBE_RESPONSE: u16 = 5;

#[cfg(test)]
mod tests;
