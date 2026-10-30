//! Validated association-scoped receive BA management and its control TX.
use super::*;
use oer_ieee80211_upper_mac::block_ack::{AddbaResponse, AddbaStatus};
use oer_ieee80211_wire::block_ack::frame::{BlockAckAction, parse_block_ack_action};

#[must_use = "a parsed ADDBA request needs a response from the receive bank owner"]
pub struct PendingRxBlockAck<'id, Path: PoolPath> {
    epoch: LinkEpoch<'id, Path>,
    bssid: Bssid,
    action: BlockAckAction,
    supported: bool,
}

impl<'id, Path: PoolPath> PendingRxBlockAck<'id, Path> {
    pub(super) fn new(
        epoch: LinkEpoch<'id, Path>,
        bssid: Bssid,
        action: BlockAckAction,
        supported: bool,
    ) -> Self {
        Self {
            epoch,
            bssid,
            action,
            supported,
        }
    }
    pub const fn supported(&self) -> bool {
        self.supported
    }
    pub const fn action(&self) -> BlockAckAction {
        self.action
    }
    pub const fn bssid(&self) -> Bssid {
        self.bssid
    }
    pub const fn epoch(&self) -> LinkEpoch<'id, Path> {
        self.epoch
    }

    /// Consume the received request into a response of that association.
    /// A response with another dialog or TID returns the unchanged request.
    pub fn reply(self, response: AddbaResponse) -> Result<RxBlockAckReply<'id, Path>, Self> {
        let BlockAckAction::AddbaRequest {
            dialog_token, tid, ..
        } = self.action
        else {
            return Err(self);
        };
        if response.dialog_token != dialog_token || response.tid != tid {
            return Err(self);
        }
        let Some(body) = response.encode() else {
            return Err(self);
        };
        Ok(RxBlockAckReply {
            epoch: self.epoch,
            bssid: self.bssid,
            body,
        })
    }

    pub fn decline(self) -> Result<RxBlockAckReply<'id, Path>, Self> {
        let BlockAckAction::AddbaRequest {
            dialog_token,
            tid,
            window,
            timeout_tu,
            ..
        } = self.action
        else {
            return Err(self);
        };
        self.reply(AddbaResponse {
            dialog_token,
            status: AddbaStatus::RequestDeclined,
            tid,
            window,
            timeout_tu,
            amsdu: false,
        })
    }
}

#[must_use = "retain a busy response until a transmit credit returns"]
pub struct RxBlockAckReply<'id, Path: PoolPath> {
    epoch: LinkEpoch<'id, Path>,
    bssid: Bssid,
    body: [u8; 9],
}

impl<'id, Path: PoolPath> RxBlockAckReply<'id, Path> {
    pub const fn epoch(&self) -> LinkEpoch<'id, Path> {
        self.epoch
    }

    /// Encode a response retained by this association's receive bank owner.
    /// The station rechecks the supplied epoch before reserving TX credits.
    pub fn for_association(
        epoch: LinkEpoch<'id, Path>,
        bssid: Bssid,
        response: AddbaResponse,
    ) -> Option<Self> {
        Some(Self {
            epoch,
            bssid,
            body: response.encode()?,
        })
    }

    /// Restore a queued encoded ADDBA response after checking its frame kind.
    pub fn from_body(epoch: LinkEpoch<'id, Path>, bssid: Bssid, body: [u8; 9]) -> Option<Self> {
        if !matches!(
            parse_block_ack_action(&body),
            Some(BlockAckAction::AddbaResponse { .. } | BlockAckAction::Delba { .. })
        ) {
            return None;
        }
        Some(Self { epoch, bssid, body })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAckReplyRefusal {
    Busy,
    LinkChanged,
    Encode,
}

pub type BlockAckReplyResult<'a, 'id, T, K> = Result<
    StaAction<'a, 'id, T, K>,
    (
        RxBlockAckReply<'id, <T as StaTokens>::EpochPath>,
        BlockAckReplyRefusal,
    ),
>;

pub(super) fn unprotected_action(
    management: Management<'_>,
    local: OwnAddress,
    bssid: Bssid,
) -> Option<BlockAckAction> {
    let header = management.header();
    let control = header.control();
    if management.subtype() != 13
        || control.protected()
        || control.ds() != 0
        || control.more_fragments()
        || header.sequence_control() & 15 != 0
        || header.ht_control().is_some()
        || header.receiver() != local.get()
        || header.transmitter() != bssid.get()
        || header.address3() != bssid.get()
    {
        return None;
    }
    parse_block_ack_action(header.body())
}

impl<'id, T: StaTokens, K: Ieee80211Capabilities, C: StaCallbacks> Station<'id, T, K, C> {
    /// Attempt a response using the station's existing control credits and,
    /// when negotiated, its management protection. Busy returns the owner.
    #[inline(never)]
    pub fn try_block_ack_reply(
        &mut self,
        reply: RxBlockAckReply<'id, T::EpochPath>,
        events: &mut impl EventSink<StaEvent>,
    ) -> BlockAckReplyResult<'_, 'id, T, K> {
        if self.epoch != Some(reply.epoch) || !self.is_connected() {
            return Err((reply, BlockAckReplyRefusal::LinkChanged));
        }
        if self.link().is_none() {
            return Err((reply, BlockAckReplyRefusal::Busy));
        }
        let State::Connected(connected) = &self.state else {
            return Err((reply, BlockAckReplyRefusal::LinkChanged));
        };
        if connected.bssid != reply.bssid {
            return Err((reply, BlockAckReplyRefusal::LinkChanged));
        }
        if connected.sending {
            return Err((reply, BlockAckReplyRefusal::Busy));
        }
        let protected = connected
            .rsn
            .as_ref()
            .is_some_and(|rsn| rsn.management.is_some());
        let bssid = connected.bssid;
        let length = if reply.body.get(1) == Some(&2) { 6 } else { 9 };
        let Some(body) = reply.body.get(..length) else {
            return Err((reply, BlockAckReplyRefusal::Encode));
        };
        let (mut transition, _) = self.transition();
        let transmit = match transition.encode_management(
            StaManagementSubtype::Action,
            bssid,
            body,
            TxPurpose::Connected(Sending::BlockAck),
            protected,
            events,
        ) {
            Ok(transmit) => transmit,
            Err(Unsent::NoTokens) => return Err((reply, BlockAckReplyRefusal::Busy)),
            Err(Unsent::Encode) => return Err((reply, BlockAckReplyRefusal::Encode)),
            Err(Unsent::NoConnection) => return Err((reply, BlockAckReplyRefusal::LinkChanged)),
        };
        if protected && !matches!(&self.keys.pairwise, KeyCell::Installed(_)) {
            self.tickets.put(transmit.ticket, events);
            self.buffers.put(transmit.buffer, events);
            return Err((reply, BlockAckReplyRefusal::LinkChanged));
        }
        if let State::Connected(connected) = &mut self.state {
            connected.sending = true;
        }
        if protected {
            match &self.keys.pairwise {
                KeyCell::Installed(key) => Ok(StaAction::TransmitProtected { transmit, key }),
                _ => {
                    self.tickets.put(transmit.ticket, events);
                    self.buffers.put(transmit.buffer, events);
                    Err((reply, BlockAckReplyRefusal::LinkChanged))
                }
            }
        } else {
            Ok(StaAction::Transmit(transmit))
        }
    }
}
