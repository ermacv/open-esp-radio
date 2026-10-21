//! A paired, same-boot RX-DC acceptance rule. Range bounds and margins
//! remain the measured vendor values, including in the reported summary.
use super::{FieldSummary, OutputField, Result, Tolerance, Verdict, element};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize)]
pub(super) enum Rule {
    #[serde(rename = "rx-dc-gain0")]
    RxDcGain0,
}

impl Rule {
    pub(super) fn gains(self, field: &OutputField, review: &Tolerance) -> Result<u32> {
        // SOURCE: committed::CALIBRATION and rx/gain_calibration.rs::previous.
        // Wi-Fi's last five pairs belong to fine calibration, not gain search.
        let gains = match (field.name, field.width, field.count) {
            ("shared-rx-dc", 2, 22) => 11,
            ("wifi-rx-dc", 2, 26) => 8,
            _ => return Err(format!("initial-pair rule is not defined for {}", field.name).into()),
        };
        if review.signed || review.mask.is_some() || review.excluded {
            return Err("initial-pair rule requires complete unsigned RX-DC pairs".into());
        }
        Ok(gains)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum PairMatchReason {
    #[serde(rename = "initial pair, vendor-faithful")]
    InitialPairVendorFaithful,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct InitialPairMatch {
    /// One-based index in the compared subject's captures (also for CONTROL).
    pub capture: usize,
    /// Zero-based gain index in this RX-DC bank.
    pub gain: u32,
    pub pair: [i64; 2],
    pub reason: PairMatchReason,
}

pub(super) fn compare(
    field: &OutputField,
    review: &Tolerance,
    production: &[Vec<u8>],
    summary: &mut FieldSummary,
) -> Result<()> {
    let rule = review.initial_pair.ok_or("missing initial-pair rule")?;
    let gains = rule.gains(field, review)?;
    if summary.vendor.len() != field.count as usize {
        return Err("RX-DC bounds do not cover every pair".into());
    }
    let mut within = true;
    for (capture, bytes) in production.iter().enumerate() {
        let pair_at = |gain| -> Result<[i64; 2]> {
            Ok([
                element(bytes, field.output, field, gain * 2, false)?,
                element(bytes, field.output, field, gain * 2 + 1, false)?,
            ])
        };
        let initial = pair_at(0)?;
        for (gain, bounds) in summary.vendor.chunks_exact(2).enumerate() {
            let gain = gain as u32;
            let pair = pair_at(gain)?;
            if pair
                .iter()
                .zip(bounds)
                .all(|(&v, &[low, high])| low - summary.margin <= v && v <= high + summary.margin)
            {
                continue;
            }
            if gain > 0 && gain < gains && pair == initial {
                summary.initial_pairs.push(InitialPairMatch {
                    capture: capture + 1,
                    gain,
                    pair,
                    reason: PairMatchReason::InitialPairVendorFaithful,
                });
            } else {
                within = false;
            }
        }
    }
    summary.verdict = if within {
        Verdict::Match
    } else {
        Verdict::Diff
    };
    Ok(())
}

#[cfg(test)]
mod tests;
