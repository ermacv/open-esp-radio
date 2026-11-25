//! Station power-management signalling on the IEEE 802.11 wire.
//!
//! This module constructs the Null Data frame by which an associated station
//! tells its access point that its Power Management mode changed and the
//! legacy PS-Poll control frame used to retrieve one buffered unicast MPDU.
//! It owns no timer, retry, sleep, MMIO or executor state. In particular,
//! merely encoding either frame never grants permission to stop the radio:
//! the runtime must retain the corresponding acknowledged exchange.

use crate::address::{Bssid, OwnAddress};
use crate::{sequence::SequenceNumber, station::StationFrameError};

/// Length of a legacy Null Data MPDU, excluding the hardware-owned FCS.
pub const STA_NULL_DATA_FRAME_LEN: usize = 24;

const NULL_DATA_TO_DS_FRAME_CONTROL: crate::frame::FrameControlFields =
    crate::frame::FrameControlFields::from_bytes([0x48, 0x01]);

/// Power-management state advertised by a station to its access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPowerManagement {
    /// The station remains continuously available to receive frames.
    Active,
    /// The access point must buffer traffic while the station sleeps.
    PowerSave,
}

/// Complete inputs for one station-originated legacy Null Data frame.
///
/// The address geometry is fixed by the To-DS data-frame form: Address 1 and
/// Address 3 are the BSSID, while Address 2 is the station transmitter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaNullDataFrame {
    pub station_address: OwnAddress,
    pub bssid: Bssid,
    pub sequence_number: SequenceNumber,
    pub power_management: StaPowerManagement,
}

impl StaNullDataFrame {
    /// Encode the MPDU without an FCS.
    ///
    /// This is the same standard frame shape exposed by Linux mac80211's
    /// `ieee80211_nullfunc_get`: Data/NullFunc + ToDS, BSSID/STA/BSSID address
    /// geometry, with the caller selecting the Power Management bit.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        let (frame, _) = output
            .split_first_chunk_mut::<STA_NULL_DATA_FRAME_LEN>()
            .ok_or(StationFrameError::OutputTooSmall {
                required: STA_NULL_DATA_FRAME_LEN,
            })?;
        frame.fill(0);
        let frame_control = NULL_DATA_TO_DS_FRAME_CONTROL.with_power_management(matches!(
            self.power_management,
            StaPowerManagement::PowerSave
        ));
        crate::encoding::write_array::<0, _, _>(frame, &frame_control.to_bytes());
        crate::encoding::write_array::<4, _, _>(frame, &self.bssid.get().octets());
        crate::encoding::write_array::<10, _, _>(frame, &self.station_address.get().octets());
        crate::encoding::write_array::<16, _, _>(frame, &self.bssid.get().octets());
        crate::encoding::write_array::<22, _, _>(
            frame,
            &self.sequence_number.sequence_control().to_bytes(),
        );
        Ok(STA_NULL_DATA_FRAME_LEN)
    }
}

#[cfg(test)]
mod tests;
