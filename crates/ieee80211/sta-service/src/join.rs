//! Executor-independent infrastructure-STA Authentication and Association
//! driver.
//!
//! The state machines, retry policy and the [`StaJoinBackend`] port live in
//! `oer_ieee80211_sta::join`. This module orders the finite hardware
//! transactions of that port against absolute deadlines, receiving time
//! through the `oer-time` [`Timer`] port.

use oer_time::{Duration, Instant, Timer};

use oer_ieee80211_mac::security::LinkProtection;
use oer_ieee80211_mac::station::StaSequenceCounter;
use oer_ieee80211_sta::join::{
    StaAssociationSuccess, StaAuthenticationSuccess, StaJoinBackend, StaJoinError,
    StaJoinRxDirective, StaJoinRxObserver,
    association::{
        StaAssociationEvent, StaAssociationPoll, StaAssociationRuntime, StaAssociationRuntimeError,
    },
    authentication::{
        StaAuthenticationEvent, StaAuthenticationRuntime, StaAuthenticationRuntimeError,
    },
    sae::{StaSaeAuthentication, StaSaeEvent, StaSaePmk},
};

struct AuthenticationObserver<'runtime> {
    runtime: &'runtime mut StaAuthenticationRuntime,
    terminal: Option<Result<StaAuthenticationEvent, StaAuthenticationRuntimeError>>,
}

impl StaJoinRxObserver for AuthenticationObserver<'_> {
    fn observe_completed(&mut self, management_frame: Option<&[u8]>) -> StaJoinRxDirective {
        if let Err(error) = self.runtime.observe_received_frame() {
            self.terminal = Some(Err(error));
            return StaJoinRxDirective::Stop;
        }
        let Some(frame) = management_frame else {
            return StaJoinRxDirective::Continue;
        };
        match self.runtime.observe_management_frame(frame) {
            Ok(StaAuthenticationEvent::Irrelevant) => StaJoinRxDirective::Continue,
            terminal => {
                self.terminal = Some(terminal);
                StaJoinRxDirective::Stop
            }
        }
    }
}

struct SaeObserver<'exchange> {
    exchange: &'exchange mut StaSaeAuthentication,
    now: Instant,
    event: Option<StaSaeEvent>,
}

impl StaJoinRxObserver for SaeObserver<'_> {
    fn observe_completed(&mut self, management_frame: Option<&[u8]>) -> StaJoinRxDirective {
        let Some(frame) = management_frame else {
            return StaJoinRxDirective::Continue;
        };
        match self.exchange.observe_management_frame(frame, self.now) {
            StaSaeEvent::Irrelevant => StaJoinRxDirective::Continue,
            event => {
                self.event = Some(event);
                StaJoinRxDirective::Stop
            }
        }
    }
}

struct AssociationObserver<'runtime> {
    runtime: &'runtime mut StaAssociationRuntime,
    now: Instant,
    terminal: Option<Result<StaAssociationEvent, StaAssociationRuntimeError>>,
}

impl StaJoinRxObserver for AssociationObserver<'_> {
    fn observe_completed(&mut self, management_frame: Option<&[u8]>) -> StaJoinRxDirective {
        if let Err(error) = self.runtime.observe_received_frame() {
            self.terminal = Some(Err(error));
            return StaJoinRxDirective::Stop;
        }
        let Some(frame) = management_frame else {
            return StaJoinRxDirective::Continue;
        };
        match self.runtime.observe_management_frame(frame, self.now) {
            Ok(StaAssociationEvent::Irrelevant) => StaJoinRxDirective::Continue,
            terminal => {
                self.terminal = Some(terminal);
                StaJoinRxDirective::Stop
            }
        }
    }
}

/// How often the runner services received frames while it waits. The join
/// backends poll RX; an event-driven wait replaces this cadence when the roles
/// run on the lower-MAC port (radio plan 8).
pub const RX_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Unique transaction runner for one pre-connected station exchange.
// CAPABILITY: authentication-association
pub struct StaJoinRunner<B, T> {
    backend: B,
    timer: T,
}

impl<B, T> StaJoinRunner<B, T>
where
    B: StaJoinBackend,
    T: Timer,
{
    pub const fn new(backend: B, timer: T) -> Self {
        Self { backend, timer }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn into_parts(self) -> (B, T) {
        (self.backend, self.timer)
    }

    async fn stop_receive(&mut self) -> Result<(), StaJoinError<B::Error>> {
        self.backend
            .stop_receive()
            .await
            .map_err(StaJoinError::Backend)
    }

    /// Wait for the next receive poll after `previous`, returning its time.
    async fn wait_next_poll(
        &mut self,
        previous: Instant,
    ) -> Result<Instant, StaJoinError<B::Error>> {
        let poll = previous
            .checked_add(RX_POLL_INTERVAL)
            .ok_or(StaJoinError::ClockOverflow)?;
        self.timer.wait_until(poll).await;
        Ok(poll)
    }

    /// Run one SAE Authentication exchange and return its PMK.
    ///
    /// RX is drained at every millisecond boundary; a frame the exchange
    /// answers (the Confirm after the peer's Commit, or the Commit repeated
    /// with an anti-clogging token) is sent before the next boundary.
    pub async fn authenticate_sae(
        &mut self,
        mut exchange: StaSaeAuthentication,
        sequence: &mut StaSequenceCounter,
    ) -> Result<StaSaePmk, StaJoinError<B::Error>> {
        self.backend
            .start_receive()
            .await
            .map_err(StaJoinError::Backend)?;
        let commit = exchange.commit();
        if let Err(error) = self
            .backend
            .transmit_sae_authentication(sequence.take(), &commit)
            .await
        {
            self.stop_receive().await?;
            return Err(StaJoinError::Backend(error));
        }
        let mut poll = self.timer.now();
        exchange.start(poll);
        loop {
            poll = self.wait_next_poll(poll).await?;
            let mut observer = SaeObserver {
                exchange: &mut exchange,
                now: poll,
                event: None,
            };
            if let Err(error) = self.backend.service_receive(&mut observer).await {
                self.stop_receive().await?;
                return Err(StaJoinError::Backend(error));
            }
            let event = match observer.event {
                Some(event) => event,
                None => exchange.on_deadline(poll),
            };
            match event {
                StaSaeEvent::Irrelevant => {}
                StaSaeEvent::Transmit(transmission) => {
                    if let Err(error) = self
                        .backend
                        .transmit_sae_authentication(sequence.take(), &transmission)
                        .await
                    {
                        self.stop_receive().await?;
                        return Err(StaJoinError::Backend(error));
                    }
                }
                StaSaeEvent::Authenticated(pmk) => {
                    self.stop_receive().await?;
                    return Ok(pmk);
                }
                StaSaeEvent::Failed(failure) => {
                    self.stop_receive().await?;
                    return Err(StaJoinError::SaeFailed(failure));
                }
            }
        }
    }

    /// Run bounded Open Authentication.
    ///
    /// The exact one-second deadline is measured from completion of each TX
    /// publication. RX is drained at every millisecond boundary, including
    /// the final boundary, before timeout is declared. This makes an RX event
    /// simultaneous with the deadline win deterministically.
    pub async fn authenticate(
        &mut self,
        local: [u8; 6],
        bssid: [u8; 6],
        sequence: &mut StaSequenceCounter,
    ) -> Result<StaAuthenticationSuccess, StaJoinError<B::Error>> {
        let mut runtime = StaAuthenticationRuntime::new(local, bssid);
        loop {
            let attempt = runtime
                .begin_attempt(sequence)
                .map_err(StaJoinError::AuthenticationRuntime)?;
            self.backend
                .start_receive()
                .await
                .map_err(StaJoinError::Backend)?;
            if let Err(error) = self.backend.transmit_open_authentication(attempt).await {
                self.stop_receive().await?;
                return Err(StaJoinError::Backend(error));
            }
            let mut poll = self.timer.now();
            let deadline = poll
                .checked_add(attempt.response_timeout)
                .ok_or(StaJoinError::ClockOverflow)?;
            let mut terminal = None;
            while poll < deadline {
                poll = self.wait_next_poll(poll).await?;
                let mut observer = AuthenticationObserver {
                    runtime: &mut runtime,
                    terminal: None,
                };
                if let Err(error) = self.backend.service_receive(&mut observer).await {
                    self.stop_receive().await?;
                    return Err(StaJoinError::Backend(error));
                }
                if observer.terminal.is_some() {
                    terminal = observer.terminal;
                    break;
                }
            }
            self.stop_receive().await?;
            let event = match terminal {
                Some(Ok(event)) => event,
                Some(Err(error)) => return Err(StaJoinError::AuthenticationRuntime(error)),
                None => runtime
                    .response_timed_out()
                    .map_err(StaJoinError::AuthenticationRuntime)?,
            };
            match event {
                StaAuthenticationEvent::Authenticated {
                    attempt,
                    total_received_frames,
                } => {
                    return Ok(StaAuthenticationSuccess {
                        attempt,
                        total_received_frames,
                    });
                }
                StaAuthenticationEvent::Retry { .. } => {}
                StaAuthenticationEvent::Failed {
                    attempts,
                    failure,
                    total_received_frames,
                } => {
                    return Err(StaJoinError::AuthenticationFailed {
                        attempts,
                        failure,
                        total_received_frames,
                    });
                }
                StaAuthenticationEvent::Irrelevant => {
                    return Err(StaJoinError::InvalidAuthenticationEvent);
                }
            }
        }
    }

    /// Run one Association epoch and leave RX live only on success.
    ///
    /// Every tick ends at an absolute deadline, avoiding cumulative
    /// drift from RX parsing or TX publication. The final RX drain occurs at
    /// exactly 1,000 ms before the protocol timeout transition.
    pub async fn associate(
        &mut self,
        local: [u8; 6],
        bssid: [u8; 6],
        security: LinkProtection,
        sequence: &mut StaSequenceCounter,
    ) -> Result<StaAssociationSuccess, StaJoinError<B::Error>> {
        let mut runtime = StaAssociationRuntime::new(local, bssid, security);
        self.backend
            .start_receive()
            .await
            .map_err(StaJoinError::Backend)?;
        let mut poll = self.timer.now();
        loop {
            loop {
                match runtime
                    .poll(poll, sequence)
                    .map_err(StaJoinError::AssociationRuntime)?
                {
                    StaAssociationPoll::Idle => break,
                    StaAssociationPoll::Transmit(attempt) => {
                        if let Err(error) = self.backend.transmit_association(attempt).await {
                            self.stop_receive().await?;
                            return Err(StaJoinError::Backend(error));
                        }
                    }
                    StaAssociationPoll::Failed {
                        failure,
                        total_received_frames,
                    } => {
                        self.stop_receive().await?;
                        return Err(StaJoinError::AssociationFailed {
                            failure,
                            total_received_frames,
                        });
                    }
                }
            }
            poll = self.wait_next_poll(poll).await?;

            let mut observer = AssociationObserver {
                runtime: &mut runtime,
                now: poll,
                terminal: None,
            };
            if let Err(error) = self.backend.service_receive(&mut observer).await {
                self.stop_receive().await?;
                return Err(StaJoinError::Backend(error));
            }
            match observer.terminal {
                Some(Ok(StaAssociationEvent::Associated {
                    response,
                    total_received_frames,
                })) => {
                    return Ok(StaAssociationSuccess {
                        response,
                        total_received_frames,
                    });
                }
                Some(Ok(StaAssociationEvent::Failed {
                    failure,
                    total_received_frames,
                })) => {
                    self.stop_receive().await?;
                    return Err(StaJoinError::AssociationFailed {
                        failure,
                        total_received_frames,
                    });
                }
                Some(Ok(StaAssociationEvent::Irrelevant)) => {
                    self.stop_receive().await?;
                    return Err(StaJoinError::InvalidAssociationEvent);
                }
                Some(Err(error)) => {
                    self.stop_receive().await?;
                    return Err(StaJoinError::AssociationRuntime(error));
                }
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
