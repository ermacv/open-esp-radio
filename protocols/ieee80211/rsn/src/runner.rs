//! Ports and values of the WPA2-Personal station four-way-handshake runners.
//!
//! The runners that own absolute response deadlines, key-publication
//! rollback and the ordering of finite RX, RX restart and EAPOL TX
//! transactions are `RsnHandshakeRunner` and `RsnKeyInstallRunner` of
//! `oer-ieee80211-rsn-service`. This module declares the hardware ports they
//! wait on and the values they exchange. Concrete DMA, timer, key-slot and
//! frame-transmit bindings remain in chip/runtime adapters.

use core::future::Future;

use oer_ieee80211_mac::sequence::SequenceNumber;

use crate::{
    OwnedEapolFrame, Pmk,
    frames::RsnTxFrame,
    supplicant::{
        RsnStaKeyInstallRequest, RsnStaProcessError, RsnStaResponseWait, RsnStaSupplicantError,
    },
};

/// EAPOL storage of one handshake frame at the runner ports.
pub const RSN_HANDSHAKE_EAPOL_CAPACITY: usize = 512;

const EAPOL_CAPACITY: usize = RSN_HANDSHAKE_EAPOL_CAPACITY;

/// One finite RX pass. `more` requests another pass at the same executor
/// boundary, normally because the backend stopped after copying one EAPOL
/// frame and deliberately left later completed descriptors untouched.
pub struct RsnRxProgress {
    pub completed_frames: u32,
    pub eapol: Option<OwnedEapolFrame<EAPOL_CAPACITY>>,
    pub more: bool,
}

impl RsnRxProgress {
    pub const fn drained(completed_frames: u32) -> Self {
        Self {
            completed_frames,
            eapol: None,
            more: false,
        }
    }

    pub const fn eapol(completed_frames: u32, eapol: OwnedEapolFrame<EAPOL_CAPACITY>) -> Self {
        Self {
            completed_frames,
            eapol: Some(eapol),
            more: true,
        }
    }
}

/// Finite hardware operations required by the `RsnHandshakeRunner` of
/// `oer-ieee80211-rsn-service`.
///
/// The backend enters with the Association RX ring live. It copies at most
/// one EAPOL packet into Rust-owned storage per pass, releases every PAC/DMA
/// borrow before returning, and leaves RX stopped on successful return from
/// the runner so key installation has an unambiguous hardware boundary.
pub trait RsnHandshakeBackend {
    type Error;

    fn service_receive(&mut self) -> impl Future<Output = Result<RsnRxProgress, Self::Error>> + '_;

    fn restart_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn transmit_message2<'a>(
        &'a mut self,
        frame: &'a RsnTxFrame<EAPOL_CAPACITY>,
        sequence_number: SequenceNumber,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;
}

/// HMAC-owned sequence-number source used for EAPOL Message 2.
///
/// WPA2 consumes a number but does not own the IEEE 802.11 sequence space.
/// The callback-shaped blanket implementation keeps this crate independent
/// from a particular HMAC state type.
pub trait RsnTxSequence {
    fn take_sequence(&mut self) -> SequenceNumber;
}

impl<F: FnMut() -> SequenceNumber> RsnTxSequence for F {
    fn take_sequence(&mut self) -> SequenceNumber {
        self()
    }
}

pub struct RsnHandshakeConfig<'config> {
    pub local: [u8; 6],
    pub authenticator: [u8; 6],
    pub supplicant_nonce: [u8; 32],
    pub association_security_ies: &'config [u8],
    pub authenticator_rsn_ie: &'config [u8],
    pub authenticator_rsnxe: &'config [u8],
    pub pmk: &'config Pmk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnHandshakeError<BackendError, UnwrapError> {
    Backend(BackendError),
    ClockOverflow,
    Create(RsnStaSupplicantError),
    Process(RsnStaProcessError<UnwrapError>),
    InvalidMessage2,
    UnexpectedAction,
    Timeout {
        wait: RsnStaResponseWait,
        elapsed: oer_time::Duration,
        completed_frames: u32,
    },
}

/// Finite hardware boundary for publishing a validated PTK/GTK pair and
/// transmitting the corresponding Message 4.
///
/// `install_keys` must be atomic from the caller's point of view: on error it
/// leaves no published key behind. Once it succeeds, every later error is
/// routed through `rollback_keys` before the `RsnKeyInstallRunner` of
/// `oer-ieee80211-rsn-service` returns.
pub trait RsnKeyInstallBackend {
    type Error;
    type InstalledKeys;

    fn install_keys(
        &mut self,
        request: &RsnStaKeyInstallRequest,
    ) -> Result<Self::InstalledKeys, Self::Error>;

    fn rollback_keys(&mut self, keys: Self::InstalledKeys) -> Result<(), Self::Error>;

    fn transmit_message4<'a>(
        &'a mut self,
        frame: &'a RsnTxFrame<EAPOL_CAPACITY>,
        keys: &'a mut Self::InstalledKeys,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnKeyInstallRequestError {
    PairwiseInterface,
    PairwiseKind,
    GroupInterface,
    GroupKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnKeyInstallMetadata {
    pub replay_counter: u64,
    pub encrypted_key_data: bool,
    pub plain_key_data_len: usize,
    pub group_key_id: oer_ieee80211_mac::ccmp::GroupKeyId,
    pub group_transmit: bool,
    pub completed_frames: u32,
    pub message2_transmissions: u16,
    pub message4_len: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub enum RsnKeyInstallFailure<BackendError> {
    Complete(RsnStaSupplicantError),
    InvalidMessage4,
    Transmit(BackendError),
}

#[derive(Debug, Eq, PartialEq)]
pub enum RsnKeyInstallError<BackendError> {
    Request(RsnKeyInstallRequestError),
    Install(BackendError),
    Failed(RsnKeyInstallFailure<BackendError>),
    Rollback {
        failure: RsnKeyInstallFailure<BackendError>,
        rollback: BackendError,
    },
}
