//! The access point's side of SAE cryptography: the validity of a station's
//! Commit and the anti-clogging tokens an access point issues and checks.
//!
//! This file is outside the build: no `mod` names it. Its one consumer, the
//! access point's SAE responder (`authenticator/sae.rs`), is outside the
//! build too, until the access point's role serves SAE. It is written as a
//! part of `crate::sae` (its items and tests use that module's names) and
//! returns there with the responder. The fuzz harness covered
//! `SaeComebackTokens::check`, `SaeReceivedCommit::parse` and
//! `valid_peer_commit` (`fuzz/tests.rs`, `sae_parsers`); that coverage
//! returns with them.

use super::*;

/// Whether a station's commit values are valid for an access point to
/// process: a scalar with 1 < scalar < r and an element on the curve, as the
/// vendor's `sae_parse_commit` requires.
pub fn valid_peer_commit(values: &SaeCommitValues) -> bool {
    let Some(scalar) = scalar_from(&values.scalar) else {
        return false;
    };
    !bool::from(scalar.is_zero() | scalar.ct_eq(&Scalar::ONE))
        && peer_element(&values.element).is_some()
}

/// Seconds an access point keeps one comeback-token key.
const COMEBACK_KEY_LIFETIME: Duration = Duration::from_secs(60);

/// The anti-clogging tokens an access point issues and checks.
///
/// A token is a big-endian index followed by the tail of
/// HMAC-SHA256(key, station address || index); a table indexed by the first
/// octet of HMAC-SHA256(key, station address) holds each station's pending
/// index, and a checked token invalidates it. The key is redrawn when a token
/// is issued more than 60 seconds after the last draw. The vendor passes its
/// index counter by value, so its counter never advances and every newly
/// pending index is 1.
///
/// SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
/// `components/wpa_supplicant/src/ap/comeback_token.c`
/// (`auth_build_token_req`, `check_comeback_token`, `comeback_token_hash`).
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SaeComebackTokens {
    key: [u8; 32],
    #[zeroize(skip)]
    key_drawn: Option<Instant>,
    pending: [u16; 256],
}

impl SaeComebackTokens {
    pub const fn new() -> Self {
        Self {
            key: [0; 32],
            key_drawn: None,
            pending: [0; 256],
        }
    }

    /// The token requested from `station`, drawing a fresh key with
    /// `draw_key` when the current one is missing or older than 60 seconds.
    pub fn issue(
        &mut self,
        station: [u8; 6],
        now: Instant,
        draw_key: impl FnOnce() -> [u8; 32],
    ) -> [u8; SAE_ANTI_CLOGGING_TOKEN_LEN] {
        let expired = self
            .key_drawn
            .is_none_or(|drawn| now.saturating_duration_since(drawn) > COMEBACK_KEY_LIFETIME);
        if expired {
            self.key = draw_key();
            self.key_drawn = Some(now);
            self.pending = [0; 256];
        }
        let (slot, pending) = self.pending_index(station);
        let pending = u16::conditional_select(&pending, &1, pending.ct_eq(&0));
        self.set_pending(slot, pending);
        let index = TokenIndex::of(pending).to_bytes();
        let mut token = hmac_sha256(&self.key, &[&station, &index]);
        for (target, source) in token.iter_mut().zip(index) {
            *target = source;
        }
        token
    }

    /// `station`'s pending index, in the slot of the first octet of
    /// HMAC-SHA256(key, station address).
    fn pending_index(&self, station: [u8; 6]) -> (u8, u16) {
        let [first, ..] = hmac_sha256(&self.key, &[&station]);
        let pending = self
            .pending
            .iter()
            .zip(0_u8..=u8::MAX)
            .fold(0, |value, (pending, slot)| {
                u16::conditional_select(&value, pending, slot.ct_eq(&first))
            });
        (first, pending)
    }

    fn set_pending(&mut self, selected: u8, value: u16) {
        for (pending, slot) in self.pending.iter_mut().zip(0_u8..=u8::MAX) {
            pending.conditional_assign(&value, slot.ct_eq(&selected));
        }
    }

    /// Whether `token` is the one pending for `station`; a valid token is
    /// consumed.
    pub fn check(&mut self, station: [u8; 6], token: &[u8]) -> bool {
        let Some((index_bytes, tail)) = token.split_first_chunk::<2>() else {
            return false;
        };
        if token.len() != SAE_ANTI_CLOGGING_TOKEN_LEN {
            return false;
        }
        let mac = hmac_sha256(&self.key, &[&station, index_bytes]);
        let [_, _, mac_tail @ ..] = &mac;
        let (slot, pending) = self.pending_index(station);
        if pending == 0 || TokenIndex::from_bytes(*index_bytes).value() != pending {
            return false;
        }
        if !bool::from(mac_tail.ct_eq(tail)) {
            return false;
        }
        self.set_pending(slot, 0);
        true
    }
}

impl Default for SaeComebackTokens {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for SaeComebackTokens {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SaeComebackTokens(<redacted>)")
    }
}

oer_portable_wire::bitfield! {
    /// An anti-clogging token's first two octets: its pending index,
    /// big-endian, as this crate's tokens carry it.
    struct TokenIndex[2] (big) {
        value, with_value: {u16} = 0..16;
    }
}

impl TokenIndex {
    const fn of(value: u16) -> Self {
        Self::from_bytes([0; 2]).with_value(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_access_point_refuses_commit_values_off_the_scalar_range_or_curve() {
        let peer = SaeCommitValues::parse(
            &hex::<SAE_COMMIT_LEN>(PEER_COMMIT),
            oer_protocols_ieee80211_wire::security::SaePwe::HuntingAndPecking,
        )
        .unwrap();
        assert!(valid_peer_commit(&peer));
        let mut zero_scalar = peer;
        zero_scalar.scalar.fill(0);
        assert!(!valid_peer_commit(&zero_scalar));
        let mut one_scalar = peer;
        one_scalar.scalar = [0; 32];
        one_scalar.scalar[31] = 1;
        assert!(!valid_peer_commit(&one_scalar));
        let mut off_curve = peer;
        off_curve.element[63] ^= 1;
        assert!(!valid_peer_commit(&off_curve));
    }

    #[test]
    fn a_comeback_token_is_bound_to_its_station_and_used_once() {
        let station = [2, 0, 0, 0, 0, 7];
        let mut tokens = SaeComebackTokens::new();
        let token = tokens.issue(
            station,
            oer_portable_time::Instant::from_micros(1_000),
            || [0x42; 32],
        );
        assert_eq!(token[..2], [0, 1]);
        assert_eq!(
            tokens.issue(
                station,
                oer_portable_time::Instant::from_micros(2_000),
                || unreachable!()
            ),
            token
        );
        assert!(!tokens.check([2, 0, 0, 0, 0, 8], &token));
        let mut forged = token;
        forged[31] ^= 1;
        assert!(!tokens.check(station, &forged));
        assert!(tokens.check(station, &token));
        assert!(!tokens.check(station, &token));

        let reissued = tokens.issue(
            station,
            oer_portable_time::Instant::from_micros(70_000_000),
            || [0x43; 32],
        );
        assert_ne!(reissued, token);
        assert!(tokens.check(station, &reissued));
    }

    #[test]
    fn every_comeback_slot_is_updated_without_affecting_another_slot() {
        let mut tokens = SaeComebackTokens::new();
        for slot in 0_u8..=u8::MAX {
            tokens.set_pending(slot, u16::from(slot) + 1);
        }
        for (pending, slot) in tokens.pending.iter().zip(0_u8..=u8::MAX) {
            assert_eq!(*pending, u16::from(slot) + 1);
        }
        tokens.set_pending(0, 0);
        tokens.set_pending(255, 0);
        assert_eq!(tokens.pending[0], 0);
        assert_eq!(tokens.pending[255], 0);
        assert_eq!(tokens.pending[1], 2);
        assert_eq!(tokens.pending[254], 255);
    }

    /// A token naming another index than the station's pending one is refused,
    /// even with a valid MAC for that index.
    #[test]
    fn a_comeback_token_of_another_index_is_refused() {
        let station = [2, 0, 0, 0, 0, 7];
        let key = [0x42; 32];
        let mut tokens = SaeComebackTokens::new();
        let token = tokens.issue(
            station,
            oer_portable_time::Instant::from_micros(1_000),
            || key,
        );
        assert_eq!(token[..2], [0, 1]);
        let other_index = [0, 2];
        let mac = hmac_sha256(&key, &[&station, &other_index]);
        let mut other = token;
        other[..2].copy_from_slice(&other_index);
        other[2..].copy_from_slice(&mac[2..]);
        assert!(!tokens.check(station, &other));
        assert!(tokens.check(station, &token));
    }

    #[test]
    fn comeback_tokens_wipe_and_redact_debug() {
        fn wipes_on_drop<T: ZeroizeOnDrop>() {}
        wipes_on_drop::<SaeComebackTokens>();
        let mut comeback = SaeComebackTokens::new();
        comeback.issue(ADDR1, Instant::EPOCH, || [0x42; 32]);
        assert_eq!(
            std::format!("{comeback:?}"),
            "SaeComebackTokens(<redacted>)"
        );
        comeback.zeroize();
        assert_eq!(comeback.key, [0; 32]);
    }
}
