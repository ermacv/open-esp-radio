//! Exact PHY durations of a PPDU, the reference of the latch calibration.
//!
//! The arithmetic follows mac80211's `ieee80211_frame_duration`
//! (`net/mac80211/util.c`, v6.18.26) for DSSS/CCK and ERP-OFDM, without its
//! SIFS, and IEEE Std 802.11-2020 for each field:
//!
//! - DSSS/CCK (15.3.3, 16.3.4): preamble + PLCP header 144 + 48 us, or
//!   72 + 24 us short, then `ceil(8 * PSDU / rate)`.
//! - OFDM (17.4.3) at 2.4 GHz (ERP, 18.5.2): 16 us training, 4 us SIGNAL,
//!   `4 * ceil((16 + 8 * PSDU + 6) / N_DBPS)` (SERVICE 16 bits, tail 6 bits),
//!   then the 6-us signal extension.
//! - HT-mixed, BCC, one encoder, long guard interval (19.4.3, eq. 19-91):
//!   L-STF 8, L-LTF 8, L-SIG 4, HT-SIG 8, HT-STF 4, one HT-LTF 4 per
//!   spatial stream (one here), then `4 * ceil((16 + 8 * PSDU + 6) /
//!   N_DBPS)` and the 6-us signal extension at 2.4 GHz.
//!
//! The PSDU is the MPDU with its FCS. Each duration splits into the part
//! before the MPDU's first bit (`preamble`) and the MPDU's own (`mpdu`).

/// A PPDU's timing, microseconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timing {
    /// From the PPDU's start to the MPDU's first bit.
    pub preamble: u32,
    /// From the MPDU's first bit to the PPDU's end, signal extension
    /// included.
    pub mpdu: u32,
}

impl Timing {
    pub const fn total(self) -> u32 {
        self.preamble + self.mpdu
    }
}

/// The PHY modes the calibration uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phy {
    /// DSSS/CCK at `rate_kbps` (1000, 2000, 5500, 11000), long or short
    /// preamble.
    Dsss { rate_kbps: u32, short: bool },
    /// ERP-OFDM at 2.4 GHz, `rate_mbps` in 6..=54.
    ErpOfdm { rate_mbps: u32 },
    /// HT-mixed 20 MHz, one stream, long GI, BCC, MCS 0..=7.
    HtMixed { mcs: u8 },
}

const SIGNAL_EXTENSION_US: u32 = 6;
const SERVICE_BITS: u32 = 16;
const TAIL_BITS: u32 = 6;

/// Data bits per OFDM symbol of an ERP-OFDM rate (17.4.3, Table 17-4).
fn ofdm_dbps(rate_mbps: u32) -> Option<u32> {
    match rate_mbps {
        6 | 9 | 12 | 18 | 24 | 36 | 48 | 54 => Some(rate_mbps * 4),
        _ => None,
    }
}

/// Data bits per symbol of HT MCS 0..=7 at 20 MHz (19.5, Table 19-27).
fn ht_dbps(mcs: u8) -> Option<u32> {
    [26, 52, 78, 104, 156, 208, 234, 260]
        .get(usize::from(mcs))
        .copied()
}

fn ofdm_symbols(psdu: u32, dbps: u32) -> u32 {
    (SERVICE_BITS + 8 * psdu + TAIL_BITS).div_ceil(dbps)
}

/// The timing of a PPDU carrying `psdu` octets (MPDU and FCS).
pub fn timing(phy: Phy, psdu: u32) -> Option<Timing> {
    match phy {
        Phy::Dsss { rate_kbps, short } => {
            if !matches!(rate_kbps, 1000 | 2000 | 5500 | 11000) || (short && rate_kbps == 1000) {
                return None;
            }
            let preamble = if short { 72 + 24 } else { 144 + 48 };
            Some(Timing {
                preamble,
                mpdu: (8 * 1000 * psdu).div_ceil(rate_kbps),
            })
        }
        Phy::ErpOfdm { rate_mbps } => {
            let dbps = ofdm_dbps(rate_mbps)?;
            Some(Timing {
                preamble: 16 + 4,
                mpdu: 4 * ofdm_symbols(psdu, dbps) + SIGNAL_EXTENSION_US,
            })
        }
        Phy::HtMixed { mcs } => {
            let dbps = ht_dbps(mcs)?;
            Some(Timing {
                preamble: 8 + 8 + 4 + 8 + 4 + 4,
                mpdu: 4 * ofdm_symbols(psdu, dbps) + SIGNAL_EXTENSION_US,
            })
        }
    }
}

/// The control rate of the ACK to a frame on `phy` at 2.4 GHz: the basic
/// rate set {1, 2, 5.5, 11} Mb/s DSSS/CCK and {6, 12, 24} Mb/s OFDM, the
/// highest not above the data rate in the same modulation class
/// (10.6.6.5.2); HT data is answered in non-HT OFDM.
pub fn ack_phy(data: Phy) -> Phy {
    match data {
        Phy::Dsss { rate_kbps, short } => Phy::Dsss {
            rate_kbps: match rate_kbps {
                11000 => 11000,
                5500 => 5500,
                2000 => 2000,
                _ => 1000,
            },
            short: short && rate_kbps != 1000,
        },
        Phy::ErpOfdm { rate_mbps } => Phy::ErpOfdm {
            rate_mbps: if rate_mbps >= 24 {
                24
            } else if rate_mbps >= 12 {
                12
            } else {
                6
            },
        },
        Phy::HtMixed { mcs } => Phy::ErpOfdm {
            rate_mbps: match mcs {
                0 => 6,
                1 | 2 => 12,
                _ => 24,
            },
        },
    }
}

/// An ACK's PSDU: frame control, duration, RA and FCS.
pub const ACK_PSDU: u32 = 14;

#[cfg(test)]
mod tests {
    use super::*;

    /// mac80211's `ieee80211_frame_duration` for `len` octets without the
    /// FCS, SIFS included (10 us at 2.4 GHz), as its formula computes it.
    fn mac80211_frame_duration(len: u32, rate_100kbps: u32, erp: bool, short: bool) -> u32 {
        if erp {
            16 + 16 + 4 + 4 * ((16 + 8 * (len + 4) + 6) * 10).div_ceil(4 * rate_100kbps)
        } else {
            10 + if short { 72 + 24 } else { 144 + 48 }
                + (8 * (len + 4) * 10).div_ceil(rate_100kbps)
        }
    }

    #[test]
    fn dsss_and_erp_ofdm_match_mac80211_less_its_sifs() {
        for len in [10u32, 100, 496, 1000, 1524, 2000] {
            for (rate, short) in [
                (10, false),
                (20, false),
                (20, true),
                (55, true),
                (110, false),
            ] {
                let phy = Phy::Dsss {
                    rate_kbps: rate * 100,
                    short,
                };
                let ours = timing(phy, len + 4).unwrap().total();
                assert_eq!(ours + 10, mac80211_frame_duration(len, rate, false, short));
            }
            for rate in [6u32, 9, 12, 18, 24, 36, 48, 54] {
                let ours = timing(Phy::ErpOfdm { rate_mbps: rate }, len + 4)
                    .unwrap()
                    .total();
                assert_eq!(
                    ours + 10,
                    mac80211_frame_duration(len, rate * 10, true, false)
                );
            }
        }
    }

    #[test]
    fn known_durations() {
        // An ACK at 1 Mb/s, long preamble: 192 + 112.
        let ack = timing(
            Phy::Dsss {
                rate_kbps: 1000,
                short: false,
            },
            ACK_PSDU,
        )
        .unwrap();
        assert_eq!((ack.preamble, ack.mpdu), (192, 112));
        // The probe's blocker: 1528 octets at 1 Mb/s.
        assert_eq!(
            timing(
                Phy::Dsss {
                    rate_kbps: 1000,
                    short: false
                },
                1528
            )
            .unwrap()
            .total(),
            12_416
        );
        // An ACK at 6 Mb/s ERP-OFDM: 20 + 4 * ceil(134 / 24) + 6 = 50.
        let ack = timing(Phy::ErpOfdm { rate_mbps: 6 }, ACK_PSDU).unwrap();
        assert_eq!((ack.preamble, ack.mpdu), (20, 30));
        // 1500 octets at 54 Mb/s: 20 + 4 * ceil(12022 / 216) + 6 = 250.
        assert_eq!(
            timing(Phy::ErpOfdm { rate_mbps: 54 }, 1500)
                .unwrap()
                .total(),
            250
        );
        // HT-mixed MCS 7: 36 + 4 * ceil(12022 / 260) + 6 = 230; MCS 0:
        // 36 + 4 * ceil(12022 / 26) + 6 = 1894.
        let mcs7 = timing(Phy::HtMixed { mcs: 7 }, 1500).unwrap();
        assert_eq!((mcs7.preamble, mcs7.total()), (36, 230));
        assert_eq!(timing(Phy::HtMixed { mcs: 0 }, 1500).unwrap().total(), 1894);
        // Symbol rounding: 1 octet more crosses a symbol at 54 Mb/s.
        assert_eq!(
            timing(Phy::ErpOfdm { rate_mbps: 54 }, 1501)
                .unwrap()
                .total(),
            250
        );
        assert_eq!(
            timing(Phy::ErpOfdm { rate_mbps: 54 }, 1522)
                .unwrap()
                .total(),
            254
        );
    }

    #[test]
    fn unknown_rates_have_no_timing_and_acks_take_the_control_rate() {
        assert_eq!(timing(Phy::ErpOfdm { rate_mbps: 7 }, 10), None);
        assert_eq!(timing(Phy::HtMixed { mcs: 8 }, 10), None);
        assert_eq!(
            timing(
                Phy::Dsss {
                    rate_kbps: 1000,
                    short: true
                },
                10
            ),
            None
        );
        assert_eq!(
            ack_phy(Phy::Dsss {
                rate_kbps: 1000,
                short: false
            }),
            Phy::Dsss {
                rate_kbps: 1000,
                short: false
            }
        );
        assert_eq!(
            ack_phy(Phy::ErpOfdm { rate_mbps: 54 }),
            Phy::ErpOfdm { rate_mbps: 24 }
        );
        assert_eq!(
            ack_phy(Phy::ErpOfdm { rate_mbps: 9 }),
            Phy::ErpOfdm { rate_mbps: 6 }
        );
        assert_eq!(
            ack_phy(Phy::HtMixed { mcs: 7 }),
            Phy::ErpOfdm { rate_mbps: 24 }
        );
    }
}
