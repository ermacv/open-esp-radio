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

use crate::mac::header::FrameAddress;

/// The metrics an initiator asked for (`otLinkMetrics`).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct LinkMetrics {
    /// The count of frames received (`mPduCount`); never part of an
    /// enhanced ACK.
    pub pdu_count: bool,
    /// The link quality of the acknowledged frame (`mLqi`).
    pub lqi: bool,
    /// The link margin of the acknowledged frame (`mLinkMargin`).
    pub link_margin: bool,
    /// The RSSI of the acknowledged frame (`mRssi`).
    pub rssi: bool,
}

impl LinkMetrics {
    /// No metric: configuring it removes an initiator.
    pub const NONE: Self = Self {
        pdu_count: false,
        lqi: false,
        link_margin: false,
        rssi: false,
    };

    const fn is_clear(self) -> bool {
        !self.pdu_count && !self.lqi && !self.link_margin && !self.rssi
    }
}

/// Why a probing configuration changed nothing
/// (`otLinkMetricsConfigureEnhAckProbing`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProbingError {
    /// Clearing an initiator the table does not hold (`OT_ERROR_NOT_FOUND`).
    NotFound,
    /// The table has no room for another initiator (`OT_ERROR_NO_BUFS`).
    NoBufs,
}

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

/// The RSSI OpenThread treats as invalid (`Radio::kInvalidRssi`).
const INVALID_RSSI: i8 = 127;

/// The Link Metrics data of one enhanced ACK, at most
/// [`ENH_ACK_PROBING_DATA_CAPACITY`] bytes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ProbingData {
    bytes: [u8; ENH_ACK_PROBING_DATA_CAPACITY],
    len: usize,
}

impl ProbingData {
    /// The data bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn push(&mut self, byte: u8) {
        self.bytes[self.len] = byte;
        self.len += 1;
    }

    /// Write the enhanced-ACK probing IE carrying the data into `dest`, as
    /// `otMacFrameGenerateEnhAckProbingIe` does, and return its length.
    /// Nothing is written, and zero returned, for empty data or a `dest`
    /// shorter than the IE.
    pub fn write_ie(&self, dest: &mut [u8]) -> usize {
        let content = 4 + self.len;
        let length = 2 + content;
        if self.len == 0 || dest.len() < length {
            return 0;
        }
        let descriptor = content as u16 | VENDOR_IE_ID << 7;
        dest[..2].copy_from_slice(&descriptor.to_le_bytes());
        dest[2..5].copy_from_slice(&THREAD_OUI.to_le_bytes()[..3]);
        dest[5] = ENH_ACK_PROBING_SUBTYPE;
        dest[6..length].copy_from_slice(self.bytes());
        length
    }
}

/// The link margin of a frame received at `rssi` over `noise_floor`
/// (`ComputeLinkMargin`): zero below the floor or for an invalid RSSI, the
/// difference wrapping in eight signed bits as in OpenThread.
pub const fn link_margin(noise_floor: i8, rssi: i8) -> u8 {
    let margin = rssi.wrapping_sub(noise_floor);
    if margin < 0 || rssi == INVALID_RSSI {
        0
    } else {
        margin as u8
    }
}

/// One probing initiator as a stack configures it
/// (`otPlatRadioConfigureEnhAckProbing`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProbingInitiator {
    /// The initiator's short address.
    pub short_address: u16,
    /// The initiator's extended address in frame byte order.
    pub extended_address: [u8; 8],
    /// The metrics its enhanced ACKs carry.
    pub metrics: LinkMetrics,
}

/// One probing initiator (`LinkMetricsDataInfo`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Initiator {
    metrics: LinkMetrics,
    short: u16,
    extended: [u8; 8],
}

impl Initiator {
    /// `GetEnhAckData`: the link quality, then the link margin scaled from
    /// [0, 130] and the RSSI scaled from [-130, 0] to [0, 255], at most two
    /// bytes, in that fixed order.
    fn data(&self, lqi: u8, rssi: i8, noise_floor: i8) -> ProbingData {
        let mut data = ProbingData::default();
        if self.metrics.lqi {
            data.push(lqi);
        }
        if self.metrics.link_margin {
            data.push((i32::from(link_margin(noise_floor, rssi)) * 255 / 130) as u8);
        }
        if data.len < ENH_ACK_PROBING_DATA_CAPACITY && self.metrics.rssi {
            data.push(((i32::from(rssi) + 130) * 255 / 130) as u8);
        }
        data
    }
}

/// The probing initiators of a Link Metrics subject, at most `N`
/// (`OPENTHREAD_CONFIG_MLE_LINK_METRICS_MAX_SERIES_SUPPORTED`), and the
/// noise floor link margins are measured from (`otLinkMetricsInit`).
///
/// An initiator is keyed by its short address and matched by either
/// address; the most recently added initiator matches first, as in
/// OpenThread's list.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EnhAckProbing<const N: usize> {
    initiators: [Option<Initiator>; N],
    noise_floor: i8,
}

impl<const N: usize> Default for EnhAckProbing<N> {
    fn default() -> Self {
        Self::new(0)
    }
}

impl<const N: usize> EnhAckProbing<N> {
    /// An empty table measuring link margins from `noise_floor` dBm.
    pub const fn new(noise_floor: i8) -> Self {
        Self {
            initiators: [None; N],
            noise_floor,
        }
    }

    /// The noise floor in dBm.
    pub const fn noise_floor(&self) -> i8 {
        self.noise_floor
    }

    /// Set the noise floor in dBm.
    pub fn set_noise_floor(&mut self, noise_floor: i8) {
        self.noise_floor = noise_floor;
    }

    /// `otLinkMetricsConfigureEnhAckProbing`: probe the initiator with the
    /// short address `short` and the extended address `extended`, in frame
    /// byte order, for `metrics`, replacing its earlier metrics;
    /// [`LinkMetrics::NONE`] removes it.
    ///
    /// # Errors
    ///
    /// Removing an absent initiator, or adding one to a full table.
    pub fn configure(
        &mut self,
        short: u16,
        extended: [u8; 8],
        metrics: LinkMetrics,
    ) -> Result<(), ProbingError> {
        let position = self
            .initiators
            .iter()
            .position(|entry| entry.is_some_and(|initiator| initiator.short == short));
        if metrics.is_clear() {
            let position = position.ok_or(ProbingError::NotFound)?;
            self.initiators[position..].rotate_left(1);
            self.initiators[N - 1] = None;
            return Ok(());
        }
        let initiator = Initiator {
            metrics,
            short,
            extended,
        };
        if let Some(position) = position {
            self.initiators[position] = Some(initiator);
            return Ok(());
        }
        if self.initiators.last().is_none_or(Option::is_some) {
            return Err(ProbingError::NoBufs);
        }
        self.initiators.rotate_right(1);
        self.initiators[0] = Some(initiator);
        Ok(())
    }

    /// `otLinkMetricsResetEnhAckProbing`: remove every initiator.
    pub fn reset(&mut self) {
        self.initiators = [None; N];
    }

    /// Replace every initiator with `initiators`, listed most recently
    /// added first as `otPlatRadioConfigureEnhAckProbing` built its table,
    /// keeping the noise floor. Configuring them oldest first keeps the
    /// newest matching first; an initiator the table has no room for is
    /// left out.
    pub fn replace(&mut self, initiators: &[ProbingInitiator]) {
        self.reset();
        for initiator in initiators.iter().rev() {
            let _ = self.configure(
                initiator.short_address,
                initiator.extended_address,
                initiator.metrics,
            );
        }
    }

    /// The metrics probed for the initiator at `source`, if any.
    pub fn metrics(&self, source: FrameAddress) -> Option<LinkMetrics> {
        self.find(source).map(|initiator| initiator.metrics)
    }

    /// `otLinkMetricsEnhAckGenData`: the Link Metrics data of an enhanced
    /// ACK to a frame from `source` received with `lqi` and `rssi`; empty
    /// when `source` is no initiator.
    pub fn data(&self, source: FrameAddress, lqi: u8, rssi: i8) -> ProbingData {
        self.find(source)
            .map(|initiator| initiator.data(lqi, rssi, self.noise_floor))
            .unwrap_or_default()
    }

    fn find(&self, source: FrameAddress) -> Option<&Initiator> {
        self.initiators
            .iter()
            .flatten()
            .find(|initiator| match source {
                FrameAddress::Short(short) => initiator.short == u16::from_le_bytes(short),
                FrameAddress::Extended(extended) => initiator.extended == extended,
            })
    }
}

#[cfg(test)]
mod tests;
