//! PMF state owned by one association, separate from data replay frontiers.

use oer_ieee80211_mac::{
    address::{Bssid, OwnAddress},
    frame::DecodedHeader,
    management_protection::SaQuery,
};
use oer_ieee80211_rsn::{
    bip::{BipReceiver, MANAGEMENT_MIC_ELEMENT_LEN},
    frames::RsnIgtk,
    management_ccmp::ManagementCcmpReceiver,
};
use oer_time::Instant;

use crate::sa_query::{SaQueryStep, StationSaQuery};

/// All management events returned here were authenticated for this BSS.
pub(super) enum ManagementEvent {
    Disconnect(u16),
    Query(SaQuery),
}

pub(super) struct ManagementProtection {
    individual: ManagementCcmpReceiver,
    // The IGTK becomes usable at the corresponding group-install terminal.
    group: Option<BipReceiver>,
    query: StationSaQuery,
    request: Option<[u8; 2]>,
    response: Option<[u8; 2]>,
}

impl ManagementProtection {
    pub(super) fn new(temporal_key: &[u8; 16]) -> Self {
        Self {
            individual: ManagementCcmpReceiver::new(temporal_key),
            group: None,
            query: StationSaQuery::new(),
            request: None,
            response: None,
        }
    }

    pub(super) fn install_igtk(&mut self, igtk: &RsnIgtk) {
        match &mut self.group {
            Some(group) => group.rekey(igtk),
            None => self.group = Some(BipReceiver::new(igtk)),
        }
    }

    /// Metadata claiming integrity never bypasses CCMP/BIP verification.
    pub(super) fn receive(
        &mut self,
        frame: &[u8],
        local: OwnAddress,
        bssid: Bssid,
    ) -> Option<ManagementEvent> {
        let header = DecodedHeader::decode(frame)?;
        let control = header.control();
        let subtype = control.get() & 0x00fc;
        if !matches!(subtype, 0x00a0 | 0x00c0 | 0x00d0)
            || control.ds() != 0
            || control.more_fragments()
            || header.sequence_control() & 15 != 0
            || header.ht_control().is_some()
            || (header.receiver() != local.get() && header.receiver() != [0xff; 6])
            || header.transmitter() != bssid.get()
            || header.address3() != bssid.get()
        {
            return None;
        }
        let group = header.receiver() == [0xff; 6];
        // All management events this machine handles fit in this bounded copy:
        // disconnect (26 + protection), SA Query (28 + protection).
        let mut copied = [0; 96];
        let body = if group {
            // BIP has a clear Protected bit and leaves the body in plaintext.
            if control.protected() {
                return None;
            }
            self.group.as_mut()?.verify(frame).ok()?;
            frame.get(24..frame.len().checked_sub(MANAGEMENT_MIC_ELEMENT_LEN)?)?
        } else {
            let copied = copied.get_mut(..frame.len())?;
            copied.copy_from_slice(frame);
            self.individual.open(copied).ok()?
        };
        match subtype {
            0x00a0 | 0x00c0 => {
                let [lo, hi, ..] = *body else {
                    return None;
                };
                Some(ManagementEvent::Disconnect(u16::from_le_bytes([lo, hi])))
            }
            0x00d0 if !group => SaQuery::parse(body).map(ManagementEvent::Query),
            _ => None,
        }
    }

    pub(super) fn start_query(
        &mut self,
        now: Instant,
        nonce: fn() -> [u8; 32],
    ) -> Result<(), crate::sa_query::SaQueryDeadlineOverflow> {
        if !self.query.is_active() {
            let [a, b, c, d, ..] = nonce();
            self.request = self.query.start(now, u32::from_le_bytes([a, b, c, d]))?;
        }
        Ok(())
    }

    pub(super) fn received_query(&mut self, query: SaQuery) {
        match query {
            SaQuery::Request { transaction } => {
                // One response place is reserved. Retransmissions coalesce;
                // additional received requests wait for the peer's retry.
                self.response.get_or_insert(transaction);
            }
            SaQuery::Response { transaction } => {
                if self.query.response(transaction) {
                    self.request = None;
                }
            }
        }
    }

    pub(super) fn timed_out(&self, now: Instant) -> bool {
        self.query.timed_out(now)
    }

    pub(super) fn pending(&mut self, now: Instant) -> Option<SaQuery> {
        if self.request.is_none()
            && let SaQueryStep::Request(transaction) = self.query.step(now)
        {
            self.request = Some(transaction);
        }
        self.response
            .map(|transaction| SaQuery::Response { transaction })
            .or_else(|| {
                self.request
                    .map(|transaction| SaQuery::Request { transaction })
            })
    }

    pub(super) fn submitted(&mut self, query: SaQuery) {
        match query {
            SaQuery::Request { .. } => self.request = None,
            SaQuery::Response { .. } => self.response = None,
        }
    }

    pub(super) fn deadline(&self, blocked: bool) -> Option<Instant> {
        // A pending request waits for credit/TX return. Its absolute timeout
        // still runs; an elapsed retry must not keep the service ready-spinning.
        if self.request.is_some() || blocked {
            self.query.timeout()
        } else {
            self.query.next_deadline()
        }
    }
}
