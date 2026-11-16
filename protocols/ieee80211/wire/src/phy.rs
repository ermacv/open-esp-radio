//! Portable IEEE 802.11 PHY rates.
//!
//! [`PhyRate`] names the PPDU format and modulation of one transmission or
//! reception: a non-HT rate, an HT MCS or an HE SU MCS with the parameters
//! that select it. It is what a radio port submits and reports; each backend
//! lowers it into its own rate codes and refuses what its PHY cannot send.
//! Every constructor validates the combination, so a held value is a rate
//! IEEE 802.11-2020/802.11ax-2021 defines.

/// Preamble of a DSSS or CCK PPDU (IEEE 802.11-2020 16.2.2).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DsssPreamble {
    Long,
    Short,
}

/// Non-HT rate: DSSS, CCK (clause 16) or OFDM/ERP-OFDM (clauses 17, 18).
///
/// 1 Mb/s has only the long preamble; the short preamble's header is sent at
/// 2 Mb/s.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LegacyRate {
    Dsss1M,
    Dsss2M(DsssPreamble),
    Cck5M5(DsssPreamble),
    Cck11M(DsssPreamble),
    Ofdm6M,
    Ofdm9M,
    Ofdm12M,
    Ofdm18M,
    Ofdm24M,
    Ofdm36M,
    Ofdm48M,
    Ofdm54M,
}

impl LegacyRate {
    /// The data rate in kb/s.
    pub const fn kbps(self) -> u32 {
        match self {
            Self::Dsss1M => 1_000,
            Self::Dsss2M(_) => 2_000,
            Self::Cck5M5(_) => 5_500,
            Self::Cck11M(_) => 11_000,
            Self::Ofdm6M => 6_000,
            Self::Ofdm9M => 9_000,
            Self::Ofdm12M => 12_000,
            Self::Ofdm18M => 18_000,
            Self::Ofdm24M => 24_000,
            Self::Ofdm36M => 36_000,
            Self::Ofdm48M => 48_000,
            Self::Ofdm54M => 54_000,
        }
    }

    /// Whether the rate is OFDM rather than DSSS or CCK.
    pub const fn is_ofdm(self) -> bool {
        !matches!(
            self,
            Self::Dsss1M | Self::Dsss2M(_) | Self::Cck5M5(_) | Self::Cck11M(_)
        )
    }
}

/// Bandwidth of one PPDU.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PpduBandwidth {
    Mhz20,
    Mhz40,
    Mhz80,
    /// 160 MHz or 80+80 MHz.
    Mhz160,
}

impl PpduBandwidth {
    pub const fn mhz(self) -> u16 {
        match self {
            Self::Mhz20 => 20,
            Self::Mhz40 => 40,
            Self::Mhz80 => 80,
            Self::Mhz160 => 160,
        }
    }
}

/// HT MCS index 0 through 31 (IEEE 802.11-2020 19.5): the equal-modulation
/// MCSs of one to four spatial streams. MCS 32 (duplicate) and the
/// unequal-modulation MCSs 33-76 are not ordinary rates of this type.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HtMcs(u8);

impl HtMcs {
    /// The highest HT MCS using one spatial stream.
    pub const SINGLE_STREAM_MAX: Self = Self(7);

    pub const fn new(index: u8) -> Option<Self> {
        if index <= 31 { Some(Self(index)) } else { None }
    }

    pub const fn index(self) -> u8 {
        self.0
    }

    /// The spatial streams the MCS uses.
    pub const fn spatial_streams(self) -> u8 {
        match self.0 {
            0..=7 => 1,
            8..=15 => 2,
            16..=23 => 3,
            _ => 4,
        }
    }
}

/// The guard interval of an HT PPDU: 800 ns, or the short 400 ns one
/// (IEEE Std 802.11-2020, 19.3.9.4.6, the Short GI field of HT-SIG).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HtGuardInterval {
    Normal,
    Short,
}

/// Whether an HE SU PPDU uses dual carrier modulation (IEEE 802.11ax-2021,
/// 27.3.12.10, the DCM field of HE-SIG-A).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Dcm {
    Used,
    NotUsed,
}

/// One HT rate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HtRate {
    mcs: HtMcs,
    bandwidth: PpduBandwidth,
    short_gi: bool,
}

impl HtRate {
    /// An HT rate; `None` for a bandwidth HT does not have (80 or 160 MHz).
    pub const fn new(
        mcs: HtMcs,
        bandwidth: PpduBandwidth,
        guard_interval: HtGuardInterval,
    ) -> Option<Self> {
        match bandwidth {
            PpduBandwidth::Mhz20 | PpduBandwidth::Mhz40 => Some(Self {
                mcs,
                bandwidth,
                short_gi: matches!(guard_interval, HtGuardInterval::Short),
            }),
            PpduBandwidth::Mhz80 | PpduBandwidth::Mhz160 => None,
        }
    }

    pub const fn mcs(self) -> HtMcs {
        self.mcs
    }

    pub const fn bandwidth(self) -> PpduBandwidth {
        self.bandwidth
    }

    /// 400 ns rather than 800 ns guard interval.
    pub const fn short_gi(self) -> bool {
        self.short_gi
    }

    /// The data rate in kb/s: the one-stream rate of IEEE Std 802.11-2020
    /// Tables 19-27 and 19-28, rounded to 100 kb/s as the tables print it,
    /// times the spatial streams.
    pub const fn nominal_kbps(self) -> u32 {
        let (long, short, _) = self.parameters();
        if self.short_gi { short } else { long }
    }

    /// Tables 19-27/19-28: long/short GI rates and exact N_DBPS.
    const fn parameters(self) -> (u32, u32, u64) {
        macro_rules! rate {
            ($long:literal, $short:literal, $streams:literal) => {
                const {
                    (
                        $long * $streams,
                        $short * $streams,
                        $long * $streams * 4 / 1_000,
                    )
                }
            };
        }
        match (self.bandwidth, self.mcs.index()) {
            (PpduBandwidth::Mhz40, 0) => rate!(13_500, 15_000, 1),
            (PpduBandwidth::Mhz40, 1) => rate!(27_000, 30_000, 1),
            (PpduBandwidth::Mhz40, 2) => rate!(40_500, 45_000, 1),
            (PpduBandwidth::Mhz40, 3) => rate!(54_000, 60_000, 1),
            (PpduBandwidth::Mhz40, 4) => rate!(81_000, 90_000, 1),
            (PpduBandwidth::Mhz40, 5) => rate!(108_000, 120_000, 1),
            (PpduBandwidth::Mhz40, 6) => rate!(121_500, 135_000, 1),
            (PpduBandwidth::Mhz40, 7) => rate!(135_000, 150_000, 1),
            (PpduBandwidth::Mhz40, 8) => rate!(13_500, 15_000, 2),
            (PpduBandwidth::Mhz40, 9) => rate!(27_000, 30_000, 2),
            (PpduBandwidth::Mhz40, 10) => rate!(40_500, 45_000, 2),
            (PpduBandwidth::Mhz40, 11) => rate!(54_000, 60_000, 2),
            (PpduBandwidth::Mhz40, 12) => rate!(81_000, 90_000, 2),
            (PpduBandwidth::Mhz40, 13) => rate!(108_000, 120_000, 2),
            (PpduBandwidth::Mhz40, 14) => rate!(121_500, 135_000, 2),
            (PpduBandwidth::Mhz40, 15) => rate!(135_000, 150_000, 2),
            (PpduBandwidth::Mhz40, 16) => rate!(13_500, 15_000, 3),
            (PpduBandwidth::Mhz40, 17) => rate!(27_000, 30_000, 3),
            (PpduBandwidth::Mhz40, 18) => rate!(40_500, 45_000, 3),
            (PpduBandwidth::Mhz40, 19) => rate!(54_000, 60_000, 3),
            (PpduBandwidth::Mhz40, 20) => rate!(81_000, 90_000, 3),
            (PpduBandwidth::Mhz40, 21) => rate!(108_000, 120_000, 3),
            (PpduBandwidth::Mhz40, 22) => rate!(121_500, 135_000, 3),
            (PpduBandwidth::Mhz40, 23) => rate!(135_000, 150_000, 3),
            (PpduBandwidth::Mhz40, 24) => rate!(13_500, 15_000, 4),
            (PpduBandwidth::Mhz40, 25) => rate!(27_000, 30_000, 4),
            (PpduBandwidth::Mhz40, 26) => rate!(40_500, 45_000, 4),
            (PpduBandwidth::Mhz40, 27) => rate!(54_000, 60_000, 4),
            (PpduBandwidth::Mhz40, 28) => rate!(81_000, 90_000, 4),
            (PpduBandwidth::Mhz40, 29) => rate!(108_000, 120_000, 4),
            (PpduBandwidth::Mhz40, 30) => rate!(121_500, 135_000, 4),
            (PpduBandwidth::Mhz40, _) => rate!(135_000, 150_000, 4),
            (_, 0) => rate!(6_500, 7_200, 1),
            (_, 1) => rate!(13_000, 14_400, 1),
            (_, 2) => rate!(19_500, 21_700, 1),
            (_, 3) => rate!(26_000, 28_900, 1),
            (_, 4) => rate!(39_000, 43_300, 1),
            (_, 5) => rate!(52_000, 57_800, 1),
            (_, 6) => rate!(58_500, 65_000, 1),
            (_, 7) => rate!(65_000, 72_200, 1),
            (_, 8) => rate!(6_500, 7_200, 2),
            (_, 9) => rate!(13_000, 14_400, 2),
            (_, 10) => rate!(19_500, 21_700, 2),
            (_, 11) => rate!(26_000, 28_900, 2),
            (_, 12) => rate!(39_000, 43_300, 2),
            (_, 13) => rate!(52_000, 57_800, 2),
            (_, 14) => rate!(58_500, 65_000, 2),
            (_, 15) => rate!(65_000, 72_200, 2),
            (_, 16) => rate!(6_500, 7_200, 3),
            (_, 17) => rate!(13_000, 14_400, 3),
            (_, 18) => rate!(19_500, 21_700, 3),
            (_, 19) => rate!(26_000, 28_900, 3),
            (_, 20) => rate!(39_000, 43_300, 3),
            (_, 21) => rate!(52_000, 57_800, 3),
            (_, 22) => rate!(58_500, 65_000, 3),
            (_, 23) => rate!(65_000, 72_200, 3),
            (_, 24) => rate!(6_500, 7_200, 4),
            (_, 25) => rate!(13_000, 14_400, 4),
            (_, 26) => rate!(19_500, 21_700, 4),
            (_, 27) => rate!(26_000, 28_900, 4),
            (_, 28) => rate!(39_000, 43_300, 4),
            (_, 29) => rate!(52_000, 57_800, 4),
            (_, 30) => rate!(58_500, 65_000, 4),
            (_, _) => rate!(65_000, 72_200, 4),
        }
    }
}

/// HE MCS index 0 through 11 (IEEE 802.11ax-2021 27.5).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HeMcs(u8);

impl HeMcs {
    pub const fn new(index: u8) -> Option<Self> {
        if index <= 11 { Some(Self(index)) } else { None }
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Number of spatial streams, one through eight.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SpatialStreams(u8);

impl SpatialStreams {
    pub const ONE: Self = Self(1);

    pub const fn new(count: u8) -> Option<Self> {
        if count >= 1 && count <= 8 {
            Some(Self(count))
        } else {
            None
        }
    }

    pub const fn count(self) -> u8 {
        self.0
    }
}

/// HE-LTF size and guard interval of an HE SU PPDU: the four combinations
/// the GI+LTF Size field of HE-SIG-A selects (IEEE 802.11ax-2021 Table 27-18).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HeGiLtf {
    /// 1x HE-LTF with a 0.8 µs guard interval.
    Ltf1xGi800Ns,
    /// 2x HE-LTF with a 0.8 µs guard interval.
    Ltf2xGi800Ns,
    /// 2x HE-LTF with a 1.6 µs guard interval.
    Ltf2xGi1600Ns,
    /// 4x HE-LTF with a 3.2 µs guard interval.
    Ltf4xGi3200Ns,
}

impl HeGiLtf {
    pub const fn guard_interval_ns(self) -> u16 {
        match self {
            Self::Ltf1xGi800Ns | Self::Ltf2xGi800Ns => 800,
            Self::Ltf2xGi1600Ns => 1_600,
            Self::Ltf4xGi3200Ns => 3_200,
        }
    }
}

/// Forward error correction of an HT or HE PPDU.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FecCoding {
    Bcc,
    Ldpc,
}

/// One HE SU rate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HeRate {
    mcs: HeMcs,
    spatial_streams: SpatialStreams,
    bandwidth: PpduBandwidth,
    gi_ltf: HeGiLtf,
    fec: FecCoding,
    dcm: bool,
}

// Full-bandwidth N_SD and modulation/coding of IEEE Std 802.11ax-2021
// 27.5.1. Each leaf is evaluated at compilation, not on received parameters.
/// The largest HE nominal rate in kb/s fits a `u32`: the largest data
/// subcarriers (1 960), bits per subcarrier (10), code-rate numerator (5)
/// and streams (8), over the smallest divisor of [`HeRate::nominal_kbps`]'s
/// arms (`2 · 13 600`), bound every arm.
const HE_KBPS_FITS: () = {
    let largest = 1_960_u64 * 10 * 5 * 8 * 1_000_000 / (2 * 13_600);
    if largest > u32::MAX as u64 {
        panic!("HE's nominal rate exceeds a u32");
    }
};

/// The code rate of an HE-MCS.
#[derive(Clone, Copy)]
enum CodeRate {
    Half,
    TwoThirds,
    ThreeQuarters,
    FiveSixths,
}

impl CodeRate {
    const fn numerator(self) -> u64 {
        match self {
            Self::Half => 1,
            Self::TwoThirds => 2,
            Self::ThreeQuarters => 3,
            Self::FiveSixths => 5,
        }
    }

    const fn denominator(self) -> u64 {
        match self {
            Self::Half => 2,
            Self::TwoThirds => 3,
            Self::ThreeQuarters => 4,
            Self::FiveSixths => 6,
        }
    }
}

impl HeRate {
    /// An HE SU rate; `None` for dual carrier modulation outside MCS 0, 1, 3
    /// and 4 with one or two spatial streams (IEEE 802.11ax-2021 27.3.12.10).
    pub const fn new(
        mcs: HeMcs,
        spatial_streams: SpatialStreams,
        bandwidth: PpduBandwidth,
        gi_ltf: HeGiLtf,
        fec: FecCoding,
        dcm: Dcm,
    ) -> Option<Self> {
        let dcm = matches!(dcm, Dcm::Used);
        if dcm && (!matches!(mcs.0, 0 | 1 | 3 | 4) || spatial_streams.0 > 2) {
            return None;
        }
        Some(Self {
            mcs,
            spatial_streams,
            bandwidth,
            gi_ltf,
            fec,
            dcm,
        })
    }

    pub const fn mcs(self) -> HeMcs {
        self.mcs
    }

    pub const fn spatial_streams(self) -> SpatialStreams {
        self.spatial_streams
    }

    pub const fn bandwidth(self) -> PpduBandwidth {
        self.bandwidth
    }

    pub const fn gi_ltf(self) -> HeGiLtf {
        self.gi_ltf
    }

    pub const fn fec(self) -> FecCoding {
        self.fec
    }

    /// Dual carrier modulation.
    pub const fn dcm(self) -> bool {
        self.dcm
    }

    /// The data rate in kb/s, rounded down: data bits per OFDM symbol over
    /// the 12.8 µs symbol plus guard interval (IEEE Std 802.11ax-2021
    /// 27.5.1), for the full-bandwidth resource unit; DCM halves the data
    /// subcarriers.
    pub const fn nominal_kbps(self) -> u32 {
        let (numerator, _) = self.data_bits_per_symbol();
        let (_, rate) = self.modulation();
        // N_DBPS over the symbol's duration (12.8 us and the guard
        // interval), in kb/s, rounded down once: every divisor is a constant
        // of the standard's tables.
        let bits = numerator.saturating_mul(1_000_000);
        let kbps = match (rate, self.dcm, self.gi_ltf) {
            (CodeRate::Half, false, HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns) => {
                bits / (2 * 13_600)
            }
            (CodeRate::Half, false, HeGiLtf::Ltf2xGi1600Ns) => bits / (2 * 14_400),
            (CodeRate::Half, false, HeGiLtf::Ltf4xGi3200Ns) => bits / (2 * 16_000),
            (CodeRate::Half, true, HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns) => {
                bits / (4 * 13_600)
            }
            (CodeRate::Half, true, HeGiLtf::Ltf2xGi1600Ns) => bits / (4 * 14_400),
            (CodeRate::Half, true, HeGiLtf::Ltf4xGi3200Ns) => bits / (4 * 16_000),
            (CodeRate::TwoThirds, false, HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns) => {
                bits / (3 * 13_600)
            }
            (CodeRate::TwoThirds, false, HeGiLtf::Ltf2xGi1600Ns) => bits / (3 * 14_400),
            (CodeRate::TwoThirds, false, HeGiLtf::Ltf4xGi3200Ns) => bits / (3 * 16_000),
            (CodeRate::TwoThirds, true, HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns) => {
                bits / (6 * 13_600)
            }
            (CodeRate::TwoThirds, true, HeGiLtf::Ltf2xGi1600Ns) => bits / (6 * 14_400),
            (CodeRate::TwoThirds, true, HeGiLtf::Ltf4xGi3200Ns) => bits / (6 * 16_000),
            (CodeRate::ThreeQuarters, false, HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns) => {
                bits / (4 * 13_600)
            }
            (CodeRate::ThreeQuarters, false, HeGiLtf::Ltf2xGi1600Ns) => bits / (4 * 14_400),
            (CodeRate::ThreeQuarters, false, HeGiLtf::Ltf4xGi3200Ns) => bits / (4 * 16_000),
            (CodeRate::ThreeQuarters, true, HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns) => {
                bits / (8 * 13_600)
            }
            (CodeRate::ThreeQuarters, true, HeGiLtf::Ltf2xGi1600Ns) => bits / (8 * 14_400),
            (CodeRate::ThreeQuarters, true, HeGiLtf::Ltf4xGi3200Ns) => bits / (8 * 16_000),
            (CodeRate::FiveSixths, false, HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns) => {
                bits / (6 * 13_600)
            }
            (CodeRate::FiveSixths, false, HeGiLtf::Ltf2xGi1600Ns) => bits / (6 * 14_400),
            (CodeRate::FiveSixths, false, HeGiLtf::Ltf4xGi3200Ns) => bits / (6 * 16_000),
            (CodeRate::FiveSixths, true, HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns) => {
                bits / (12 * 13_600)
            }
            (CodeRate::FiveSixths, true, HeGiLtf::Ltf2xGi1600Ns) => bits / (12 * 14_400),
            (CodeRate::FiveSixths, true, HeGiLtf::Ltf4xGi3200Ns) => bits / (12 * 16_000),
        };
        // In u32 by `HE_KBPS_FITS`: no arm exceeds the largest numerator
        // over the smallest divisor.
        let () = HE_KBPS_FITS;
        kbps as u32
    }

    /// The bits per subcarrier and the code rate of the HE-MCS (IEEE Std
    /// 802.11ax-2021, 27.5.1), and the data subcarriers of the bandwidth.
    const fn modulation(self) -> (u64, CodeRate) {
        match self.mcs.0 {
            0 => (1, CodeRate::Half),
            1 => (2, CodeRate::Half),
            2 => (2, CodeRate::ThreeQuarters),
            3 => (4, CodeRate::Half),
            4 => (4, CodeRate::ThreeQuarters),
            5 => (6, CodeRate::TwoThirds),
            6 => (6, CodeRate::ThreeQuarters),
            7 => (6, CodeRate::FiveSixths),
            8 => (8, CodeRate::ThreeQuarters),
            9 => (8, CodeRate::FiveSixths),
            10 => (10, CodeRate::ThreeQuarters),
            _ => (10, CodeRate::FiveSixths),
        }
    }

    /// Exact N_DBPS (IEEE Std 802.11ax-2021 27.5.1), including DCM, as a
    /// fraction: the data subcarriers of the bandwidth, the bits per
    /// subcarrier, the code rate's numerator and the streams, over the code
    /// rate's denominator and DCM's two.
    const fn data_bits_per_symbol(self) -> (u64, u64) {
        let subcarriers: u64 = match self.bandwidth {
            PpduBandwidth::Mhz20 => 234,
            PpduBandwidth::Mhz40 => 468,
            PpduBandwidth::Mhz80 => 980,
            PpduBandwidth::Mhz160 => 1_960,
        };
        let (bits, rate) = self.modulation();
        let streams: u64 = match self.spatial_streams.0 {
            count @ 1..=7 => count as u64,
            _ => 8,
        };
        let dcm: u64 = if self.dcm { 2 } else { 1 };
        (
            subcarriers
                .saturating_mul(bits)
                .saturating_mul(rate.numerator())
                .saturating_mul(streams),
            rate.denominator().saturating_mul(dcm),
        )
    }
}

/// The longest TXTIME of an HT-mixed PPDU less its signal extension, in
/// microseconds: the L-SIG's 12-bit LENGTH at 6 Mb/s (IEEE Std
/// 802.11-2020, 19.3.9.3.5). See [`PhyRate::max_txtime_micros`].
pub const HT_MIXED_MAX_TXTIME_MICROS: u32 = const {
    // L-STF, L-LTF and L-SIG, then 4 us symbols of 24 data bits carrying
    // SERVICE, LENGTH octets and the tail (17.4.3).
    let symbols = (16_u32 + 8 * 4_095 + 6 + 23) / 24;
    20 + 4 * symbols
};

/// The PHY rate of one PPDU.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PhyRate {
    Legacy(LegacyRate),
    Ht(HtRate),
    He(HeRate),
}

impl PhyRate {
    /// The PPDU bandwidth; a non-HT PPDU occupies 20 MHz.
    pub const fn bandwidth(self) -> PpduBandwidth {
        match self {
            Self::Legacy(_) => PpduBandwidth::Mhz20,
            Self::Ht(rate) => rate.bandwidth,
            Self::He(rate) => rate.bandwidth,
        }
    }

    /// The nominal data rate in kb/s.
    pub const fn nominal_kbps(self) -> u32 {
        match self {
            Self::Legacy(rate) => rate.kbps(),
            Self::Ht(rate) => rate.nominal_kbps(),
            Self::He(rate) => rate.nominal_kbps(),
        }
    }

    /// Octets between MPDU starts required by the HT A-MPDU Parameters
    /// field's spacing encoding (0, 0.25, 0.5, 1, 2, 4, 8 or 16 us).
    /// Uses the exact symbol rate and rounds up, including short GI and HE
    /// fractional coding rates. Legacy rates cannot carry an A-MPDU.
    pub const fn minimum_mpdu_spacing_bytes(self, encoding: u8) -> Option<u32> {
        macro_rules! checked {
            ($value:expr) => {
                match $value {
                    Some(value) => value,
                    None => return None,
                }
            };
        }
        let spacing_ns = match encoding {
            0 => 0,
            1 => 250,
            2 => 500,
            3 => 1_000,
            4 => 2_000,
            5 => 4_000,
            6 => 8_000,
            7 => 16_000,
            _ => return None,
        };
        let (bits, divisor, symbol_ns) = match self {
            Self::Legacy(_) => return None,
            Self::Ht(rate) => (
                rate.parameters().2,
                1_u64,
                if rate.short_gi { 3_600 } else { 4_000 },
            ),
            Self::He(rate) => {
                let (numerator, denominator) = rate.data_bits_per_symbol();
                (
                    numerator,
                    denominator,
                    checked!(12_800_u64.checked_add(rate.gi_ltf.guard_interval_ns() as u64)),
                )
            }
        };
        let bytes = checked!(ceil_div(
            checked!(bits.checked_mul(spacing_ns)),
            checked!(checked!(divisor.checked_mul(symbol_ns)).checked_mul(8))
        ));
        if bytes > u32::MAX as u64 {
            None
        } else {
            Some(bytes as u32)
        }
    }

    /// The longest PPDU at this rate, in microseconds, that a transmitter
    /// may send; `None` where this module states no limit (non-HT and HE
    /// rates).
    ///
    /// An HT-mixed PPDU carries its duration in the L-SIG: RATE 6 Mb/s and
    /// a 12-bit LENGTH (IEEE Std 802.11-2020, 19.3.9.3.5, Equation (19-1)),
    /// so its TXTIME less any signal extension is at most
    /// 20 + 4 * ceil((16 + 8 * 4095 + 6) / 24) = 5484 us. Compared with
    /// [`Self::max_ppdu_duration_micros`], which includes the 6 us signal
    /// extension, the limit errs short by that extension.
    pub const fn max_txtime_micros(self) -> Option<u32> {
        match self {
            Self::Ht(_) => Some(HT_MIXED_MAX_TXTIME_MICROS),
            Self::Legacy(_) | Self::He(_) => None,
        }
    }

    /// An upper bound of the time on air, in microseconds, of a 2.4 GHz
    /// PPDU carrying `psdu_octets` at this rate (TXTIME, IEEE Std
    /// 802.11-2020 15.4.3, 17.4.3, 19.4.3 and 802.11ax-2021 27.4.3): the
    /// preamble, every data symbol with the BCC tail bits, the 6 us signal
    /// extension of an OFDM PPDU, and for HE the longest packet extension
    /// (16 us). A caller keeping an exchange within a TXOP limit stays
    /// within the limit by this bound. `None` refuses a duration that cannot
    /// be represented in microseconds by a `u32`.
    pub const fn max_ppdu_duration_micros(self, psdu_octets: u32) -> Option<u32> {
        macro_rules! checked {
            ($value:expr) => {
                match $value {
                    Some(value) => value,
                    None => return None,
                }
            };
        }
        const SIGNAL_EXTENSION: u64 = 6;
        let psdu_bits = checked!(8_u64.checked_mul(psdu_octets as u64));
        let micros = match self {
            Self::Legacy(rate) if !rate.is_ofdm() => {
                let preamble: u64 = match rate {
                    LegacyRate::Dsss2M(DsssPreamble::Short)
                    | LegacyRate::Cck5M5(DsssPreamble::Short)
                    | LegacyRate::Cck11M(DsssPreamble::Short) => 96,
                    _ => 192,
                };
                checked!(preamble.checked_add(checked!(ceil_div(
                    checked!(psdu_bits.checked_mul(1_000)),
                    rate.kbps() as u64
                ))))
            }
            Self::Legacy(rate) => {
                // N_DBPS: the rate times the 4 us symbol (17.4.3).
                let bits_per_symbol = checked!((rate.kbps() as u64).checked_mul(4)) / 1_000;
                let symbols = checked!(ceil_div(
                    checked!(checked!(16_u64.checked_add(psdu_bits)).checked_add(6)),
                    bits_per_symbol
                ));
                checked!(
                    checked!(20_u64.checked_add(checked!(4_u64.checked_mul(symbols))))
                        .checked_add(SIGNAL_EXTENSION)
                )
            }
            Self::Ht(rate) => {
                // N_DBPS uses long GI; short GI changes symbol time, not bits
                // (IEEE Std 802.11-2020 19.4.3, Tables 19-27/19-28).
                let bits_per_symbol = rate.parameters().2;
                let streams = rate.mcs.spatial_streams() as u64;
                // One BCC encoder per 300 Mb/s (19.4.3).
                let encoders = checked!(ceil_div(rate.nominal_kbps() as u64, 300_000));
                let symbols = checked!(ceil_div(
                    checked!(
                        checked!(16_u64.checked_add(psdu_bits))
                            .checked_add(checked!(6_u64.checked_mul(encoders)))
                    ),
                    bits_per_symbol
                ));
                // 3.6 us symbols, the PPDU rounded up to 4 us (19.4.3).
                let data = if rate.short_gi {
                    checked!(
                        4_u64.checked_mul(checked!(ceil_div(checked!(symbols.checked_mul(9)), 10)))
                    )
                } else {
                    checked!(4_u64.checked_mul(symbols))
                };
                // L-STF, L-LTF, L-SIG, HT-SIG, HT-STF and HT-LTFs (19.4.3).
                let ltfs = if streams > 2 { 4 } else { streams };
                checked!(
                    checked!(
                        checked!(36_u64.checked_add(checked!(
                            4_u64.checked_mul(checked!(ltfs.checked_sub(1)))
                        )))
                        .checked_add(data)
                    )
                    .checked_add(SIGNAL_EXTENSION)
                )
            }
            Self::He(rate) => {
                let (numerator, denominator) = rate.data_bits_per_symbol();
                let streams = rate.spatial_streams.0 as u64;
                // BCC tail bound: one encoder per 600 Mb/s (802.11ax-2021 27.4.3).
                let encoders = checked!(ceil_div(rate.nominal_kbps() as u64, 600_000));
                let bits = checked!(
                    checked!(16_u64.checked_add(psdu_bits))
                        .checked_add(checked!(6_u64.checked_mul(encoders)))
                );
                // Symbols: SERVICE, PSDU and BCC tails over exact N_DBPS,
                // rounded up (27.4.3).
                let symbols =
                    checked!(ceil_div(checked!(bits.checked_mul(denominator)), numerator));
                let guard_ns = rate.gi_ltf.guard_interval_ns() as u64;
                let ltf: u64 = match rate.gi_ltf {
                    HeGiLtf::Ltf1xGi800Ns => 3_200,
                    HeGiLtf::Ltf2xGi800Ns | HeGiLtf::Ltf2xGi1600Ns => 6_400,
                    HeGiLtf::Ltf4xGi3200Ns => 12_800,
                };
                let ltf_ns = checked!(ltf.checked_add(guard_ns));
                // L-STF, L-LTF, L-SIG, RL-SIG, HE-SIG-A, HE-STF, then
                // HE-LTFs and data symbols (27.4.3).
                let ns = checked!(
                    checked!(36_000_u64.checked_add(checked!(
                        (if streams > 2 { 4 } else { streams }).checked_mul(ltf_ns)
                    )))
                    .checked_add(checked!(
                        symbols.checked_mul(checked!(12_800_u64.checked_add(guard_ns)))
                    ))
                );
                checked!(
                    checked!(checked!(ceil_div(ns, 1_000)).checked_add(16))
                        .checked_add(SIGNAL_EXTENSION)
                )
            }
        };
        if micros > u32::MAX as u64 {
            None
        } else {
            Some(micros as u32)
        }
    }

    /// The instant, in nanoseconds from the start of a PPDU at this rate, at
    /// which octet `offset` (from 0) of its PSDU has been transmitted: the
    /// end of the data symbol that carries it, or of its last bit at a
    /// DSSS or CCK rate.
    ///
    /// The instant counts the preamble and the data up to that octet, with
    /// the 16-bit SERVICE field ahead of the PSDU. It leaves out what
    /// follows the PSDU: the tail and pad bits, the 6 us signal extension,
    /// an HE packet extension and the 4-us rounding of a short-GI PPDU's
    /// end, which [`Self::max_ppdu_duration_micros`] adds. The preamble and
    /// symbol terms are that function's TXTIME terms (IEEE Std 802.11-2020
    /// 15.4.3, 17.4.3, 19.4.3 and 802.11ax-2021 27.4.3). `None` refuses an
    /// instant a `u64` of nanoseconds cannot hold.
    pub const fn psdu_octet_sent_nanos(self, offset: u32) -> Option<u64> {
        macro_rules! checked {
            ($value:expr) => {
                match $value {
                    Some(value) => value,
                    None => return None,
                }
            };
        }
        const SERVICE_BITS: u64 = 16;
        // The bits of the PSDU through this octet.
        let bits = checked!(checked!((offset as u64).checked_add(1)).checked_mul(8));
        let data = checked!(bits.checked_add(SERVICE_BITS));
        let nanos = match self {
            Self::Legacy(rate) if !rate.is_ofdm() => {
                // No SERVICE field: the PLCP header lies in the preamble.
                let preamble: u64 = match rate {
                    LegacyRate::Dsss2M(DsssPreamble::Short)
                    | LegacyRate::Cck5M5(DsssPreamble::Short)
                    | LegacyRate::Cck11M(DsssPreamble::Short) => 96,
                    _ => 192,
                };
                checked!(
                    checked!(preamble.checked_mul(1_000)).checked_add(checked!(ceil_div(
                        checked!(bits.checked_mul(1_000_000)),
                        rate.kbps() as u64
                    )))
                )
            }
            Self::Legacy(rate) => {
                // L-STF, L-LTF and L-SIG, then 4 us symbols (17.4.3).
                let bits_per_symbol = checked!((rate.kbps() as u64).checked_mul(4)) / 1_000;
                let symbols = checked!(ceil_div(data, bits_per_symbol));
                checked!(20_000_u64.checked_add(checked!(symbols.checked_mul(4_000))))
            }
            Self::Ht(rate) => {
                // HT-mixed preamble and its HT-LTFs, then 4 us or 3.6 us
                // symbols of N_DBPS bits (19.4.3).
                let streams = rate.mcs.spatial_streams() as u64;
                let ltfs = if streams > 2 { 4 } else { streams };
                let preamble =
                    checked!(
                        checked!(36_u64.checked_add(checked!(
                            4_u64.checked_mul(checked!(ltfs.checked_sub(1)))
                        )))
                        .checked_mul(1_000)
                    );
                let symbols = checked!(ceil_div(data, rate.parameters().2));
                let symbol_ns: u64 = if rate.short_gi { 3_600 } else { 4_000 };
                checked!(preamble.checked_add(checked!(symbols.checked_mul(symbol_ns))))
            }
            Self::He(rate) => {
                // L-STF to HE-STF, the HE-LTFs, then symbols of 12.8 us
                // plus the guard interval over exact N_DBPS (27.4.3).
                let (numerator, denominator) = rate.data_bits_per_symbol();
                let streams = rate.spatial_streams.0 as u64;
                let guard_ns = rate.gi_ltf.guard_interval_ns() as u64;
                let ltf: u64 = match rate.gi_ltf {
                    HeGiLtf::Ltf1xGi800Ns => 3_200,
                    HeGiLtf::Ltf2xGi800Ns | HeGiLtf::Ltf2xGi1600Ns => 6_400,
                    HeGiLtf::Ltf4xGi3200Ns => 12_800,
                };
                let ltf_ns = checked!(ltf.checked_add(guard_ns));
                let preamble = checked!(36_000_u64.checked_add(checked!(
                    (if streams > 2 { 4 } else { streams }).checked_mul(ltf_ns)
                )));
                let symbols =
                    checked!(ceil_div(checked!(data.checked_mul(denominator)), numerator));
                checked!(preamble.checked_add(checked!(
                    symbols.checked_mul(checked!(12_800_u64.checked_add(guard_ns)))
                )))
            }
        };
        Some(nanos)
    }
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.3 (Supported Rates and BSS Membership Selectors element).
    pub struct SupportedRate[1] (little) {
        pub rate, with_rate: u8 = 0..7;
        pub basic, with_basic: bool = 7..8;
    }
}

#[cfg(test)]
mod tests;

/// Ceiling division which refuses a zero divisor and an unrepresentable result.
const fn ceil_div(value: u64, divisor: u64) -> Option<u64> {
    let Some(quotient) = value.checked_div(divisor) else {
        return None;
    };
    let Some(remainder) = value.checked_rem(divisor) else {
        return None;
    };
    quotient.checked_add((remainder != 0) as u64)
}
