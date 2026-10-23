//! Client-neutral RF epoch, health and membership custody.
use super::*;
#[derive(Debug, Default)]
struct State {
    poisoned: bool,
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
    pub(super) fn new(mint: MemberMint) -> Self {
        Self {
            mint,
            state: State::default(),
            #[cfg(feature = "bluetooth")]
            attached: false,
        }
    }
    pub(super) fn is_poisoned(&self) -> bool {
        self.state.poisoned
    }
    pub(super) fn tracking_active(&self) -> bool {
        !self.state.poisoned && self.state.tracked > 0
    }
    pub(super) fn poison(&mut self) {
        self.state.poisoned = true;
    }
    pub(super) fn joined(&mut self) {
        self.state.members += 1;
    }
    pub(super) fn tracked(&mut self) {
        self.state.tracked += 1;
    }
    pub(super) fn failed(&mut self, cause: Failure) {
        if cause == Failure::Poisoned {
            self.poison();
        }
    }
    pub(super) fn failure(&self, cause: Failure) -> Failure {
        if self.state.poisoned {
            Failure::Poisoned
        } else {
            cause
        }
    }
    pub(super) fn health(&self) -> DomainHealth {
        if self.state.poisoned {
            DomainHealth::Poisoned
        } else {
            DomainHealth::Healthy
        }
    }
    pub(super) fn failed_enter(&mut self, cause: Failure) -> Action {
        self.failed(cause);
        if !self.state.poisoned && self.state.members == 0 {
            Action::CloseRf
        } else {
            Action::Continue
        }
    }
    pub(super) fn leaving(&mut self, tracked: bool) {
        if tracked {
            self.state.tracked = self.state.tracked.saturating_sub(1);
        }
    }
    pub(super) fn exited(&mut self, exit: Exit) -> Action {
        self.state.members = self.state.members.saturating_sub(1);
        if exit.health == DomainHealth::Poisoned {
            self.poison();
        }
        if exit.last && !self.state.poisoned {
            Action::CloseRf
        } else {
            Action::Continue
        }
    }
    pub(super) fn prepared(
        &mut self,
        result: Result<PhyPreparation, Fault>,
    ) -> Result<(PhyPreparation, RfEpoch), Failure> {
        let phy = match result {
            Ok(phy) => phy,
            Err(Fault) => {
                self.state.poisoned = true;
                return Err(Failure::Poisoned);
            }
        };
        if phy != PhyPreparation::AlreadyOpen {
            self.state.epoch = Some(self.mint.next_epoch(self.state.epoch));
        }
        let epoch = self
            .state
            .epoch
            .unwrap_or_else(|| self.mint.next_epoch(None));
        self.state.epoch = Some(epoch);
        Ok((phy, epoch))
    }
    pub(super) fn closed(&mut self, result: Result<(), CloseFault>) {
        if matches!(result, Err(CloseFault::Poisoned)) {
            self.state.poisoned = true;
        }
    }
}
