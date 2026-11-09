//! Where the sniffer's TSFT latches in a received PPDU, from immediate
//! responses: the latch calibration of the lease's preflight.
//!
//! The probe sends data frames individually addressed to the responder
//! board, which answers each with an ACK one SIFS after the data PPDU's
//! end (IEEE Std 802.11-2020 10.3.2.3; aSIFSTime 10 us at 2.4 GHz, 15.4.4.5
//! and 18.5.2). That interval is fixed by the standard and the responder's
//! hardware: it is independent of the probe's publication times and of the
//! sniffer. With the data PPDU's preamble `P_d` and MPDU part `T_d`, and
//! the ACK's `P_a` and `T_a` (`airtime.rs`), the TSFT difference ACK − data
//! is predicted as:
//!
//! - latch at the PPDU start: `P_d + T_d + SIFS`;
//! - latch at the MPDU's first bit (radiotap's definition): `T_d + SIFS + P_a`;
//! - latch at the PPDU end: `SIFS + P_a + T_a`.
//!
//! The probe sweeps the data length at DSSS 1 Mb/s (both start latches
//! move with `T_d`, the end latch does not) and sends HT-mixed MCS 0 data
//! answered by a 6-Mb/s OFDM ACK (`P_d` = 36 us, `P_a` = 20 us: the two
//! start latches differ by 16 us). A constant offset of the latch cancels in
//! a difference; the TSFT scale is the run's continuous 1-us scale, its rate
//! bounded by the decision's fit. This names no hardware counter.
//!
//! **Rule.** A hypothesis is compatible when every pair's residual against
//! its prediction is within `T = 2 Q + E_SIFS + RHO * dS + 4 sigma`, with
//! `sigma` the pooled scatter of the pairs that repeat one length and PHY.
//! The error must be bounded: at least `REPEATS` pairs in each of at least
//! `GROUPS` groups and `sigma <= SIGMA_MAX`. The outcome is the one
//! compatible hypothesis; none or more than one is insufficient evidence.

use crate::airtime::{ACK_PSDU, Phy, timing};
use crate::capture::{AirFrame, Capture};

/// The calibration frames' transmitter and BSSID (the probe's `frames.rs`).
pub const CALIBRATION_BSSID: [u8; 6] = [0x02, 0x4f, 0x45, 0x00, 0x00, 0x03];
/// The responder board's address.
pub const RESPONDER: [u8; 6] = [0x02, 0x4f, 0x45, 0x00, 0x00, 0x02];
/// The probe's calibration frames per boot.
pub const FRAMES: usize = 24;
const ACK: u8 = 0xd4;

fn phy_of(code: u8) -> Option<Phy> {
    match code {
        b'l' => Some(Phy::Dsss {
            rate_kbps: 1000,
            short: false,
        }),
        b'h' => Some(Phy::HtMixed { mcs: 0 }),
        _ => None,
    }
}

fn tsft_interval(from: u64, to: u64) -> f64 {
    if from < 1 << 32 && to < 1 << 32 {
        // A 32-bit sniffer TSFT (mt76): the difference across its wrap.
        f64::from((to as u32).wrapping_sub(from as u32) as i32)
    } else {
        (i128::from(to) - i128::from(from)) as f64
    }
}

/// The calibration pairs of one boot (`run`, or the first boot whatever
/// its run when `None`): each calibration data frame to the responder and
/// the ACK to its transmitter that the capture holds right after it. How
/// many calibration frames had no such ACK.
pub fn pairs(capture: &Capture, run: Option<u32>) -> (Vec<Pair>, usize) {
    let frames = &capture.frames;
    let mut first_run = run;
    let mut pairs = Vec::new();
    let mut unanswered = 0;
    for (index, frame) in frames.iter().enumerate() {
        let Some(tag) = frame.tag.filter(|tag| tag.kind == b'c') else {
            continue;
        };
        if frame.receiver != RESPONDER || frame.transmitter != Some(CALIBRATION_BSSID) {
            continue;
        }
        if *first_run.get_or_insert(tag.run) != tag.run {
            continue;
        }
        let Some(data) = phy_of(tag.arm) else {
            continue;
        };
        let answer: Option<&AirFrame> = frames.get(index + 1);
        match answer {
            Some(ack) if ack.frame_control == ACK && ack.receiver == CALIBRATION_BSSID => {
                pairs.push(Pair {
                    data,
                    psdu: frame.psdu,
                    interval: tsft_interval(frame.tsft, ack.tsft),
                });
            }
            _ => unanswered += 1,
        }
    }
    (pairs, unanswered)
}

/// TSFT resolution, us.
pub const Q_US: f64 = 1.0;
/// The responder's SIFS tolerance, us.
pub const E_SIFS_US: f64 = 2.0;
/// The largest TSFT rate error the decision's fit admits.
pub const RHO: f64 = 100e-6;
pub const SIGMA_MAX_US: f64 = 2.0;
pub const REPEATS: usize = 3;
pub const GROUPS: usize = 3;
pub const SIFS_US: f64 = 10.0;

/// One data frame and the ACK the capture saw after it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub data: Phy,
    /// The data's MPDU and FCS, octets.
    pub psdu: u32,
    /// `TSFT(ACK) − TSFT(data)`, us.
    pub interval: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Latch {
    PpduStart,
    MpduStart,
    PpduEnd,
}

const LATCHES: [Latch; 3] = [Latch::PpduStart, Latch::MpduStart, Latch::PpduEnd];

#[derive(Clone, Debug, PartialEq)]
pub enum Shortfall {
    /// A PHY whose duration the reference cannot compute.
    UnknownPhy,
    /// Fewer than `GROUPS` groups of at least `REPEATS` pairs.
    Pairs { groups: usize },
    /// The pairs' scatter exceeds `SIGMA_MAX_US`.
    Scatter { sigma: f64 },
    /// More than one hypothesis fits within the bound.
    Ambiguous(Vec<Latch>),
    /// No hypothesis fits within the bound.
    NoneFits,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Latch(Latch),
    Insufficient(Shortfall),
}

/// What the outcome rests on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Evidence {
    pub sigma: f64,
    pub tolerance_max: f64,
    /// Each hypothesis's largest residual, us.
    pub worst: [f64; 3],
}

fn predicted(latch: Latch, pair: &Pair) -> Option<f64> {
    let data = timing(pair.data, pair.psdu)?;
    let ack = timing(crate::airtime::ack_phy(pair.data), ACK_PSDU)?;
    let sifs = SIFS_US;
    Some(match latch {
        Latch::PpduStart => f64::from(data.total()) + sifs,
        Latch::MpduStart => f64::from(data.mpdu) + sifs + f64::from(ack.preamble),
        Latch::PpduEnd => sifs + f64::from(ack.total()),
    })
}

/// The pooled within-group standard deviation, and how many groups have
/// at least `REPEATS` pairs.
fn scatter(pairs: &[Pair]) -> (f64, usize) {
    let mut groups: Vec<(Phy, u32, Vec<f64>)> = Vec::new();
    for pair in pairs {
        match groups
            .iter_mut()
            .find(|(phy, psdu, _)| *phy == pair.data && *psdu == pair.psdu)
        {
            Some((_, _, values)) => values.push(pair.interval),
            None => groups.push((pair.data, pair.psdu, vec![pair.interval])),
        }
    }
    let mut squares = 0.0;
    let mut freedom = 0usize;
    let mut full = 0usize;
    for (_, _, values) in &groups {
        if values.len() >= REPEATS {
            full += 1;
        }
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        squares += values.iter().map(|v| (v - mean).powi(2)).sum::<f64>();
        freedom += values.len().saturating_sub(1);
    }
    let sigma = if freedom == 0 {
        f64::INFINITY
    } else {
        (squares / freedom as f64).sqrt()
    };
    (sigma, full)
}

pub fn decide(pairs: &[Pair]) -> (Outcome, Evidence) {
    let mut evidence = Evidence::default();
    let (sigma, groups) = scatter(pairs);
    evidence.sigma = sigma;
    if groups < GROUPS {
        return (Outcome::Insufficient(Shortfall::Pairs { groups }), evidence);
    }
    if sigma > SIGMA_MAX_US {
        return (
            Outcome::Insufficient(Shortfall::Scatter { sigma }),
            evidence,
        );
    }
    let mut compatible = Vec::new();
    for (index, latch) in LATCHES.iter().enumerate() {
        let mut fits = true;
        for pair in pairs {
            let Some(prediction) = predicted(*latch, pair) else {
                return (Outcome::Insufficient(Shortfall::UnknownPhy), evidence);
            };
            let tolerance = 2.0 * Q_US + E_SIFS_US + RHO * pair.interval.abs() + 4.0 * sigma;
            evidence.tolerance_max = evidence.tolerance_max.max(tolerance);
            let residual = (pair.interval - prediction).abs();
            if let Some(worst) = evidence.worst.get_mut(index) {
                *worst = worst.max(residual);
            }
            fits &= residual <= tolerance;
        }
        if fits {
            compatible.push(*latch);
        }
    }
    let outcome = match compatible.as_slice() {
        [latch] => Outcome::Latch(*latch),
        [] => Outcome::Insufficient(Shortfall::NoneFits),
        _ => Outcome::Insufficient(Shortfall::Ambiguous(compatible)),
    };
    (outcome, evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DSSS: Phy = Phy::Dsss {
        rate_kbps: 1000,
        short: false,
    };
    const HT0: Phy = Phy::HtMixed { mcs: 0 };

    /// The pairs a sniffer latching at `latch` measures, with a scatter of
    /// `jitter` around the prediction.
    fn sweep(latch: Latch, phys: &[Phy], jitter: f64) -> Vec<Pair> {
        let mut pairs = Vec::new();
        for phy in phys {
            for psdu in [100u32, 500, 1000] {
                for repeat in 0..4 {
                    let probe = Pair {
                        data: *phy,
                        psdu,
                        interval: 0.0,
                    };
                    let interval =
                        predicted(latch, &probe).unwrap() + jitter * [-1.0, 1.0, -0.5, 0.5][repeat];
                    pairs.push(Pair { interval, ..probe });
                }
            }
        }
        pairs
    }

    #[test]
    fn pairs_take_the_ack_that_follows_each_calibration_frame() {
        use crate::capture::Tag;
        let tag = |n: u16, arm: u8, run: u32| {
            Some(Tag {
                kind: b'c',
                arm,
                n,
                run,
                k: 0,
            })
        };
        let data = |tsft: u64, n: u16, arm: u8, run: u32, psdu: u32| AirFrame {
            tsft,
            frame_control: 0x08,
            receiver: RESPONDER,
            transmitter: Some(CALIBRATION_BSSID),
            psdu,
            stamp: None,
            tag: tag(n, arm, run),
        };
        let ack = |tsft: u64| AirFrame {
            tsft,
            frame_control: ACK,
            receiver: CALIBRATION_BSSID,
            transmitter: None,
            psdu: 14,
            stamp: None,
            tag: None,
        };
        let capture = Capture {
            frames: vec![
                data(1_000, 0, b'l', 7, 100),
                ack(1_000 + 1002),
                // Unanswered: the next frame is another data frame.
                data(5_000, 1, b'h', 7, 500),
                data(9_000, 2, b'h', 7, 500),
                ack(9_000 + 210),
                // Across the 32-bit wrap.
                data((1 << 32) - 100, 3, b'l', 7, 1000),
                ack(8_100),
                // Another boot's frame.
                data(20_000, 0, b'l', 8, 100),
                ack(21_002),
            ],
            without_tsft: 0,
            bad_fcs: 0,
        };
        let (pairs, unanswered) = pairs(&capture, None);
        assert_eq!(unanswered, 1);
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0].interval, 1002.0);
        assert_eq!(pairs[1].data, HT0);
        assert_eq!(pairs[2].interval, 8_200.0);
        assert_eq!(pairs[2].psdu, 1000);
    }

    #[test]
    fn each_latch_is_found_when_the_sweep_separates_it() {
        for latch in LATCHES {
            let (outcome, evidence) = decide(&sweep(latch, &[DSSS, HT0], 0.6));
            assert_eq!(outcome, Outcome::Latch(latch), "{evidence:?}");
        }
    }

    #[test]
    fn the_two_start_latches_need_a_preamble_that_differs() {
        // DSSS data answered by a DSSS ACK: the same 192-us preamble.
        let (outcome, _) = decide(&sweep(Latch::MpduStart, &[DSSS], 0.5));
        assert_eq!(
            outcome,
            Outcome::Insufficient(Shortfall::Ambiguous(vec![
                Latch::PpduStart,
                Latch::MpduStart
            ]))
        );
        let (outcome, _) = decide(&sweep(Latch::PpduEnd, &[DSSS], 0.5));
        assert_eq!(outcome, Outcome::Latch(Latch::PpduEnd));
    }

    #[test]
    fn a_slope_between_the_hypotheses_is_insufficient_whatever_its_scatter() {
        // Half way between a start latch and the end latch: 4 us/octet.
        let mut pairs = Vec::new();
        for psdu in [100u32, 500, 1000] {
            for repeat in 0..4 {
                pairs.push(Pair {
                    data: DSSS,
                    psdu,
                    interval: 300.0 + 4.0 * f64::from(psdu) + [-0.5, 0.5, 0.0, 0.2][repeat],
                });
            }
        }
        assert_eq!(decide(&pairs).0, Outcome::Insufficient(Shortfall::NoneFits));
        // The owner's case: slope 4 us/octet with a scatter of 10 us/octet
        // over the sweep is no bounded error.
        let wide: Vec<Pair> = pairs
            .iter()
            .enumerate()
            .map(|(index, pair)| Pair {
                interval: pair.interval + if index % 2 == 0 { 4000.0 } else { -4000.0 },
                ..*pair
            })
            .collect();
        assert!(matches!(
            decide(&wide).0,
            Outcome::Insufficient(Shortfall::Scatter { .. })
        ));
    }

    #[test]
    fn too_few_repeats_or_an_unknown_phy_are_insufficient() {
        let pairs = sweep(Latch::MpduStart, &[DSSS, HT0], 0.5);
        assert!(matches!(
            decide(&pairs[..6]).0,
            Outcome::Insufficient(Shortfall::Pairs { .. })
        ));
        let mut unknown = pairs.clone();
        unknown[0].data = Phy::ErpOfdm { rate_mbps: 7 };
        assert_eq!(
            decide(&unknown).0,
            Outcome::Insufficient(Shortfall::UnknownPhy)
        );
    }
}
