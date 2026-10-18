//! Connection-local application authorization, independent of radio failure policy.
use trouble_host::{BondInformation, Identity, prelude::SecurityLevel};

pub(super) fn valid_bond(bond: &BondInformation) -> bool {
    bond.is_bonded && bond.security_level == SecurityLevel::EncryptedAuthenticated
}

#[cfg(test)]
mod tests {
    use super::*;
    use trouble_host::{Address, LongTermKey};
    fn bond(key: u128) -> BondInformation {
        BondInformation::new(
            Identity::from(Address::random([1, 2, 3, 4, 5, 0xc6])),
            LongTermKey::new(key),
            SecurityLevel::EncryptedAuthenticated,
            true,
        )
    }
    #[test]
    fn pairing_requires_user_acceptance_and_store_commit_before_att() {
        let bond = bond(1);
        let level = SecurityLevel::EncryptedAuthenticated;
        let mut state = Trust::new(None, true);
        assert!(!state.admit_pairing(level, &bond, &bond.identity));
        assert!(state.confirm());
        assert!(state.admit_pairing(level, &bond, &bond.identity));
        assert!(!state.authorized(level));
        state.saved();
        assert!(state.authorized(level));
        assert!(!state.authorized(SecurityLevel::Encrypted));
        assert!(!state.authorized(SecurityLevel::NoEncryption));
        assert!(!state.confirm());
    }
    #[test]
    fn resumed_key_must_match_stored_record_and_cannot_repair_silently() {
        let old = bond(1);
        let mut state = Trust::new(Some(old.clone()), false);
        assert!(!state.can_enroll());
        assert!(!state.confirm());
        assert!(!state.resume(SecurityLevel::EncryptedAuthenticated, Some(&bond(2))));
        assert!(!state.resume(SecurityLevel::Encrypted, Some(&old)));
        assert!(state.resume(SecurityLevel::EncryptedAuthenticated, Some(&old)));
        assert!(state.authorized(SecurityLevel::EncryptedAuthenticated));
        state.reject();
        assert!(!state.resume(SecurityLevel::EncryptedAuthenticated, Some(&old)));
    }
    #[test]
    fn full_store_and_rejection_never_allow_new_trust() {
        let mut state = Trust::new(None, false);
        assert!(state.rejected());
        assert!(!state.confirm());
        let mut enrolling = Trust::new(None, true);
        enrolling.reject();
        assert!(!enrolling.confirm());
        assert!(!enrolling.admit_pairing(
            SecurityLevel::EncryptedAuthenticated,
            &bond(1),
            &bond(1).identity
        ));
    }
}

pub(super) struct Trust {
    known: Option<BondInformation>,
    phase: Phase,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Enrolling,
    Confirmed,
    Restoring,
    Authorized,
    Rejected,
}

impl Trust {
    pub(super) fn new(known: Option<BondInformation>, free: bool) -> Self {
        let phase = if known.is_some() {
            Phase::Restoring
        } else if free {
            Phase::Enrolling
        } else {
            Phase::Rejected
        };
        Self { known, phase }
    }
    pub(super) fn can_enroll(&self) -> bool {
        self.phase == Phase::Enrolling
    }
    pub(super) fn confirm(&mut self) -> bool {
        if !self.can_enroll() {
            return false;
        }
        self.phase = Phase::Confirmed;
        true
    }
    pub(super) fn admit_pairing(
        &self,
        level: SecurityLevel,
        bond: &BondInformation,
        peer: &Identity,
    ) -> bool {
        self.phase == Phase::Confirmed
            && level == SecurityLevel::EncryptedAuthenticated
            && valid_bond(bond)
            && bond.identity.match_identity(peer)
    }
    pub(super) fn saved(&mut self) {
        self.phase = Phase::Authorized;
    }
    pub(super) fn expects_resume(&self) -> bool {
        self.phase == Phase::Restoring
    }
    pub(super) fn resume(&mut self, level: SecurityLevel, bond: Option<&BondInformation>) -> bool {
        if !self.expects_resume()
            || level != SecurityLevel::EncryptedAuthenticated
            || !bond.is_some_and(|b| valid_bond(b) && self.known.as_ref() == Some(b))
        {
            return false;
        }
        self.phase = Phase::Authorized;
        true
    }
    pub(super) fn authorized(&self, level: SecurityLevel) -> bool {
        self.phase == Phase::Authorized && level == SecurityLevel::EncryptedAuthenticated
    }
    pub(super) fn reject(&mut self) {
        self.phase = Phase::Rejected;
    }
    pub(super) fn rejected(&self) -> bool {
        self.phase == Phase::Rejected
    }
}
