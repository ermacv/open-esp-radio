//! Protection inputs and decisions shared by upper-MAC and the family policy.
use core::num::NonZeroU16;
use oer_ieee80211_port::Protection;
use oer_ieee80211_wire::{
    phy::{LegacyRate, PhyRate},
    protection::{ErpProtection, HtProtectionMode},
    rates::{LEGACY_RATES, LegacyRateSet, RateSelection},
};

/// Finite HE TXOP Duration RTS Threshold in the element's native 32-µs
/// units.
///
/// Encodings zero and 1023 disable the rule and do not construct this type.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HeTxopDurationRtsThreshold(NonZeroU16);

impl HeTxopDurationRtsThreshold {
    /// Admit an enabled threshold in the range 1 through 1022.
    pub const fn new(units_32_us: u16) -> Option<Self> {
        if units_32_us >= 0x03ff {
            return None;
        }
        match NonZeroU16::new(units_32_us) {
            Some(units) => Some(Self(units)),
            None => None,
        }
    }

    /// Threshold in native 32 microsecond units.
    pub const fn units_32_us(self) -> u16 {
        self.0.get()
    }
}

/// The peer's nominal packet padding (HE Capabilities).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HePacketPadding {
    #[default]
    /// No additional packet padding or protection exchange.
    None,
    /// Eight microseconds of nominal packet padding.
    Us8,
    /// Sixteen microseconds of nominal packet padding.
    Us16,
}

impl HePacketPadding {
    /// Nominal packet padding in microseconds.
    pub const fn micros(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Us8 => 8,
            Self::Us16 => 16,
        }
    }
}

/// Non-HT rates in one BSSBasicRateSet.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BasicRates(LegacyRateSet);

impl BasicRates {
    /// No rate.
    pub const EMPTY: Self = Self(LegacyRateSet::EMPTY);

    /// Rates that every ERP station supports when a BSS names no usable
    /// rate: 1, 2, 5.5, 11, 6, 12 and 24 Mb/s.
    pub const ERP_MANDATORY: Self = Self(LegacyRateSet::from_bits(0b0000_0001_0101_1111));

    /// Lowest mandatory ERP control rate.
    pub const ERP_MANDATORY_HEAD: LegacyRate = {
        let [(_, first), ..] = LEGACY_RATES;
        first
    };
}

// The mandatory set's first entry is present and eligible both with and
// without ERP protection, so its iterator has this head before skip(1).
const _: [(); 1] = [(); (BasicRates::ERP_MANDATORY.0.bits() & 1 != 0
    && matches!(BasicRates::ERP_MANDATORY_HEAD, LegacyRate::Dsss1M))
    as usize];

impl BasicRates {
    /// Collect rates marked basic in Supported Rates and Extended Supported
    /// Rates element bodies.
    pub fn from_rate_elements(supported: &[u8], extended: &[u8]) -> Self {
        Self(LegacyRateSet::from_rate_elements(
            supported,
            extended,
            RateSelection::Basic,
        ))
    }

    /// Whether no requirement or rate is present.
    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// The rates of the set, each with its long preamble.
    pub fn rates(self) -> impl Iterator<Item = LegacyRate> {
        self.0.rates()
    }
}

/// Protection facts advertised by the BSS that one PPDU belongs to.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BssProtection {
    /// ERP protection requirements advertised by the BSS.
    pub erp: ErpProtection,
    /// HT protection mode advertised by the BSS.
    pub ht: HtProtectionMode,
    /// HE Operation TXOP Duration RTS Threshold of the BSS.
    pub he_txop_rts_threshold: Option<HeTxopDurationRtsThreshold>,
    /// Peer nominal packet padding, fixed at association.
    pub he_packet_padding: HePacketPadding,
    /// Advertised BSS basic rate set.
    pub basic_rates: BasicRates,
    /// Capability Information Short Preamble.
    pub short_preamble: bool,
}

impl BssProtection {
    /// A BSS without protection requirements whose basic rates are unknown.
    pub const UNPROTECTED: Self = Self {
        erp: ErpProtection::NONE,
        ht: HtProtectionMode::None,
        he_txop_rts_threshold: None,
        he_packet_padding: HePacketPadding::None,
        basic_rates: BasicRates::EMPTY,
        short_preamble: false,
    };
}

/// Local dot11RTSThreshold in PSDU bytes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RtsLengthThreshold(u16);

impl RtsLengthThreshold {
    /// Construct the local dot11RTSThreshold in PSDU octets.
    pub const fn new(bytes: u16) -> Self {
        Self(bytes)
    }

    /// Threshold in PSDU octets.
    pub const fn bytes(self) -> u16 {
        self.0
    }
}

/// Receiver class taken from the MPDU Address 1 field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxReceiver {
    /// Unicast receiver.
    Individual,
    /// Multicast or broadcast receiver.
    Group,
}

impl TxReceiver {
    /// Classify the receiver address (RA). The Ethernet destination of a
    /// To-DS frame is Address 3 and does not select the exchange.
    pub const fn from_address1(address1: &[u8; 6]) -> Self {
        if address1[0] & 1 != 0 {
            Self::Group
        } else {
            Self::Individual
        }
    }
}

/// The PSDU carried by one PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxPsdu {
    /// One MPDU; `length` includes MIC and FCS.
    Mpdu {
        #[doc = "Complete PSDU length in octets, including MIC and FCS."]
        length: u32,
    },
    /// One A-MPDU; `length` is the complete aggregate, the HE APEP length.
    Ampdu {
        #[doc = "Complete PSDU length in octets, including MIC and FCS."]
        length: u32,
    },
}

impl TxPsdu {
    /// Complete PSDU length in octets, including MIC and FCS.
    pub const fn length(self) -> u32 {
        match self {
            Self::Mpdu { length } | Self::Ampdu { length } => length,
        }
    }
}

/// One PPDU presented for protection selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtectedPpdu {
    /// PHY rate of the PPDU or preceding control frame.
    pub rate: PhyRate,
    /// Receiver class of Address 1.
    pub receiver: TxReceiver,
    /// Payload form and complete PSDU length.
    pub psdu: TxPsdu,
}

/// Why one PPDU is protected; several sources can apply at once.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxProtectionReasons(u8);

impl TxProtectionReasons {
    /// ERP protection applies.
    pub const ERP: Self = Self(1 << 0);
    /// HT protection applies.
    pub const HT: Self = Self(1 << 1);
    /// HE duration exceeds the advertised threshold.
    pub const HE_TXOP_DURATION: Self = Self(1 << 2);
    /// PSDU length exceeds the local RTS threshold.
    pub const LENGTH: Self = Self(1 << 3);

    /// Whether all supplied reasons are present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no requirement or rate is present.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Include a reason when its requirement applies.
    pub const fn with(self, other: Self, present: bool) -> Self {
        if present {
            Self(self.0 | other.0)
        } else {
            self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Selected exchange together with every applicable requirement.
pub struct TxProtectionDecision {
    /// Control exchange selected by the profile.
    pub protection: Protection,
    /// Requirements that selected this exchange.
    pub reasons: TxProtectionReasons,
}

impl TxProtectionDecision {
    /// No preceding control exchange or applicable reason.
    pub const UNPROTECTED: Self = Self {
        protection: Protection::None,
        reasons: TxProtectionReasons(0),
    };
}
