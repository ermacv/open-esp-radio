//! SAE Authentication of a WPA3-Personal association.
//!
//! The station sends one SAE Commit and waits for the access point's Commit;
//! it then sends its Confirm and waits for the access point's Confirm, which
//! authenticates both peers and yields the PMK. An anti-clogging refusal
//! (status 76) makes the station repeat the same commit with the access
//! point's token. There are no retransmissions inside one exchange: the
//! vendor uses 4000 ms for the commit exchange and 2000 ms for confirmation.
//! This owner starts each checked deadline when it produces that phase's
//! transmission. Waiting for a frame credit or a TX terminal cannot extend it.
//!
//! SOURCE(esp32s31): complete pinned `libnet80211.a[ieee80211_sta.o]::sta_auth_sae`
//! and the SAE branch of `ieee80211_sta_new_state`; ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/wpa_supplicant/esp_supplicant/src/esp_wpa3.c`.

use oer_ieee80211_mac::address::{Bssid, OwnAddress};
use oer_ieee80211_mac::security::SaePwe;
use oer_ieee80211_mac::station::{
    SAE_COMMIT_TRANSACTION, SAE_CONFIRM_TRANSACTION, StaDisconnect, parse_sae_authentication,
    parse_sta_disconnect,
};
use oer_ieee80211_rsn::Pmk;
use oer_ieee80211_rsn::sae::{
    SAE_COMMIT_LEN, SAE_PMKID_LEN, SaeCommit, SaeCommitValues, SaeError, SaeKeys,
    anti_clogging_token,
};
use oer_time::{Duration, Instant};

pub const STA_SAE_COMMIT_TIMEOUT: Duration = Duration::from_secs(4);
pub const STA_SAE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);
pub const STA_SAE_TOKEN_CAPACITY: usize = 64;
pub const STA_SAE_BODY_CAPACITY: usize = SAE_COMMIT_LEN + 3 + STA_SAE_TOKEN_CAPACITY;

const STATUS_SUCCESS: u16 = 0;
const STATUS_ANTI_CLOGGING_TOKEN_REQUIRED: u16 = 76;
const STATUS_SAE_HASH_TO_ELEMENT: u16 = 126;

/// One owned Authentication body, retained until a TX credit is available.
#[derive(Debug, Eq, PartialEq)]
#[must_use]
pub struct StaSaeTransmission {
    pub transaction: u16,
    pub status_code: u16,
    body: heapless::Vec<u8, STA_SAE_BODY_CAPACITY>,
}

impl StaSaeTransmission {
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// Successful authentication transfers the zeroizing PMK to the association.
#[must_use]
pub struct StaSaePmk {
    pub pmk: Pmk,
    pub pmkid: [u8; SAE_PMKID_LEN],
}

impl core::fmt::Debug for StaSaePmk {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("StaSaePmk(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaSaeFailure {
    Timeout,
    DeadlineOverflow,
    PeerDisconnect(StaDisconnect),
    Rejected { status_code: u16 },
    Protocol(SaeError),
    TokenTooLong,
}

/// Only continuing results return an exchange. Terminal results consume it.
#[derive(Debug)]
#[must_use]
pub enum StaSaeEvent {
    Irrelevant(StaSaeAuthentication),
    Transmit {
        authentication: StaSaeAuthentication,
        transmission: StaSaeTransmission,
    },
    Authenticated(StaSaePmk),
    Failed(StaSaeFailure),
}

enum StaSaePhase {
    Committed { commit: SaeCommit, pwe: SaePwe },
    Confirmed(SaeKeys),
}

/// One live SAE exchange. Its checked deadline includes waiting for TX credit
/// and completion. Neither failure nor success leaves a usable exchange behind.
#[must_use]
pub struct StaSaeAuthentication {
    local: OwnAddress,
    bssid: Bssid,
    phase: StaSaePhase,
    deadline: Instant,
}

impl core::fmt::Debug for StaSaeAuthentication {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StaSaeAuthentication")
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl StaSaeAuthentication {
    /// Start the bounded commit exchange and transfer its first frame.
    pub fn new(
        local: OwnAddress,
        bssid: Bssid,
        commit: SaeCommit,
        pwe: SaePwe,
        now: Instant,
    ) -> Result<(Self, StaSaeTransmission), StaSaeFailure> {
        let deadline = now
            .checked_add(STA_SAE_COMMIT_TIMEOUT)
            .ok_or(StaSaeFailure::DeadlineOverflow)?;
        let transmission = encode_commit(&commit, pwe, None)?;
        Ok((
            Self {
                local,
                bssid,
                phase: StaSaePhase::Committed { commit, pwe },
                deadline,
            },
            transmission,
        ))
    }

    pub const fn next_deadline(&self) -> Instant {
        self.deadline
    }

    /// Consume one observation; a continuing result returns the same owner.
    pub fn observe_management_frame(self, frame: &[u8], now: Instant) -> StaSaeEvent {
        if now >= self.deadline {
            return StaSaeEvent::Failed(StaSaeFailure::Timeout);
        }
        if let Some(disconnect) = parse_sta_disconnect(frame, self.local, self.bssid) {
            return StaSaeEvent::Failed(StaSaeFailure::PeerDisconnect(disconnect));
        }
        let Some(frame) = parse_sae_authentication(frame, self.local.get(), self.bssid.get())
        else {
            return StaSaeEvent::Irrelevant(self);
        };
        match (frame.transaction, &self.phase) {
            (SAE_COMMIT_TRANSACTION, StaSaePhase::Committed { .. }) => {
                self.receive_commit(frame.status_code, frame.body, now)
            }
            (SAE_CONFIRM_TRANSACTION, StaSaePhase::Confirmed(_)) => {
                self.receive_confirm(frame.status_code, frame.body)
            }
            _ => StaSaeEvent::Irrelevant(self),
        }
    }

    fn receive_commit(mut self, status: u16, body: &[u8], now: Instant) -> StaSaeEvent {
        let StaSaePhase::Committed { commit, pwe } = &self.phase else {
            return StaSaeEvent::Irrelevant(self);
        };
        let h2e = matches!(pwe, SaePwe::HashToElement);
        if status == STATUS_ANTI_CLOGGING_TOKEN_REQUIRED {
            let token = match anti_clogging_token(body, h2e) {
                Ok(token) if token.len() <= STA_SAE_TOKEN_CAPACITY => token,
                Ok(_) => return StaSaeEvent::Failed(StaSaeFailure::TokenTooLong),
                Err(error) => return StaSaeEvent::Failed(StaSaeFailure::Protocol(error)),
            };
            return match encode_commit(commit, *pwe, Some(token)) {
                Ok(transmission) => StaSaeEvent::Transmit {
                    authentication: self,
                    transmission,
                },
                Err(error) => StaSaeEvent::Failed(error),
            };
        }
        if status != commit_status(*pwe) {
            return StaSaeEvent::Failed(
                if matches!(status, STATUS_SUCCESS | STATUS_SAE_HASH_TO_ELEMENT) {
                    StaSaeFailure::Protocol(SaeError::Malformed)
                } else {
                    StaSaeFailure::Rejected {
                        status_code: status,
                    }
                },
            );
        }
        let peer = match SaeCommitValues::parse(body, h2e) {
            Ok(peer) => peer,
            Err(error) => return StaSaeEvent::Failed(StaSaeFailure::Protocol(error)),
        };
        let keys = match commit.process(peer) {
            Ok(keys) => keys,
            Err(SaeError::Reflection) => return StaSaeEvent::Irrelevant(self),
            Err(error) => return StaSaeEvent::Failed(StaSaeFailure::Protocol(error)),
        };
        let body = match heapless::Vec::from_slice(&keys.own_confirm(1)) {
            Ok(body) => body,
            Err(_) => return StaSaeEvent::Failed(StaSaeFailure::Protocol(SaeError::Malformed)),
        };
        let Some(deadline) = now.checked_add(STA_SAE_CONFIRM_TIMEOUT) else {
            return StaSaeEvent::Failed(StaSaeFailure::DeadlineOverflow);
        };
        self.phase = StaSaePhase::Confirmed(keys);
        self.deadline = deadline;
        StaSaeEvent::Transmit {
            authentication: self,
            transmission: StaSaeTransmission {
                transaction: SAE_CONFIRM_TRANSACTION,
                status_code: STATUS_SUCCESS,
                body,
            },
        }
    }

    fn receive_confirm(self, status_code: u16, body: &[u8]) -> StaSaeEvent {
        if status_code != STATUS_SUCCESS {
            return StaSaeEvent::Failed(StaSaeFailure::Rejected { status_code });
        }
        match self.phase {
            StaSaePhase::Confirmed(keys) => match keys.verify_peer_confirm(body) {
                Ok(_) => StaSaeEvent::Authenticated(StaSaePmk {
                    pmk: Pmk::from_bytes(keys.pmk),
                    pmkid: keys.pmkid,
                }),
                Err(error) => StaSaeEvent::Failed(StaSaeFailure::Protocol(error)),
            },
            _ => StaSaeEvent::Irrelevant(self),
        }
    }

    pub fn on_deadline(self, now: Instant) -> StaSaeEvent {
        if now >= self.deadline {
            StaSaeEvent::Failed(StaSaeFailure::Timeout)
        } else {
            StaSaeEvent::Irrelevant(self)
        }
    }
}

const fn commit_status(pwe: SaePwe) -> u16 {
    match pwe {
        SaePwe::HashToElement => STATUS_SAE_HASH_TO_ELEMENT,
        SaePwe::HuntingAndPecking => STATUS_SUCCESS,
    }
}

fn encode_commit(
    commit: &SaeCommit,
    pwe: SaePwe,
    token: Option<&[u8]>,
) -> Result<StaSaeTransmission, StaSaeFailure> {
    let mut bytes = [0; STA_SAE_BODY_CAPACITY];
    let length = commit
        .values()
        .encode(token, matches!(pwe, SaePwe::HashToElement), &mut bytes)
        .map_err(StaSaeFailure::Protocol)?;
    let body = heapless::Vec::from_slice(
        bytes
            .split_at_checked(length)
            .ok_or(StaSaeFailure::Protocol(SaeError::Malformed))?
            .0,
    )
    .map_err(|_| StaSaeFailure::Protocol(SaeError::Malformed))?;
    Ok(StaSaeTransmission {
        transaction: SAE_COMMIT_TRANSACTION,
        status_code: commit_status(pwe),
        body,
    })
}

#[cfg(test)]
mod tests;
