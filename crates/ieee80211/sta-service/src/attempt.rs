//! The finite station attempt transaction.
//!
//! [`StaAttempt`] runs every pre-connected phase of one attempt over a
//! [`StaAttemptPort`] exactly once and in order, and returns either the
//! connected frontier or the exact owner with the stage that failed. The
//! values and the port are `oer_ieee80211_sta::attempt`.

use oer_ieee80211_sta::attempt::{
    AssociationAttemptFailure, AssociationAttemptOutcome, StaAttemptObserver, StaAttemptPort,
    StaAttemptProgress, StaAttemptStage, StaAttemptStepError,
};

/// Shared production transaction for initial join and reconnect.
pub struct StaAttempt<P, O = ()> {
    port: P,
    observer: O,
}

impl<P> StaAttempt<P, ()> {
    pub const fn new(port: P) -> Self {
        Self { port, observer: () }
    }
}

impl<P, O> StaAttempt<P, O> {
    pub const fn with_observer(port: P, observer: O) -> Self {
        Self { port, observer }
    }

    pub fn port(&self) -> &P {
        &self.port
    }

    pub fn port_mut(&mut self) -> &mut P {
        &mut self.port
    }

    pub fn into_parts(self) -> (P, O) {
        (self.port, self.observer)
    }
}

impl<P, O> StaAttempt<P, O>
where
    P: StaAttemptPort,
    O: StaAttemptObserver,
{
    /// Execute every finite pre-connected phase exactly once and in order.
    pub async fn run(
        &mut self,
        mut owner: P::Owner,
    ) -> AssociationAttemptOutcome<P::Owner, P::Connected, P::Error> {
        let mut progress = StaAttemptProgress::default();

        self.observer.stage_started(StaAttemptStage::Candidate);
        if let Err(failure) = self.port.prepare_candidate(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Candidate, failure, progress);
        }
        self.completed(StaAttemptStage::Candidate, &mut progress);

        self.observer.stage_started(StaAttemptStage::Channel);
        if let Err(failure) = self.port.select_channel(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Channel, failure, progress);
        }
        self.completed(StaAttemptStage::Channel, &mut progress);

        self.observer.stage_started(StaAttemptStage::Authentication);
        if let Err(failure) = self.port.authenticate(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Authentication, failure, progress);
        }
        self.completed(StaAttemptStage::Authentication, &mut progress);

        self.observer.stage_started(StaAttemptStage::Association);
        if let Err(failure) = self.port.associate(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Association, failure, progress);
        }
        self.completed(StaAttemptStage::Association, &mut progress);

        self.observer
            .stage_started(StaAttemptStage::PeerProgramming);
        if let Err(failure) = self.port.program_peer(&mut owner).await {
            return self.failed(owner, StaAttemptStage::PeerProgramming, failure, progress);
        }
        self.completed(StaAttemptStage::PeerProgramming, &mut progress);

        self.observer.stage_started(StaAttemptStage::Wpa2Handshake);
        if let Err(failure) = self.port.run_wpa2_handshake(&mut owner).await {
            return self.failed(owner, StaAttemptStage::Wpa2Handshake, failure, progress);
        }
        self.completed(StaAttemptStage::Wpa2Handshake, &mut progress);

        self.observer.stage_started(StaAttemptStage::RsnKeyInstall);
        if let Err(failure) = self.port.install_wpa2_keys(&mut owner).await {
            return self.failed(owner, StaAttemptStage::RsnKeyInstall, failure, progress);
        }
        self.completed(StaAttemptStage::RsnKeyInstall, &mut progress);

        self.observer.stage_started(StaAttemptStage::ConnectedEntry);
        match self.port.enter_connected(owner).await {
            Ok(connected) => {
                self.completed(StaAttemptStage::ConnectedEntry, &mut progress);
                AssociationAttemptOutcome::Connected {
                    connected,
                    progress,
                }
            }
            Err(failure) => {
                self.observer
                    .stage_failed(StaAttemptStage::ConnectedEntry, failure.disposition);
                AssociationAttemptOutcome::Failed(AssociationAttemptFailure {
                    owner: failure.owner,
                    stage: StaAttemptStage::ConnectedEntry,
                    disposition: failure.disposition,
                    error: failure.error,
                    progress,
                })
            }
        }
    }

    fn completed(&mut self, stage: StaAttemptStage, progress: &mut StaAttemptProgress) {
        progress.mark_completed(stage);
        self.observer.stage_completed(stage);
    }

    fn failed(
        &mut self,
        owner: P::Owner,
        stage: StaAttemptStage,
        failure: StaAttemptStepError<P::Error>,
        progress: StaAttemptProgress,
    ) -> AssociationAttemptOutcome<P::Owner, P::Connected, P::Error> {
        self.observer.stage_failed(stage, failure.disposition);
        AssociationAttemptOutcome::Failed(AssociationAttemptFailure {
            owner,
            stage,
            disposition: failure.disposition,
            error: failure.error,
            progress,
        })
    }
}

#[cfg(test)]
mod tests;
