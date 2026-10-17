//! Bounded ownership of authenticated frames awaiting the current TX terminal.
//!
//! The vendor checks the increasing replay counter in
//! [`wpa.c:2238`], sends each new group response in [`wpa.c:1847`], and handles
//! the group TX callback in [`wpa.c:3072–3076`]. It processes a new frame
//! immediately. Our D4 connection boundary is `Ended(Delivered)` of M4, so an
//! authenticated successor waits at most for the current M4/GM2 TX terminal.
//! Each kind has one affine owner; a newer authenticated frame replaces it.
//! M3 and GM1 share the vendor's `sm->rx_replay_counter`. When both kinds
//! await a terminal, the smaller counter is processed first. Advancing that
//! common frontier past a valid older frame would incorrectly discard it.
//!
//! In Connected there is no station rekey timer. The vendor's
//! [`group exchange`] processes GM1 immediately, its [`TX callback`] only
//! clears the group flag, and [`wpa_sm_cancel_auth_timeout`] is empty.
//! Waiting is bounded by the port's terminal contract: its watchdog must
//! eventually yield Ended, Withdrawn, or Indeterminate. Failed delivery does
//! not make the station disconnect; that decision belongs to the AP. Before
//! connection, the original D4 M1/M3 response deadline remains authoritative
//! and replacing a deferred M3 never extends it.
//!
//! [`wpa.c:2238`]: https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/wpa_supplicant/src/rsn_supp/wpa.c#L2238
//! [`wpa.c:1847`]: https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/wpa_supplicant/src/rsn_supp/wpa.c#L1847
//! [`wpa.c:3072–3076`]: https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/wpa_supplicant/src/rsn_supp/wpa.c#L3072-L3076
//! [`group exchange`]: https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/wpa_supplicant/src/rsn_supp/wpa.c#L1885-L1905
//! [`TX callback`]: https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/wpa_supplicant/src/rsn_supp/wpa.c#L3072-L3076
//! [`wpa_sm_cancel_auth_timeout`]: https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/wpa_supplicant/src/rsn_supp/wpa.c#L79-L82

use super::{DEFAULT_EAPOL_FRAME_CAPACITY, EapolProtection, RsnConnectedSupplicantError};
use oer_ieee80211_rsn::supplicant::RsnGroupKeyDataRequest;

type VerifiedFrame = RsnGroupKeyDataRequest<DEFAULT_EAPOL_FRAME_CAPACITY>;

/// The sole deferred, authenticated pairwise Message 3. It cannot grant PTK
/// installation authority, be copied, or be fabricated from unchecked bytes.
#[must_use = "process this frame at the TX terminal or return it in the retired resource bank"]
pub struct PendingM3 {
    pub(super) request: VerifiedFrame,
    pub(super) protection: EapolProtection,
}

/// The sole deferred, authenticated Group Message 1.
#[must_use = "process this frame at the TX terminal or return it in the retired resource bank"]
pub struct PendingGroupM1 {
    pub(super) request: VerifiedFrame,
}

impl PendingM3 {
    /// The replay counter authenticated with this frame's MIC.
    pub fn replay_counter(&self) -> u64 {
        self.request.replay_counter()
    }

    /// Return the original verified EAPOL frame with no installation effect.
    pub fn into_frame(self) -> oer_ieee80211_rsn::OwnedEapolFrame {
        self.request.into_frame()
    }
}

impl PendingGroupM1 {
    /// The replay counter authenticated with this frame's MIC.
    pub fn replay_counter(&self) -> u64 {
        self.request.replay_counter()
    }

    /// Return the original verified EAPOL frame with no installation effect.
    pub fn into_frame(self) -> oer_ieee80211_rsn::OwnedEapolFrame {
        self.request.into_frame()
    }
}

/// All possible occupants of the two distinct deferred-frame places. The
/// variants own the frames; there is no separate presence flag or optional
/// frame to get out of step with the exchange state.
#[expect(
    clippy::large_enum_variant,
    reason = "the two affine frame places are bounded and owned in place without allocation"
)]
pub enum DeferredEapol {
    Empty,
    Message3(PendingM3),
    GroupMessage1(PendingGroupM1),
    Both {
        message3: PendingM3,
        group: PendingGroupM1,
    },
}

impl DeferredEapol {
    pub(super) fn has_newer_message3(&self, counter: u64) -> bool {
        match self {
            Self::Message3(held) | Self::Both { message3: held, .. } => {
                held.replay_counter() >= counter
            }
            _ => false,
        }
    }

    pub(super) fn has_newer_group(&self, counter: u64) -> bool {
        match self {
            Self::GroupMessage1(held) | Self::Both { group: held, .. } => {
                held.replay_counter() >= counter
            }
            _ => false,
        }
    }

    pub(super) fn message3(
        self,
        incoming: PendingM3,
    ) -> (Self, Result<(), RsnConnectedSupplicantError>) {
        match self {
            Self::Message3(ref held)
            | Self::Both {
                message3: ref held, ..
            } if incoming.replay_counter() <= held.replay_counter() => {
                (self, Err(RsnConnectedSupplicantError::StaleReplayCounter))
            }
            Self::Empty | Self::Message3(_) => (Self::Message3(incoming), Ok(())),
            Self::GroupMessage1(group) | Self::Both { group, .. } => (
                Self::Both {
                    message3: incoming,
                    group,
                },
                Ok(()),
            ),
        }
    }

    pub(super) fn group(
        self,
        incoming: PendingGroupM1,
    ) -> (Self, Result<(), RsnConnectedSupplicantError>) {
        match self {
            Self::GroupMessage1(ref held)
            | Self::Both {
                group: ref held, ..
            } if incoming.replay_counter() <= held.replay_counter() => {
                (self, Err(RsnConnectedSupplicantError::StaleReplayCounter))
            }
            Self::Empty | Self::GroupMessage1(_) => (Self::GroupMessage1(incoming), Ok(())),
            Self::Message3(message3) | Self::Both { message3, .. } => (
                Self::Both {
                    message3,
                    group: incoming,
                },
                Ok(()),
            ),
        }
    }
}
