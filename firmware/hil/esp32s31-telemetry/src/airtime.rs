//! Bounded AP-epoch history. Association generations are never merged/evicted.
use oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::{
    AccessPointAirtimePeer, AirtimeAction, AirtimeObservation,
};
use oer_hil_protocol::{
    wifi::WifiAirtimePeer, wifi::WifiAirtimePeerEvidence, wifi::WifiAirtimeReport,
};

#[derive(Clone, Copy)]
pub struct AirtimeHistory {
    pub peers: [Option<WifiAirtimePeerEvidence>; 8],
    pub report: WifiAirtimeReport,
}

impl Default for AirtimeHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl AirtimeHistory {
    pub const fn new() -> Self {
        Self {
            peers: [None; 8],
            report: WifiAirtimeReport {
                peer_records: 0,
                dropped_events: 0,
                saturated: false,
            },
        }
    }

    pub fn observe(&mut self, event: AirtimeObservation<AccessPointAirtimePeer>) {
        let peer = match event.key {
            AccessPointAirtimePeer::Group => WifiAirtimePeer::Group,
            AccessPointAirtimePeer::Unicast(identity) => WifiAirtimePeer::Unicast {
                address: identity.address(),
                association_id: identity.association_id(),
                association_epoch: identity.association_epoch(),
            },
        };
        let index = self
            .peers
            .iter()
            .position(|entry| entry.is_some_and(|e| e.peer == peer))
            .or_else(|| self.peers.iter().position(Option::is_none));
        let Some(index) = index else {
            add(
                &mut self.report.dropped_events,
                1,
                &mut self.report.saturated,
            );
            return;
        };
        if self.peers[index].is_none() {
            self.report.peer_records += 1;
            self.peers[index] = Some(WifiAirtimePeerEvidence {
                peer,
                grants: 0,
                granted_micros: 0,
                settlements: 0,
                settled_grants_micros: 0,
                charged_micros: 0,
                cancellations: 0,
                cancelled_grants_micros: 0,
                balance_after_last_event_micros: 0,
                outstanding: 0,
                outstanding_micros: 0,
                maximum_outstanding: 0,
            });
        }
        let entry = self.peers[index].as_mut().expect("inserted history entry");
        let saturated = &mut self.report.saturated;
        match event.action {
            AirtimeAction::Granted => {
                add(&mut entry.grants, 1, saturated);
                add(
                    &mut entry.granted_micros,
                    u64::from(event.grant_micros),
                    saturated,
                );
            }
            AirtimeAction::Settled => {
                add(&mut entry.settlements, 1, saturated);
                add(
                    &mut entry.settled_grants_micros,
                    u64::from(event.grant_micros),
                    saturated,
                );
                add(
                    &mut entry.charged_micros,
                    u64::from(event.charged_micros),
                    saturated,
                );
            }
            AirtimeAction::Cancelled => {
                add(&mut entry.cancellations, 1, saturated);
                add(
                    &mut entry.cancelled_grants_micros,
                    u64::from(event.grant_micros),
                    saturated,
                );
            }
        }
        entry.balance_after_last_event_micros = event.balance_after_event_micros;
        entry.outstanding = event.outstanding;
        entry.outstanding_micros = event.outstanding_micros;
        entry.maximum_outstanding = entry.maximum_outstanding.max(event.outstanding);
    }
}

fn add(counter: &mut u64, amount: u64, saturated: &mut bool) {
    match counter.checked_add(amount) {
        Some(value) => *counter = value,
        None => {
            *counter = u64::MAX;
            *saturated = true;
        }
    }
}

#[cfg(test)]
mod tests;
