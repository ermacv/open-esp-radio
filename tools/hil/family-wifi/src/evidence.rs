//! Secret-free observations. The host computes every gate from these values;
//! an image's verdict is never an input.
use oer_lab_router::w3::{Echo, Lease, Station};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyEvidence {
    pub channel: u8,
    pub prepared: oer_hil_protocol::lower_mac::Prepared,
    pub hardware: Vec<oer_hil_protocol::lower_mac::CaseTrace>,
}
impl KeyEvidence {
    pub(crate) fn complete(&self) -> bool {
        use oer_ieee80211_port::conformance::{BarrierCase, Execution, catalog::Case};
        let required: Vec<_> = Case::all()
            .enumerate()
            .filter(|(_, case)| {
                matches!(case, Case::Group(_) | Case::Barrier(BarrierCase::RemoveKey))
                    && case.execution() == Execution::Hardware
            })
            .collect();
        self.hardware.len() == required.len()
            && !required.is_empty()
            && required.into_iter().all(|(index, case)| {
                let mut rows = self
                    .hardware
                    .iter()
                    .filter(|row| usize::from(row.case.0) == index);
                let Some(row) = rows.next() else {
                    return false;
                };
                rows.next().is_none()
                    && oer_hil_family_lower_mac::model_trace(case, &self.prepared, self.channel)
                        .is_ok_and(|model| model == row.trace)
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handshake {
    pub epoch: u32,
    pub association_us: u64,
    pub m1_us: u64,
    pub m2_us: u64,
    pub m3_us: u64,
    pub m4_us: u64,
    pub connected_us: u64,
    pub router_authorized: bool,
}
impl Handshake {
    fn complete(&self) -> bool {
        self.epoch > 0
            && self.association_us > 0
            && self.association_us <= self.m1_us
            && self.m1_us < self.m2_us
            && self.m2_us < self.m3_us
            && self.m3_us < self.m4_us
            && self.m4_us <= self.connected_us
            && self.router_authorized
    }
    fn within_bound(&self) -> bool {
        self.complete() && self.m4_us - self.association_us <= 1_000_000
    }
}

/// ICMP identities come from the independent router capture, with target-side
/// counts retained separately. Timestamps are the capture's epoch microseconds.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Traffic {
    pub count: u16,
    pub address: Ipv4Addr,
    pub router: Ipv4Addr,
    pub target_sent: u16,
    pub target_received: u16,
    pub target_peer_requests: u16,
    pub target_peer_replies: u16,
    pub router_sent: u16,
    pub router_received: u16,
    pub echoes: Vec<Echo>,
}
impl Traffic {
    fn pairs(&self, source: Ipv4Addr, destination: Ipv4Addr) -> bool {
        use std::collections::BTreeMap;
        if self.count == 0 || self.address == self.router {
            return false;
        }
        if self.echoes.len() != usize::from(self.count) * 4 {
            return false;
        }
        let mut requests = BTreeMap::new();
        let mut replies = BTreeMap::new();
        let mut previous = None;
        for echo in &self.echoes {
            if previous.is_some_and(|previous| echo.at_us < previous) {
                return false;
            }
            previous = Some(echo.at_us);
            let key = (echo.identifier, echo.sequence);
            if echo.source == source && echo.destination == destination && !echo.reply {
                requests.entry(key).or_insert(echo.at_us);
            } else if echo.source == destination && echo.destination == source && echo.reply {
                let Some(&request) = requests.get(&key) else {
                    return false;
                };
                if echo.at_us < request {
                    return false;
                }
                replies.entry(key).or_insert(echo.at_us);
            }
        }
        if requests.len() != usize::from(self.count) || replies.len() != requests.len() {
            return false;
        }
        let mut identity = None;
        let mut times: Vec<_> = requests.into_iter().collect();
        times.sort_by_key(|(_, at)| *at);
        times
            .iter()
            .enumerate()
            .all(|(index, ((id, sequence), at))| {
                let (expected, first) = *identity.get_or_insert((*id, *sequence));
                *id == expected
                    && sequence.wrapping_sub(first) == index as u16
                    && (index == 0
                        || at
                            .checked_sub(times[index - 1].1)
                            .is_some_and(|gap| (750_000..=1_250_000).contains(&gap)))
            })
    }
    fn complete(&self, count: u16) -> bool {
        self.count == count
            && [
                self.target_sent,
                self.target_received,
                self.target_peer_requests,
                self.target_peer_replies,
                self.router_sent,
                self.router_received,
            ]
            .into_iter()
            .all(|value| value == count)
            && self.pairs(self.address, self.router)
            && self.pairs(self.router, self.address)
    }
    fn window(&self) -> Option<(u64, u64)> {
        Some((self.echoes.first()?.at_us, self.echoes.last()?.at_us))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rekey {
    /// Observed successful group handshake, not the configured rekey timer.
    pub completed_us: u64,
    pub old_key_id: u8,
    pub new_key_id: u8,
    pub epoch: u32,
    pub previous_generation: u32,
    pub generation: u32,
    pub triggered_us: u64,
    pub arp_frames: u32,
    pub request: Option<ArpObservation>,
    pub reply: Option<ArpObservation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArpObservation {
    pub received_us: u64,
    pub frame: oer_hil_protocol::wifi::StationArpFrame,
    /// Independently captured protected MPDU with this exact SN, PN and KeyID.
    pub air_us: Option<u64>,
}
impl Rekey {
    fn exchange(&self, t: &Traffic, end: u64) -> bool {
        let Some((request, reply)) = self.request.as_ref().zip(self.reply.as_ref()) else {
            return false;
        };
        let (q, a) = (&request.frame, &reply.frame);
        let Some((q_air, a_air)) = request.air_us.zip(reply.air_us) else {
            return false;
        };
        let within = |at: u64| {
            at >= self.triggered_us
                && at <= end
                && at.saturating_sub(self.triggered_us) <= 5_000_000
        };
        self.epoch > 0
            && self.generation > self.previous_generation
            && self.completed_us <= self.triggered_us
            && self.arp_frames > 0
            && [q, a].into_iter().all(|f| {
                f.epoch == self.epoch
                    && f.generation == self.generation
                    && f.sequence < 4096
                    && (1..(1 << 48)).contains(&f.packet_number)
            })
            && !q.transmitted
            && q.group_addressed
            && q.operation == 1
            && q.key_id == self.new_key_id
            && q.sender == t.router.octets()
            && q.target == t.address.octets()
            && a.transmitted
            && !a.group_addressed
            && a.operation == 2
            && a.key_id == 0
            && a.sender == t.address.octets()
            && a.target == t.router.octets()
            && [request.received_us, reply.received_us, q_air, a_air]
                .into_iter()
                .all(within)
            && q_air <= request.received_us
            && q_air < a_air
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Air {
    pub protected_station_frames: u32,
    pub plaintext_station_frames: u32,
    pub first_us: u64,
    pub last_us: u64,
    pub captured_frames: u32,
    pub dropped_frames: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub channel: u8,
    pub wpa2_ccmp: bool,
    pub ht20: bool,
    pub he_disabled: bool,
    pub pmf_disabled: bool,
    pub before: Station,
    pub joined: Station,
    pub lease: Lease,
    pub target_address: Ipv4Addr,
    pub target_gateway: Ipv4Addr,
    pub target_prefix: u8,
    pub tx_timeouts: u32,
    pub queue_collisions: u32,
    pub deauth_transmissions: u32,
    pub final_departure_us: u64,
    pub after: Station,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub baseline: Option<Baseline>,
    pub handshake: Option<Handshake>,
    pub traffic: Option<Traffic>,
    pub rekey: Option<Rekey>,
    pub air: Option<Air>,
    pub forced_deauth_us: Option<u64>,
    pub reconnect: Option<Handshake>,
    pub resumed: Option<Traffic>,
    pub keys: Option<KeyEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Gate {
    pub id: char,
    pub reason: Option<&'static str>,
}
impl Gate {
    pub fn passed(&self) -> bool {
        self.reason.is_none()
    }
}

pub fn evaluate(e: &Evidence) -> [Gate; 6] {
    let a = e
        .baseline
        .as_ref()
        .zip(e.traffic.as_ref())
        .is_some_and(|(b, t)| {
            (1..=13).contains(&b.channel)
                && b.wpa2_ccmp
                && b.ht20
                && b.he_disabled
                && b.pmf_disabled
                && b.before.valid()
                && !b.before.present
                && b.joined.joined()
                && b.lease.remaining_seconds > 0
                && b.lease.address == b.target_address
                && b.target_address == t.address
                && b.target_gateway == t.router
                && b.target_prefix == 24
                && t.complete(100)
                && b.tx_timeouts == 0
                && b.queue_collisions == 0
                && b.deauth_transmissions > 0
                && b.after.valid()
                && !b.after.present
                && b.final_departure_us > 0
        });
    let b = e.handshake.as_ref().is_some_and(Handshake::within_bound);
    let c = e
        .traffic
        .as_ref()
        .zip(e.rekey.as_ref())
        .is_some_and(|(t, r)| {
            t.complete(100)
                && t.window().is_some_and(|(start, end)| {
                    start < r.completed_us
                        && r.exchange(t, end)
                        && e.handshake.as_ref().is_some_and(|h| h.epoch == r.epoch)
                        && (1..=3).contains(&r.old_key_id)
                        && (1..=3).contains(&r.new_key_id)
                        && r.old_key_id != r.new_key_id
                })
        });
    let d = e
        .air
        .as_ref()
        .zip(e.handshake.as_ref())
        .zip(e.baseline.as_ref())
        .is_some_and(|((air, h), base)| {
            h.complete()
                && air.captured_frames > 0
                && air.protected_station_frames > 0
                && air.protected_station_frames <= air.captured_frames
                && air.plaintext_station_frames == 0
                && air.dropped_frames == 0
                && air.first_us <= h.association_us
                && air.first_us > 0
                && air.last_us >= air.first_us
                && base.final_departure_us > h.m4_us
                && air.last_us >= base.final_departure_us
        });
    let x = e
        .forced_deauth_us
        .zip(e.handshake.as_ref())
        .zip(e.reconnect.as_ref())
        .zip(e.resumed.as_ref())
        .is_some_and(|(((deauth, first), next), t)| {
            first.complete()
                && next.within_bound()
                && deauth > first.connected_us
                && next.association_us > deauth
                && next.epoch > first.epoch
                && t.complete(10)
                && t.window()
                    .is_some_and(|(start, _)| start >= next.connected_us)
        });
    let f = e.keys.as_ref().is_some_and(KeyEvidence::complete);
    [
        ('a', a, "missing or contradictory W3 association/DHCP/ICMP/departure/statistics evidence"),
        ('b', b, "missing/contradictory four-way handshake, Connected or router authorization; association to M4 exceeds 1 s"),
        ('c', c, "missing bidirectional 100/100, GTK change or protected broadcast ARP request/station reply on the new key"),
        ('d', d, "missing/truncated air window, zero protected station frames, capture loss or plaintext data"),
        ('e', x, "missing forced departure, fresh authorized handshake or bidirectional 10/10 after reconnect"),
        ('f', f, "missing or mismatching required hardware/model key traces"),
    ].map(|(id,pass,reason)| Gate { id, reason: (!pass).then_some(reason) })
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) fn test_evidence() -> Evidence {
    tests::full()
}
