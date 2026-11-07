//! Validated A-MPDU frame geometry and rate-policy requests.

use crate::mac::tx::{HeEdcaTxopLimit, HeRate, HtAmpduDensity};

/// Encoded MPDU bytes and the trailer bytes appended by hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduFrameSize {
    mpdu_length: usize,
    hardware_mic_length: u8,
}

impl AmpduFrameSize {
    pub const fn new(mpdu_length: usize, hardware_mic_length: u8) -> Self {
        Self {
            mpdu_length,
            hardware_mic_length,
        }
    }

    pub const fn mpdu_length(self) -> usize {
        self.mpdu_length
    }

    pub const fn hardware_mic_length(self) -> u8 {
        self.hardware_mic_length
    }
}

/// Geometry of one encoded MPDU inside a retained DMA backing.
///
/// The offset points at the private A-MPDU metadata prefix immediately before
/// the encoded 802.11 frame. Construction proves the word alignment required
/// by the S31 TX descriptor path. This value describes a region; it does not
/// grant access to it. [`super::RetainedDmaAmpduTx`] remains the sole owner
/// allowed to resolve the offset inside a retained
/// [`oer_portable_memory::StableDmaBacking`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduFrameLayout {
    dma_offset: usize,
    frame_size: AmpduFrameSize,
}

impl AmpduFrameLayout {
    pub const fn new(dma_offset: usize, frame_size: AmpduFrameSize) -> Option<Self> {
        if dma_offset & 3 != 0 {
            return None;
        }
        Some(Self {
            dma_offset,
            frame_size,
        })
    }

    pub const fn dma_offset(self) -> usize {
        self.dma_offset
    }

    pub const fn mpdu_length(self) -> usize {
        self.frame_size.mpdu_length()
    }

    pub const fn hardware_mic_length(self) -> u8 {
        self.frame_size.hardware_mic_length()
    }

    pub const fn frame_size(self) -> AmpduFrameSize {
        self.frame_size
    }
}

/// Rate, delimiter-density and duration policy for one HE aggregate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeAmpduPolicy {
    rate: HeRate,
    density: HtAmpduDensity,
    txop_limit: HeEdcaTxopLimit,
}

impl HeAmpduPolicy {
    pub const fn new(rate: HeRate, density: HtAmpduDensity, txop_limit: HeEdcaTxopLimit) -> Self {
        Self {
            rate,
            density,
            txop_limit,
        }
    }

    pub const fn rate(self) -> HeRate {
        self.rate
    }

    pub const fn density(self) -> HtAmpduDensity {
        self.density
    }

    pub const fn txop_limit(self) -> HeEdcaTxopLimit {
        self.txop_limit
    }
}
