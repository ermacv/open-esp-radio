//! The values and port of one finite station attempt.
//!
//! One attempt consumes a selected candidate and orders every pre-connected
//! phase up to the connected-entry frontier: candidate preparation, channel,
//! Authentication, Association, peer programming, the WPA2 handshake and key
//! installation. [`StaAttemptPort`] is the backend of those phases, and the
//! `StaAttempt` transaction of `oer-ieee80211-sta-service` runs them in
//! order. Initial join and reconnect provide different owner types but
//! share that transaction; a failed phase returns the exact owner supplied
//! by the caller.
//!
//! The security material every phase carries ([`StaAttemptSecurity`],
//! [`StaPersonalCredentials`]) is chip-independent: the PSK, the SAE
//! password and its commit, the supplicant nonce and the connected
//! supplicant. The hardware keys an attempt installs stay with the backend.

use core::{future::Future, marker::PhantomData};

use oer_ieee80211_mac::{
    scan::ScanRecord,
    security::{SaePwe, StaSecurityPolicy},
    station::StaTxSequenceCounters,
};
use oer_ieee80211_rsn::{
    Pmk,
    sae::{SaeCommit, SaeError, SaePassword, SaePasswordElement, SaePasswordToken},
    supplicant::RsnConnectedSupplicant,
};

use crate::{
    pmksa::StaSharedPmksa,
    station::{StaFailureDisposition, StaLifecycleStage},
};

/// How the station sends EAPOL Message 4: in the clear, or under the
/// pairwise CCMP key just installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2Message4Protection {
    Unprotected,
    PairwiseCcmp,
}

/// The personal credentials of one station request.
///
/// A WPA2-Personal request keeps its PSK and, because the station upgrades to
/// SAE whenever the access point offers it, the password as well; a
/// WPA3-Personal request keeps only the password. `sae_random` draws the
/// random scalars of every SAE commit, as the vendor's `os_get_random` does.
pub struct StaPersonalCredentials {
    policy: StaSecurityPolicy,
    psk: Option<Pmk>,
    sae_password: SaePassword,
    sae_random: fn() -> u32,
    pmksa: &'static StaSharedPmksa,
}

impl StaPersonalCredentials {
    pub const fn wpa2(
        psk: Pmk,
        sae_password: SaePassword,
        sae_random: fn() -> u32,
        pmksa: &'static StaSharedPmksa,
    ) -> Self {
        Self {
            policy: StaSecurityPolicy::Wpa2Personal,
            psk: Some(psk),
            sae_password,
            sae_random,
            pmksa,
        }
    }

    pub const fn wpa3(
        sae_password: SaePassword,
        sae_random: fn() -> u32,
        pmksa: &'static StaSharedPmksa,
    ) -> Self {
        Self {
            policy: StaSecurityPolicy::Wpa3Personal,
            psk: None,
            sae_password,
            sae_random,
            pmksa,
        }
    }

    pub const fn policy(&self) -> StaSecurityPolicy {
        self.policy
    }

    /// The station's cached SAE associations.
    pub const fn pmksa(&self) -> &'static StaSharedPmksa {
        self.pmksa
    }

    /// This station's SAE commit toward `access_point`, deriving the password
    /// element by hash to element when `h2e`, by hunting and pecking
    /// otherwise. Random values outside 1 < value < r are redrawn, up to the
    /// vendor's 100 draws.
    ///
    /// SOURCE: ESP-IDF ``
    /// `components/wpa_supplicant/src/common/dragonfly.c`
    /// (`dragonfly_generate_scalar`).
    pub fn sae_commit(
        &self,
        local: [u8; 6],
        access_point: &ScanRecord,
        method: SaePwe,
    ) -> Result<SaeCommit, SaeError> {
        let password = self.sae_password.as_bytes();
        let pwe = match method {
            SaePwe::HashToElement => {
                SaePasswordToken::derive(access_point.ssid_bytes(), password, None)
                    .password_element(local, access_point.bssid)
            }
            SaePwe::HuntingAndPecking => {
                SaePasswordElement::hunting_and_pecking(password, local, access_point.bssid)?
            }
        };
        let draw = || -> [u8; 32] {
            let mut bytes = [0; 32];
            for word in bytes.chunks_exact_mut(4) {
                word.copy_from_slice(&(self.sae_random)().to_le_bytes());
            }
            bytes
        };
        for _ in 0..SAE_SCALAR_DRAWS {
            match SaeCommit::new(pwe, draw(), draw()) {
                Err(SaeError::UnsuitableRandom) => {}
                commit => return commit,
            }
        }
        Err(SaeError::UnsuitableRandom)
    }
}

/// The vendor's bound on drawing SAE rand and mask.
const SAE_SCALAR_DRAWS: u8 = 100;

/// Security and sequence ownership retained across every finite phase.
///
/// These values are owned, rather than borrowed from a composition root, so a
/// complete station owner can move into an executor task without becoming
/// self-referential. A supervisor can replace credentials only after this
/// value returns through the finite task's terminal edge.
#[allow(
    clippy::large_enum_variant,
    reason = "the no-alloc attempt owner keeps its credentials and PMK inline"
)]
pub enum StaAttemptSecurityMaterial {
    Open,
    Personal {
        credentials: StaPersonalCredentials,
        /// The PMK an SAE authentication of the current attempt derived.
        sae_pmk: Option<Pmk>,
        supplicant_nonce: [u8; 32],
        message4_protection: Wpa2Message4Protection,
        connected: Option<RsnConnectedSupplicant>,
    },
}

pub struct StaAttemptSecurity<'role> {
    pub sequences: StaTxSequenceCounters,
    material: StaAttemptSecurityMaterial,
    role: PhantomData<&'role mut ()>,
}

impl StaAttemptSecurity<'_> {
    pub const fn new(
        credentials: StaPersonalCredentials,
        supplicant_nonce: [u8; 32],
        sequences: StaTxSequenceCounters,
        message4_protection: Wpa2Message4Protection,
    ) -> Self {
        Self {
            sequences,
            material: StaAttemptSecurityMaterial::Personal {
                credentials,
                sae_pmk: None,
                supplicant_nonce,
                message4_protection,
                connected: None,
            },
            role: PhantomData,
        }
    }

    pub const fn open(sequences: StaTxSequenceCounters) -> Self {
        Self {
            sequences,
            material: StaAttemptSecurityMaterial::Open,
            role: PhantomData,
        }
    }

    pub const fn policy(&self) -> StaSecurityPolicy {
        match &self.material {
            StaAttemptSecurityMaterial::Open => StaSecurityPolicy::Open,
            StaAttemptSecurityMaterial::Personal { credentials, .. } => credentials.policy,
        }
    }

    pub const fn credentials(&self) -> Option<&StaPersonalCredentials> {
        match &self.material {
            StaAttemptSecurityMaterial::Open => None,
            StaAttemptSecurityMaterial::Personal { credentials, .. } => Some(credentials),
        }
    }

    /// Record the PMK the current attempt's authentication derived: the SAE
    /// PMK, or `None` when the attempt authenticated by PSK.
    pub fn set_sae_pmk(&mut self, pmk: Option<Pmk>) {
        if let StaAttemptSecurityMaterial::Personal { sae_pmk, .. } = &mut self.material {
            *sae_pmk = pmk;
        }
    }

    pub fn wpa2_material(&self) -> Option<(&Pmk, [u8; 32], Wpa2Message4Protection)> {
        match &self.material {
            StaAttemptSecurityMaterial::Open => None,
            StaAttemptSecurityMaterial::Personal {
                credentials,
                sae_pmk,
                supplicant_nonce,
                message4_protection,
                ..
            } => sae_pmk
                .as_ref()
                .or(credentials.psk.as_ref())
                .map(|pmk| (pmk, *supplicant_nonce, *message4_protection)),
        }
    }

    pub fn wpa2_handshake_parts(&mut self) -> Option<(&Pmk, [u8; 32], &mut StaTxSequenceCounters)> {
        match &self.material {
            StaAttemptSecurityMaterial::Open => None,
            StaAttemptSecurityMaterial::Personal {
                credentials,
                sae_pmk,
                supplicant_nonce,
                ..
            } => sae_pmk
                .as_ref()
                .or(credentials.psk.as_ref())
                .map(|pmk| (pmk, *supplicant_nonce, &mut self.sequences)),
        }
    }

    pub fn set_connected(&mut self, value: RsnConnectedSupplicant) -> bool {
        match &mut self.material {
            StaAttemptSecurityMaterial::Open => false,
            StaAttemptSecurityMaterial::Personal { connected, .. } => {
                *connected = Some(value);
                true
            }
        }
    }

    /// The sequence counters and, after a WPA2 handshake, the connected
    /// supplicant that answers the access point's group rekeys.
    pub fn connected_parts(
        &mut self,
    ) -> (
        &mut StaTxSequenceCounters,
        Option<&mut RsnConnectedSupplicant>,
    ) {
        let connected = match &mut self.material {
            StaAttemptSecurityMaterial::Open => None,
            StaAttemptSecurityMaterial::Personal { connected, .. } => connected.as_mut(),
        };
        (&mut self.sequences, connected)
    }

    pub const fn has_connected_wpa2(&self) -> bool {
        matches!(
            &self.material,
            StaAttemptSecurityMaterial::Personal {
                connected: Some(_),
                ..
            }
        )
    }

    pub fn into_parts(self) -> (StaTxSequenceCounters, StaAttemptSecurityMaterial) {
        (self.sequences, self.material)
    }

    /// Retag the owned security state for the next finite role scope.
    ///
    /// No borrow is extended: PMK and sequence counters move by value. The
    /// marker only prevents a composition from accidentally mixing two live
    /// role scopes while the wider station API still carries that lifetime.
    pub fn into_role<'next>(self) -> StaAttemptSecurity<'next> {
        StaAttemptSecurity {
            sequences: self.sequences,
            material: self.material,
            role: PhantomData,
        }
    }
}

/// Internal state invariant failure. The complete outer owner is still
/// returned, but retrying without resetting the transaction would be unsafe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAttemptStateError {
    MissingReceive,
    MissingPreparedPeer,
    MissingAssociation,
    MissingConnectedPeer,
    MissingHandshake,
    MissingKeys,
    MissingConnectedSecurity,
    /// Association ran before authentication selected its security elements.
    MissingSelectedRsn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAttemptSecurityExecution {
    OpenHandshakeAndKeyInstallSkipped,
    Wpa2Personal,
}

/// Connected-entry proof returned only after all preceding phases.
pub struct StaAttemptConnected<O> {
    owner: O,
}

impl<O> StaAttemptConnected<O> {
    pub const fn new(owner: O) -> Self {
        Self { owner }
    }

    pub fn into_owner(self) -> O {
        self.owner
    }
}

/// Exact finite phase currently owned by a station attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum StaAttemptStage {
    Candidate = 0,
    Channel = 1,
    Authentication = 2,
    Association = 3,
    PeerProgramming = 4,
    Wpa2Handshake = 5,
    RsnKeyInstall = 6,
    ConnectedEntry = 7,
}

impl StaAttemptStage {
    pub const COUNT: u8 = 8;

    /// Coarser stage consumed by the chip-independent reconnect policy.
    pub const fn lifecycle_stage(self) -> StaLifecycleStage {
        match self {
            Self::Candidate => StaLifecycleStage::CandidateSelection,
            Self::Channel | Self::PeerProgramming => StaLifecycleStage::Hardware,
            Self::Authentication => StaLifecycleStage::Authentication,
            Self::Association => StaLifecycleStage::Association,
            Self::Wpa2Handshake | Self::RsnKeyInstall => StaLifecycleStage::Security,
            Self::ConnectedEntry => StaLifecycleStage::Connected,
        }
    }

    const fn bit(self) -> u16 {
        1_u16 << self as u8
    }
}

/// Bounded evidence returned at both success and failure edges.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaAttemptProgress {
    completed: u16,
}

impl StaAttemptProgress {
    pub const fn completed(self, stage: StaAttemptStage) -> bool {
        self.completed & stage.bit() != 0
    }

    pub const fn completed_count(self) -> u8 {
        self.completed.count_ones() as u8
    }

    /// Record that `stage` completed.
    pub fn mark_completed(&mut self, stage: StaAttemptStage) {
        self.completed |= stage.bit();
    }
}

/// Port-classified error from a phase which only borrowed the attempt owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAttemptStepError<E> {
    pub disposition: StaFailureDisposition,
    pub error: E,
}

impl<E> StaAttemptStepError<E> {
    pub const fn new(disposition: StaFailureDisposition, error: E) -> Self {
        Self { disposition, error }
    }

    pub const fn retry_current(error: E) -> Self {
        Self::new(StaFailureDisposition::RetryCurrentCandidate, error)
    }

    pub const fn refresh_candidate(error: E) -> Self {
        Self::new(StaFailureDisposition::RefreshCandidate, error)
    }

    pub const fn terminal(error: E) -> Self {
        Self::new(StaFailureDisposition::Terminal, error)
    }
}

/// Failed consuming connected-entry edge with the exact input owner restored.
#[derive(Debug, Eq, PartialEq)]
pub struct StaConnectedEntryFailure<O, E> {
    pub owner: O,
    pub disposition: StaFailureDisposition,
    pub error: E,
}

impl<O, E> StaConnectedEntryFailure<O, E> {
    pub const fn new(owner: O, disposition: StaFailureDisposition, error: E) -> Self {
        Self {
            owner,
            disposition,
            error,
        }
    }
}

/// One finite attempt failure with complete retry or terminal ownership.
#[derive(Debug, Eq, PartialEq)]
pub struct AssociationAttemptFailure<O, E> {
    pub owner: O,
    pub stage: StaAttemptStage,
    pub disposition: StaFailureDisposition,
    pub error: E,
    pub progress: StaAttemptProgress,
}

impl<O, E> AssociationAttemptFailure<O, E> {
    pub const fn lifecycle_stage(&self) -> StaLifecycleStage {
        self.stage.lifecycle_stage()
    }

    pub fn into_parts(
        self,
    ) -> (
        O,
        StaAttemptStage,
        StaFailureDisposition,
        E,
        StaAttemptProgress,
    ) {
        (
            self.owner,
            self.stage,
            self.disposition,
            self.error,
            self.progress,
        )
    }
}

/// Successful connected frontier or a fully owned finite failure.
#[derive(Debug, Eq, PartialEq)]
pub enum AssociationAttemptOutcome<O, C, E> {
    Connected {
        connected: C,
        progress: StaAttemptProgress,
    },
    Failed(AssociationAttemptFailure<O, E>),
}

/// Value-only observation hooks around the production transaction.
///
/// Observers cannot touch the owner or alter driver policy. The normal driver
/// uses `()`; HIL may map these boundaries to UART timing evidence.
pub trait StaAttemptObserver {
    fn stage_started(&mut self, _stage: StaAttemptStage) {}

    fn stage_completed(&mut self, _stage: StaAttemptStage) {}

    fn stage_failed(&mut self, _stage: StaAttemptStage, _disposition: StaFailureDisposition) {}
}

impl StaAttemptObserver for () {}

/// Concrete finite operations needed by the `StaAttempt` transaction of
/// `oer-ieee80211-sta-service`.
///
/// Every non-consuming operation must leave `Owner` at a valid retry or
/// terminal frontier before returning `Err`. Only connected entry consumes
/// the owner, so that edge must return it explicitly on failure.
pub trait StaAttemptPort: Copy {
    type Owner;
    type Connected;
    type Error;

    fn prepare_candidate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn select_channel<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn authenticate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn associate<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn program_peer<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn run_wpa2_handshake<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn install_wpa2_keys<'a>(
        &'a mut self,
        owner: &'a mut Self::Owner,
    ) -> impl Future<Output = Result<(), StaAttemptStepError<Self::Error>>> + 'a;

    fn enter_connected(
        &mut self,
        owner: Self::Owner,
    ) -> impl Future<
        Output = Result<Self::Connected, StaConnectedEntryFailure<Self::Owner, Self::Error>>,
    > + '_;
}

#[cfg(test)]
mod tests;
