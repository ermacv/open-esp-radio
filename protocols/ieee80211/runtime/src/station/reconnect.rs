//! Product reconnect policy. Runner owns admission; Control only supplies intent.
use super::{Request, ServiceCommand, ServiceOutput, contract::Command};
use oer_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectPolicy {
    enabled: bool,
    interval: Duration,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZeroReconnectInterval;
impl ReconnectPolicy {
    pub const DISABLED: Self = Self {
        enabled: false,
        interval: Duration::from_secs(1),
    };
    pub const fn every(interval: Duration) -> Result<Self, ZeroReconnectInterval> {
        if interval.as_micros() == 0 {
            return Err(ZeroReconnectInterval);
        }
        Ok(Self {
            enabled: true,
            interval,
        })
    }
}
#[derive(Clone, Copy)]
pub(super) struct Policy {
    config: ReconnectPolicy,
    active: bool,
    /// An application connect was admitted and its terminal decides
    /// whether the policy is active.
    awaiting: bool,
    retry: bool,
    ready: bool,
    next: Option<Instant>,
    stopping: bool,
    disconnect_sent: bool,
}
impl Policy {
    /// The policy reconnects only after an application connect linked the
    /// station to its network: before that the station has no profile,
    /// and a connect that ended without a link leaves the retries to the
    /// application.
    pub const fn new(config: ReconnectPolicy) -> Self {
        Self {
            config,
            active: false,
            awaiting: false,
            retry: false,
            ready: true,
            next: None,
            stopping: false,
            disconnect_sent: false,
        }
    }
    pub fn admitted(&mut self, command: Command) {
        match command {
            Command::Connect => {
                self.awaiting = true;
                self.retry = false;
                self.ready = false;
            }
            Command::Disconnect => {
                self.active = false;
                self.awaiting = false;
                self.retry = false;
            }
            Command::BackgroundScan => {}
        }
    }
    pub fn observe<K>(&mut self, event: &ServiceOutput<K>, linked: bool) {
        match event {
            ServiceOutput::Notice(super::contract::Notice::Disconnected(_)) => {
                self.retry = self.active && !self.awaiting
            }
            ServiceOutput::CommandCompleted(terminal)
                if terminal.request.command() == Command::Connect =>
            {
                self.ready = true;
                // The application's connect activates the policy only
                // with a link; a policy attempt's end leaves it as it is.
                if self.awaiting {
                    self.awaiting = false;
                    self.active = self.config.enabled
                        && matches!(
                            terminal.outcome,
                            super::contract::CommandOutcome::Connected { .. }
                        );
                }
            }
            ServiceOutput::CommandRefused(refused)
                if refused.request.command() == Command::Connect =>
            {
                // A refused application connect changed nothing.
                self.awaiting = false;
                self.ready = true;
                self.retry = self.active && !linked;
            }
            _ => {}
        }
    }
    pub fn stop(&mut self) {
        self.stopping = true;
        self.active = false;
        self.retry = false;
    }
    /// The station request the policy issues next, whose permission it
    /// must take first; `None` when its next command needs none.
    pub fn wanted(&self, linked: bool) -> Option<Command> {
        if self.stopping {
            (!self.disconnect_sent).then_some(Command::Disconnect)
        } else {
            (self.active && !linked).then_some(Command::Connect)
        }
    }
    pub fn deadline(&self) -> Option<Instant> {
        if self.active { self.next } else { None }
    }
    pub fn ready(&self, now: Instant, linked: bool) -> bool {
        if self.stopping {
            return !self.disconnect_sent || !linked;
        }
        self.active && self.next.is_none_or(|deadline| now >= deadline)
    }
    pub fn command<K>(
        &mut self,
        now: Instant,
        linked: bool,
    ) -> Result<Option<ServiceCommand<K>>, DeadlineOverflow> {
        if self.stopping {
            return Ok(if !self.disconnect_sent {
                self.disconnect_sent = true;
                Some(ServiceCommand::Station(Request::new(
                    0xffff,
                    Command::Disconnect,
                )))
            } else if !linked {
                Some(ServiceCommand::Stop)
            } else {
                None
            });
        }
        if !self.ready(now, linked) {
            return Ok(None);
        }
        self.next = Some(
            now.checked_add(self.config.interval)
                .ok_or(DeadlineOverflow)?,
        );
        if self.retry && self.ready && !linked {
            self.retry = false;
            self.ready = false;
            Ok(Some(ServiceCommand::Station(Request::new(
                1,
                Command::Connect,
            ))))
        } else {
            Ok(None)
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DeadlineOverflow;

#[cfg(test)]
mod tests;
