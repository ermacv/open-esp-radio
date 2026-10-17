//! Bounded EAPOL/Ethernet transmission and typed handshake action encoding.

use super::*;
use crate::AkmKeys;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RsnTxFrame<const N: usize = RSN_TX_EAPOL_CAPACITY> {
    interface: RsnInterface,
    peer: [u8; 6],
    retransmission: bool,
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> RsnTxFrame<N> {
    pub fn message1(
        akm: Akm,
        peer: [u8; 6],
        replay_counter: u64,
        authenticator_nonce: [u8; 32],
    ) -> Result<Self, RsnFrameError> {
        Self::build(
            RsnInterface::AccessPoint,
            peer,
            2,
            KEY_INFO_PAIRWISE | KEY_INFO_ACK | u16::from(akm.key_descriptor_version()),
            RSN_GTK_LEN as u16,
            replay_counter,
            authenticator_nonce,
            [0; 8],
            &[],
        )
    }

    pub fn message2<const R: usize>(
        akm: Akm,
        peer: [u8; 6],
        replay_counter: u64,
        supplicant_nonce: [u8; 32],
        rsn_ie: &OwnedRsnIe<R>,
    ) -> Result<Self, RsnFrameError> {
        let security_ies = OwnedAssociationSecurityIes::<R>::try_copy(rsn_ie, &[])?;
        Self::message2_with_security_ies(akm, peer, replay_counter, supplicant_nonce, &security_ies)
    }

    pub fn message2_with_security_ies<const R: usize>(
        akm: Akm,
        peer: [u8; 6],
        replay_counter: u64,
        supplicant_nonce: [u8; 32],
        security_ies: &OwnedAssociationSecurityIes<R>,
    ) -> Result<Self, RsnFrameError> {
        Self::build(
            RsnInterface::Station,
            peer,
            1,
            KEY_INFO_PAIRWISE | KEY_INFO_MIC | u16::from(akm.key_descriptor_version()),
            0,
            replay_counter,
            supplicant_nonce,
            [0; 8],
            security_ies.as_bytes(),
        )
    }

    pub fn message3(
        akm: Akm,
        peer: [u8; 6],
        replay_counter: u64,
        authenticator_nonce: [u8; 32],
        key_rsc: [u8; 8],
        encrypted_key_data: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if encrypted_key_data.is_empty() {
            return Err(RsnFrameError::EmptyKeyData);
        }
        Self::build(
            RsnInterface::AccessPoint,
            peer,
            2,
            KEY_INFO_PAIRWISE
                | KEY_INFO_INSTALL
                | KEY_INFO_ACK
                | KEY_INFO_MIC
                | KEY_INFO_SECURE
                | KEY_INFO_ENCRYPTED_KEY_DATA
                | u16::from(akm.key_descriptor_version()),
            RSN_GTK_LEN as u16,
            replay_counter,
            authenticator_nonce,
            key_rsc,
            encrypted_key_data,
        )
    }

    pub fn message4(akm: Akm, peer: [u8; 6], replay_counter: u64) -> Result<Self, RsnFrameError> {
        Self::build(
            RsnInterface::Station,
            peer,
            1,
            KEY_INFO_PAIRWISE
                | KEY_INFO_MIC
                | KEY_INFO_SECURE
                | u16::from(akm.key_descriptor_version()),
            0,
            replay_counter,
            [0; 32],
            [0; 8],
            &[],
        )
    }

    /// Build the station response to one connected-state Group Message 1.
    pub fn group_message2(
        akm: Akm,
        peer: [u8; 6],
        replay_counter: u64,
    ) -> Result<Self, RsnFrameError> {
        Self::build(
            RsnInterface::Station,
            peer,
            1,
            KEY_INFO_MIC | KEY_INFO_SECURE | u16::from(akm.key_descriptor_version()),
            0,
            replay_counter,
            [0; 32],
            [0; 8],
            &[],
        )
    }

    /// Build an authenticator Group Message 1 for protocol tests and the
    /// future AP authenticator. The caller supplies RFC3394-wrapped GTK data.
    pub fn group_message1(
        akm: Akm,
        peer: [u8; 6],
        replay_counter: u64,
        key_rsc: [u8; 8],
        encrypted_key_data: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if encrypted_key_data.is_empty() {
            return Err(RsnFrameError::EmptyKeyData);
        }
        Self::build(
            RsnInterface::AccessPoint,
            peer,
            2,
            KEY_INFO_ACK
                | KEY_INFO_MIC
                | KEY_INFO_SECURE
                | KEY_INFO_ENCRYPTED_KEY_DATA
                | u16::from(akm.key_descriptor_version()),
            RSN_GTK_LEN as u16,
            replay_counter,
            [0; 32],
            key_rsc,
            encrypted_key_data,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        interface: RsnInterface,
        peer: [u8; 6],
        protocol_version: u8,
        key_info: u16,
        key_length: u16,
        replay_counter: u64,
        nonce: [u8; 32],
        key_rsc: [u8; 8],
        key_data: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if key_info & (KEY_INFO_PAIRWISE | KEY_INFO_ACK) == (KEY_INFO_PAIRWISE | KEY_INFO_ACK)
            && nonce.iter().all(|byte| *byte == 0)
        {
            return Err(RsnFrameError::ZeroNonce);
        }
        let len = EAPOL_KEY_PACKET_LEN
            .checked_add(key_data.len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        let body_len = EAPOL_KEY_FIXED_LEN
            .checked_add(key_data.len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        if len > N || body_len > u16::MAX as usize || key_data.len() > u16::MAX as usize {
            return Err(RsnFrameError::CapacityExceeded);
        }

        let mut bytes = [0; N];
        bytes[0] = protocol_version;
        bytes[1] = EAPOL_PACKET_TYPE_KEY;
        bytes[2..4].copy_from_slice(&(body_len as u16).to_be_bytes());
        bytes[4] = RSN_KEY_DESCRIPTOR_TYPE;
        bytes[5..7].copy_from_slice(&key_info.to_be_bytes());
        bytes[7..9].copy_from_slice(&key_length.to_be_bytes());
        bytes[9..17].copy_from_slice(&replay_counter.to_be_bytes());
        bytes[17..49].copy_from_slice(&nonce);
        bytes[65..73].copy_from_slice(&key_rsc);
        bytes[97..99].copy_from_slice(&(key_data.len() as u16).to_be_bytes());
        bytes[EAPOL_KEY_PACKET_LEN..len].copy_from_slice(key_data);
        Ok(Self {
            interface,
            peer,
            retransmission: false,
            len,
            bytes,
        })
    }

    pub const fn interface(&self) -> RsnInterface {
        self.interface
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.peer
    }

    /// Semantic retry classification supplied by the WPA state transition.
    /// EAPOL-Key does not carry a standalone retransmission flag, so keeping
    /// this metadata prevents the TX-complete owner from guessing from bytes.
    pub const fn retransmission(&self) -> bool {
        self.retransmission
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn key_frame(&self) -> EapolKeyFrame<'_> {
        EapolKeyFrame::parse(self.as_bytes()).expect("RsnTxFrame is validated on construction")
    }

    /// Authenticate a supplicant or authenticator action with the pairwise
    /// KCK. The builder always initializes the MIC field to zero; clearing it
    /// here as well makes repeated authentication deterministic.
    pub fn authenticate(mut self, ptk: &Ptk) -> Self {
        self.authenticate_with_kck(ptk.akm(), ptk.kck());
        self
    }

    pub(crate) fn authenticate_with_confirmation_key(
        mut self,
        key: &RsnKeyConfirmationKey,
    ) -> Self {
        self.authenticate_with_kck(key.akm(), key.as_bytes());
        self
    }

    fn authenticate_with_kck(&mut self, akm: Akm, kck: &[u8; RSN_KCK_LEN]) {
        self.set_mic(&[0; RSN_MIC_LEN]);
        let mut mac = akm.mic(kck);
        mac.update(self.as_bytes());
        self.set_mic(&mac.finalize());
    }

    pub(crate) const fn mark_retransmission(mut self) -> Self {
        self.retransmission = true;
        self
    }

    fn set_mic(&mut self, mic: &[u8; RSN_MIC_LEN]) {
        self.bytes[81..97].copy_from_slice(mic);
    }
}

pub fn build_sta_action_frame<const N: usize, const R: usize>(
    state: &RsnStaState,
    transmit: RsnTransmit,
    security_ies: &OwnedAssociationSecurityIes<R>,
) -> Result<RsnTxFrame<N>, RsnFrameError> {
    let mut frame = match transmit.message {
        RsnTxMessage::PairwiseMessage2 => RsnTxFrame::message2_with_security_ies(
            state.akm(),
            *state.peer(),
            transmit.replay_counter,
            *state.supplicant_nonce(),
            security_ies,
        ),
        RsnTxMessage::PairwiseMessage4 => {
            RsnTxFrame::message4(state.akm(), *state.peer(), transmit.replay_counter)
        }
        RsnTxMessage::PairwiseMessage1 | RsnTxMessage::PairwiseMessage3 => {
            Err(RsnFrameError::UnexpectedTransmitAction)
        }
    }?;
    frame.retransmission = transmit.retransmission;
    Ok(frame)
}

pub fn build_ap_action_frame<const N: usize>(
    state: &RsnApState,
    transmit: RsnTransmit,
    key_rsc: [u8; 8],
    encrypted_key_data: &[u8],
) -> Result<RsnTxFrame<N>, RsnFrameError> {
    let mut frame = match transmit.message {
        RsnTxMessage::PairwiseMessage1 => RsnTxFrame::message1(
            state.akm(),
            *state.peer(),
            transmit.replay_counter,
            *state.authenticator_nonce(),
        ),
        RsnTxMessage::PairwiseMessage3 => RsnTxFrame::message3(
            state.akm(),
            *state.peer(),
            transmit.replay_counter,
            *state.authenticator_nonce(),
            key_rsc,
            encrypted_key_data,
        ),
        RsnTxMessage::PairwiseMessage2 | RsnTxMessage::PairwiseMessage4 => {
            Err(RsnFrameError::UnexpectedTransmitAction)
        }
    }?;
    frame.retransmission = transmit.retransmission;
    Ok(frame)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RsnEthernetFrame<const N: usize = RSN_TX_ETHERNET_CAPACITY> {
    interface: RsnInterface,
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> RsnEthernetFrame<N> {
    pub fn build<const E: usize>(
        local: [u8; 6],
        eapol: &RsnTxFrame<E>,
    ) -> Result<Self, RsnFrameError> {
        let len = 14_usize
            .checked_add(eapol.as_bytes().len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        if len > N {
            return Err(RsnFrameError::CapacityExceeded);
        }
        let mut bytes = [0; N];
        bytes[..6].copy_from_slice(eapol.peer());
        bytes[6..12].copy_from_slice(&local);
        bytes[12..14].copy_from_slice(&EAPOL_ETHERTYPE);
        bytes[14..len].copy_from_slice(eapol.as_bytes());
        Ok(Self {
            interface: eapol.interface(),
            len,
            bytes,
        })
    }

    pub const fn interface(&self) -> RsnInterface {
        self.interface
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[cfg(test)]
mod tests;
