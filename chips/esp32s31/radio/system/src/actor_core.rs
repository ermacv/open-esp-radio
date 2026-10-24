//! Client-neutral RF epoch, health and membership custody.
use super::*;
#[derive(Debug, Default)]
struct State {
    /// The first failure that poisoned the domain.
    poisoned: Option<Poison>,
    members: u32,
    tracked: u32,
    epoch: Option<RfEpoch>,
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
        Self {
            mint,
            state: State::default(),
            #[cfg(feature = "bluetooth")]
            attached: false,
        }
    }
    #[inline(always)]
    pub(super) fn poisoned(&self) -> Option<Poison> {
        self.state.poisoned
    }
    #[inline(always)]
    pub(super) fn tracking_active(&self) -> bool {
        self.state.poisoned.is_none() && self.state.tracked > 0
    }
    /// Keep `poison` unless an earlier one is kept, and return the kept one.
    #[inline(always)]
    pub(super) fn poison(&mut self, poison: Poison) -> Poison {
        *self.state.poisoned.get_or_insert(poison)
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
        if let Failure::Poisoned(poison) = cause {
            self.poison(poison);
        }
    }
    /// The answer's cause: the kept poison once the domain is poisoned.
    #[inline(always)]
    pub(super) fn failure(&self, cause: Failure) -> Failure {
        self.state.poisoned.map_or(cause, Failure::Poisoned)
    }
    #[inline(always)]
    pub(super) fn health(&self) -> DomainHealth {
        self.state
            .poisoned
            .map_or(DomainHealth::Healthy, DomainHealth::Poisoned)
    }
    #[inline(always)]
    pub(super) fn failed_enter(&mut self, cause: Failure) -> Action {
        // Both joins admit and prepare from a healthy core before entering;
        // no other actor operation runs during that entered transaction.
        if let Failure::Poisoned(poison) = cause {
            self.poison(poison);
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
        if let DomainHealth::Poisoned(poison) = exit.health {
            self.poison(poison);
            Action::Continue
        } else if exit.last && self.state.poisoned.is_none() {
            Action::CloseRf
        } else {
            Action::Continue
        }
    }
    #[inline(always)]
    pub(super) fn prepared(&mut self, already_open: bool) -> RfEpoch {
        if !already_open {
            self.state.epoch = Some(self.mint.next_epoch(self.state.epoch));
        }
        let epoch = self
            .state
            .epoch
            .unwrap_or_else(|| self.mint.next_epoch(None));
        self.state.epoch = Some(epoch);
        epoch
    }
    #[inline(always)]
    pub(super) fn closed(&mut self, result: Result<(), CloseFault>) {
        if let Err(CloseFault::Poisoned(cause)) = result {
            self.poison(Poison {
                origin: PoisonOrigin::CloseRf,
                cause,
            });
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
        let zero = core.prepared(true);
        assert_eq!(core.prepared(true), zero);
        let next = core.prepared(false);
        assert_ne!(next, zero);
        assert_eq!(core.prepared(true), next);
        let (end, _) = oer_esp32s31_radio_client::claim_for_host_test();
        let mut fresh = ActorCore::new(end.mint);
        assert_eq!(fresh.prepared(false), zero);
    }
}
