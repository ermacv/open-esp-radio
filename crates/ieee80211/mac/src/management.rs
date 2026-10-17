//! Allocation-free IEEE 802.11 management-frame construction.

use crate::sequence::SequenceNumber;

pub const MANAGEMENT_HEADER_LEN: usize = 24;
pub const MAX_SSID_LEN: usize = 32;
pub const MAX_SUPPORTED_RATES_LEN: usize = 24;

const PROBE_REQUEST_FRAME_CONTROL: u16 = 0x0040;
pub const BROADCAST_ADDRESS: [u8; 6] = [0xff; 6];
const SSID_ELEMENT_ID: u8 = 0;
const SUPPORTED_RATES_ELEMENT_ID: u8 = 1;
const EXTENDED_SUPPORTED_RATES_ELEMENT_ID: u8 = 50;
const SUPPORTED_RATES_ELEMENT_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeRequestError {
    SsidTooLong,
    NoSupportedRates,
    TooManySupportedRates,
    OutputTooSmall { required: usize },
}

/// A borrowed Probe Request description.
///
/// The lifetime ties `ssid` and `supported_rates` to their caller-owned
/// storage. Encoding copies the finished frame into caller-owned DMA-capable
/// memory, so this type needs neither allocation nor global buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeRequest<'a> {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub bssid: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub ssid: &'a [u8],
    pub supported_rates: &'a [u8],
}

impl ProbeRequest<'_> {
    /// Encode a wildcard or directed Probe Request into `output`.
    ///
    /// `sequence_number` is the 12-bit 802.11 sequence number; the fragment
    /// number is emitted as zero. Rate bytes use the standard 500 kbit/s
    /// representation, including any basic-rate bit supplied by the caller.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ProbeRequestError> {
        if self.ssid.len() > MAX_SSID_LEN {
            return Err(ProbeRequestError::SsidTooLong);
        }
        if self.supported_rates.is_empty() {
            return Err(ProbeRequestError::NoSupportedRates);
        }
        if self.supported_rates.len() > MAX_SUPPORTED_RATES_LEN {
            return Err(ProbeRequestError::TooManySupportedRates);
        }

        let first_rates_len = self
            .supported_rates
            .len()
            .min(SUPPORTED_RATES_ELEMENT_CAPACITY);
        let extended_rates_len = self.supported_rates.len() - first_rates_len;
        let mut header = [0; MANAGEMENT_HEADER_LEN];
        header[0..2].copy_from_slice(&PROBE_REQUEST_FRAME_CONTROL.to_le_bytes());
        header[4..10].copy_from_slice(&self.destination);
        header[10..16].copy_from_slice(&self.source);
        header[16..22].copy_from_slice(&self.bssid);
        header[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        let extended_header = [
            EXTENDED_SUPPORTED_RATES_ELEMENT_ID,
            extended_rates_len as u8,
        ];
        crate::encoding::FrameParts([
            header.as_slice(),
            &[SSID_ELEMENT_ID, self.ssid.len() as u8],
            self.ssid,
            &[SUPPORTED_RATES_ELEMENT_ID, first_rates_len as u8],
            &self.supported_rates[..first_rates_len],
            if extended_rates_len == 0 {
                &[]
            } else {
                &extended_header
            },
            &self.supported_rates[first_rates_len..],
        ])
        .write(output)
        .map_err(|required| ProbeRequestError::OutputTooSmall { required })
    }
}

#[cfg(test)]
mod tests;
