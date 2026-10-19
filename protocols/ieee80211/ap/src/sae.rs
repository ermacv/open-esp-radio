//! The access point's SAE responder.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/wpa_supplicant/esp_supplicant/src/esp_wpa3.c` (Copyright
//! Espressif Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!
//! This ports the vendor softAP's SAE authentication: `handle_auth_sae` and
//! `sae_sm_step` of its hostapd, driven the way its WPA3 hostap task drives
//! them. Each station owns one session that moves through Nothing,
//! Committed, Confirmed and Accepted. A station's Commit is answered by this
//! access point's Commit; its Confirm by this access point's Confirm, after
//! which the session is Accepted and yields the PMK and PMKID. With two or
//! more sessions awaiting a Confirm, counting Commits still queued for
//! processing, a Commit without a valid anti-clogging token is answered with
//! a token request (status 76). The responder offers group 19 only and both
//! password element methods, as the vendor's default `sae_pwe` does.
//!
//! Every Commit costs elliptic-curve work, so the caller runs this responder
//! outside its radio loop, as the vendor runs it in its own task.
//!
//! Two vendor quirks are kept: `sae_sync` is never configured, so a second
//! Commit received while Confirmed resets the session, and a Confirm handled
//! without error, even one silently ignored, re-sends the stored Confirm. A
//! Confirm with a non-zero status is dropped: the vendor returns success for
//! it and would report the station authenticated without a verified
//! exchange.
//!
//! SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/wpa_supplicant/src/ap/ieee802_11.c` (`handle_auth_sae`,
//! `sae_sm_step`, `use_sae_anti_clogging`, `sae_accept_sta`,
//! `auth_build_sae_commit`), `src/ap/ap_config.h`
//! (`SAE_ANTI_CLOGGING_THRESHOLD`), `src/common/sae.c`
//! (`sae_write_confirm`) and `esp_supplicant/src/esp_wpa3.c`
//! (`wpa3_process_rx_commit`, `wpa3_process_rx_confirm`).

use oer_ieee80211_rsn::sae::{
    SAE_ANTI_CLOGGING_TOKEN_LEN, SAE_COMMIT_LEN, SAE_CONFIRM_LEN, SAE_GROUP_P256, SAE_KEY_LEN,
    SAE_PMKID_LEN, SAE_PRIME_LEN, SaeComebackTokens, SaeCommit, SaeCommitRefusal, SaeCommitValues,
    SaeError, SaeKeys, SaePassword, SaePasswordElement, SaePasswordToken, SaeReceivedCommit,
};

use crate::limits::AP_MAX_CLIENTS;

/// Sessions awaiting a Confirm, with queued Commits, beyond which a Commit
/// must carry an anti-clogging token.
pub const AP_SAE_ANTI_CLOGGING_THRESHOLD: usize = 2;
/// The vendor never sets `sae_sync`, so its zero-initialised bound applies.
const AP_SAE_SYNC: u8 = 0;
/// The vendor's bound on drawing SAE rand and mask.
const SAE_SCALAR_DRAWS: u8 = 100;

const COMMIT_TRANSACTION: u16 = 1;
const CONFIRM_TRANSACTION: u16 = 2;

/// Status codes of SAE Authentication frames.
pub const AP_SAE_STATUS_SUCCESS: u16 = 0;
pub const AP_SAE_STATUS_UNSPECIFIED_FAILURE: u16 = 1;
pub const AP_SAE_STATUS_CHALLENGE_FAILURE: u16 = 15;
pub const AP_SAE_STATUS_UNABLE_TO_HANDLE_NEW_STA: u16 = 17;
pub const AP_SAE_STATUS_ANTI_CLOGGING_TOKEN_REQUIRED: u16 = 76;
pub const AP_SAE_STATUS_UNSUPPORTED_GROUP: u16 = 77;
pub const AP_SAE_STATUS_HASH_TO_ELEMENT: u16 = 126;

/// The longest body this responder sends: its Commit.
pub const AP_SAE_REPLY_BODY_CAPACITY: usize = SAE_COMMIT_LEN;

/// The password this access point authenticates with and its H2E password
/// token, derived once for the BSS's SSID.
pub struct ApSaeCredential {
    password: SaePassword,
    token: SaePasswordToken,
}

impl ApSaeCredential {
    /// Derive the password token of `ssid`, as the vendor does when its
    /// access point starts.
    pub fn derive(ssid: &[u8], password: SaePassword) -> Self {
        let token = SaePasswordToken::derive(ssid, password.as_bytes(), None);
        Self { password, token }
    }
}

/// Uniform random octets for SAE scalars and comeback-token keys.
pub trait ApSaeRandom {
    fn fill(&mut self, bytes: &mut [u8]);
}

/// One SAE Authentication frame a station sent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApSaeFrame<'a> {
    pub peer: [u8; 6],
    pub transaction: u16,
    pub status: u16,
    pub body: &'a [u8],
}

/// One SAE Authentication frame to send to `peer`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApSaeReply {
    pub peer: [u8; 6],
    pub transaction: u16,
    pub status: u16,
    len: u8,
    body: [u8; AP_SAE_REPLY_BODY_CAPACITY],
}

impl ApSaeReply {
    fn new(peer: [u8; 6], transaction: u16, status: u16, body: &[u8]) -> Self {
        let mut reply = Self {
            peer,
            transaction,
            status,
            len: body.len() as u8,
            body: [0; AP_SAE_REPLY_BODY_CAPACITY],
        };
        reply.body[..body.len()].copy_from_slice(body);
        reply
    }

    pub fn body(&self) -> &[u8] {
        &self.body[..usize::from(self.len)]
    }
}

/// What one received SAE frame changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSaeResult {
    /// The exchange continues, or the frame was dropped.
    Continue,
    /// The station's Confirm verified: the session is Accepted.
    Accepted {
        pmk: [u8; SAE_KEY_LEN],
        pmkid: [u8; SAE_PMKID_LEN],
    },
    /// The exchange failed with `status`. The vendor deauthenticates a
    /// station without an association with this status as the reason.
    Failed { status: u16 },
}

/// The frames to send, in order, and the result of one received frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApSaeOutput {
    replies: [Option<ApSaeReply>; 2],
    pub result: ApSaeResult,
}

impl ApSaeOutput {
    const fn new() -> Self {
        Self {
            replies: [None, None],
            result: ApSaeResult::Continue,
        }
    }

    pub fn replies(&self) -> impl Iterator<Item = &ApSaeReply> {
        self.replies.iter().flatten()
    }

    fn send(&mut self, reply: ApSaeReply) {
        let slot = self
            .replies
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("one received frame sends at most two replies");
        *slot = Some(reply);
    }

    fn fail(mut self, peer: [u8; 6], transaction: u16, status: u16, body: &[u8]) -> Self {
        self.send(ApSaeReply::new(peer, transaction, status, body));
        self.result = ApSaeResult::Failed { status };
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Nothing,
    Committed,
    Confirmed,
    Accepted,
}

struct Session {
    peer: [u8; 6],
    phase: Phase,
    h2e: bool,
    password_identifier: bool,
    own: Option<SaeCommit>,
    peer_commit: Option<SaeCommitValues>,
    keys: Option<SaeKeys>,
    accepted_scalar: Option<[u8; SAE_PRIME_LEN]>,
    sync: u8,
    send_confirm: u16,
    received_confirm: u16,
    stored_confirm: Option<[u8; SAE_CONFIRM_LEN]>,
}

impl Session {
    const fn new(peer: [u8; 6]) -> Self {
        Self {
            peer,
            phase: Phase::Nothing,
            h2e: false,
            password_identifier: false,
            own: None,
            peer_commit: None,
            keys: None,
            accepted_scalar: None,
            sync: 0,
            send_confirm: 0,
            received_confirm: 0,
            stored_confirm: None,
        }
    }

    fn clear_exchange(&mut self) {
        self.own = None;
        self.peer_commit = None;
        self.keys = None;
    }

    fn awaits_confirm(&self) -> bool {
        matches!(self.phase, Phase::Committed | Phase::Confirmed)
    }
}

/// The SAE sessions of one access point epoch.
pub struct ApSaeResponder {
    local: [u8; 6],
    credential: ApSaeCredential,
    comeback: SaeComebackTokens,
    sessions: [Option<Session>; AP_MAX_CLIENTS],
}

impl ApSaeResponder {
    pub fn new(local: [u8; 6], credential: ApSaeCredential) -> Self {
        Self {
            local,
            credential,
            comeback: SaeComebackTokens::new(),
            sessions: [const { None }; AP_MAX_CLIENTS],
        }
    }

    /// Handle one received SAE Authentication frame. `queued_commits`
    /// counts Commits the caller holds for later processing, which the
    /// anti-clogging decision includes.
    pub fn receive<R: ApSaeRandom>(
        &mut self,
        frame: ApSaeFrame<'_>,
        now: oer_time::Instant,
        queued_commits: usize,
        random: &mut R,
    ) -> ApSaeOutput {
        let ApSaeFrame {
            peer,
            transaction,
            status,
            body,
        } = frame;
        match transaction {
            COMMIT_TRANSACTION => {
                self.receive_commit(peer, status, body, now, queued_commits, random)
            }
            CONFIRM_TRANSACTION => self.receive_confirm(peer, status, body),
            _ => ApSaeOutput::new(),
        }
    }

    /// Drop the session of a station this access point removed.
    pub fn forget(&mut self, peer: [u8; 6]) {
        if let Some(slot) = self.slot(peer) {
            self.sessions[slot] = None;
        }
    }

    fn slot(&self, peer: [u8; 6]) -> Option<usize> {
        self.sessions
            .iter()
            .position(|session| session.as_ref().is_some_and(|session| session.peer == peer))
    }

    fn use_anti_clogging(&self, queued_commits: usize) -> bool {
        let open = self
            .sessions
            .iter()
            .flatten()
            .filter(|session| session.awaits_confirm())
            .count();
        open + queued_commits >= AP_SAE_ANTI_CLOGGING_THRESHOLD
    }

    fn receive_commit<R: ApSaeRandom>(
        &mut self,
        peer: [u8; 6],
        status: u16,
        body: &[u8],
        now: oer_time::Instant,
        queued_commits: usize,
        random: &mut R,
    ) -> ApSaeOutput {
        let output = ApSaeOutput::new();
        let slot = match self.slot(peer) {
            Some(slot) => slot,
            None => {
                let Some(slot) = self.sessions.iter().position(Option::is_none) else {
                    let mut output = output;
                    output.send(ApSaeReply::new(
                        peer,
                        COMMIT_TRANSACTION,
                        AP_SAE_STATUS_UNABLE_TO_HANDLE_NEW_STA,
                        &[],
                    ));
                    return output;
                };
                self.sessions[slot] = Some(Session::new(peer));
                slot
            }
        };
        if !matches!(
            status,
            AP_SAE_STATUS_SUCCESS | AP_SAE_STATUS_HASH_TO_ELEMENT
        ) {
            return output;
        }
        let group_offered = body.get(..2) == Some(&SAE_GROUP_P256.to_le_bytes()[..]);
        let session = self.sessions[slot]
            .as_mut()
            .expect("the slot holds a session");
        let mut allow_reuse = false;
        if session.phase == Phase::Committed {
            session.phase = Phase::Nothing;
            if group_offered {
                allow_reuse = true;
            } else {
                session.clear_exchange();
            }
        }
        let h2e = status == AP_SAE_STATUS_HASH_TO_ELEMENT;
        let commit = match SaeReceivedCommit::parse(body, h2e) {
            Ok(commit) => commit,
            Err(SaeCommitRefusal::UnsupportedGroup) => {
                return output.fail(
                    peer,
                    COMMIT_TRANSACTION,
                    AP_SAE_STATUS_UNSUPPORTED_GROUP,
                    &body[..2],
                );
            }
            Err(SaeCommitRefusal::Unspecified) => {
                return output.fail(
                    peer,
                    COMMIT_TRANSACTION,
                    AP_SAE_STATUS_UNSPECIFIED_FAILURE,
                    &[],
                );
            }
        };
        if session.phase == Phase::Accepted && session.accepted_scalar == Some(commit.values.scalar)
        {
            return output.fail(
                peer,
                COMMIT_TRANSACTION,
                AP_SAE_STATUS_UNSPECIFIED_FAILURE,
                &[],
            );
        }
        if session
            .own
            .as_ref()
            .is_some_and(|own| own.values() == commit.values)
        {
            // A reflected Commit is dropped without a reply.
            return output;
        }
        if let Some(token) = commit.token
            && !self.comeback.check(peer, token)
        {
            let mut output = output;
            output.result = ApSaeResult::Failed {
                status: AP_SAE_STATUS_UNSPECIFIED_FAILURE,
            };
            return output;
        }
        if commit.rejects_offered_group() {
            return output.fail(
                peer,
                COMMIT_TRANSACTION,
                AP_SAE_STATUS_UNSPECIFIED_FAILURE,
                &[],
            );
        }
        if commit.token.is_none() && !allow_reuse && self.use_anti_clogging(queued_commits) {
            let session = self.sessions[slot]
                .as_ref()
                .expect("the slot holds a session");
            let h2e = h2e || session.h2e;
            let token = self.comeback.issue(peer, now, || {
                let mut key = [0; 32];
                random.fill(&mut key);
                key
            });
            let mut request = [0_u8; 2 + 3 + SAE_ANTI_CLOGGING_TOKEN_LEN];
            request[..2].copy_from_slice(&SAE_GROUP_P256.to_le_bytes());
            let len = if h2e {
                request[2..5].copy_from_slice(&[255, 1 + SAE_ANTI_CLOGGING_TOKEN_LEN as u8, 93]);
                request[5..].copy_from_slice(&token);
                request.len()
            } else {
                request[2..2 + SAE_ANTI_CLOGGING_TOKEN_LEN].copy_from_slice(&token);
                2 + SAE_ANTI_CLOGGING_TOKEN_LEN
            };
            let mut output = output;
            output.send(ApSaeReply::new(
                peer,
                COMMIT_TRANSACTION,
                AP_SAE_STATUS_ANTI_CLOGGING_TOKEN_REQUIRED,
                &request[..len],
            ));
            return output;
        }
        let local = self.local;
        let credential = &self.credential;
        let session = self.sessions[slot]
            .as_mut()
            .expect("the slot holds a session");
        session.password_identifier = commit.password_identifier;
        commit_step(
            local,
            credential,
            session,
            commit.values,
            h2e,
            allow_reuse,
            random,
        )
    }

    fn receive_confirm(&mut self, peer: [u8; 6], status: u16, body: &[u8]) -> ApSaeOutput {
        let mut output = ApSaeOutput::new();
        let Some(slot) = self.slot(peer) else {
            return output;
        };
        let session = self.sessions[slot]
            .as_mut()
            .expect("the slot holds a session");
        if status != AP_SAE_STATUS_SUCCESS {
            return output;
        }
        let Some(send_confirm) = body
            .get(..2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        else {
            return output.fail(
                peer,
                CONFIRM_TRANSACTION,
                AP_SAE_STATUS_UNSPECIFIED_FAILURE,
                &[],
            );
        };
        if session.phase == Phase::Accepted
            && (send_confirm <= session.received_confirm || send_confirm == u16::MAX)
        {
            if let Some(confirm) = session.stored_confirm {
                output.send(ApSaeReply::new(peer, CONFIRM_TRANSACTION, 0, &confirm));
            }
            return output;
        }
        let verified = session
            .keys
            .as_ref()
            .is_some_and(|keys| keys.verify_peer_confirm(body).is_ok());
        if !verified {
            return output.fail(
                peer,
                CONFIRM_TRANSACTION,
                AP_SAE_STATUS_CHALLENGE_FAILURE,
                &[],
            );
        }
        session.received_confirm = send_confirm;
        match session.phase {
            Phase::Nothing => {}
            Phase::Committed => {
                store_confirm(session);
                session.phase = Phase::Confirmed;
                output.result = accept(session);
            }
            Phase::Confirmed => output.result = accept(session),
            Phase::Accepted => {
                if session.sync > AP_SAE_SYNC {
                    session.phase = Phase::Nothing;
                    session.sync = 0;
                } else {
                    session.sync += 1;
                    store_confirm(session);
                    session.clear_exchange();
                }
            }
        }
        if let Some(confirm) = session.stored_confirm {
            output.send(ApSaeReply::new(peer, CONFIRM_TRANSACTION, 0, &confirm));
        }
        output
    }
}

/// `sae_sm_step` for a Commit that passed every check.
fn commit_step<R: ApSaeRandom>(
    local: [u8; 6],
    credential: &ApSaeCredential,
    session: &mut Session,
    peer_commit: SaeCommitValues,
    h2e: bool,
    allow_reuse: bool,
    random: &mut R,
) -> ApSaeOutput {
    let mut output = ApSaeOutput::new();
    let peer = session.peer;
    let refresh = match session.phase {
        Phase::Nothing => {
            session.h2e = h2e;
            !allow_reuse
        }
        Phase::Committed => unreachable!("a Committed session is reset before its step"),
        Phase::Confirmed => {
            if session.sync > AP_SAE_SYNC {
                session.phase = Phase::Nothing;
                session.sync = 0;
                return output;
            }
            session.sync += 1;
            true
        }
        Phase::Accepted => true,
    };
    if refresh || session.own.is_none() {
        let use_token = session.password_identifier || h2e;
        match draw_commit(local, peer, credential, use_token, random) {
            Ok(own) => session.own = Some(own),
            Err(_) => {
                return output.fail(
                    peer,
                    COMMIT_TRANSACTION,
                    AP_SAE_STATUS_UNSPECIFIED_FAILURE,
                    &[],
                );
            }
        }
    }
    let own = session
        .own
        .as_ref()
        .expect("the session holds its own commit");
    let mut body = [0_u8; SAE_COMMIT_LEN];
    let len = own
        .values()
        .encode(None, session.h2e, &mut body)
        .expect("a commit without a token fits its capacity");
    let commit_status = if session.h2e {
        AP_SAE_STATUS_HASH_TO_ELEMENT
    } else {
        AP_SAE_STATUS_SUCCESS
    };
    output.send(ApSaeReply::new(
        peer,
        COMMIT_TRANSACTION,
        commit_status,
        &body[..len],
    ));
    let continuing = session.phase;
    if continuing != Phase::Confirmed {
        session.phase = Phase::Committed;
    }
    match own.process(peer_commit) {
        Ok(keys) => {
            session.keys = Some(keys);
            session.peer_commit = Some(peer_commit);
        }
        Err(_) => {
            return output.fail(
                peer,
                COMMIT_TRANSACTION,
                AP_SAE_STATUS_UNSPECIFIED_FAILURE,
                &[],
            );
        }
    }
    if continuing == Phase::Confirmed {
        store_confirm(session);
    } else {
        session.sync = 0;
    }
    output
}

fn draw_commit<R: ApSaeRandom>(
    local: [u8; 6],
    peer: [u8; 6],
    credential: &ApSaeCredential,
    use_token: bool,
    random: &mut R,
) -> Result<SaeCommit, SaeError> {
    let pwe = if use_token {
        credential.token.password_element(local, peer)
    } else {
        SaePasswordElement::hunting_and_pecking(credential.password.as_bytes(), local, peer)?
    };
    for _ in 0..SAE_SCALAR_DRAWS {
        let mut rand = [0; SAE_PRIME_LEN];
        let mut mask = [0; SAE_PRIME_LEN];
        random.fill(&mut rand);
        random.fill(&mut mask);
        match SaeCommit::new(pwe, rand, mask) {
            Err(SaeError::UnsuitableRandom) => {}
            commit => return commit,
        }
    }
    Err(SaeError::UnsuitableRandom)
}

/// `sae_write_confirm`: advance send-confirm and keep the Confirm for the
/// reply the next handled Confirm sends.
fn store_confirm(session: &mut Session) {
    session.send_confirm = session.send_confirm.saturating_add(1);
    let keys = session
        .keys
        .as_ref()
        .expect("a confirm follows a processed commit");
    session.stored_confirm = Some(keys.own_confirm(session.send_confirm));
}

/// `sae_accept_sta`.
fn accept(session: &mut Session) -> ApSaeResult {
    session.send_confirm = u16::MAX;
    session.accepted_scalar = session.peer_commit.map(|commit| commit.scalar);
    session.phase = Phase::Accepted;
    let keys = session
        .keys
        .as_ref()
        .expect("an accepted session holds its keys");
    ApSaeResult::Accepted {
        pmk: keys.pmk,
        pmkid: keys.pmkid,
    }
}

#[cfg(test)]
mod tests;
