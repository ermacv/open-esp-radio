//! The application's station vocabulary: lifecycle stages, fault reasons
//! and the reconnect policy. The station itself is [`crate::machine`].

/// The stage of the station's lifecycle a fault or a report names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaLifecycleStage {
    CandidateSelection,
    Authentication,
    Association,
    Security,
    Connected,
    Hardware,
}

/// Station phase whose transition faulted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaFaultPhase {
    /// The link had not connected in this attempt: it was joining, or
    /// entering its connection over a port it does not own.
    Attempt,
    /// Bringing up the connected epoch on hardware the station owns: its policy, driver, interrupts,
    /// static resources, transmit owner and security installation.
    Start,
    /// The running connected epoch.
    Connected,
    /// Returning the connected epoch's owners to the disconnected station.
    Teardown,
}

/// What a faulted transition could not prove.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaFaultCause {
    /// A hardware owner or a bounded hardware transaction.
    Hardware,
    /// Ownership of keys, packet numbers or replay state.
    Security,
    /// The connected policy derived from the association.
    Configuration,
    /// Static memory, queues or interrupt routes the phase needs.
    Resources,
    /// The radio's RF power state.
    RfState,
    /// The lower-MAC port the station is a client of was lost: it is
    /// poisoned, or it refused or failed a lifecycle command. The station
    /// held no hardware; only the port's owner restores the port.
    PortLost,
}

/// Portable reason of a station fault: the backend's own fault type names
/// it, so every backend reports the same vocabulary to the application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaFaultReason {
    pub phase: StaFaultPhase,
    pub cause: StaFaultCause,
}

impl StaFaultReason {
    pub const fn new(phase: StaFaultPhase, cause: StaFaultCause) -> Self {
        Self { phase, cause }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaReconnectPolicyError {
    ZeroAttemptLimit,
    ZeroRetryBackoff,
    RetryBackoffRange,
    ZeroDisconnectBackoff,
}

/// Bounded retry/backoff policy for one station service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaReconnectPolicy {
    attempt_limit: u16,
    initial_retry_backoff_millis: u32,
    maximum_retry_backoff_millis: u32,
    disconnect_backoff_millis: u32,
}

impl StaReconnectPolicy {
    pub const fn new(
        attempt_limit: u16,
        initial_retry_backoff_millis: u32,
        maximum_retry_backoff_millis: u32,
        disconnect_backoff_millis: u32,
    ) -> Result<Self, StaReconnectPolicyError> {
        if attempt_limit == 0 {
            return Err(StaReconnectPolicyError::ZeroAttemptLimit);
        }
        if initial_retry_backoff_millis == 0 {
            return Err(StaReconnectPolicyError::ZeroRetryBackoff);
        }
        if maximum_retry_backoff_millis < initial_retry_backoff_millis {
            return Err(StaReconnectPolicyError::RetryBackoffRange);
        }
        if disconnect_backoff_millis == 0 {
            return Err(StaReconnectPolicyError::ZeroDisconnectBackoff);
        }
        Ok(Self {
            attempt_limit,
            initial_retry_backoff_millis,
            maximum_retry_backoff_millis,
            disconnect_backoff_millis,
        })
    }

    pub const fn attempt_limit(self) -> u16 {
        self.attempt_limit
    }

    pub const fn disconnect_backoff_millis(self) -> u32 {
        self.disconnect_backoff_millis
    }

    /// Exponential retry delay capped before integer overflow.
    pub const fn retry_backoff_millis(self, failed_attempt: u16) -> u32 {
        let mut delay = self.initial_retry_backoff_millis;
        let mut remaining = failed_attempt.saturating_sub(1);
        while remaining != 0 && delay < self.maximum_retry_backoff_millis {
            let doubled = delay.saturating_mul(2);
            delay = if doubled < self.maximum_retry_backoff_millis {
                doubled
            } else {
                self.maximum_retry_backoff_millis
            };
            remaining = remaining.saturating_sub(1);
        }
        delay
    }
}
