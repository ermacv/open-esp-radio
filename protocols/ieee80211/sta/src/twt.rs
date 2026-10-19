//! Portable individual TWT requester policy and fixed-capacity runtime.
//!
//! This owner performs no I/O and reads no clock. A caller supplies monotonic
//! runtime deadlines for negotiation and the associated station TSF for wake
//! planning, as [`TsfInstant`]s: the wake plan is computed within one TSF
//! generation, which the caller tracks. A TSF crossing 2^64 is a jump that
//! starts a new generation, so planning never wraps: a service window past
//! 2^64 is refused ([`IndividualTwtWakePlanError::BeyondTsfRange`]). Chip code must separately prove that it can install the accepted
//! agreement before any TWT Setup frame is published.

use oer_ieee80211_mac::tsf::{TsfDuration, TsfInstant};
use oer_ieee80211_mac::twt::{
    INDIVIDUAL_TWT_FLOW_CAPACITY, INDIVIDUAL_TWT_SETUP_BODY_LEN, INDIVIDUAL_TWT_TEARDOWN_BODY_LEN,
    IndividualTwtAction, IndividualTwtControl, IndividualTwtFlowId, IndividualTwtParameterSet,
    IndividualTwtSetup, IndividualTwtSetupCommand, IndividualTwtTeardown, TwtWakeDurationUnit,
    TwtWireError,
};
use oer_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtRequesterConfigError {
    ZeroResponseTimeout,
    ZeroRetryInterval,
    ZeroSetupAttemptLimit,
    ZeroTeardownAttemptLimit,
}

/// The flow identifier of each entry of the flow array, checked at build:
/// the array holds only representable flows.
const FLOW_IDS: [IndividualTwtFlowId; INDIVIDUAL_TWT_FLOW_CAPACITY] = IndividualTwtFlowId::ALL;

/// Association-scoped retry and deadline policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtRequesterConfig {
    response_timeout: Duration,
    retry_interval: Duration,
    setup_attempt_limit: u8,
    teardown_attempt_limit: u8,
}

impl IndividualTwtRequesterConfig {
    pub const fn new(
        response_timeout: Duration,
        retry_interval: Duration,
        setup_attempt_limit: u8,
        teardown_attempt_limit: u8,
    ) -> Result<Self, IndividualTwtRequesterConfigError> {
        if response_timeout.as_micros() == 0 {
            return Err(IndividualTwtRequesterConfigError::ZeroResponseTimeout);
        }
        if retry_interval.as_micros() == 0 {
            return Err(IndividualTwtRequesterConfigError::ZeroRetryInterval);
        }
        if setup_attempt_limit == 0 {
            return Err(IndividualTwtRequesterConfigError::ZeroSetupAttemptLimit);
        }
        if teardown_attempt_limit == 0 {
            return Err(IndividualTwtRequesterConfigError::ZeroTeardownAttemptLimit);
        }
        Ok(Self {
            response_timeout,
            retry_interval,
            setup_attempt_limit,
            teardown_attempt_limit,
        })
    }

    pub const fn response_timeout(self) -> Duration {
        self.response_timeout
    }

    pub const fn retry_interval(self) -> Duration {
        self.retry_interval
    }

    pub const fn setup_attempt_limit(self) -> u8 {
        self.setup_attempt_limit
    }

    pub const fn teardown_attempt_limit(self) -> u8 {
        self.teardown_attempt_limit
    }
}

/// Request parameters supplied before a Dialog Token is allocated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtProposal {
    pub control: IndividualTwtControl,
    pub parameters: IndividualTwtParameterSet,
}

impl IndividualTwtProposal {
    pub fn validate(self) -> Result<Self, IndividualTwtRequesterError> {
        if !self.parameters.requesting_sta {
            return Err(TwtWireError::RequestCommandFromResponder.into());
        }
        if !self.parameters.setup_command.is_requester_command() {
            return Err(TwtWireError::ResponseCommandFromRequester.into());
        }
        self.parameters.validate(self.control)?;
        if !self.parameters.implicit {
            return Err(
                IndividualTwtRequesterError::ExplicitTwtInformationUnsupported(
                    IndividualTwtInformationFrontier::from_fields(self.control, self.parameters),
                ),
            );
        }
        Ok(self)
    }
}

/// Exact protocol state missing before an explicit agreement can be live.
///
/// Explicit agreements need subsequent TWT Information actions to move or
/// suspend their next service edge. The current codec/runtime owns neither
/// that action body nor its schedule-update semantics. Retaining the accepted
/// fields makes the missing frontier observable without treating the initial
/// target as an implicit periodic schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtInformationFrontier {
    pub flow_id: IndividualTwtFlowId,
    pub initial_target_wake_time: TsfInstant,
    pub information_frames_disabled: bool,
}

impl IndividualTwtInformationFrontier {
    const fn from_fields(
        control: IndividualTwtControl,
        parameters: IndividualTwtParameterSet,
    ) -> Self {
        Self {
            flow_id: parameters.flow_id,
            initial_target_wake_time: TsfInstant::from_micros(parameters.target_wake_time_tsf),
            information_frames_disabled: control.information_frames_disabled,
        }
    }
}

/// Accepted, validated individual TWT agreement in the station TSF domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtAgreement {
    pub flow_id: IndividualTwtFlowId,
    pub control: IndividualTwtControl,
    pub trigger: bool,
    pub implicit: bool,
    pub flow_type: oer_ieee80211_mac::twt::IndividualTwtFlowType,
    pub protection: bool,
    pub target_wake_time: TsfInstant,
    pub wake_interval: TsfDuration,
    pub wake_duration: TsfDuration,
}

impl IndividualTwtAgreement {
    fn from_response(response: IndividualTwtSetup) -> Result<Self, TwtWireError> {
        let parameters = response.parameters.validate(response.control)?;
        Ok(Self {
            flow_id: parameters.flow_id,
            control: response.control,
            trigger: parameters.trigger,
            implicit: parameters.implicit,
            flow_type: parameters.flow_type,
            protection: parameters.protection,
            target_wake_time: TsfInstant::from_micros(parameters.target_wake_time_tsf),
            wake_interval: TsfDuration::from_micros(parameters.wake_interval_micros()?),
            wake_duration: TsfDuration::from_micros(u64::from(
                parameters.wake_duration_micros(response.control)?,
            )),
        })
    }

    /// Return the explicit-information frontier for a non-periodic agreement.
    pub const fn information_frontier(self) -> Option<IndividualTwtInformationFrontier> {
        if self.implicit {
            None
        } else {
            Some(IndividualTwtInformationFrontier {
                flow_id: self.flow_id,
                initial_target_wake_time: self.target_wake_time,
                information_frames_disabled: self.control.information_frames_disabled,
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtFlowStatus {
    Idle,
    SetupQueued,
    SetupTransmitting,
    AwaitingResponse,
    AwaitingHardwareInstall,
    Active,
    TeardownQueued,
    TeardownTransmitting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtTxKind {
    Setup,
    Teardown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtTxBody {
    Setup([u8; INDIVIDUAL_TWT_SETUP_BODY_LEN]),
    Teardown([u8; INDIVIDUAL_TWT_TEARDOWN_BODY_LEN]),
}

impl IndividualTwtTxBody {
    pub const fn as_slice(&self) -> &[u8] {
        match self {
            Self::Setup(body) => body,
            Self::Teardown(body) => body,
        }
    }
}

/// Affine identity for one action handed to a shared TX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtTransmission {
    pub flow_id: IndividualTwtFlowId,
    pub generation: u32,
    pub kind: IndividualTwtTxKind,
    pub body: IndividualTwtTxBody,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtRequesterEvent {
    SetupPublished {
        flow_id: IndividualTwtFlowId,
        response_deadline: Instant,
    },
    SetupRetryScheduled {
        flow_id: IndividualTwtFlowId,
        retry_at: Instant,
    },
    SetupTimedOut {
        flow_id: IndividualTwtFlowId,
    },
    SetupTxFailed {
        flow_id: IndividualTwtFlowId,
    },
    TeardownComplete {
        flow_id: IndividualTwtFlowId,
    },
    TeardownRetryScheduled {
        flow_id: IndividualTwtFlowId,
        retry_at: Instant,
    },
    TeardownTxFailed {
        flow_id: IndividualTwtFlowId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtService {
    Idle,
    Transmit(IndividualTwtTransmission),
    Event(IndividualTwtRequesterEvent),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtSetupDisposition {
    Stale,
    Rejected {
        flow_id: IndividualTwtFlowId,
    },
    Alternative {
        flow_id: IndividualTwtFlowId,
        command: IndividualTwtSetupCommand,
        proposed: IndividualTwtAgreement,
    },
    InstallRequired {
        flow_id: IndividualTwtFlowId,
        generation: u32,
        agreement: IndividualTwtAgreement,
    },
    /// The AP accepted an explicit agreement that this requester cannot keep
    /// synchronized. A teardown is already queued; no hardware install or
    /// wake-plan publication is permitted.
    ExplicitInformationUnsupported {
        flow_id: IndividualTwtFlowId,
        frontier: IndividualTwtInformationFrontier,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtRequesterError {
    Wire(TwtWireError),
    /// Explicit agreements require TWT Information updates before another
    /// wake can be derived; this requester currently supports periodic
    /// implicit agreements only.
    ExplicitTwtInformationUnsupported(IndividualTwtInformationFrontier),
    /// Every nonzero generation has been issued. Reuse would let a stale
    /// completion alias a new TX/install obligation, so this requester stays
    /// fail-closed until it is replaced by a new owner with a wider epoch.
    GenerationExhausted,
    DeadlineOverflow,
    FlowBusy(IndividualTwtFlowId),
    NoAgreement(IndividualTwtFlowId),
    StaleTransmission,
    UnexpectedInstall,
    UnexpectedRemove,
    DemandResponseMismatch,
}

impl From<TwtWireError> for IndividualTwtRequesterError {
    fn from(error: TwtWireError) -> Self {
        Self::Wire(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlowPhase {
    Idle,
    SetupQueued {
        proposal: IndividualTwtProposal,
        attempts_remaining: u8,
        ready_at: Instant,
    },
    SetupTransmitting {
        proposal: IndividualTwtProposal,
        dialog_token: u8,
        generation: u32,
        attempts_remaining: u8,
    },
    AwaitingResponse {
        proposal: IndividualTwtProposal,
        dialog_token: u8,
        generation: u32,
        attempts_remaining: u8,
        deadline: Instant,
    },
    AwaitingHardwareInstall {
        generation: u32,
        agreement: IndividualTwtAgreement,
    },
    Active(IndividualTwtAgreement),
    TeardownQueued {
        attempts_remaining: u8,
        ready_at: Instant,
    },
    TeardownTransmitting {
        generation: u32,
        attempts_remaining: u8,
    },
}

impl FlowPhase {
    const IDLE: Self = Self::Idle;

    const fn status(self) -> IndividualTwtFlowStatus {
        match self {
            Self::Idle => IndividualTwtFlowStatus::Idle,
            Self::SetupQueued { .. } => IndividualTwtFlowStatus::SetupQueued,
            Self::SetupTransmitting { .. } => IndividualTwtFlowStatus::SetupTransmitting,
            Self::AwaitingResponse { .. } => IndividualTwtFlowStatus::AwaitingResponse,
            Self::AwaitingHardwareInstall { .. } => {
                IndividualTwtFlowStatus::AwaitingHardwareInstall
            }
            Self::Active(_) => IndividualTwtFlowStatus::Active,
            Self::TeardownQueued { .. } => IndividualTwtFlowStatus::TeardownQueued,
            Self::TeardownTransmitting { .. } => IndividualTwtFlowStatus::TeardownTransmitting,
        }
    }
}

/// Exactly eight flow slots, matching the three-bit wire identifier.
pub struct IndividualTwtRequester {
    config: IndividualTwtRequesterConfig,
    flows: [FlowPhase; INDIVIDUAL_TWT_FLOW_CAPACITY],
    next_dialog_token: u8,
    generation: u32,
}

impl IndividualTwtRequester {
    pub const fn new(config: IndividualTwtRequesterConfig) -> Self {
        Self {
            config,
            flows: [FlowPhase::IDLE; INDIVIDUAL_TWT_FLOW_CAPACITY],
            next_dialog_token: 1,
            generation: 0,
        }
    }

    pub const fn config(&self) -> IndividualTwtRequesterConfig {
        self.config
    }

    pub const fn status(&self, flow_id: IndividualTwtFlowId) -> IndividualTwtFlowStatus {
        flow_id.select(&self.flows).status()
    }

    pub const fn agreement(&self, flow_id: IndividualTwtFlowId) -> Option<IndividualTwtAgreement> {
        match *flow_id.select(&self.flows) {
            FlowPhase::Active(agreement) => Some(agreement),
            _ => None,
        }
    }

    /// Proposal currently crossing the chip-admission/TX boundary.
    pub const fn transmitting_proposal(
        &self,
        flow_id: IndividualTwtFlowId,
    ) -> Option<IndividualTwtProposal> {
        match *flow_id.select(&self.flows) {
            FlowPhase::SetupTransmitting { proposal, .. } => Some(proposal),
            _ => None,
        }
    }

    pub fn queue_setup(
        &mut self,
        proposal: IndividualTwtProposal,
        now: Instant,
    ) -> Result<(), IndividualTwtRequesterError> {
        let proposal = proposal.validate()?;
        let flow_id = proposal.parameters.flow_id;
        if !matches!(*flow_id.select(&self.flows), FlowPhase::Idle) {
            return Err(IndividualTwtRequesterError::FlowBusy(flow_id));
        }
        *flow_id.select_mut(&mut self.flows) = FlowPhase::SetupQueued {
            proposal,
            attempts_remaining: self.config.setup_attempt_limit,
            ready_at: now,
        };
        Ok(())
    }

    /// Queue a teardown after the chip owner has synchronously removed any
    /// installed wake agreement. This method itself never asserts that the
    /// hardware schedule was removed.
    pub fn queue_teardown(
        &mut self,
        flow_id: IndividualTwtFlowId,
        now: Instant,
    ) -> Result<(), IndividualTwtRequesterError> {
        if matches!(*flow_id.select(&self.flows), FlowPhase::Idle) {
            return Err(IndividualTwtRequesterError::NoAgreement(flow_id));
        }
        if matches!(
            *flow_id.select(&self.flows),
            FlowPhase::SetupTransmitting { .. } | FlowPhase::TeardownTransmitting { .. }
        ) {
            return Err(IndividualTwtRequesterError::FlowBusy(flow_id));
        }
        *flow_id.select_mut(&mut self.flows) = FlowPhase::TeardownQueued {
            attempts_remaining: self.config.teardown_attempt_limit,
            ready_at: now,
        };
        Ok(())
    }

    pub fn service(
        &mut self,
        now: Instant,
    ) -> Result<IndividualTwtService, IndividualTwtRequesterError> {
        for &flow_id in &FLOW_IDS {
            let FlowPhase::AwaitingResponse {
                proposal,
                attempts_remaining,
                deadline,
                ..
            } = *flow_id.select(&self.flows)
            else {
                continue;
            };
            if now < deadline {
                continue;
            }
            if attempts_remaining == 0 {
                *flow_id.select_mut(&mut self.flows) = FlowPhase::Idle;
                return Ok(IndividualTwtService::Event(
                    IndividualTwtRequesterEvent::SetupTimedOut { flow_id },
                ));
            }
            let retry_at = now
                .checked_add(self.config.retry_interval)
                .ok_or(IndividualTwtRequesterError::DeadlineOverflow)?;
            *flow_id.select_mut(&mut self.flows) = FlowPhase::SetupQueued {
                proposal,
                attempts_remaining,
                ready_at: retry_at,
            };
            return Ok(IndividualTwtService::Event(
                IndividualTwtRequesterEvent::SetupRetryScheduled { flow_id, retry_at },
            ));
        }

        for &flow_id in &FLOW_IDS {
            match *flow_id.select(&self.flows) {
                FlowPhase::SetupQueued {
                    proposal,
                    attempts_remaining,
                    ready_at,
                } if now >= ready_at => {
                    let generation = self.take_generation()?;
                    let dialog_token = self.take_dialog_token();
                    let setup = IndividualTwtSetup {
                        dialog_token,
                        control: proposal.control,
                        parameters: proposal.parameters,
                    };
                    let body = setup.encode_body()?;
                    let attempts_remaining = attempts_remaining.saturating_sub(1);
                    *flow_id.select_mut(&mut self.flows) = FlowPhase::SetupTransmitting {
                        proposal,
                        dialog_token,
                        generation,
                        attempts_remaining,
                    };
                    return Ok(IndividualTwtService::Transmit(IndividualTwtTransmission {
                        flow_id,
                        generation,
                        kind: IndividualTwtTxKind::Setup,
                        body: IndividualTwtTxBody::Setup(body),
                    }));
                }
                FlowPhase::TeardownQueued {
                    attempts_remaining,
                    ready_at,
                } if now >= ready_at => {
                    let body = IndividualTwtTeardown::one(flow_id).encode_body()?;
                    let generation = self.take_generation()?;
                    let attempts_remaining = attempts_remaining.saturating_sub(1);
                    *flow_id.select_mut(&mut self.flows) = FlowPhase::TeardownTransmitting {
                        generation,
                        attempts_remaining,
                    };
                    return Ok(IndividualTwtService::Transmit(IndividualTwtTransmission {
                        flow_id,
                        generation,
                        kind: IndividualTwtTxKind::Teardown,
                        body: IndividualTwtTxBody::Teardown(body),
                    }));
                }
                _ => {}
            }
        }
        Ok(IndividualTwtService::Idle)
    }

    pub fn complete_transmission(
        &mut self,
        transmission: IndividualTwtTransmission,
        acknowledged: bool,
        now: Instant,
    ) -> Result<IndividualTwtRequesterEvent, IndividualTwtRequesterError> {
        let flow_id = transmission.flow_id;
        match (*flow_id.select(&self.flows), transmission.kind) {
            (
                FlowPhase::SetupTransmitting {
                    proposal,
                    dialog_token,
                    generation,
                    attempts_remaining,
                },
                IndividualTwtTxKind::Setup,
            ) if generation == transmission.generation => {
                if acknowledged {
                    let deadline = now
                        .checked_add(self.config.response_timeout)
                        .ok_or(IndividualTwtRequesterError::DeadlineOverflow)?;
                    *flow_id.select_mut(&mut self.flows) = FlowPhase::AwaitingResponse {
                        proposal,
                        dialog_token,
                        generation,
                        attempts_remaining,
                        deadline,
                    };
                    Ok(IndividualTwtRequesterEvent::SetupPublished {
                        flow_id: transmission.flow_id,
                        response_deadline: deadline,
                    })
                } else {
                    self.retry_or_finish_setup(
                        transmission.flow_id,
                        proposal,
                        attempts_remaining,
                        now,
                        false,
                    )
                }
            }
            (
                FlowPhase::TeardownTransmitting {
                    generation,
                    attempts_remaining,
                },
                IndividualTwtTxKind::Teardown,
            ) if generation == transmission.generation => {
                if acknowledged {
                    *flow_id.select_mut(&mut self.flows) = FlowPhase::Idle;
                    Ok(IndividualTwtRequesterEvent::TeardownComplete {
                        flow_id: transmission.flow_id,
                    })
                } else if attempts_remaining == 0 {
                    *flow_id.select_mut(&mut self.flows) = FlowPhase::Idle;
                    Ok(IndividualTwtRequesterEvent::TeardownTxFailed {
                        flow_id: transmission.flow_id,
                    })
                } else {
                    let retry_at = now
                        .checked_add(self.config.retry_interval)
                        .ok_or(IndividualTwtRequesterError::DeadlineOverflow)?;
                    *flow_id.select_mut(&mut self.flows) = FlowPhase::TeardownQueued {
                        attempts_remaining,
                        ready_at: retry_at,
                    };
                    Ok(IndividualTwtRequesterEvent::TeardownRetryScheduled {
                        flow_id: transmission.flow_id,
                        retry_at,
                    })
                }
            }
            _ => Err(IndividualTwtRequesterError::StaleTransmission),
        }
    }

    /// Cancel an action before physical publication. Used when a chip's
    /// hardware-admission boundary reports `Unsupported`.
    pub fn abort_transmission(
        &mut self,
        transmission: IndividualTwtTransmission,
    ) -> Result<(), IndividualTwtRequesterError> {
        let flow_id = transmission.flow_id;
        let matches = match (*flow_id.select(&self.flows), transmission.kind) {
            (FlowPhase::SetupTransmitting { generation, .. }, IndividualTwtTxKind::Setup)
            | (FlowPhase::TeardownTransmitting { generation, .. }, IndividualTwtTxKind::Teardown) => {
                generation == transmission.generation
            }
            _ => false,
        };
        if !matches {
            return Err(IndividualTwtRequesterError::StaleTransmission);
        }
        *flow_id.select_mut(&mut self.flows) = FlowPhase::Idle;
        Ok(())
    }

    pub fn on_action(
        &mut self,
        action: IndividualTwtAction,
    ) -> Result<IndividualTwtSetupDisposition, IndividualTwtRequesterError> {
        match action {
            IndividualTwtAction::Setup(setup) => self.on_setup_response(setup),
            IndividualTwtAction::Teardown(_) => Ok(IndividualTwtSetupDisposition::Stale),
        }
    }

    pub fn on_setup_response(
        &mut self,
        response: IndividualTwtSetup,
    ) -> Result<IndividualTwtSetupDisposition, IndividualTwtRequesterError> {
        response.validate()?;
        let flow_id = response.parameters.flow_id;

        let FlowPhase::AwaitingResponse {
            proposal,
            dialog_token,
            generation,
            ..
        } = *flow_id.select(&self.flows)
        else {
            return Ok(IndividualTwtSetupDisposition::Stale);
        };
        if response.dialog_token != dialog_token || response.parameters.requesting_sta {
            return Ok(IndividualTwtSetupDisposition::Stale);
        }

        if response.parameters.setup_command == IndividualTwtSetupCommand::Accept
            && !response.parameters.implicit
        {
            let frontier = IndividualTwtInformationFrontier::from_fields(
                response.control,
                response.parameters,
            );
            *flow_id.select_mut(&mut self.flows) = FlowPhase::TeardownQueued {
                attempts_remaining: self.config.teardown_attempt_limit,
                // A response has already crossed the wire. Zero is due in
                // every monotonic runtime domain without inventing a new
                // timestamp parameter at this protocol edge.
                ready_at: Instant::EPOCH,
            };
            return Ok(
                IndividualTwtSetupDisposition::ExplicitInformationUnsupported { flow_id, frontier },
            );
        }

        match response.parameters.setup_command {
            IndividualTwtSetupCommand::Reject => {
                *flow_id.select_mut(&mut self.flows) = FlowPhase::Idle;
                Ok(IndividualTwtSetupDisposition::Rejected { flow_id })
            }
            command @ (IndividualTwtSetupCommand::Alternate
            | IndividualTwtSetupCommand::Dictate) => {
                let proposed = IndividualTwtAgreement::from_response(response)?;
                *flow_id.select_mut(&mut self.flows) = FlowPhase::Idle;
                Ok(IndividualTwtSetupDisposition::Alternative {
                    flow_id,
                    command,
                    proposed,
                })
            }
            IndividualTwtSetupCommand::Accept => {
                if proposal.parameters.setup_command == IndividualTwtSetupCommand::Demand
                    && !demand_response_matches(proposal, response)
                {
                    *flow_id.select_mut(&mut self.flows) = FlowPhase::Idle;
                    return Err(IndividualTwtRequesterError::DemandResponseMismatch);
                }
                let agreement = IndividualTwtAgreement::from_response(response)?;
                *flow_id.select_mut(&mut self.flows) = FlowPhase::AwaitingHardwareInstall {
                    generation,
                    agreement,
                };
                Ok(IndividualTwtSetupDisposition::InstallRequired {
                    flow_id,
                    generation,
                    agreement,
                })
            }
            _ => Ok(IndividualTwtSetupDisposition::Stale),
        }
    }

    pub fn commit_hardware_install(
        &mut self,
        flow_id: IndividualTwtFlowId,
        generation: u32,
    ) -> Result<IndividualTwtAgreement, IndividualTwtRequesterError> {
        let FlowPhase::AwaitingHardwareInstall {
            generation: expected,
            agreement,
        } = *flow_id.select(&self.flows)
        else {
            return Err(IndividualTwtRequesterError::UnexpectedInstall);
        };
        if expected != generation {
            return Err(IndividualTwtRequesterError::UnexpectedInstall);
        }
        *flow_id.select_mut(&mut self.flows) = FlowPhase::Active(agreement);
        Ok(agreement)
    }

    /// Roll back an accepted peer agreement when chip installation fails.
    /// The AP must subsequently receive the queued teardown; the local wake
    /// planner never observes the failed agreement as active.
    pub fn reject_hardware_install(
        &mut self,
        flow_id: IndividualTwtFlowId,
        generation: u32,
        now: Instant,
    ) -> Result<(), IndividualTwtRequesterError> {
        let FlowPhase::AwaitingHardwareInstall {
            generation: expected,
            ..
        } = *flow_id.select(&self.flows)
        else {
            return Err(IndividualTwtRequesterError::UnexpectedInstall);
        };
        if expected != generation {
            return Err(IndividualTwtRequesterError::UnexpectedInstall);
        }
        *flow_id.select_mut(&mut self.flows) = FlowPhase::TeardownQueued {
            attempts_remaining: self.config.teardown_attempt_limit,
            ready_at: now,
        };
        Ok(())
    }

    /// Snapshot installed agreements affected by a peer teardown without
    /// changing portable state. The chip owner must commit each successful
    /// hardware removal before the teardown is applied to the remaining
    /// protocol phases.
    pub fn installed_for_teardown(
        &self,
        teardown: IndividualTwtTeardown,
    ) -> [Option<IndividualTwtAgreement>; INDIVIDUAL_TWT_FLOW_CAPACITY] {
        let mut installed = [None; INDIVIDUAL_TWT_FLOW_CAPACITY];
        if teardown.all_flows {
            for (slot, phase) in installed.iter_mut().zip(&self.flows) {
                if let FlowPhase::Active(agreement) = *phase {
                    *slot = Some(agreement);
                }
            }
        } else if let FlowPhase::Active(agreement) = *teardown.flow_id.select(&self.flows) {
            *teardown.flow_id.select_mut(&mut installed) = Some(agreement);
        }
        installed
    }

    /// Commit one successful chip removal. A stale or duplicated completion
    /// cannot silently erase a newer agreement for the same flow ID.
    pub fn commit_hardware_remove(
        &mut self,
        agreement: IndividualTwtAgreement,
    ) -> Result<(), IndividualTwtRequesterError> {
        let phase = agreement.flow_id.select_mut(&mut self.flows);
        if !matches!(*phase, FlowPhase::Active(current) if current == agreement) {
            return Err(IndividualTwtRequesterError::UnexpectedRemove);
        }
        *phase = FlowPhase::Idle;
        Ok(())
    }

    /// Apply the peer teardown to every remaining protocol phase. Hardware
    /// owners use `installed_for_teardown` plus `commit_hardware_remove`
    /// first; the returned snapshot supports portable owners with no chip
    /// schedule to roll back.
    pub fn on_peer_teardown(
        &mut self,
        teardown: IndividualTwtTeardown,
    ) -> [Option<IndividualTwtAgreement>; INDIVIDUAL_TWT_FLOW_CAPACITY] {
        let mut removed = [None; INDIVIDUAL_TWT_FLOW_CAPACITY];
        if teardown.all_flows {
            for (slot, phase) in removed.iter_mut().zip(&mut self.flows) {
                if let FlowPhase::Active(agreement) = *phase {
                    *slot = Some(agreement);
                }
                *phase = FlowPhase::Idle;
            }
        } else {
            let flow_id = teardown.flow_id;
            if let FlowPhase::Active(agreement) = *flow_id.select(&self.flows) {
                *flow_id.select_mut(&mut removed) = Some(agreement);
            }
            *flow_id.select_mut(&mut self.flows) = FlowPhase::Idle;
        }
        removed
    }

    /// Reconnect/stop boundary. No portable agreement survives a new BSSID
    /// epoch; installed agreements are returned for chip rollback.
    pub fn reset_for_reconnect(
        &mut self,
    ) -> [Option<IndividualTwtAgreement>; INDIVIDUAL_TWT_FLOW_CAPACITY] {
        let mut removed = [None; INDIVIDUAL_TWT_FLOW_CAPACITY];
        for (slot, phase) in removed.iter_mut().zip(&mut self.flows) {
            if let FlowPhase::Active(agreement) = *phase {
                *slot = Some(agreement);
            }
            *phase = FlowPhase::Idle;
        }
        self.next_dialog_token = 1;
        // Never wrap an affine identity. Saturation permanently prevents a
        // stale generation from being reissued after enough reconnects.
        self.generation = self.generation.saturating_add(1);
        removed
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.flows
            .iter()
            .filter_map(|phase| match phase {
                FlowPhase::SetupQueued { ready_at, .. }
                | FlowPhase::TeardownQueued { ready_at, .. } => Some(*ready_at),
                FlowPhase::AwaitingResponse { deadline, .. } => Some(*deadline),
                _ => None,
            })
            .min()
    }

    /// The earliest active or upcoming service window across the installed
    /// flows at station TSF `now`, waking `wake_guard` before it starts.
    /// `now` and the plan belong to one TSF generation.
    pub fn plan_next_wake(
        &self,
        now: TsfInstant,
        wake_guard: TsfDuration,
    ) -> Result<Option<IndividualTwtWakePlan>, IndividualTwtWakePlanError> {
        let mut best: Option<IndividualTwtWakePlan> = None;
        for phase in self.flows {
            let FlowPhase::Active(agreement) = phase else {
                continue;
            };
            let candidate = plan_agreement_wake(agreement, now, wake_guard)?;
            best = Some(match best {
                None => candidate,
                Some(current) => current.merge_or_earlier(candidate),
            });
        }
        Ok(best)
    }

    fn retry_or_finish_setup(
        &mut self,
        flow_id: IndividualTwtFlowId,
        proposal: IndividualTwtProposal,
        attempts_remaining: u8,
        now: Instant,
        timed_out: bool,
    ) -> Result<IndividualTwtRequesterEvent, IndividualTwtRequesterError> {
        if attempts_remaining == 0 {
            *flow_id.select_mut(&mut self.flows) = FlowPhase::Idle;
            return Ok(if timed_out {
                IndividualTwtRequesterEvent::SetupTimedOut { flow_id }
            } else {
                IndividualTwtRequesterEvent::SetupTxFailed { flow_id }
            });
        }
        let retry_at = now
            .checked_add(self.config.retry_interval)
            .ok_or(IndividualTwtRequesterError::DeadlineOverflow)?;
        *flow_id.select_mut(&mut self.flows) = FlowPhase::SetupQueued {
            proposal,
            attempts_remaining,
            ready_at: retry_at,
        };
        Ok(IndividualTwtRequesterEvent::SetupRetryScheduled { flow_id, retry_at })
    }

    fn take_dialog_token(&mut self) -> u8 {
        let token = self.next_dialog_token;
        self.next_dialog_token = next_dialog_token(token);
        token
    }

    fn take_generation(&mut self) -> Result<u32, IndividualTwtRequesterError> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(IndividualTwtRequesterError::GenerationExhausted)?;
        Ok(self.generation)
    }
}

fn demand_response_matches(proposal: IndividualTwtProposal, response: IndividualTwtSetup) -> bool {
    let requested = proposal.parameters;
    let accepted = response.parameters;
    proposal.control.wake_duration_unit == response.control.wake_duration_unit
        && requested.trigger == accepted.trigger
        && requested.implicit == accepted.implicit
        && requested.flow_type == accepted.flow_type
        && requested.protection == accepted.protection
        && requested.target_wake_time_tsf == accepted.target_wake_time_tsf
        && requested.nominal_minimum_wake_duration == accepted.nominal_minimum_wake_duration
        && requested.wake_interval_mantissa == accepted.wake_interval_mantissa
        && requested.wake_interval_exponent == accepted.wake_interval_exponent
        && requested.twt_channel == accepted.twt_channel
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtWakePlanError {
    InvalidAgreement,
    WakeGuardOutsideInterval {
        flow_id: IndividualTwtFlowId,
        wake_guard: TsfDuration,
        interval: TsfDuration,
    },
    /// The service window ends past 2^64: beyond the TSF generation the plan
    /// is computed in.
    BeyondTsfRange,
    /// The guard would wake before the beginning of the TSF epoch.
    BeforeTsfEpoch,
}

/// Earliest active or upcoming service window across all installed flows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtWakePlan {
    pub flow_bitmap: u8,
    pub wake: TsfInstant,
    pub service_start: TsfInstant,
    pub service_end: TsfInstant,
    pub service_open: bool,
}

impl IndividualTwtWakePlan {
    fn merge_or_earlier(self, other: Self) -> Self {
        if self.service_open && other.service_open {
            return Self {
                flow_bitmap: self.flow_bitmap | other.flow_bitmap,
                wake: self.wake,
                service_start: self.service_start.min(other.service_start),
                service_end: self.service_end.max(other.service_end),
                service_open: true,
            };
        }
        if other.service_open || (!self.service_open && other.wake < self.wake) {
            other
        } else if self.service_open || self.wake < other.wake {
            self
        } else {
            Self {
                flow_bitmap: self.flow_bitmap | other.flow_bitmap,
                service_end: self.service_end.max(other.service_end),
                ..self
            }
        }
    }
}

fn plan_agreement_wake(
    agreement: IndividualTwtAgreement,
    now: TsfInstant,
    wake_guard: TsfDuration,
) -> Result<IndividualTwtWakePlan, IndividualTwtWakePlanError> {
    let interval = agreement.wake_interval;
    let duration = agreement.wake_duration;
    if interval == TsfDuration::ZERO || duration == TsfDuration::ZERO || duration > interval {
        return Err(IndividualTwtWakePlanError::InvalidAgreement);
    }
    if wake_guard >= interval {
        return Err(IndividualTwtWakePlanError::WakeGuardOutsideInterval {
            flow_id: agreement.flow_id,
            wake_guard,
            interval,
        });
    }
    let target = agreement.target_wake_time;
    let (service_start, service_open) = if now >= target {
        let elapsed = now
            .checked_duration_since(target)
            .ok_or(IndividualTwtWakePlanError::BeyondTsfRange)?;
        // The interval is non-zero (checked above).
        let offset = elapsed
            .checked_rem(interval)
            .ok_or(IndividualTwtWakePlanError::InvalidAgreement)?;
        let current_start = now
            .checked_sub(offset)
            .ok_or(IndividualTwtWakePlanError::BeforeTsfEpoch)?;
        if offset < duration {
            (current_start, true)
        } else {
            (
                current_start
                    .checked_add(interval)
                    .ok_or(IndividualTwtWakePlanError::BeyondTsfRange)?,
                false,
            )
        }
    } else {
        (target, false)
    };
    let service_end = service_start
        .checked_add(duration)
        .ok_or(IndividualTwtWakePlanError::BeyondTsfRange)?;
    let wake = if service_open {
        now
    } else {
        service_start
            .checked_sub(wake_guard)
            .ok_or(IndividualTwtWakePlanError::BeforeTsfEpoch)?
            .max(now)
    };
    Ok(IndividualTwtWakePlan {
        flow_bitmap: 1 << agreement.flow_id.get(),
        wake,
        service_start,
        service_end,
        service_open,
    })
}

const fn next_dialog_token(current: u8) -> u8 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

/// Public default stays disabled. It is a named value for compositions that
/// must explicitly prove hardware admission before enabling requester work.
pub const INDIVIDUAL_TWT_REQUESTER_DISABLED: Option<IndividualTwtRequesterConfig> = None;

/// Portable profile whose use requires an explicit backend admission proof.
/// Its existence does not advertise hardware TWT support.
pub const fn conservative_individual_twt_requester_config() -> IndividualTwtRequesterConfig {
    IndividualTwtRequesterConfig {
        response_timeout: Duration::from_secs(1),
        retry_interval: Duration::from_millis(250),
        setup_attempt_limit: 3,
        teardown_attempt_limit: 3,
    }
}

/// Helper for constructing a control field without importing the wire crate.
pub const fn individual_twt_control(
    responder_power_save: bool,
    information_frames_disabled: bool,
    wake_duration_unit: TwtWakeDurationUnit,
) -> IndividualTwtControl {
    IndividualTwtControl {
        responder_power_save,
        information_frames_disabled,
        wake_duration_unit,
    }
}

#[cfg(test)]
mod tests;
