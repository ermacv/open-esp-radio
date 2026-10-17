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
        self.0 / 8 + 1
    }
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
    pub const fn new(mcs: HtMcs, bandwidth: PpduBandwidth, short_gi: bool) -> Option<Self> {
        match bandwidth {
            PpduBandwidth::Mhz20 | PpduBandwidth::Mhz40 => Some(Self {
                mcs,
                bandwidth,
                short_gi,
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
        const HT20_LGI: [u32; 8] = [
            6_500, 13_000, 19_500, 26_000, 39_000, 52_000, 58_500, 65_000,
        ];
        const HT20_SGI: [u32; 8] = [
            7_200, 14_400, 21_700, 28_900, 43_300, 57_800, 65_000, 72_200,
        ];
        const HT40_LGI: [u32; 8] = [
            13_500, 27_000, 40_500, 54_000, 81_000, 108_000, 121_500, 135_000,
        ];
        const HT40_SGI: [u32; 8] = [
            15_000, 30_000, 45_000, 60_000, 90_000, 120_000, 135_000, 150_000,
        ];
        let table = match (self.bandwidth, self.short_gi) {
            (PpduBandwidth::Mhz40, false) => &HT40_LGI,
            (PpduBandwidth::Mhz40, true) => &HT40_SGI,
            (_, false) => &HT20_LGI,
            (_, true) => &HT20_SGI,
        };
        table[(self.mcs.0 % 8) as usize] * self.mcs.spatial_streams() as u32
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

impl HeRate {
    /// An HE SU rate; `None` for dual carrier modulation outside MCS 0, 1, 3
    /// and 4 with one or two spatial streams (IEEE 802.11ax-2021 27.3.12.10).
    pub const fn new(
        mcs: HeMcs,
        spatial_streams: SpatialStreams,
        bandwidth: PpduBandwidth,
        gi_ltf: HeGiLtf,
        fec: FecCoding,
        dcm: bool,
    ) -> Option<Self> {
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
        let symbol_ns = 12_800 + self.gi_ltf.guard_interval_ns() as u64;
        let (numerator, denominator) = self.data_bits_per_symbol();
        (numerator * 1_000_000 / (denominator * symbol_ns)) as u32
    }

    /// Data bits per OFDM symbol (N_DBPS) of the full-bandwidth resource
    /// unit, as a fraction: data subcarriers times coded bits, coding rate
    /// and spatial streams; DCM halves the data subcarriers.
    const fn data_bits_per_symbol(self) -> (u64, u64) {
        // Data subcarriers of the 242-, 484-, 996- and 2x996-tone RUs.
        let subcarriers: u64 = match self.bandwidth {
            PpduBandwidth::Mhz20 => 234,
            PpduBandwidth::Mhz40 => 468,
            PpduBandwidth::Mhz80 => 980,
            PpduBandwidth::Mhz160 => 1_960,
        };
        // Coded bits per subcarrier and coding rate of MCS 0-11.
        const MODULATION: [(u64, u64, u64); 12] = [
            (1, 1, 2),
            (2, 1, 2),
            (2, 3, 4),
            (4, 1, 2),
            (4, 3, 4),
            (6, 2, 3),
            (6, 3, 4),
            (6, 5, 6),
            (8, 3, 4),
            (8, 5, 6),
            (10, 3, 4),
            (10, 5, 6),
        ];
        let (bits, rate_numerator, rate_denominator) = MODULATION[self.mcs.0 as usize];
        let dcm = if self.dcm { 2 } else { 1 };
        (
            subcarriers * bits * rate_numerator * self.spatial_streams.0 as u64,
            rate_denominator * dcm,
        )
    }
}

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
        let spacing_ns = match encoding {
            0 => 0,
            1..=7 => 250_u64 << (encoding - 1),
            _ => return None,
        };
        let (bits, divisor, symbol_ns) = match self {
            Self::Legacy(_) => return None,
            Self::Ht(rate) => {
                let long_gi = HtRate {
                    short_gi: false,
                    ..rate
                };
                (
                    long_gi.nominal_kbps() as u64 * 4 / 1_000,
                    1,
                    if rate.short_gi { 3_600 } else { 4_000 },
                )
            }
            Self::He(rate) => {
                let (numerator, denominator) = rate.data_bits_per_symbol();
                (
                    numerator,
                    denominator,
                    12_800 + rate.gi_ltf.guard_interval_ns() as u64,
                )
            }
        };
        // At most eight streams, 160 MHz and 16 us: every intermediate is
        // bounded well below u64::MAX and the byte result fits in u32.
        Some((bits * spacing_ns).div_ceil(divisor * symbol_ns * 8) as u32)
    }

    /// An upper bound of the time on air, in microseconds, of a 2.4 GHz
    /// PPDU carrying `psdu_octets` at this rate (TXTIME, IEEE Std
    /// 802.11-2020 15.4.3, 17.4.3, 19.4.3 and 802.11ax-2021 27.4.3): the
    /// preamble, every data symbol with the BCC tail bits, the 6 us signal
    /// extension of an OFDM PPDU, and for HE the longest packet extension
    /// (16 us). A caller keeping an exchange within a TXOP limit stays
    /// within the limit by this bound.
    pub const fn max_ppdu_duration_micros(self, psdu_octets: u32) -> u32 {
        const SIGNAL_EXTENSION: u64 = 6;
        let psdu_bits = 8 * psdu_octets as u64;
        match self {
            Self::Legacy(rate) if !rate.is_ofdm() => {
                let preamble = match rate {
                    LegacyRate::Dsss2M(DsssPreamble::Short)
                    | LegacyRate::Cck5M5(DsssPreamble::Short)
                    | LegacyRate::Cck11M(DsssPreamble::Short) => 96,
                    _ => 192,
                };
                (preamble + (psdu_bits * 1_000).div_ceil(rate.kbps() as u64)) as u32
            }
            Self::Legacy(rate) => {
                // N_DBPS: the rate times the 4 us symbol.
                let bits_per_symbol = rate.kbps() as u64 * 4 / 1_000;
                let symbols = (16 + psdu_bits + 6).div_ceil(bits_per_symbol);
                (20 + 4 * symbols + SIGNAL_EXTENSION) as u32
            }
            Self::Ht(rate) => {
                // N_DBPS of the long-GI rate times its 4 us symbol; the
                // short GI shortens the symbol, not its bits.
                let long_gi = HtRate {
                    short_gi: false,
                    ..rate
                };
                let bits_per_symbol = long_gi.nominal_kbps() as u64 * 4 / 1_000;
                let streams = rate.mcs.spatial_streams() as u64;
                // One BCC encoder per 300 Mb/s.
                let encoders = (rate.nominal_kbps() as u64).div_ceil(300_000);
                let symbols = (16 + psdu_bits + 6 * encoders).div_ceil(bits_per_symbol);
                let data = if rate.short_gi {
                    // 3.6 us symbols, the PPDU rounded up to 4 us.
                    4 * (symbols * 9).div_ceil(10)
                } else {
                    4 * symbols
                };
                // L-STF, L-LTF, L-SIG, HT-SIG, HT-STF and the HT-LTFs.
                let ltfs = if streams > 2 { 4 } else { streams };
                (36 + 4 * (ltfs - 1) + data + SIGNAL_EXTENSION) as u32
            }
            Self::He(rate) => {
                let (numerator, denominator) = rate.data_bits_per_symbol();
                let streams = rate.spatial_streams.0 as u64;
                let encoders = (rate.nominal_kbps() as u64).div_ceil(600_000);
                // Symbols: the bits over N_DBPS, rounded up.
                let symbols = ((16 + psdu_bits + 6 * encoders) * denominator).div_ceil(numerator);
                let guard_ns = rate.gi_ltf.guard_interval_ns() as u64;
                let ltf_ns = match rate.gi_ltf {
                    HeGiLtf::Ltf1xGi800Ns => 3_200,
                    HeGiLtf::Ltf2xGi800Ns | HeGiLtf::Ltf2xGi1600Ns => 6_400,
                    HeGiLtf::Ltf4xGi3200Ns => 12_800,
                } + guard_ns;
                let ltfs = if streams > 2 { 4 } else { streams };
                // L-STF, L-LTF, L-SIG, RL-SIG, HE-SIG-A, HE-STF, then the
                // HE-LTFs and the data symbols.
                let ns = 36_000 + ltfs * ltf_ns + symbols * (12_800 + guard_ns);
                (ns.div_ceil(1_000) + 16 + SIGNAL_EXTENSION) as u32
            }
        }
    }
}

#[cfg(test)]
mod tests;
