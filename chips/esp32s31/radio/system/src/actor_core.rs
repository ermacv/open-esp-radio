//! Client-neutral RF epoch, health and membership custody.
use super::*;
#[derive(Debug)]
struct State {
    poisoned: bool,
    has_epoch: bool,
    members: u32,
    tracked: u32,
    epoch: RfEpoch,
}

pub(super) struct ActorCore {
    pub(super) mint: MemberMint,
    state: State,
    #[cfg(feature = "bluetooth")]
    pub(super) attached: bool,
}
pub(super) enum Action {
    Continue,
    CloseRf,
}
impl ActorCore {
    #[inline(always)]
    pub(super) fn new(mint: MemberMint) -> Self {
        let epoch = mint.next_epoch(None);
        Self {
            mint,
            state: State {
                poisoned: false,
                has_epoch: false,
                members: 0,
                tracked: 0,
                epoch,
            },
            #[cfg(feature = "bluetooth")]
            attached: false,
        }
    }
    #[inline(always)]
    pub(super) fn is_poisoned(&self) -> bool {
        self.state.poisoned
    }
    #[inline(always)]
    pub(super) fn tracking_active(&self) -> bool {
        !self.state.poisoned && self.state.tracked > 0
    }
    #[inline(always)]
    pub(super) fn poison(&mut self) {
        self.state.poisoned = true;
    }
    #[inline(always)]
    pub(super) fn joined(&mut self) {
        self.state.members += 1;
    }
    #[inline(always)]
    pub(super) fn tracked(&mut self) {
        self.state.tracked += 1;
    }
    #[inline(always)]
    pub(super) fn failed(&mut self, cause: Failure) {
        if cause == Failure::Poisoned {
            self.poison();
        }
    }
    #[inline(always)]
    pub(super) fn failure(&self, cause: Failure) -> Failure {
        if self.state.poisoned {
            Failure::Poisoned
        } else {
            cause
        }
    }
    #[inline(always)]
    pub(super) fn health(&self) -> DomainHealth {
        if self.state.poisoned {
            DomainHealth::Poisoned
        } else {
            DomainHealth::Healthy
        }
    }
    #[inline(always)]
    pub(super) fn failed_enter(&mut self, cause: Failure) -> Action {
        // Both joins admit and prepare from a healthy core before entering;
        // no other actor operation runs during that entered transaction.
        if cause == Failure::Poisoned {
            self.poison();
            Action::Continue
        } else if self.state.members == 0 {
            Action::CloseRf
        } else {
            Action::Continue
        }
    }
    #[inline(always)]
    pub(super) fn leaving(&mut self, tracked: bool) {
        if tracked {
            self.state.tracked = self.state.tracked.saturating_sub(1);
        }
    }
    #[inline(always)]
    pub(super) fn exited(&mut self, exit: Exit) -> Action {
        self.state.members = self.state.members.saturating_sub(1);
        if exit.health == DomainHealth::Poisoned {
            self.poison();
            Action::Continue
        } else if exit.last && !self.state.poisoned {
            Action::CloseRf
        } else {
            Action::Continue
        }
    }
    #[inline(always)]
    pub(super) fn prepared(&mut self, phy: &PhyPreparation) -> RfEpoch {
        if *phy != PhyPreparation::AlreadyOpen && self.state.has_epoch {
            self.state.epoch = self.mint.next_epoch(Some(self.state.epoch));
        }
        self.state.has_epoch = true;
        self.state.epoch
    }
    #[inline(always)]
    pub(super) fn closed(&mut self, result: Result<(), CloseFault>) {
        if matches!(result, Err(CloseFault::Poisoned)) {
            self.state.poisoned = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_open_epoch_is_zero_and_only_a_later_reopen_advances_it() {
        let (end, _) = oer_esp32s31_radio_client::claim_for_host_test();
        let mut core = ActorCore::new(end.mint);
        let zero = core.prepared(&PhyPreparation::AlreadyOpen);
        assert_eq!(core.prepared(&PhyPreparation::AlreadyOpen), zero);
        let next = core.prepared(&PhyPreparation::Woke { micros: 0 });
        assert_ne!(next, zero);
        assert_eq!(core.prepared(&PhyPreparation::AlreadyOpen), next);
        let (end, _) = oer_esp32s31_radio_client::claim_for_host_test();
        let mut fresh = ActorCore::new(end.mint);
        assert_eq!(fresh.prepared(&PhyPreparation::Woke { micros: 0 }), zero);
    }
}
