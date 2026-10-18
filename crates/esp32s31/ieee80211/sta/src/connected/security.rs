//! Connected WPA2 transaction and the unique installed group-key authority.
//!
//! The owner retains supplicant, GTK and replay state across each finite
//! transaction. Runtime adapters supply classified EAPOL and existing hardware
//! and TX ports; no mailbox, executor timer or task owner belongs here.

use crate::connected::management_protection::StationManagementProtection;
use crate::{
    connected_control::{ConnectedControlTx, ConnectedDisconnectReason},
    connected_rx::{StaCcmpRxReplayControlEndpoint, StaCcmpRxReplayError},
    hardware::control::ConnectedControlHardware,
};

use oer_esp32s31_ieee80211::datapath::DatapathControlProgress;

use oer_esp32s31_ieee80211_mac::crypto::{
    CryptoKeyError, StaGroupCcmpKeyMaterial, StaGroupCcmpReplaceError, StaGroupCcmpSlot,
};

use oer_ieee80211_rsn::{
    OwnedEapolFrame,
    aes::{SoftwareAesKeyUnwrapError, software_aes128_key_unwrap},
    keys::RsnKeyKind,
    supplicant::{
        RsnConnectedAction, RsnConnectedProcessError, RsnConnectedSupplicant,
        RsnConnectedSupplicantError,
    },
};

/// Protection provenance retained across the borrowed RX-to-control handoff.
///
/// Connected WPA2 admits plaintext only through the dedicated duplicate-M3
/// lane. Keeping that fact outside `OwnedEapolFrame` prevents the control task
/// from treating an unprotected packet as a Group Message 1. The variants carry
/// the RX binding's classification; they do not validate protection themselves.
pub enum ConnectedSecurityFrame {
    Protected(OwnedEapolFrame),
    Unprotected(OwnedEapolFrame),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedWpa2SecurityFailure {
    Protocol(RsnConnectedSupplicantError),
    KeyUnwrap(SoftwareAesKeyUnwrapError),
    InvalidGroupKeyKind,
    InvalidGroupKeyMaterial(CryptoKeyError),
    ReplayRotation(StaCcmpRxReplayError),
    SameKeyIdGenerationUnavailable,
    RetiredKeyIdGenerationUnavailable { key_id: u8 },
    KeyReplace(StaGroupCcmpReplaceError),
    KeyInstall(CryptoKeyError),
    TxStart(crate::single_mpdu_tx::SingleMpduTxError),
    TxOutcome(crate::single_mpdu_tx::SingleMpduTxOutcome),
    MissingTxOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedWpa2SecurityEvidence {
    pub replay_counter: u64,
    pub group_message1: u32,
    pub duplicate_message3: u32,
    pub ignored_duplicate_message3: u32,
    pub last_ignored_duplicate_message3: Option<RsnConnectedSupplicantError>,
    pub installed: u32,
    pub retransmitted: u32,
    pub tx_in_flight: bool,
    pub last_failure: Option<ConnectedWpa2SecurityFailure>,
}

/// Association-scoped WPA2 state and the unique installed GTK authority.
///
/// This owner lives inside connected control while RX may publish Group
/// Message 1. It is explicitly recovered after IRQ/task quiescence, before
/// the ordinary station teardown clears the hardware key.
pub struct ConnectedWpa2Security {
    supplicant: RsnConnectedSupplicant,
    group: StaGroupCcmpSlot,
    group_material: StaGroupCcmpKeyMaterial,
    used_group_key_ids: u8,
    replay: StaCcmpRxReplayControlEndpoint,
    /// Receive protection of robust management frames, when negotiated.
    management: Option<StationManagementProtection>,
    tx_in_flight: bool,
    group_message1: u32,
    duplicate_message3: u32,
    ignored_duplicate_message3: u32,
    last_ignored_duplicate_message3: Option<RsnConnectedSupplicantError>,
    installed: u32,
    retransmitted: u32,
    last_failure: Option<ConnectedWpa2SecurityFailure>,
}

impl ConnectedWpa2Security {
    pub const fn new(
        supplicant: RsnConnectedSupplicant,
        group: StaGroupCcmpSlot,
        group_material: StaGroupCcmpKeyMaterial,
        replay: StaCcmpRxReplayControlEndpoint,
        management: Option<StationManagementProtection>,
    ) -> Self {
        let used_group_key_ids = 1_u8 << group_material.key_id();
        Self {
            supplicant,
            group,
            group_material,
            used_group_key_ids,
            replay,
            management,
            tx_in_flight: false,
            group_message1: 0,
            duplicate_message3: 0,
            ignored_duplicate_message3: 0,
            last_ignored_duplicate_message3: None,
            installed: 0,
            retransmitted: 0,
            last_failure: None,
        }
    }

    /// Receive protection of the association's robust management frames.
    pub fn management_protection(&mut self) -> Option<&mut StationManagementProtection> {
        self.management.as_mut()
    }

    /// Whether this owner awaits completion of its protected control response.
    pub const fn tx_in_flight(&self) -> bool {
        self.tx_in_flight
    }

    pub const fn evidence(&self) -> ConnectedWpa2SecurityEvidence {
        ConnectedWpa2SecurityEvidence {
            replay_counter: self.supplicant.replay_counter(),
            group_message1: self.group_message1,
            duplicate_message3: self.duplicate_message3,
            ignored_duplicate_message3: self.ignored_duplicate_message3,
            last_ignored_duplicate_message3: self.last_ignored_duplicate_message3,
            installed: self.installed,
            retransmitted: self.retransmitted,
            tx_in_flight: self.tx_in_flight,
            last_failure: self.last_failure,
        }
    }

    pub fn into_parts(mut self) -> (RsnConnectedSupplicant, StaGroupCcmpSlot) {
        // Connected lifecycle calls this only after the RX task has returned.
        // A stale stop still leaves the resource quarantined by its endpoint
        // Drop implementation and cannot reopen group publication.
        let _ = self.replay.stop();
        (self.supplicant, self.group)
    }

    fn fail(
        &mut self,
        failure: ConnectedWpa2SecurityFailure,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        self.last_failure = Some(failure);
        DatapathControlProgress::Exit(ConnectedDisconnectReason::GroupKeyHandshakeFailed)
    }

    /// Consume the terminal report from the same TX port used for the response.
    /// The caller invokes this only after that physical transaction completes.
    pub fn complete_tx<X: ConnectedControlTx>(
        &mut self,
        tx: &mut X,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        let Some(outcome) = tx.take_last_outcome() else {
            return self.fail(ConnectedWpa2SecurityFailure::MissingTxOutcome);
        };
        self.tx_in_flight = false;
        if outcome.is_success() {
            DatapathControlProgress::More
        } else {
            self.fail(ConnectedWpa2SecurityFailure::TxOutcome(outcome))
        }
    }

    /// Process one classified EAPOL frame while retaining the installed GTK and
    /// replay authority. The caller supplies its existing hardware and TX ports;
    /// mailbox delivery, waits and task shutdown remain outside this owner.
    pub async fn process<H: ConnectedControlHardware, X: ConnectedControlTx>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        frame: ConnectedSecurityFrame,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        match frame {
            ConnectedSecurityFrame::Protected(frame) => {
                self.process_group_message1(hardware, tx, frame)
            }
            ConnectedSecurityFrame::Unprotected(frame) => {
                self.process_duplicate_message3(hardware, tx, frame)
            }
        }
    }

    fn process_duplicate_message3<H: ConnectedControlHardware, X: ConnectedControlTx>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        frame: oer_ieee80211_rsn::OwnedEapolFrame,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        self.duplicate_message3 = self.duplicate_message3.saturating_add(1);
        let response = match self.supplicant.on_duplicate_message3(frame) {
            Ok(response) => response,
            Err(error) => {
                // A connected peer or nearby attacker may inject malformed or
                // stale plaintext EAPOL. It has no authority to tear down the
                // installed association; only an exact authenticated duplicate
                // M3 is actionable.
                self.ignored_duplicate_message3 = self.ignored_duplicate_message3.saturating_add(1);
                self.last_ignored_duplicate_message3 = Some(error);
                return DatapathControlProgress::More;
            }
        };
        self.retransmitted = self.retransmitted.saturating_add(1);
        match tx.start_protected_eapol(hardware, response.as_bytes()) {
            Ok(progress) => {
                self.tx_in_flight = true;
                progress
            }
            Err(error) => self.fail(ConnectedWpa2SecurityFailure::TxStart(error)),
        }
    }

    fn process_group_message1<H: ConnectedControlHardware, X: ConnectedControlTx>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        frame: oer_ieee80211_rsn::OwnedEapolFrame,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        self.group_message1 = self.group_message1.saturating_add(1);
        let action = match self.supplicant.on_group_message1(frame) {
            Ok(action) => action,
            Err(error) => {
                return self.fail(ConnectedWpa2SecurityFailure::Protocol(error));
            }
        };
        let response = match action {
            RsnConnectedAction::Retransmit(response) => {
                self.retransmitted = self.retransmitted.saturating_add(1);
                response
            }
            RsnConnectedAction::UnwrapGroupKeyData(request) => {
                // The RustCrypto key unwrap completes synchronously; the
                // connected owner needs no asynchronous AES backend.
                let unwrapped =
                    software_aes128_key_unwrap(request.kek(), request.wrapped_key_data());
                let request = match self
                    .supplicant
                    .complete_group_key_data_unwrap(request, unwrapped)
                {
                    Ok(request) => request,
                    Err(RsnConnectedProcessError::Supplicant(error)) => {
                        return self.fail(ConnectedWpa2SecurityFailure::Protocol(error));
                    }
                    Err(RsnConnectedProcessError::KeyUnwrap(error)) => {
                        return self.fail(ConnectedWpa2SecurityFailure::KeyUnwrap(error));
                    }
                };
                let RsnKeyKind::Group { key_id, .. } = request.group().kind() else {
                    let _ = self.supplicant.complete_group_key_install(request, false);
                    return self.fail(ConnectedWpa2SecurityFailure::InvalidGroupKeyKind);
                };
                let replacement =
                    match StaGroupCcmpKeyMaterial::new(key_id, *request.group().key().as_bytes()) {
                        Ok(replacement) => replacement,
                        Err(error) => {
                            let _ = self.supplicant.complete_group_key_install(request, false);
                            return self.fail(
                                ConnectedWpa2SecurityFailure::InvalidGroupKeyMaterial(error),
                            );
                        }
                    };
                let same_key_id = self.group_material.key_id() == replacement.key_id();
                let same_temporal_key = self.group_material.same_temporal_key(&replacement);
                if same_key_id && !same_temporal_key {
                    // S31 exposes one STA group slot and no RX descriptor key
                    // generation. A frame authenticated with the old GTK can
                    // already be staged when control receives Group M1. With
                    // the same logical KeyID it cannot be distinguished after
                    // a key replacement, so do not manufacture unsafe support.
                    let _ = self.supplicant.complete_group_key_install(request, false);
                    return self.fail(ConnectedWpa2SecurityFailure::SameKeyIdGenerationUnavailable);
                }
                let key_id_mask = 1_u8 << replacement.key_id();
                if !same_key_id && self.used_group_key_ids & key_id_mask != 0 {
                    // The retired key ID may still label an authenticated
                    // frame in the RX staging pipeline. Reusing it for a new
                    // generation would make that frame indistinguishable
                    // from replacement traffic after hardware installation.
                    let _ = self.supplicant.complete_group_key_install(request, false);
                    return self.fail(
                        ConnectedWpa2SecurityFailure::RetiredKeyIdGenerationUnavailable {
                            key_id: replacement.key_id(),
                        },
                    );
                }
                let prepared = match self
                    .replay
                    .prepare_group_rotation(key_id, *request.group().receive_sequence())
                {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let _ = self.supplicant.complete_group_key_install(request, false);
                        return self.fail(ConnectedWpa2SecurityFailure::ReplayRotation(error));
                    }
                };
                let installing = match self.replay.begin_group_rotation(prepared) {
                    Ok(installing) => installing,
                    Err(error) => {
                        let _ = self.supplicant.complete_group_key_install(request, false);
                        return self.fail(ConnectedWpa2SecurityFailure::ReplayRotation(error));
                    }
                };

                if same_key_id {
                    // An authenticated repeat of the exact GTK changes no
                    // hardware generation. Rotating only the RSC is safe even
                    // when old frames are staged because they authenticate
                    // under the still-current key.
                    if let Err(error) = self.replay.commit_group_rotation(installing) {
                        let _ = self.supplicant.complete_group_key_install(request, false);
                        return self.fail(ConnectedWpa2SecurityFailure::ReplayRotation(error));
                    }
                } else {
                    match hardware.replace_sta_group_ccmp(
                        &mut self.group,
                        &self.group_material,
                        &replacement,
                    ) {
                        Ok(()) => {
                            if let Err(error) = self.replay.commit_group_rotation(installing) {
                                // Hardware now contains the replacement. The
                                // replay resource quarantines group RX on a
                                // failed commit; outer disconnect clears the
                                // slot without publishing a mixed epoch.
                                let _ = self.supplicant.complete_group_key_install(request, false);
                                return self
                                    .fail(ConnectedWpa2SecurityFailure::ReplayRotation(error));
                            }
                            self.group_material = replacement;
                            self.used_group_key_ids |= key_id_mask;
                            oer_trace::emit(&oer_ieee80211_trace::LinkControlTrace {
                                event: oer_ieee80211_trace::LinkEvent::GroupKeyRotated { key_id },
                            });
                        }
                        Err(error @ StaGroupCcmpReplaceError::ReplacementRolledBack(_)) => {
                            let abort = self.replay.abort_group_rotation(installing);
                            let _ = self.supplicant.complete_group_key_install(request, false);
                            if let Err(replay_error) = abort {
                                return self.fail(ConnectedWpa2SecurityFailure::ReplayRotation(
                                    replay_error,
                                ));
                            }
                            return self.fail(ConnectedWpa2SecurityFailure::KeyReplace(error));
                        }
                        Err(error) => {
                            self.replay.quarantine_group_rotation(installing);
                            let _ = self.supplicant.complete_group_key_install(request, false);
                            return self.fail(ConnectedWpa2SecurityFailure::KeyReplace(error));
                        }
                    }
                }
                if let (Some(management), Some(igtk)) = (self.management.as_mut(), request.igtk()) {
                    management.install_igtk(igtk);
                }
                match self.supplicant.complete_group_key_install(request, true) {
                    Ok(response) => {
                        self.installed = self.installed.saturating_add(1);
                        response
                    }
                    Err(error) => {
                        return self.fail(ConnectedWpa2SecurityFailure::Protocol(error));
                    }
                }
            }
        };
        match tx.start_protected_eapol(hardware, response.as_bytes()) {
            Ok(progress) => {
                self.tx_in_flight = true;
                progress
            }
            Err(error) => self.fail(ConnectedWpa2SecurityFailure::TxStart(error)),
        }
    }
}
