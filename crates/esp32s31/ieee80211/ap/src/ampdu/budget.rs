//! Prospective AP HT length admission before DMA promotion and CCMP encoding.

use oer_esp32s31_ieee80211_mac::tx::ampdu::{HtAmpduLengthAccumulator, HtAmpduLengthError};

use oer_ieee80211_mac::ap::AP_PROTECTED_QOS_ETHERNET_OVERHEAD;

use super::*;

/// Value-only prefix budget. A refusal leaves it unchanged, so the caller can
/// retain the unencoded Ethernet owner at the front of its next exchange.
pub struct ApAmpduBudget {
    length: HtAmpduLengthAccumulator,
}

impl ApAmpduBudget {
    /// Intersect radio geometry with a caller's modelled exchange limit.
    /// This is a prospective software budget, not a hardware reconfiguration.
    /// Zero refuses any new aggregate; an existing prefix must still fit.
    pub fn cap_bytes(&mut self, maximum_bytes: u16) -> Result<(), ApAmpduError> {
        self.length
            .cap_bytes(maximum_bytes)
            .map_err(|error| HtAmpduTxError::Length(error).into())
    }

    pub fn admit_ethernet(&mut self, ethernet: &[u8]) -> Result<bool, ApAmpduError> {
        self.admit_ethernet_len(ethernet.len())
    }

    /// Account prospective geometry from queue metadata, without borrowing or
    /// claiming its packet. This estimates only the protected QoS format used
    /// by this A-MPDU builder; it does not authorize a peer/key or reserve DMA.
    /// Revalidate the actual packet if the queue changes before its claim.
    pub fn admit_ethernet_len(&mut self, ethernet_bytes: usize) -> Result<bool, ApAmpduError> {
        if ethernet_bytes < oer_ieee80211_mac::data::ETHERNET_HEADER_LEN {
            return Err(ApAmpduError::Geometry);
        }
        let psdu = ethernet_bytes
            .checked_add(AP_PROTECTED_QOS_ETHERNET_OVERHEAD)
            .and_then(|length| {
                length.checked_add(oer_esp32s31_ieee80211::ordinary_tx::TX_CCMP_MIC_SIZE)
            })
            .and_then(|length| length.checked_add(4)) // MAC-generated FCS
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length <= 0x3fff)
            .ok_or(ApAmpduError::Geometry)?;
        match self.length.push(u32::from(psdu), 0) {
            Ok(()) => Ok(true),
            Err(HtAmpduLengthError::AggregateTooLong(_) | HtAmpduLengthError::WindowFull) => {
                Ok(false)
            }
            Err(error) => Err(HtAmpduTxError::Length(error).into()),
        }
    }
}

impl<'storage, B: StableDmaBacking + 'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
    ApAmpduTx<'storage, B, SLOTS, BUFFER_SIZE>
{
    /// Inspect a free or building owner before preparing a finite burst.
    /// This does not reserve DMA capacity or replace per-frame peer/key checks.
    pub fn length_budget(&self, rate: HtRate) -> Result<ApAmpduBudget, ApAmpduError> {
        match self.state {
            ApAmpduState::Idle => {}
            ApAmpduState::Building { rate: current, .. } if rate == current => {}
            _ => return Err(ApAmpduError::Busy),
        }
        Ok(ApAmpduBudget {
            length: self.inner.ht_length_budget(rate)?,
        })
    }
}

#[cfg(test)]
mod tests;
