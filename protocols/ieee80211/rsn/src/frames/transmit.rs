//! Bounded EAPOL/Ethernet transmission and typed handshake action encoding.

use super::*;
use crate::AkmKeys;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RsnTxFrame<const N: usize = RSN_TX_EAPOL_CAPACITY> {
    interface: RsnInterface,
    peer: [u8; 6],
    retransmission: bool,
    bytes: heapless::Vec<u8, N>,
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

        let mut bytes = heapless::Vec::new();
        bytes
            .resize(len, 0)
            .map_err(|_| RsnFrameError::CapacityExceeded)?;
        let mut output = bytes.as_mut_slice();
        for field in [
            &[protocol_version, EAPOL_PACKET_TYPE_KEY][..],
            &(body_len as u16).to_be_bytes(),
            &[RSN_KEY_DESCRIPTOR_TYPE],
            &key_info.to_be_bytes(),
            &key_length.to_be_bytes(),
            &replay_counter.to_be_bytes(),
            &nonce,
            &[0; 16],
            &key_rsc,
            &[0; 8],
            &[0; RSN_MIC_LEN],
            &(key_data.len() as u16).to_be_bytes(),
            key_data,
        ] {
            let (front, tail) = output
                .split_at_mut_checked(field.len())
                .ok_or(RsnFrameError::CapacityExceeded)?;
            for (target, source) in front.iter_mut().zip(field) {
                *target = *source;
            }
            output = tail;
        }
        Ok(Self {
            interface,
            peer,
            retransmission: false,
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
        self.bytes.as_slice()
    }

    pub fn key_frame(&self) -> Result<EapolKeyFrame<'_>, crate::EapolParseError> {
        EapolKeyFrame::parse(self.as_bytes())
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
        for (target, source) in self.bytes.iter_mut().skip(81).take(RSN_MIC_LEN).zip(mic) {
            *target = *source;
        }
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
    bytes: heapless::Vec<u8, N>,
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
        let mut bytes = heapless::Vec::new();
        for field in [
            eapol.peer().as_slice(),
            local.as_slice(),
            EAPOL_ETHERTYPE.as_slice(),
            eapol.as_bytes(),
        ] {
            bytes
                .extend_from_slice(field)
                .map_err(|_| RsnFrameError::CapacityExceeded)?;
        }
        Ok(Self {
            interface: eapol.interface(),
            bytes,
        })
    }

    pub const fn interface(&self) -> RsnInterface {
        self.interface
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

#[cfg(test)]
mod tests;
