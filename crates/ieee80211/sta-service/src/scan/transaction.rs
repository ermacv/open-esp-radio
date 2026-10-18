//! The channel-visit transaction of one scan over a [`StaScanPort`].
//!
//! [`StaScanBackend`] implements the candidate-scan port with one fixed
//! order per channel: switch, start receiving, the optional active probe,
//! the bounded dwell, stop receiving and prepare the next channel. Cold scan
//! and running rescan owners of any backend share it; a failed edge still
//! closes a live receive epoch before it is reported.

use core::marker::PhantomData;

use oer_ieee80211_sta::scan::{
    StaCandidateScanBackend, StaScanChannelContext, StaScanConfig, StaScanError, StaScanPort,
    StaScanSelectionOutcome, StaScanStepOutcome,
};

/// The channel-visit transaction of the [`StaCandidateScanService`](super::StaCandidateScanService)
/// over a [`StaScanPort`].
///
/// The owner type remains explicit so the compiler cannot conflate a cold
/// radio owner with a later running-rescan owner. Both nevertheless reuse this
/// exact ordering and failure cleanup.
pub struct StaScanBackend<O> {
    config: StaScanConfig,
    _owner: PhantomData<fn() -> O>,
}

impl<O> StaScanBackend<O> {
    pub const fn new(config: StaScanConfig) -> Self {
        Self {
            config,
            _owner: PhantomData,
        }
    }

    pub const fn config(&self) -> StaScanConfig {
        self.config
    }
}

impl<O> StaCandidateScanBackend for StaScanBackend<O>
where
    O: StaScanPort,
{
    type Owner = O;
    type Channel = O::Channel;
    type Candidate = O::Candidate;
    type Error = StaScanError<O::Error>;

    async fn begin_scan(
        &mut self,
        mut owner: Self::Owner,
    ) -> StaScanStepOutcome<Self::Owner, Self::Error> {
        match owner.begin_scan().await {
            Ok(()) => StaScanStepOutcome::Completed { owner },
            Err(error) => StaScanStepOutcome::Failed {
                owner,
                error: StaScanError::Begin(error),
            },
        }
    }

    async fn scan_channel(
        &mut self,
        mut owner: Self::Owner,
        context: StaScanChannelContext<Self::Channel>,
    ) -> StaScanStepOutcome<Self::Owner, Self::Error> {
        let dwell_ticks = match owner
            .switch_channel(context, self.config.dwell_ticks())
            .await
        {
            Ok(dwell_ticks) => dwell_ticks,
            Err(error) => {
                return StaScanStepOutcome::Failed {
                    owner,
                    error: StaScanError::ChannelSwitch(error),
                };
            }
        };
        if let Err(error) = owner.start_receive(context).await {
            return StaScanStepOutcome::Failed {
                owner,
                error: StaScanError::ReceiveStart(error),
            };
        }

        let mut transaction_failure = match owner.transmit_active_probe(context).await {
            Ok(_probe) => None,
            Err(error) => Some(StaScanError::ActiveProbe(error)),
        };
        if transaction_failure.is_none() {
            for _ in 0..dwell_ticks {
                if let Err(error) = owner.observe_receive(context) {
                    transaction_failure = Some(StaScanError::ReceiveObserve(error));
                    break;
                }
                if let Err(error) = owner.wait_dwell_tick().await {
                    transaction_failure = Some(StaScanError::DwellWait(error));
                    break;
                }
            }
        }

        // Always try to close a live RX epoch after dwell began. A stop
        // failure takes precedence because descriptor ownership is then
        // uncertain and no ring mutation or retry is safe.
        if let Err(error) = owner.stop_receive(context).await {
            return StaScanStepOutcome::Failed {
                owner,
                error: StaScanError::ReceiveStop(error),
            };
        }
        if let Some(error) = transaction_failure {
            return StaScanStepOutcome::Failed { owner, error };
        }

        if !context.is_last()
            && let Err(error) = owner.prepare_next_ring(context)
        {
            return StaScanStepOutcome::Failed {
                owner,
                error: StaScanError::PrepareNextRing(error),
            };
        }
        StaScanStepOutcome::Completed { owner }
    }

    fn select_candidate(
        &mut self,
        mut owner: Self::Owner,
    ) -> StaScanSelectionOutcome<Self::Owner, Self::Candidate, Self::Error> {
        match owner.select_candidate() {
            Ok(Some(candidate)) => StaScanSelectionOutcome::Selected { owner, candidate },
            Ok(None) => StaScanSelectionOutcome::NoCandidate { owner },
            Err(error) => StaScanSelectionOutcome::Failed {
                owner,
                error: StaScanError::CandidateSelection(error),
            },
        }
    }
}

#[cfg(test)]
mod tests;
