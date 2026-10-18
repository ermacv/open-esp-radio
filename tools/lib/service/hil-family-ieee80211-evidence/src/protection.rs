//! How a station target follows BSS protection, judged from independent air
//! frames: every data PPDU it sends to the AP needs a preceding RTS/CTS
//! exchange at the control rate the protection requires, and the RTS NAV must
//! cover the exchange through the AP's response.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    Result,
    air::{AirFrame, AirPhy, BssRates, FrameKind, MacAddress},
};

/// The RTS and its CTS are SIFS apart; anything slower is not one exchange.
const EXCHANGE_GAP_MICROS: u64 = 1_000;
/// A response belongs to the exchange when it starts within the RTS NAV
/// plus this margin; the NAV check then judges it exactly.
const RESPONSE_SLACK_MICROS: u64 = 1_000;
/// TSFT stamps whole microseconds and PHYs round their durations.
const NAV_TOLERANCE_MICROS: u64 = 2;
const RTS_BYTES: u32 = 20;
const ACK_BYTES: u32 = 14;
const BLOCK_ACK_BYTES: u32 = 32;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProtectionEvidence {
    pub target: Option<String>,
    pub receiver: Option<String>,
    pub data_ppdus: u32,
    /// Data PPDUs immediately preceded, within its NAV, by the target's RTS
    /// or by a CTS to the target.
    pub protected_ppdus: u32,
    /// Protected PPDUs whose RTS the observer lost (only its CTS was seen).
    pub rts_unobserved: u32,
    /// Protected PPDUs whose CTS the observer lost (only the RTS was seen).
    pub cts_unobserved: u32,
    pub target_rts: u32,
    /// RTS frames at a rate outside the BSS basic and mandatory rates, not
    /// DSSS/HR under ERP protection, or whose rate the capture did not record.
    pub wrong_control_rate: u32,
    /// Protected PPDUs with an observed response, whose NAV was checked.
    pub nav_evaluated: u32,
    /// Protected PPDUs whose RTS NAV ended before the response ended.
    pub nav_short: u32,
    /// The first NAV shortfall, for diagnosis.
    pub first_nav_shortfall_micros: Option<u64>,
}

impl ProtectionEvidence {
    /// Share of data PPDUs preceded by RTS/CTS, in basis points.
    pub fn protected_basis_points(&self) -> u64 {
        u64::from(self.protected_ppdus) * 10_000 / u64::from(self.data_ppdus.max(1))
    }
}

/// The protected data flow: the target transmits to one receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Flow {
    pub target: MacAddress,
    pub receiver: MacAddress,
}

impl Flow {
    /// A station target: the single sender, other than `peer`, of data to
    /// the AP `bssid`.
    pub fn station(
        frames: &[AirFrame],
        bssid: MacAddress,
        peer: Option<MacAddress>,
    ) -> Result<Self> {
        let target = dominant(
            frames,
            |frame| frame.receiver == Some(bssid),
            |frame| frame.transmitter,
            peer,
            "station sending data to the AP",
        )?;
        Ok(Self {
            target,
            receiver: bssid,
        })
    }

    /// An access-point target serving `peer` and one other station: the AP
    /// is the sender of data to `peer`, and the receiver is the station, other
    /// than `peer`, that receives the AP's individually addressed data.
    pub fn access_point(frames: &[AirFrame], peer: MacAddress) -> Result<Self> {
        let target = dominant(
            frames,
            |frame| frame.receiver == Some(peer),
            |frame| frame.transmitter,
            None,
            "access point sending data to the laptop",
        )?;
        let receiver = dominant(
            frames,
            |frame| {
                frame.transmitter == Some(target) && frame.receiver != Some(MacAddress::BROADCAST)
            },
            |frame| frame.receiver,
            Some(peer),
            "station receiving the access point's data",
        )?;
        Ok(Self { target, receiver })
    }
}

/// Analyze how `flow.target` protects its data PPDUs to `flow.receiver` in
/// the BSS advertising `bss`.
pub fn analyze(frames: &[AirFrame], flow: Flow, bss: &BssRates) -> Result<ProtectionEvidence> {
    let Flow { target, receiver } = flow;
    let timeline = timeline(frames, target, receiver)?;
    let mut evidence = ProtectionEvidence {
        target: Some(target.to_string()),
        receiver: Some(receiver.to_string()),
        ..ProtectionEvidence::default()
    };
    for (index, &(time, event)) in timeline.iter().enumerate() {
        let rts = match event {
            Event::Rts(frame) => {
                evidence.target_rts += 1;
                if !control_rate_allowed(frame, bss) {
                    evidence.wrong_control_rate += 1;
                }
                continue;
            }
            Event::Ppdu { .. } => {
                evidence.data_ppdus += 1;
                // Either observed half of the exchange shows it: a CTS to the
                // target answers only the target's RTS. The observer loses
                // single control frames, so both halves are not required.
                let previous = index.checked_sub(1).map(|previous| timeline[previous]);
                let earlier = index.checked_sub(2).map(|earlier| timeline[earlier]);
                match (earlier, previous) {
                    (Some((rts_time, Event::Rts(rts))), Some((cts_time, Event::Cts(cts))))
                        if within_nav(cts_time, cts, time)
                            && cts_time - rts_time <= EXCHANGE_GAP_MICROS =>
                    {
                        Some((rts_time, rts))
                    }
                    (_, Some((cts_time, Event::Cts(cts)))) if within_nav(cts_time, cts, time) => {
                        evidence.rts_unobserved += 1;
                        None
                    }
                    (_, Some((rts_time, Event::Rts(rts)))) if within_nav(rts_time, rts, time) => {
                        evidence.cts_unobserved += 1;
                        Some((rts_time, rts))
                    }
                    _ => continue,
                }
            }
            Event::UntimedPpdu => {
                evidence.data_ppdus += 1;
                continue;
            }
            Event::Cts(_) | Event::Response(_) => continue,
        };
        evidence.protected_ppdus += 1;
        // The NAV is judged where the RTS that set it was observed.
        let Some((rts_time, rts)) = rts else {
            continue;
        };
        let Some(duration) = rts.duration_micros.map(u64::from) else {
            continue;
        };
        // The response is the next event, unless the observer lost it and a
        // later exchange's response follows outside this NAV.
        let aggregated = matches!(event, Event::Ppdu { aggregated: true });
        let response =
            timeline[index + 1..]
                .first()
                .and_then(|&(response_time, event)| match event {
                    // An Ack carries no transmitter: after an A-MPDU it answers
                    // another exchange of the target, not this one.
                    Event::Response(frame)
                        if response_time - rts_time <= duration + RESPONSE_SLACK_MICROS
                            && (frame.kind == FrameKind::BLOCK_ACK) == aggregated =>
                    {
                        Some((response_time, frame))
                    }
                    _ => None,
                });
        let Some((response_time, response)) = response else {
            continue;
        };
        let (Some(rts_airtime), Some(response_airtime)) = (
            control_airtime(rts, RTS_BYTES),
            control_airtime(
                response,
                if response.kind == FrameKind::ACK {
                    ACK_BYTES
                } else {
                    BLOCK_ACK_BYTES
                },
            ),
        ) else {
            continue;
        };
        evidence.nav_evaluated += 1;
        let nav_end = rts_time + rts_airtime + duration + NAV_TOLERANCE_MICROS;
        let response_end = response_time + response_airtime;
        if nav_end < response_end {
            evidence.nav_short += 1;
            evidence
                .first_nav_shortfall_micros
                .get_or_insert(response_end - nav_end);
        }
    }
    Ok(evidence)
}

#[derive(Clone, Copy, Debug)]
enum Event<'a> {
    Rts(&'a AirFrame),
    Cts(&'a AirFrame),
    /// A data PPDU, timed by the one MPDU the observer stamped. An A-MPDU
    /// is answered by a BlockAck, a single MPDU by an Ack.
    Ppdu {
        aggregated: bool,
    },
    /// A data PPDU whose stamped MPDU the observer lost. It counts as a PPDU
    /// but can never be shown to be protected.
    UntimedPpdu,
    Response(&'a AirFrame),
}

/// The target's exchanges ordered by TSFT. Capture order is not air order:
/// the observer delivers control frames and A-MPDUs on different paths, and
/// stamps TSFT on a single MPDU of each A-MPDU.
fn timeline<'a>(
    frames: &'a [AirFrame],
    target: MacAddress,
    receiver: MacAddress,
) -> Result<Vec<(u64, Event<'a>)>> {
    let mut timeline = Vec::new();
    let mut aggregates = BTreeMap::<u32, Option<u64>>::new();
    let mut untimed = 0;
    for frame in frames
        .iter()
        .filter(|frame| relevant(frame, target, receiver))
    {
        let event = match frame.kind {
            FrameKind::RTS => Event::Rts(frame),
            FrameKind::CTS => Event::Cts(frame),
            FrameKind::ACK | FrameKind::BLOCK_ACK => Event::Response(frame),
            _ => {
                match frame.ampdu_reference {
                    Some(reference) => {
                        let stamp = aggregates.entry(reference).or_default();
                        *stamp = stamp.or(frame.mac_time_micros);
                    }
                    None => match frame.mac_time_micros {
                        Some(time) => timeline.push((time, Event::Ppdu { aggregated: false })),
                        None => untimed += 1,
                    },
                }
                continue;
            }
        };
        timeline.push((timed(frame)?, event));
    }
    for stamp in aggregates.into_values() {
        match stamp {
            Some(time) => timeline.push((time, Event::Ppdu { aggregated: true })),
            None => untimed += 1,
        }
    }
    timeline.sort_by_key(|(time, _)| *time);
    // Untimed PPDUs precede everything, so no exchange is attributed to them.
    timeline.splice(0..0, (0..untimed).map(|_| (0, Event::UntimedPpdu)));
    Ok(timeline)
}

/// A control frame's rate must be in the BSSBasicRateSet or a mandatory
/// rate of its modulation class, and DSSS/HR under ERP protection.
fn control_rate_allowed(frame: &AirFrame, bss: &BssRates) -> bool {
    let (Some(phy), Some(rate)) = (frame.phy, frame.rate_kbps) else {
        return false;
    };
    let mandatory: &[u32] = match phy {
        phy if phy.is_dsss() => &[1_000, 2_000, 5_500, 11_000],
        AirPhy::Ofdm if !bss.erp_use_protection => &[6_000, 12_000, 24_000],
        _ => return false,
    };
    bss.basic_kbps.contains(&rate) || mandatory.contains(&rate)
}

/// A PPDU stamped at `time` lies inside the NAV a control frame set.
fn within_nav(control_time: u64, control: &AirFrame, time: u64) -> bool {
    control
        .duration_micros
        .is_some_and(|duration| time - control_time <= u64::from(duration))
}

/// The TSFT of a frame whose time the analysis measures.
fn timed(frame: &AirFrame) -> Result<u64> {
    frame.mac_time_micros.ok_or_else(|| {
        format!(
            "protection evidence requires TSFT on {:#06x} frames",
            frame.kind.0
        )
        .into()
    })
}

fn relevant(frame: &AirFrame, target: MacAddress, receiver: MacAddress) -> bool {
    let to_target = frame.receiver == Some(target);
    let flow = frame.transmitter == Some(target) && frame.receiver == Some(receiver);
    match frame.kind {
        FrameKind::RTS => flow,
        FrameKind::CTS | FrameKind::ACK => to_target,
        // The target also receives BlockAcks for other flows; keep the
        // flow's receiver so another station's response is not attributed.
        FrameKind::BLOCK_ACK => to_target && frame.transmitter == Some(receiver),
        kind => kind.is_data() && flow,
    }
}

/// The address that `address` yields most often among data frames matching
/// `selected`, excluding `excluded`; it must carry nine tenths of them.
fn dominant(
    frames: &[AirFrame],
    selected: impl Fn(&AirFrame) -> bool,
    address: impl Fn(&AirFrame) -> Option<MacAddress>,
    excluded: Option<MacAddress>,
    role: &str,
) -> Result<MacAddress> {
    let mut counts = BTreeMap::<MacAddress, u32>::new();
    for frame in frames {
        if let Some(candidate) = address(frame)
            && frame.kind.is_data()
            && selected(frame)
            && Some(candidate) != excluded
        {
            *counts.entry(candidate).or_default() += 1;
        }
    }
    let total = counts.values().sum::<u32>();
    let (winner, count) = counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .ok_or_else(|| format!("the air capture shows no {role}"))?;
    if u64::from(count) * 10 < u64::from(total) * 9 {
        return Err(format!("no single {role} dominates ({count} of {total} frames)").into());
    }
    Ok(winner)
}

/// Airtime of a control frame of `bytes` at the captured rate, including the
/// PLCP preamble and header and, for ERP-OFDM, the signal extension.
fn control_airtime(frame: &AirFrame, bytes: u32) -> Option<u64> {
    let rate_kbps = u64::from(frame.rate_kbps?);
    if rate_kbps == 0 {
        return None;
    }
    let bits = u64::from(bytes) * 8;
    match frame.phy? {
        phy if phy.is_dsss() => {
            let preamble = if frame.short_preamble? && rate_kbps > 1_000 {
                96
            } else {
                192
            };
            Some(preamble + (bits * 1_000).div_ceil(rate_kbps))
        }
        AirPhy::Ofdm => {
            let bits_per_symbol = rate_kbps * 4 / 1_000;
            let symbols = (16 + bits + 6).div_ceil(bits_per_symbol);
            Some(20 + symbols * 4 + 6)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
