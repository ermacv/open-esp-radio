//! Receive protection of one association's robust management frames.
//!
//! An association that negotiated management frame protection keeps the
//! pairwise temporal key in software to open its individually addressed
//! robust management frames, as the vendor does, and verifies group-addressed
//! ones under the IGTK with BIP. Receive dispatch copies each such frame for
//! control, which opens it here and continues with the plaintext as the
//! vendor's `sta_recv_mgmt` and Action handlers do.
//!
//! SOURCE: complete pinned `libnet80211[ieee80211_sta.o]::sta_input`
//! (`sta_bip_check`) and `sta_recv_mgmt`.

use oer_esp32s31_ieee80211_mac::tx::ampdu::parse_block_ack_action;
use oer_ieee80211_mac::{
    management_protection::SaQuery,
    station::{StaDisconnect, StaDisconnectKind},
};
use oer_ieee80211_rsn::{
    bip::{BipError, BipReceiver, MANAGEMENT_MIC_ELEMENT_LEN},
    frames::RsnIgtk,
    management_ccmp::{ManagementCcmpError, ManagementCcmpReceiver},
};

use crate::connected_rx::ConnectedRxControlEvent;

const HEADER_LEN: usize = 24;
const DISASSOCIATION: u8 = 0xa0;
const DEAUTHENTICATION: u8 = 0xc0;
const ACTION: u8 = 0xd0;

/// Largest robust management frame control copies. Protected Deauthentication,
/// Disassociation, SA Query and BlockAck Action frames all fit; control has no
/// use for a longer robust frame.
pub const PROTECTED_MANAGEMENT_FRAME_CAPACITY: usize = 96;

/// Owned copy of one robust management frame awaiting control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtectedManagementFrame {
    bytes: [u8; PROTECTED_MANAGEMENT_FRAME_CAPACITY],
    len: u8,
    group: bool,
}

impl ProtectedManagementFrame {
    /// Copy one frame; `None` when it exceeds the capacity.
    pub fn try_copy(frame: &[u8], group: bool) -> Option<Self> {
        if frame.len() > PROTECTED_MANAGEMENT_FRAME_CAPACITY {
            return None;
        }
        let mut bytes = [0; PROTECTED_MANAGEMENT_FRAME_CAPACITY];
        bytes[..frame.len()].copy_from_slice(frame);
        Some(Self {
            bytes,
            len: frame.len() as u8,
            group,
        })
    }
}

/// Why control dropped one robust management frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedManagementDrop {
    Individual(ManagementCcmpError),
    Group(BipError),
}

/// Management frame receive protection of one association.
pub struct StationManagementProtection {
    individual: ManagementCcmpReceiver,
    group: BipReceiver,
}

impl StationManagementProtection {
    /// Protection under the association's temporal key and its first IGTK.
    pub fn new(temporal_key: [u8; 16], igtk: &RsnIgtk) -> Self {
        Self {
            individual: ManagementCcmpReceiver::new(temporal_key),
            group: BipReceiver::new(igtk),
        }
    }

    /// Follow a group rekey to its IGTK.
    pub fn install_igtk(&mut self, igtk: &RsnIgtk) {
        self.group.rekey(igtk);
    }

    /// Open or verify one copied frame and return the control event its
    /// plaintext carries: a disconnect, a BlockAck Action or an SA Query.
    /// `Ok(None)` is a verified frame control has no use for.
    pub fn receive(
        &mut self,
        frame: &mut ProtectedManagementFrame,
    ) -> Result<Option<ConnectedRxControlEvent>, ProtectedManagementDrop> {
        let group = frame.group;
        let frame = &mut frame.bytes[..usize::from(frame.len)];
        let subtype = frame[0];
        let body = if group {
            self.group
                .verify(frame)
                .map_err(ProtectedManagementDrop::Group)?;
            &frame[HEADER_LEN..frame.len() - MANAGEMENT_MIC_ELEMENT_LEN]
        } else {
            self.individual
                .open(frame)
                .map_err(ProtectedManagementDrop::Individual)?
        };
        Ok(match subtype {
            DEAUTHENTICATION | DISASSOCIATION => {
                disconnect(subtype, body).map(ConnectedRxControlEvent::PeerDisconnect)
            }
            // A group-addressed Action carries nothing the station answers.
            ACTION if !group => parse_block_ack_action(body)
                .map(ConnectedRxControlEvent::BlockAck)
                .or_else(|| SaQuery::parse(body).map(ConnectedRxControlEvent::SaQuery)),
            _ => None,
        })
    }
}

fn disconnect(subtype: u8, body: &[u8]) -> Option<StaDisconnect> {
    let [low, high, ..] = *body else {
        return None;
    };
    Some(StaDisconnect {
        kind: if subtype == DEAUTHENTICATION {
            StaDisconnectKind::Deauthentication
        } else {
            StaDisconnectKind::Disassociation
        },
        reason_code: u16::from_le_bytes([low, high]),
    })
}

#[cfg(test)]
mod tests;
