//! Allocation-free candidate-scan orchestration.
//!
//! The scan values and the [`StaCandidateScanBackend`] port live in
//! `oer_ieee80211_sta::scan`; this module drives one finite channel plan.

use oer_ieee80211_sta::scan::{
    StaCandidateScanBackend, StaCandidateScanExit, StaScanChannelContext, StaScanPlanError,
    StaScanProgress, StaScanSelectionOutcome, StaScanStepOutcome,
};

/// Runs exactly one finite candidate scan.
///
/// Retry/backoff belongs to [`crate::station::StaLifecycleService`]; this
/// service deliberately performs no implicit repetition. Cold and running
/// hardware adapters may therefore use different owner variants while sharing
/// the same closed scan order and progress contract.
pub struct StaCandidateScanService<B> {
    backend: B,
}

impl<B> StaCandidateScanService<B>
where
    B: StaCandidateScanBackend,
{
    pub const fn new(backend: B) -> Self {
        Self { backend }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    pub async fn run(
        &mut self,
        mut owner: B::Owner,
        channels: &[B::Channel],
    ) -> StaCandidateScanExit<B::Owner, B::Candidate, B::Error> {
        let channels_planned = match u16::try_from(channels.len()) {
            Ok(0) => {
                return StaCandidateScanExit::InvalidPlan {
                    owner,
                    error: StaScanPlanError::Empty,
                    progress: StaScanProgress::default(),
                };
            }
            Ok(count) => count,
            Err(_) => {
                return StaCandidateScanExit::InvalidPlan {
                    owner,
                    error: StaScanPlanError::TooManyChannels,
                    progress: StaScanProgress::default(),
                };
            }
        };
        let mut progress = StaScanProgress {
            channels_planned,
            ..StaScanProgress::default()
        };

        owner = match self.backend.begin_scan(owner).await {
            StaScanStepOutcome::Completed { owner } => owner,
            StaScanStepOutcome::Stopped { owner } => {
                return StaCandidateScanExit::Stopped { owner, progress };
            }
            StaScanStepOutcome::Failed { owner, error } => {
                return StaCandidateScanExit::Failed {
                    owner,
                    error,
                    progress,
                };
            }
        };

        for (index, channel) in channels.iter().copied().enumerate() {
            let index = u16::try_from(index).expect("validated channel plan fits in u16");
            progress.channels_started = progress.channels_started.saturating_add(1);
            owner = match self
                .backend
                .scan_channel(
                    owner,
                    StaScanChannelContext {
                        channel,
                        index,
                        total_channels: channels_planned,
                    },
                )
                .await
            {
                StaScanStepOutcome::Completed { owner } => {
                    progress.channels_completed = progress.channels_completed.saturating_add(1);
                    owner
                }
                StaScanStepOutcome::Stopped { owner } => {
                    return StaCandidateScanExit::Stopped { owner, progress };
                }
                StaScanStepOutcome::Failed { owner, error } => {
                    return StaCandidateScanExit::Failed {
                        owner,
                        error,
                        progress,
                    };
                }
            };
        }

        match self.backend.select_candidate(owner) {
            StaScanSelectionOutcome::Selected { owner, candidate } => {
                StaCandidateScanExit::Selected {
                    owner,
                    candidate,
                    progress,
                }
            }
            StaScanSelectionOutcome::NoCandidate { owner } => {
                StaCandidateScanExit::NoCandidate { owner, progress }
            }
            StaScanSelectionOutcome::Stopped { owner } => {
                StaCandidateScanExit::Stopped { owner, progress }
            }
            StaScanSelectionOutcome::Failed { owner, error } => StaCandidateScanExit::Failed {
                owner,
                error,
                progress,
            },
        }
    }
}

pub mod transaction;

pub use transaction::StaScanBackend;

#[cfg(test)]
mod tests;
