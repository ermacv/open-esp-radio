//! Validated association-scoped receive BA management and its control TX.
use super::*;
use oer_protocols_ieee80211_core_upper_mac::block_ack::{AddbaResponse, AddbaStatus};
use oer_protocols_ieee80211_wire::block_ack::frame::{BlockAckAction, parse_block_ack_action};

#[must_use = "a parsed ADDBA request needs a response from the receive bank owner"]
pub struct PendingRxBlockAck {
    epoch: LinkEpoch,
    bssid: Bssid,
    action: BlockAckAction,
    admission: BlockAckAdmission,
}

/// Whether an association admits Block Ack agreements: a QoS association
/// of an HT or HE format does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAckAdmission {
    Admitted,
    Refused,
}

/// Whether the connected station has room to send now: no control
/// transmission of its own is out, and it is home, awake and keyed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SendRoom {
    Free,
    Blocked,
}

impl PendingRxBlockAck {
    pub(super) fn new(
        epoch: LinkEpoch,
        bssid: Bssid,
        action: BlockAckAction,
        admission: BlockAckAdmission,
    ) -> Self {
        Self {
            epoch,
            bssid,
            action,
            admission,
        }
    }
    /// A request of the association `epoch`, for a host test that plays the
    /// station ahead of the receive agreements.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn for_test(
        epoch: LinkEpoch,
        bssid: Bssid,
        action: BlockAckAction,
        admission: BlockAckAdmission,
    ) -> Self {
        Self {
            epoch,
            bssid,
            action,
            admission,
        }
    }
    pub const fn admission(&self) -> BlockAckAdmission {
        self.admission
    }
    pub const fn action(&self) -> BlockAckAction {
        self.action
    }
    pub const fn bssid(&self) -> Bssid {
        self.bssid
    }
    pub const fn epoch(&self) -> LinkEpoch {
        self.epoch
    }

    /// Consume the received request into a response of that association.
    /// A response with another dialog or TID returns the unchanged request.
    pub fn reply(self, response: AddbaResponse) -> Result<RxBlockAckReply, Self> {
        let BlockAckAction::AddbaRequest {
            dialog_token,
            parameters,
            ..
        } = self.action
        else {
            return Err(self);
        };
        if response.dialog_token != dialog_token || response.tid != parameters.tid() {
            return Err(self);
        }
        let body = response.encode();
        Ok(RxBlockAckReply {
            epoch: self.epoch,
            bssid: self.bssid,
            body,
        })
    }

    pub fn decline(self) -> Result<RxBlockAckReply, Self> {
        let BlockAckAction::AddbaRequest {
            dialog_token,
            parameters,
            timeout,
            ..
        } = self.action
        else {
            return Err(self);
        };
        self.reply(AddbaResponse {
            dialog_token,
            status: AddbaStatus::RequestDeclined,
            tid: parameters.tid(),
            window: parameters.buffer_size(),
            timeout,
            amsdu: false,
        })
    }
}

#[must_use = "retain a busy response until the station's frame of its purpose ended"]
pub struct RxBlockAckReply {
    epoch: LinkEpoch,
    bssid: Bssid,
    body: [u8; 9],
}

impl RxBlockAckReply {
    pub const fn epoch(&self) -> LinkEpoch {
        self.epoch
    }

    /// Encode a response retained by this association's receive bank owner.
    /// The station rechecks the supplied epoch before taking its frame's place.
    pub fn for_association(epoch: LinkEpoch, bssid: Bssid, response: AddbaResponse) -> Self {
        Self {
            epoch,
            bssid,
            body: response.encode(),
        }
    }

    /// Restore a queued encoded ADDBA response after checking its frame kind.
    pub fn from_body(epoch: LinkEpoch, bssid: Bssid, body: [u8; 9]) -> Option<Self> {
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

pub type BlockAckReplyResult<'a, 'id, T, K> =
    Result<StaAction<'a, 'id, T, K>, (RxBlockAckReply, BlockAckReplyRefusal)>;

pub(super) fn unprotected_action(
    management: Management<'_>,
    local: OwnAddress,
    bssid: Bssid,
) -> Option<BlockAckAction> {
    let header = management.header();
    let control = header.control();
    if management.subtype() != oer_protocols_ieee80211_wire::frame::ManagementSubtype::Action
        || control.protected()
        || control.ds() != 0
        || control.more_fragments()
        || header.sequence_control().fragment() != 0
        || header.ht_control().is_some()
        || header.receiver() != local.get()
        || header.transmitter() != bssid.get()
        || header.address3() != bssid.get()
    {
        return None;
    }
    parse_block_ack_action(header.body())
}

impl<'id, T: StaTypes, K: Ieee80211Capabilities, C: StaCallbacks> Station<'id, T, K, C> {
    /// Attempt a response as a connected station's frame and,
    /// when negotiated, its management protection. Busy returns the owner.
    #[inline(never)]
    pub fn try_block_ack_reply(
        &mut self,
        reply: RxBlockAckReply,
    ) -> BlockAckReplyResult<'_, 'id, T, K> {
        if self.epoch != Some(reply.epoch) || !self.is_connected() {
            return Err((reply, BlockAckReplyRefusal::LinkChanged));
        }
        let State::Connected(connected) = &self.state else {
            return Err((reply, BlockAckReplyRefusal::LinkChanged));
        };
        // Its data waits while the station sleeps or is away; so does this.
        if !super::data_link_up(connected, self.absence.as_ref(), &self.keys) {
            return Err((reply, BlockAckReplyRefusal::Busy));
        }
        if connected.bssid != reply.bssid {
            return Err((reply, BlockAckReplyRefusal::LinkChanged));
        }
        if connected.transmission != super::Transmission::Free {
            return Err((reply, BlockAckReplyRefusal::Busy));
        }
        let protection = super::management_frame_protection(connected);
        let bssid = connected.bssid;
        let length = if reply.body.get(1) == Some(&2) { 6 } else { 9 };
        let Some(body) = reply.body.get(..length) else {
            return Err((reply, BlockAckReplyRefusal::Encode));
        };
        let (mut transition, _) = self.transition();
        let encoded = match transition.encode_management(
            StaManagementSubtype::Action,
            bssid,
            body,
            TxPurpose::Connected(Sending::BlockAck),
            protection,
        ) {
            Ok(encoded) => encoded,
            Err(Unsent::Out) => return Err((reply, BlockAckReplyRefusal::Busy)),
            Err(Unsent::Encode) => return Err((reply, BlockAckReplyRefusal::Encode)),
            Err(Unsent::NoConnection) => return Err((reply, BlockAckReplyRefusal::LinkChanged)),
        };
        // A protected frame the station numbered itself goes under its
        // installed key; the handed-over link keys its own frames.
        if protection == oer_protocols_ieee80211_wire::frame::FrameProtection::Protected
            && !self.keys.keyed()
            && !matches!(encoded.number, crate::link::FrameNumber::Link { .. })
        {
            self.out.release(encoded.tag.purpose);
            return Err((reply, BlockAckReplyRefusal::LinkChanged));
        }
        if let State::Connected(connected) = &mut self.state {
            connected.transmission = super::Transmission::Busy;
        }
        let under_key = protection
            == oer_protocols_ieee80211_wire::frame::FrameProtection::Protected
            && matches!(encoded.number, crate::link::FrameNumber::Open);
        let transmit = self.hand_out(encoded);
        Ok(if under_key {
            StaAction::TransmitProtected(transmit)
        } else {
            StaAction::Transmit(transmit)
        })
    }
}
