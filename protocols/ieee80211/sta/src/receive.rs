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

use oer_ieee80211_lower_mac::{RxEvidence, RxMetadata};
use oer_ieee80211_mac::{
    frame::DecodedHeader,
    receive::RxDropReason,
    scan::{ScanObservation, ScanTable},
};
use oer_time::Instant;

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

/// Record `management`, received at `now`, in `scan`.
///
/// A record without a DS Parameter Set element takes the channel the
/// receiver was configured to. A frame without a received signal level is
/// recorded with `i8::MIN`.
pub fn observe_scan<const N: usize>(
    scan: &mut ScanTable<N>,
    now: Instant,
    management: Management<'_>,
    meta: &impl RxMetadata,
) -> ScanObservation {
    let common = meta.common();
    let rssi = match common.rssi_dbm {
        RxEvidence::HardwareObserved(rssi) | RxEvidence::ProtocolValidated(rssi) => rssi,
        RxEvidence::Unavailable => i8::MIN,
    };
    scan.observe_management(management.frame(), common.channel.number(), rssi, Some(now))
}

#[cfg(test)]
mod tests;
