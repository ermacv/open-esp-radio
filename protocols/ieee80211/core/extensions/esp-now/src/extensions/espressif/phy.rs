//! Espressif on-air PHY requests of an ESP-NOW peer.

/// Requested Espressif Long Range on-air rate, not a chip register value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspressifLongRangeRate {
    /// 250 kb/s LR modulation.
    Kbps250,
    /// 500 kb/s LR modulation.
    Kbps500,
}

/// One standard OFDM rate explicitly selected for an ESP-NOW peer.
///
/// The values are kept portable instead of exposing a chip MAC rate byte.
/// A backend may support only a subset, but must reject an unsupported value
/// before publishing a frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowOfdmRate {
    /// 6 Mb/s OFDM.
    Mbps6,
    /// 9 Mb/s OFDM.
    Mbps9,
    /// 12 Mb/s OFDM.
    Mbps12,
    /// 18 Mb/s OFDM.
    Mbps18,
    /// 24 Mb/s OFDM.
    Mbps24,
    /// 36 Mb/s OFDM.
    Mbps36,
    /// 48 Mb/s OFDM.
    Mbps48,
    /// 54 Mb/s OFDM.
    Mbps54,
}

/// One-spatial-stream MCS for the standard ESP-NOW HT20 profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowHtMcs {
    /// HT MCS0, one spatial stream.
    Mcs0,
    /// HT MCS1, one spatial stream.
    Mcs1,
    /// HT MCS2, one spatial stream.
    Mcs2,
    /// HT MCS3, one spatial stream.
    Mcs3,
    /// HT MCS4, one spatial stream.
    Mcs4,
    /// HT MCS5, one spatial stream.
    Mcs5,
    /// HT MCS6, one spatial stream.
    Mcs6,
    /// HT MCS7, one spatial stream.
    Mcs7,
}

impl EspNowHtMcs {
    /// Standard MCS index in the range zero through seven.
    pub const fn index(self) -> u8 {
        match self {
            Self::Mcs0 => 0,
            Self::Mcs1 => 1,
            Self::Mcs2 => 2,
            Self::Mcs3 => 3,
            Self::Mcs4 => 4,
            Self::Mcs5 => 5,
            Self::Mcs6 => 6,
            Self::Mcs7 => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Guard interval of a standard ESP-NOW HT20 PPDU.
pub enum EspNowHtGuardInterval {
    /// 800 ns guard interval.
    Long800Ns,
    /// 400 ns guard interval.
    Short400Ns,
}

/// A complete standard HT20 rate requested for one ESP-NOW peer.
///
/// MCS32 is deliberately absent: duplicate mode has a distinct hardware
/// certification boundary and cannot be represented as a one-stream MCS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowHt20Rate {
    mcs: EspNowHtMcs,
    guard_interval: EspNowHtGuardInterval,
}

impl EspNowHt20Rate {
    /// Combine the admitted MCS and guard interval.
    pub const fn new(mcs: EspNowHtMcs, guard_interval: EspNowHtGuardInterval) -> Self {
        Self {
            mcs,
            guard_interval,
        }
    }

    /// The requested single-stream MCS.
    pub const fn mcs(self) -> EspNowHtMcs {
        self.mcs
    }

    /// The requested guard interval.
    pub const fn guard_interval(self) -> EspNowHtGuardInterval {
        self.guard_interval
    }
}

/// PHY policy requested for one peer.
///
/// The caller selects the attempts for the requested standard peer PHY. A
/// backend must reject `LongRange` unless it owns
/// the complete LR enable, PLCP, rate and receive-status contract. Merely
/// possessing recovered LR rate schedule bytes is not sufficient.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EspNowPhyMode {
    #[default]
    /// Legacy DSSS at 1 Mb/s.
    LegacyDsss1M,
    /// Standard OFDM with the dedicated P2P retry record.
    StandardP2pOfdm(EspNowOfdmRate),
    /// Standard HT20 with the dedicated P2P retry record.
    StandardP2pHt20(EspNowHt20Rate),
    /// Espressif LR; requires complete backend LR support.
    LongRange(EspressifLongRangeRate),
}
