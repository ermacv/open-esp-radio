//! Enhanced-ACK based probing of Thread 1.2 Link Metrics, as a radio serves
//! it for OpenThread's Link Metrics subject: the probing initiators the
//! stack configures (`otPlatRadioConfigureEnhAckProbing`), the metrics an
//! enhanced ACK to one of them reports, and the vendor-specific header IE
//! that carries them (`link_metrics.cpp` and `mac_frame.cpp` of
//! OpenThread's platform utilities, which ESP-IDF's OpenThread port calls
//! from its enhanced-ACK generator at ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`).
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/openthread/src/port/esp_openthread_radio.c` (Copyright
//! Espressif Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!

/// Bytes of Link Metrics data an enhanced ACK carries at most
/// (`OT_ENH_PROBING_IE_DATA_MAX_SIZE`).
pub const ENH_ACK_PROBING_DATA_CAPACITY: usize = 2;

/// Bytes of the probing IE at most: the header IE descriptor, the Thread
/// OUI and subtype, then the data.
pub const ENH_ACK_PROBING_IE_CAPACITY: usize = 2 + 4 + ENH_ACK_PROBING_DATA_CAPACITY;

/// The vendor-specific header IE element identifier
/// (`VendorIeHeader::kHeaderIeId`).
const VENDOR_IE_ID: u16 = 0x00;

/// The Thread company OUI (`ThreadIe::kVendorOuiThreadCompanyId`).
const THREAD_OUI: u32 = 0x00ea_b89b;

/// The enhanced-ACK probing subtype of Thread IEs
/// (`ThreadIe::kEnhAckProbingIe`).
const ENH_ACK_PROBING_SUBTYPE: u8 = 0x00;

/// The Link Metrics data of one enhanced ACK, at most
/// [`ENH_ACK_PROBING_DATA_CAPACITY`] bytes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ProbingData {
    bytes: [u8; ENH_ACK_PROBING_DATA_CAPACITY],
    len: ProbingLength,
}

// At most two metrics are selected by construction, in protocol order.
// usize representation preserves ProbingData's existing storage layout.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(usize)]
enum ProbingLength {
    #[default]
    Empty,
    One,
    Two,
}

impl ProbingData {
    /// One already computed metric byte.
    pub const fn one(first: u8) -> Self {
        Self {
            bytes: [first, 0],
            len: ProbingLength::One,
        }
    }

    /// Two already computed metric bytes in their protocol order.
    pub const fn two(first: u8, second: u8) -> Self {
        Self {
            bytes: [first, second],
            len: ProbingLength::Two,
        }
    }

    /// The data bytes.
    pub fn bytes(&self) -> &[u8] {
        match self.len {
            ProbingLength::Empty => &[],
            ProbingLength::One => &self.bytes[..1],
            ProbingLength::Two => &self.bytes,
        }
    }

    /// Write the enhanced-ACK probing IE carrying the data into `dest`, as
    /// `otMacFrameGenerateEnhAckProbingIe` does, and return its length.
    /// Nothing is written, and zero returned, for empty data or a `dest`
    /// shorter than the IE.
    pub fn write_ie(&self, dest: &mut [u8]) -> usize {
        let [oui0, oui1, oui2, _] = THREAD_OUI.to_le_bytes();
        let [first, second] = self.bytes;
        match self.len {
            ProbingLength::Empty => 0,
            ProbingLength::One => {
                let Some((target, _)) = dest.split_first_chunk_mut::<7>() else {
                    return 0;
                };
                let [low, high] = (5_u16 | VENDOR_IE_ID << 7).to_le_bytes();
                *target = [low, high, oui0, oui1, oui2, ENH_ACK_PROBING_SUBTYPE, first];
                7
            }
            ProbingLength::Two => {
                let Some((target, _)) = dest.split_first_chunk_mut::<8>() else {
                    return 0;
                };
                let [low, high] = (6_u16 | VENDOR_IE_ID << 7).to_le_bytes();
                *target = [
                    low,
                    high,
                    oui0,
                    oui1,
                    oui2,
                    ENH_ACK_PROBING_SUBTYPE,
                    first,
                    second,
                ];
                8
            }
        }
    }
}

#[cfg(test)]
mod tests;
