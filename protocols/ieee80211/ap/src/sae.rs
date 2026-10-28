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
    SAE_PRIME_LEN, SaeComebackTokens, SaeCommit, SaeCommitRefusal, SaeCommitValues, SaeError,
    SaeKeys, SaePassword, SaePasswordElement, SaePasswordToken, SaeReceivedCommit,
};
use oer_ieee80211_wire::sae::{
    anti_clogging_token_container_header, finite_cyclic_group, send_confirm,
};
use oer_ieee80211_wire::security::rsn::RSN_PMKID_LEN;
use oer_ieee80211_wire::station::{SAE_COMMIT_TRANSACTION, SAE_CONFIRM_TRANSACTION};
use oer_ieee80211_wire::status::status_code;

use crate::limits::AP_MAX_CLIENTS;

/// Sessions awaiting a Confirm, with queued Commits, beyond which a Commit
/// must carry an anti-clogging token.
pub const AP_SAE_ANTI_CLOGGING_THRESHOLD: usize = 2;
/// The vendor never sets `sae_sync`, so its zero-initialised bound applies.
const AP_SAE_SYNC: u8 = 0;
/// The vendor's bound on drawing SAE rand and mask.
const SAE_SCALAR_DRAWS: u8 = 100;

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
    pub fn derive(ssid: &[u8], password: SaePassword) -> Result<Self, SaeError> {
        let token = SaePasswordToken::derive(ssid, password.as_bytes(), None)?;
        Ok(Self { password, token })
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
    body: ReplyBody,
}

// Only protocol-defined body lengths can be published. No run-time length
// accompanies the fixed arrays, so reading a reply cannot fail or truncate it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplyBody {
    Empty,
    Group([u8; 2]),
    Token([u8; 2 + SAE_ANTI_CLOGGING_TOKEN_LEN]),
    H2eToken([u8; 5 + SAE_ANTI_CLOGGING_TOKEN_LEN]),
    Commit([u8; SAE_COMMIT_LEN]),
    Confirm([u8; SAE_CONFIRM_LEN]),
}

impl ApSaeReply {
    fn new(peer: [u8; 6], transaction: u16, status: u16, body: ReplyBody) -> Self {
        Self {
            peer,
            transaction,
            status,
            body,
        }
    }

    pub fn body(&self) -> &[u8] {
        match &self.body {
            ReplyBody::Empty => &[],
            ReplyBody::Group(body) => body,
            ReplyBody::Token(body) => body,
            ReplyBody::H2eToken(body) => body,
            ReplyBody::Commit(body) => body,
            ReplyBody::Confirm(body) => body,
        }
    }
}

/// The local operation whose curve invariant failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSaeCryptoStage {
    DrawCommit,
    Encode(SaeError),
}

/// A responder invariant failure, retaining its source observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSaeFault {
    /// An RSN field or curve conversion failed its mathematical invariant.
    Crypto {
        peer: [u8; 6],
        stage: ApSaeCryptoStage,
    },
    MissingSession {
        slot: usize,
    },
    InvalidSession {
        peer: [u8; 6],
        phase: ApSaePhase,
        own_commit: bool,
        peer_commit: bool,
        keys: bool,
        sync: u8,
    },
    CommitLength {
        length: usize,
    },
}

/// What one received SAE frame changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSaeResult {
    /// A local invariant failed; the caller retains this epoch for diagnosis.
    Fault(ApSaeFault),
    /// The exchange continues, or the frame was dropped.
    Continue,
    /// The station's Confirm verified: the session is Accepted.
    Accepted {
        pmk: [u8; SAE_KEY_LEN],
        pmkid: [u8; RSN_PMKID_LEN],
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

    fn reply(reply: ApSaeReply) -> Self {
        Self {
            replies: [Some(reply), None],
            result: ApSaeResult::Continue,
        }
    }

    fn fault(fault: ApSaeFault) -> Self {
        Self {
            result: ApSaeResult::Fault(fault),
            ..Self::new()
        }
    }

    fn failure(peer: [u8; 6], transaction: u16, status: u16, body: ReplyBody) -> Self {
        Self {
            replies: [Some(ApSaeReply::new(peer, transaction, status, body)), None],
            result: ApSaeResult::Failed { status },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSaePhase {
    Nothing,
    Committed,
    Confirmed,
    Accepted,
}

struct Session {
    peer: [u8; 6],
    phase: ApSaePhase,
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
            phase: ApSaePhase::Nothing,
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

    fn fault(&self) -> ApSaeFault {
        ApSaeFault::InvalidSession {
            peer: self.peer,
            phase: self.phase,
            own_commit: self.own.is_some(),
            peer_commit: self.peer_commit.is_some(),
            keys: self.keys.is_some(),
            sync: self.sync,
        }
    }

    fn clear_exchange(&mut self) {
        self.own = None;
        self.peer_commit = None;
        self.keys = None;
    }

    fn awaits_confirm(&self) -> bool {
        matches!(self.phase, ApSaePhase::Committed | ApSaePhase::Confirmed)
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
            SAE_COMMIT_TRANSACTION => {
                self.receive_commit(peer, status, body, now, queued_commits, random)
            }
            SAE_CONFIRM_TRANSACTION => self.receive_confirm(peer, status, body),
            _ => ApSaeOutput::new(),
        }
    }

    /// Drop the session of a station this access point removed.
    pub fn forget(&mut self, peer: [u8; 6]) {
        for session in &mut self.sessions {
            if session.as_ref().is_some_and(|session| session.peer == peer) {
                *session = None;
            }
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
        // This is only a threshold predicate, not a stored count: saturation
        // preserves the mathematical comparison even for usize::MAX queued.
        open.saturating_add(queued_commits) >= AP_SAE_ANTI_CLOGGING_THRESHOLD
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
                    return ApSaeOutput::reply(ApSaeReply::new(
                        peer,
                        SAE_COMMIT_TRANSACTION,
                        status_code::DENIED_NO_MORE_STAS,
                        ReplyBody::Empty,
                    ));
                };
                let Some(entry) = self.sessions.get_mut(slot) else {
                    return ApSaeOutput::fault(ApSaeFault::MissingSession { slot });
                };
                *entry = Some(Session::new(peer));
                slot
            }
        };
        if !matches!(
            status,
            status_code::SUCCESS | status_code::SAE_HASH_TO_ELEMENT
        ) {
            return output;
        }
        let group_offered = finite_cyclic_group(body) == Some(SAE_GROUP_P256);
        let Some(session) = self.sessions.get_mut(slot).and_then(Option::as_mut) else {
            return ApSaeOutput::fault(ApSaeFault::MissingSession { slot });
        };
        let mut allow_reuse = false;
        if session.phase == ApSaePhase::Committed {
            session.phase = ApSaePhase::Nothing;
            if group_offered {
                allow_reuse = true;
            } else {
                session.clear_exchange();
            }
        }
        let h2e = status == status_code::SAE_HASH_TO_ELEMENT;
        let commit = match SaeReceivedCommit::parse(body, h2e) {
            Ok(commit) => commit,
            Err(SaeCommitRefusal::UnsupportedGroup) => {
                return ApSaeOutput::failure(
                    peer,
                    SAE_COMMIT_TRANSACTION,
                    status_code::UNSUPPORTED_FINITE_CYCLIC_GROUP,
                    ReplyBody::Group(match finite_cyclic_group(body) {
                        Some(group) => group.to_le_bytes(),
                        None => return ApSaeOutput::fault(session.fault()),
                    }),
                );
            }
            Err(SaeCommitRefusal::Unspecified) => {
                return ApSaeOutput::failure(
                    peer,
                    SAE_COMMIT_TRANSACTION,
                    status_code::UNSPECIFIED_FAILURE,
                    ReplyBody::Empty,
                );
            }
        };
        if session.phase == ApSaePhase::Accepted
            && session.accepted_scalar == Some(commit.values.scalar)
        {
            return ApSaeOutput::failure(
                peer,
                SAE_COMMIT_TRANSACTION,
                status_code::UNSPECIFIED_FAILURE,
                ReplyBody::Empty,
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
                status: status_code::UNSPECIFIED_FAILURE,
            };
            return output;
        }
        if commit.rejects_offered_group() {
            return ApSaeOutput::failure(
                peer,
                SAE_COMMIT_TRANSACTION,
                status_code::UNSPECIFIED_FAILURE,
                ReplyBody::Empty,
            );
        }
        if commit.token.is_none() && !allow_reuse && self.use_anti_clogging(queued_commits) {
            let Some(session) = self.sessions.get(slot).and_then(Option::as_ref) else {
                return ApSaeOutput::fault(ApSaeFault::MissingSession { slot });
            };
            let h2e = h2e || session.h2e;
            let token = self.comeback.issue(peer, now, || {
                let mut key = [0; 32];
                random.fill(&mut key);
                key
            });
            let group = SAE_GROUP_P256.to_le_bytes();
            let reply_body = if h2e {
                const CONTAINER: [u8; 3] =
                    match anti_clogging_token_container_header(SAE_ANTI_CLOGGING_TOKEN_LEN) {
                        Some(header) => header,
                        None => panic!("the anti-clogging token fits its container"),
                    };
                let mut request = [0; 5 + SAE_ANTI_CLOGGING_TOKEN_LEN];
                let [g0, g1] = group;
                let [c0, c1, c2] = CONTAINER;
                let header = [g0, g1, c0, c1, c2];
                for (destination, source) in
                    request.iter_mut().zip(header.iter().chain(token.iter()))
                {
                    *destination = *source;
                }
                ReplyBody::H2eToken(request)
            } else {
                let mut request = [0; 2 + SAE_ANTI_CLOGGING_TOKEN_LEN];
                for (destination, source) in
                    request.iter_mut().zip(group.iter().chain(token.iter()))
                {
                    *destination = *source;
                }
                ReplyBody::Token(request)
            };
            return ApSaeOutput::reply(ApSaeReply::new(
                peer,
                SAE_COMMIT_TRANSACTION,
                status_code::ANTI_CLOGGING_TOKEN_REQUIRED,
                reply_body,
            ));
        }
        let local = self.local;
        let credential = &self.credential;
        let Some(session) = self.sessions.get_mut(slot).and_then(Option::as_mut) else {
            return ApSaeOutput::fault(ApSaeFault::MissingSession { slot });
        };
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
        let Some(session) = self.sessions.get_mut(slot).and_then(Option::as_mut) else {
            return ApSaeOutput::fault(ApSaeFault::MissingSession { slot });
        };
        if status != status_code::SUCCESS {
            return output;
        }
        let Some(send_confirm) = send_confirm(body) else {
            return ApSaeOutput::failure(
                peer,
                SAE_CONFIRM_TRANSACTION,
                status_code::UNSPECIFIED_FAILURE,
                ReplyBody::Empty,
            );
        };
        if session.phase == ApSaePhase::Accepted
            && (send_confirm <= session.received_confirm || send_confirm == u16::MAX)
        {
            if let Some(confirm) = session.stored_confirm {
                output.replies = [
                    Some(ApSaeReply::new(
                        peer,
                        SAE_CONFIRM_TRANSACTION,
                        0,
                        ReplyBody::Confirm(confirm),
                    )),
                    None,
                ];
            }
            return output;
        }
        let verified = session
            .keys
            .as_ref()
            .is_some_and(|keys| keys.verify_peer_confirm(body).is_ok());
        if !verified {
            return ApSaeOutput::failure(
                peer,
                SAE_CONFIRM_TRANSACTION,
                status_code::CHALLENGE_FAILURE,
                ReplyBody::Empty,
            );
        }
        session.received_confirm = send_confirm;
        match session.phase {
            ApSaePhase::Nothing => {}
            ApSaePhase::Committed => {
                if let Err(fault) = store_confirm(session) {
                    return ApSaeOutput::fault(fault);
                }
                session.phase = ApSaePhase::Confirmed;
                output.result = accept(session).unwrap_or_else(ApSaeResult::Fault);
            }
            ApSaePhase::Confirmed => {
                output.result = accept(session).unwrap_or_else(ApSaeResult::Fault)
            }
            ApSaePhase::Accepted => {
                if session.sync > AP_SAE_SYNC {
                    session.phase = ApSaePhase::Nothing;
                    session.sync = 0;
                } else {
                    session.sync = match session.sync.checked_add(1) {
                        Some(sync) => sync,
                        None => return ApSaeOutput::fault(session.fault()),
                    };
                    if let Err(fault) = store_confirm(session) {
                        return ApSaeOutput::fault(fault);
                    }
                    session.clear_exchange();
                }
            }
        }
        if let Some(confirm) = session.stored_confirm {
            output.replies = [
                Some(ApSaeReply::new(
                    peer,
                    SAE_CONFIRM_TRANSACTION,
                    0,
                    ReplyBody::Confirm(confirm),
                )),
                None,
            ];
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
    commit_step_with(session, peer_commit, h2e, allow_reuse, |peer, use_token| {
        draw_commit(local, peer, credential, use_token, random)
    })
}

// Keep the impossible curve result injectable without a production corruption API.
fn commit_step_with(
    session: &mut Session,
    peer_commit: SaeCommitValues,
    h2e: bool,
    allow_reuse: bool,
    draw: impl FnOnce([u8; 6], bool) -> Result<SaeCommit, SaeError>,
) -> ApSaeOutput {
    let mut output = ApSaeOutput::new();
    let peer = session.peer;
    let refresh = match session.phase {
        ApSaePhase::Nothing => {
            session.h2e = h2e;
            !allow_reuse
        }
        ApSaePhase::Committed => return ApSaeOutput::fault(session.fault()),
        ApSaePhase::Confirmed => {
            if session.sync > AP_SAE_SYNC {
                session.phase = ApSaePhase::Nothing;
                session.sync = 0;
                return output;
            }
            session.sync = match session.sync.checked_add(1) {
                Some(sync) => sync,
                None => return ApSaeOutput::fault(session.fault()),
            };
            true
        }
        ApSaePhase::Accepted => true,
    };
    if refresh || session.own.is_none() {
        let use_token = session.password_identifier || h2e;
        match draw(peer, use_token) {
            Ok(own) => session.own = Some(own),
            Err(SaeError::Crypto) => {
                return ApSaeOutput::fault(ApSaeFault::Crypto {
                    peer,
                    stage: ApSaeCryptoStage::DrawCommit,
                });
            }
            Err(_) => {
                return ApSaeOutput::failure(
                    peer,
                    SAE_COMMIT_TRANSACTION,
                    status_code::UNSPECIFIED_FAILURE,
                    ReplyBody::Empty,
                );
            }
        }
    }
    let Some(own) = session.own.as_ref() else {
        return ApSaeOutput::fault(session.fault());
    };
    let mut body = [0_u8; SAE_COMMIT_LEN];
    let len = match own.values().encode(None, session.h2e, &mut body) {
        Ok(len) => len,
        Err(error) => {
            return ApSaeOutput::fault(ApSaeFault::Crypto {
                peer,
                stage: ApSaeCryptoStage::Encode(error),
            });
        }
    };
    if len != SAE_COMMIT_LEN {
        return ApSaeOutput::fault(ApSaeFault::CommitLength { length: len });
    }
    let commit_status = if session.h2e {
        status_code::SAE_HASH_TO_ELEMENT
    } else {
        status_code::SUCCESS
    };
    let commit_reply = ApSaeReply::new(
        peer,
        SAE_COMMIT_TRANSACTION,
        commit_status,
        ReplyBody::Commit(body),
    );
    output.replies = [Some(commit_reply), None];
    let continuing = session.phase;
    if continuing != ApSaePhase::Confirmed {
        session.phase = ApSaePhase::Committed;
    }
    match own.process(peer_commit) {
        Ok(keys) => {
            session.keys = Some(keys);
            session.peer_commit = Some(peer_commit);
        }
        Err(_) => {
            return ApSaeOutput {
                replies: [
                    Some(commit_reply),
                    Some(ApSaeReply::new(
                        peer,
                        SAE_COMMIT_TRANSACTION,
                        status_code::UNSPECIFIED_FAILURE,
                        ReplyBody::Empty,
                    )),
                ],
                result: ApSaeResult::Failed {
                    status: status_code::UNSPECIFIED_FAILURE,
                },
            };
        }
    }
    if continuing == ApSaePhase::Confirmed {
        if let Err(fault) = store_confirm(session) {
            output.result = ApSaeResult::Fault(fault);
        }
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
        match SaeCommit::new(&pwe, rand, mask) {
            Err(SaeError::UnsuitableRandom) => {}
            commit => return commit,
        }
    }
    Err(SaeError::UnsuitableRandom)
}

/// `sae_write_confirm`: advance send-confirm and keep the Confirm for the
/// reply the next handled Confirm sends.
fn store_confirm(session: &mut Session) -> Result<(), ApSaeFault> {
    let keys = session.keys.as_ref().ok_or_else(|| session.fault())?;
    session.send_confirm = session.send_confirm.saturating_add(1);
    session.stored_confirm = Some(keys.own_confirm(session.send_confirm));
    Ok(())
}

/// `sae_accept_sta`.
fn accept(session: &mut Session) -> Result<ApSaeResult, ApSaeFault> {
    let keys = session.keys.as_ref().ok_or_else(|| session.fault())?;
    session.send_confirm = u16::MAX;
    session.accepted_scalar = session.peer_commit.map(|commit| commit.scalar);
    session.phase = ApSaePhase::Accepted;
    Ok(ApSaeResult::Accepted {
        pmk: *keys.pmk(),
        pmkid: *keys.pmkid(),
    })
}

#[cfg(test)]
mod tests;
