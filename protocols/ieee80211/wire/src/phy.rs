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

/// One HT rate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HtRate {
    mcs: HtMcs,
    bandwidth: PpduBandwidth,
    short_gi: bool,
    long_kbps: u32,
    short_kbps: u32,
}

impl HtRate {
    /// An HT rate; `None` for a bandwidth HT does not have (80 or 160 MHz).
    pub const fn new(mcs: HtMcs, bandwidth: PpduBandwidth, short_gi: bool) -> Option<Self> {
        match bandwidth {
            PpduBandwidth::Mhz20 | PpduBandwidth::Mhz40 => {
                let (long, short) = match (bandwidth, mcs.index() % 8) {
                    (PpduBandwidth::Mhz40, 0) => (13_500_u32, 15_000_u32),
                    (PpduBandwidth::Mhz40, 1) => (27_000, 30_000),
                    (PpduBandwidth::Mhz40, 2) => (40_500, 45_000),
                    (PpduBandwidth::Mhz40, 3) => (54_000, 60_000),
                    (PpduBandwidth::Mhz40, 4) => (81_000, 90_000),
                    (PpduBandwidth::Mhz40, 5) => (108_000, 120_000),
                    (PpduBandwidth::Mhz40, 6) => (121_500, 135_000),
                    (PpduBandwidth::Mhz40, _) => (135_000, 150_000),
                    (_, 0) => (6_500, 7_200),
                    (_, 1) => (13_000, 14_400),
                    (_, 2) => (19_500, 21_700),
                    (_, 3) => (26_000, 28_900),
                    (_, 4) => (39_000, 43_300),
                    (_, 5) => (52_000, 57_800),
                    (_, 6) => (58_500, 65_000),
                    (_, _) => (65_000, 72_200),
                };
                let Some(long_kbps) = long.checked_mul(mcs.spatial_streams() as u32) else {
                    return None;
                };
                let Some(short_kbps) = short.checked_mul(mcs.spatial_streams() as u32) else {
                    return None;
                };
                Some(Self {
                    mcs,
                    bandwidth,
                    short_gi,
                    long_kbps,
                    short_kbps,
                })
            }
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
        if self.short_gi {
            self.short_kbps
        } else {
            self.long_kbps
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
    data_numerator: u32,
    data_denominator: u8,
    nominal_kbps: u32,
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
        macro_rules! checked {
            ($value:expr) => {
                match $value {
                    Some(value) => value,
                    None => return None,
                }
            };
        }
        let subcarriers: u32 = match bandwidth {
            PpduBandwidth::Mhz20 => 234,
            PpduBandwidth::Mhz40 => 468,
            PpduBandwidth::Mhz80 => 980,
            PpduBandwidth::Mhz160 => 1_960,
        };
        let (bits, numerator, denominator) = match mcs.0 {
            0 => (1, 1, 2_u8),
            1 => (2, 1, 2),
            2 => (2, 3, 4),
            3 => (4, 1, 2),
            4 => (4, 3, 4),
            5 => (6, 2, 3),
            6 => (6, 3, 4),
            7 => (6, 5, 6),
            8 => (8, 3, 4),
            9 => (8, 5, 6),
            10 => (10, 3, 4),
            11 => (10, 5, 6),
            _ => return None,
        };
        let data_numerator = checked!(
            checked!(checked!(subcarriers.checked_mul(bits)).checked_mul(numerator))
                .checked_mul(spatial_streams.0 as u32)
        );
        let data_denominator = checked!(denominator.checked_mul(if dcm { 2 } else { 1 }));
        let symbol_ns = checked!(12_800_u64.checked_add(gi_ltf.guard_interval_ns() as u64));
        let divisor = checked!((data_denominator as u64).checked_mul(symbol_ns));
        let nominal =
            checked!(checked!((data_numerator as u64).checked_mul(1_000_000)).checked_div(divisor));
        if nominal > u32::MAX as u64 {
            return None;
        }
        Some(Self {
            mcs,
            spatial_streams,
            bandwidth,
            gi_ltf,
            fec,
            dcm,
            data_numerator,
            data_denominator,
            nominal_kbps: nominal as u32,
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
        self.nominal_kbps
    }

    /// The exact N_DBPS fraction admitted by the constructor.
    const fn data_bits_per_symbol(self) -> (u64, u64) {
        (self.data_numerator as u64, self.data_denominator as u64)
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
                checked!((rate.long_kbps as u64).checked_mul(4)) / 1_000,
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
                let bits_per_symbol = checked!((rate.long_kbps as u64).checked_mul(4)) / 1_000;
                let streams = rate.mcs.spatial_streams() as u64;
                let encoders = checked!(ceil_div(rate.nominal_kbps() as u64, 300_000));
                let symbols = checked!(ceil_div(
                    checked!(
                        checked!(16_u64.checked_add(psdu_bits))
                            .checked_add(checked!(6_u64.checked_mul(encoders)))
                    ),
                    bits_per_symbol
                ));
                let data = if rate.short_gi {
                    checked!(
                        4_u64.checked_mul(checked!(ceil_div(checked!(symbols.checked_mul(9)), 10)))
                    )
                } else {
                    checked!(4_u64.checked_mul(symbols))
                };
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
                let encoders = checked!(ceil_div(rate.nominal_kbps() as u64, 600_000));
                let bits = checked!(
                    checked!(16_u64.checked_add(psdu_bits))
                        .checked_add(checked!(6_u64.checked_mul(encoders)))
                );
                let symbols =
                    checked!(ceil_div(checked!(bits.checked_mul(denominator)), numerator));
                let guard_ns = rate.gi_ltf.guard_interval_ns() as u64;
                let ltf: u64 = match rate.gi_ltf {
                    HeGiLtf::Ltf1xGi800Ns => 3_200,
                    HeGiLtf::Ltf2xGi800Ns | HeGiLtf::Ltf2xGi1600Ns => 6_400,
                    HeGiLtf::Ltf4xGi3200Ns => 12_800,
                };
                let ltf_ns = checked!(ltf.checked_add(guard_ns));
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
}

oer_wire::bitfield! {
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
