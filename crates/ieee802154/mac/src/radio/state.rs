//! The single finite owner that admits commands and validates backend events.
//! Validation errors and admission results describe this owner without retaining frames.

use oer_ieee802154_port::CancelError;

use super::{
    RequestId,
    capabilities::RadioCapabilities,
    channel::Channel,
    command::{CommandKind, Configuration, InterfaceSetting, RadioCommand, TxMode, TxSecurity},
    event::{RadioEvent, TxStatus},
    interface::Interface,
    power::{TransmitPowerCapability, TransmitPowerError},
};

/// Stable state to which an asynchronous operation returns.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RestingState {
    /// Radio is enabled but not receiving.
    Sleeping,
    /// Radio is receiving on one channel.
    Receiving {
        /// Active receive channel.
        channel: Channel,
    },
    /// Radio sleeps until a scheduled receive window opens, then receives
    /// on one channel until the window ends.
    ScheduledReceiving {
        /// The window's correlation identifier.
        id: RequestId,
        /// Window receive channel.
        channel: Channel,
    },
}

/// Complete finite portable controller state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioState {
    /// Platform radio ownership is not acquired.
    Disabled,
    /// Enabled stable state.
    Resting(RestingState),
    /// One transmit request is owned by the backend.
    Transmitting {
        /// Active correlation identifier.
        id: RequestId,
        /// Transmit channel.
        channel: Channel,
        /// Stable state restored by terminal completion.
        resume: RestingState,
    },
    /// One energy scan is owned by the backend.
    EnergyScanning {
        /// Active correlation identifier.
        id: RequestId,
        /// Scan channel.
        channel: Channel,
        /// Stable state restored by terminal completion.
        resume: RestingState,
    },
    /// One standalone clear-channel assessment is owned by the backend.
    AssessingChannel {
        /// Active correlation identifier.
        id: RequestId,
        /// Assessed channel.
        channel: Channel,
        /// Stable state restored by terminal completion.
        resume: RestingState,
    },
}

/// Successful pure admission of one command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AcceptedCommand {
    /// Correlation identifier.
    pub id: RequestId,
    /// Admitted operation kind.
    pub kind: CommandKind,
    /// State before admission.
    pub previous: RadioState,
    /// State owned by the backend after admission.
    pub current: RadioState,
}

/// A command cannot be admitted in the current finite state/capability set.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandError {
    /// The quiesced port holds the radio still
    /// ([`LifecycleCommand::Quiesce`](crate::LifecycleCommand::Quiesce)):
    /// a command that writes the hardware waits for the port's `Enabled`.
    Quiesced,
    /// The backend's event queue has no free slot to reserve for the
    /// operation's terminal event; taking events frees one.
    EventQueueFull,
    /// The controller is disabled: only enabling it is accepted.
    Disabled,
    /// Enabling was requested for an already enabled controller.
    AlreadyEnabled,
    /// An asynchronous operation already owns the radio.
    Busy {
        /// Complete active state.
        state: RadioState,
    },
    /// The frame-pending table has no room for the source.
    PendingTableFull,
    /// The command names an interface the radio does not have.
    UnknownInterface {
        /// The rejected interface.
        interface: Interface,
        /// The radio's interface count.
        interfaces: u8,
    },
    /// The request's transmit power was refused.
    TransmitPower(TransmitPowerError),
    /// The controller did not publish the required capability.
    Unsupported {
        /// Rejected operation kind.
        command: CommandKind,
        /// Missing capability flag.
        required: RadioCapabilities,
    },
}

/// A backend event does not match the operation/state it claims to complete.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EventError {
    /// This event kind is invalid in the current state.
    Unexpected {
        /// Complete current state.
        state: RadioState,
    },
    /// A terminal event named a different operation.
    RequestMismatch {
        /// Expected active identifier.
        expected: RequestId,
        /// Identifier published by the backend.
        actual: RequestId,
    },
    /// Receive or acknowledgement metadata named a different channel.
    ChannelMismatch {
        /// Channel owned by the active state.
        expected: Channel,
        /// Channel published in metadata.
        actual: Channel,
    },
    /// A failed transmit must not carry a successful acknowledgement frame.
    AcknowledgementOnFailedTransmit,
    /// A fault identifier is inconsistent with the active operation.
    FaultRequestMismatch {
        /// Active identifier, if any.
        expected: Option<RequestId>,
        /// Identifier published by the fault event.
        actual: Option<RequestId>,
    },
}

/// Pure finite command/event admission state.
///
/// Fields are private so a caller cannot manufacture an active operation or
/// skip capability checks:
///
/// ```compile_fail
/// use oer_i::{RadioCapabilities, RadioState, RadioStateMachine};
/// let forged = RadioStateMachine {
///     state: RadioState::Disabled,
///     capabilities: RadioCapabilities::NONE,
///     interfaces: 1,
///     transmit_power: None,
/// };
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RadioStateMachine {
    state: RadioState,
    capabilities: RadioCapabilities,
    interfaces: u8,
    transmit_power: Option<TransmitPowerCapability>,
}

impl RadioStateMachine {
    /// Construct one disabled controller contract with the primary
    /// interface alone.
    pub const fn new(capabilities: RadioCapabilities) -> Self {
        Self::with_interfaces(capabilities, 1)
    }

    /// Construct one disabled controller contract with `interfaces`
    /// addressing interfaces. Without
    /// [`RadioCapabilities::MULTI_PAN`], and for a count of zero, the radio
    /// has the primary interface alone.
    pub const fn with_interfaces(capabilities: RadioCapabilities, interfaces: u8) -> Self {
        let interfaces = if interfaces == 0 || !capabilities.contains(RadioCapabilities::MULTI_PAN)
        {
            1
        } else {
            interfaces
        };
        Self {
            state: RadioState::Disabled,
            capabilities,
            interfaces,
            transmit_power: None,
        }
    }

    /// The same contract admitting a request's transmit power under
    /// `capability`; without one, a request that names a power is refused
    /// [`TransmitPowerError::Unsupported`].
    pub const fn with_transmit_power(mut self, capability: TransmitPowerCapability) -> Self {
        self.transmit_power = Some(capability);
        self
    }

    /// The transmit power a request may name.
    pub const fn transmit_power(&self) -> Option<TransmitPowerCapability> {
        self.transmit_power
    }

    /// Return the immutable backend capability set.
    pub const fn capabilities(&self) -> RadioCapabilities {
        self.capabilities
    }

    /// The number of addressing interfaces.
    pub const fn interfaces(&self) -> u8 {
        self.interfaces
    }

    fn require_interface(&self, interface: Interface) -> Result<(), CommandError> {
        if interface.index() < self.interfaces {
            Ok(())
        } else {
            Err(CommandError::UnknownInterface {
                interface,
                interfaces: self.interfaces,
            })
        }
    }

    /// Return the complete current state.
    pub const fn state(&self) -> RadioState {
        self.state
    }

    /// Acquire the radio: a disabled controller rests asleep.
    ///
    /// # Errors
    ///
    /// The controller is already enabled; nothing changed.
    pub fn enable(&mut self) -> Result<(), CommandError> {
        if self.state != RadioState::Disabled {
            return Err(CommandError::AlreadyEnabled);
        }
        self.state = RadioState::Resting(RestingState::Sleeping);
        Ok(())
    }

    /// Release a resting radio and return the state it left.
    ///
    /// # Errors
    ///
    /// The controller is disabled, or an operation owns the radio; nothing
    /// changed.
    pub fn disable(&mut self) -> Result<RadioState, CommandError> {
        let previous = self.state;
        match previous {
            RadioState::Disabled => Err(CommandError::Disabled),
            RadioState::Resting(_) => {
                self.state = RadioState::Disabled;
                Ok(previous)
            }
            _ => Err(CommandError::Busy { state: previous }),
        }
    }

    /// The operation [`Self::admit_cancel`] would end now: the active
    /// operation or the open scheduled receive window.
    pub const fn cancellable(&self) -> Option<RequestId> {
        cancellable_id(self.state)
    }

    /// Admit the cancellation of `target`, the running transmission, energy
    /// scan, clear-channel assessment or scheduled receive window. The state
    /// does not change: the backend stops the operation and its terminal
    /// event, observed as usual, ends it, after which the radio rests as the
    /// operation would have left it. A transmission ends with
    /// [`TxStatus::Aborted`] (or with the outcome
    /// the hardware had already reached), an energy scan with
    /// [`RadioEvent::EnergyScanFailed`], an assessment with
    /// [`RadioEvent::ClearChannelAssessmentFailed`] and a window with
    /// [`RadioEvent::ScheduledReceiveDone`].
    ///
    /// # Errors
    ///
    /// [`CancelError::NotRunning`] when no such operation runs: it already
    /// ended, or never ran.
    pub fn admit_cancel(&self, target: RequestId) -> Result<(), CancelError> {
        if cancellable_id(self.state) == Some(target) {
            Ok(())
        } else {
            Err(CancelError::NotRunning)
        }
    }

    /// Validate and admit one command, advancing state exactly once.
    ///
    /// A backend must retain any borrowed transmit bytes before this call
    /// returns. The state machine itself never retains those bytes.
    pub fn admit(&mut self, command: RadioCommand<'_>) -> Result<AcceptedCommand, CommandError> {
        let previous = self.state;
        let kind = command.kind();
        let id = command.id();
        let current = match command {
            _ if previous == RadioState::Disabled => return Err(CommandError::Disabled),
            RadioCommand::Sleep { .. } => match resting(previous) {
                Some(_) => RadioState::Resting(RestingState::Sleeping),
                None => return Err(CommandError::Busy { state: previous }),
            },
            RadioCommand::Receive { channel, .. } => match resting(previous) {
                Some(_) => RadioState::Resting(RestingState::Receiving { channel }),
                None => return Err(CommandError::Busy { state: previous }),
            },
            RadioCommand::ScheduledReceive(request) => {
                let Some(_) = resting(previous) else {
                    return Err(CommandError::Busy { state: previous });
                };
                require_capability(
                    self.capabilities,
                    kind,
                    RadioCapabilities::SCHEDULED_RECEIVE,
                )?;
                RadioState::Resting(RestingState::ScheduledReceiving {
                    id,
                    channel: request.channel,
                })
            }
            RadioCommand::Configure { configuration, .. } => {
                let Some(_) = resting(previous) else {
                    return Err(CommandError::Busy { state: previous });
                };
                if !self.capabilities.supports_configuration(configuration) {
                    let required = required_configuration_capability(configuration);
                    return Err(CommandError::Unsupported {
                        command: kind,
                        required,
                    });
                }
                if let Configuration::Interface { interface, .. } = configuration {
                    self.require_interface(interface)?;
                }
                previous
            }
            RadioCommand::Transmit(request) => {
                let Some(resume) = resumable(previous) else {
                    return Err(CommandError::Busy { state: previous });
                };
                if !self.capabilities.supports_tx_mode(request.mode) {
                    return Err(CommandError::Unsupported {
                        command: kind,
                        required: required_tx_capability(request.mode),
                    });
                }
                if request.frame.acknowledgement_requested()
                    && !self
                        .capabilities
                        .contains(RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT)
                {
                    return Err(CommandError::Unsupported {
                        command: kind,
                        required: RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT,
                    });
                }
                if request.max_frame_retries > 0
                    && !self
                        .capabilities
                        .contains(RadioCapabilities::TRANSMIT_RETRIES)
                {
                    return Err(CommandError::Unsupported {
                        command: kind,
                        required: RadioCapabilities::TRANSMIT_RETRIES,
                    });
                }
                self.require_interface(request.interface)?;
                if request.time_sync.is_some()
                    && !self.capabilities.contains(RadioCapabilities::TIME_SYNC)
                {
                    return Err(CommandError::Unsupported {
                        command: kind,
                        required: RadioCapabilities::TIME_SYNC,
                    });
                }
                if let Some(power) = request.transmit_power_dbm
                    && let Err(error) = TransmitPowerCapability::check(self.transmit_power, power)
                {
                    return Err(CommandError::TransmitPower(error));
                }
                if matches!(request.security, TxSecurity::Key { .. }) {
                    require_capability(
                        self.capabilities,
                        kind,
                        RadioCapabilities::SECURITY_OFFLOAD,
                    )?;
                }
                RadioState::Transmitting {
                    id,
                    channel: request.channel,
                    resume,
                }
            }
            RadioCommand::EnergyScan(request) => {
                let Some(resume) = resumable(previous) else {
                    return Err(CommandError::Busy { state: previous });
                };
                require_capability(self.capabilities, kind, RadioCapabilities::ENERGY_SCAN)?;
                RadioState::EnergyScanning {
                    id,
                    channel: request.channel,
                    resume,
                }
            }
            RadioCommand::ClearChannelAssessment { channel, .. } => {
                let Some(resume) = resumable(previous) else {
                    return Err(CommandError::Busy { state: previous });
                };
                require_capability(
                    self.capabilities,
                    kind,
                    RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT,
                )?;
                RadioState::AssessingChannel {
                    id,
                    channel,
                    resume,
                }
            }
        };

        self.state = current;
        Ok(AcceptedCommand {
            id,
            kind,
            previous,
            current,
        })
    }

    /// Validate one backend event and apply its exact state transition.
    pub fn observe(&mut self, event: RadioEvent<'_>) -> Result<(), EventError> {
        let next = match (self.state, event) {
            (
                RadioState::Resting(
                    RestingState::Receiving { channel }
                    | RestingState::ScheduledReceiving { channel, .. },
                ),
                RadioEvent::Received(rx),
            ) => {
                require_channel(channel, rx.metadata.channel)?;
                self.state
            }
            (
                RadioState::Resting(RestingState::ScheduledReceiving { id: expected, .. }),
                RadioEvent::ScheduledReceiveDone { id },
            ) => {
                require_id(expected, id)?;
                RadioState::Resting(RestingState::Sleeping)
            }
            // A transmission from receive mode keeps receiving on its own
            // channel while it waits for the channel, as a CSMA-CA backoff
            // with receive-on-when-idle does.
            (
                RadioState::Transmitting {
                    channel,
                    resume: RestingState::Receiving { .. },
                    ..
                },
                RadioEvent::Received(rx),
            ) => {
                require_channel(channel, rx.metadata.channel)?;
                self.state
            }
            (
                RadioState::Transmitting {
                    id: expected,
                    channel,
                    resume,
                },
                RadioEvent::TransmitDone {
                    id,
                    status,
                    acknowledgement,
                    ..
                },
            ) => {
                require_id(expected, id)?;
                if status != TxStatus::Success && acknowledgement.is_some() {
                    return Err(EventError::AcknowledgementOnFailedTransmit);
                }
                if let Some(acknowledgement) = acknowledgement {
                    require_channel(channel, acknowledgement.metadata.channel)?;
                }
                RadioState::Resting(resume)
            }
            (
                RadioState::EnergyScanning {
                    id: expected,
                    resume,
                    ..
                },
                RadioEvent::EnergyScanDone { id, .. } | RadioEvent::EnergyScanFailed { id },
            ) => {
                require_id(expected, id)?;
                RadioState::Resting(resume)
            }
            (
                RadioState::AssessingChannel {
                    id: expected,
                    resume,
                    ..
                },
                RadioEvent::ClearChannelAssessmentDone { id, .. }
                | RadioEvent::ClearChannelAssessmentFailed { id },
            ) => {
                require_id(expected, id)?;
                RadioState::Resting(resume)
            }
            (state, RadioEvent::Fault { id, .. }) => {
                let expected = active_id(state);
                if id != expected {
                    return Err(EventError::FaultRequestMismatch {
                        expected,
                        actual: id,
                    });
                }
                RadioState::Disabled
            }
            // The terminal of a lifecycle command the state already took.
            (state, RadioEvent::Lifecycle(_)) => state,
            (state, _) => return Err(EventError::Unexpected { state }),
        };
        self.state = next;
        Ok(())
    }
}

const fn resting(state: RadioState) -> Option<RestingState> {
    if let RadioState::Resting(resting) = state {
        Some(resting)
    } else {
        None
    }
}

/// The resting state an operation started from `state` returns to: an
/// operation ends a scheduled receive window, and the radio sleeps after it
/// as ESP-IDF's `next_operation` does without receive-on-when-idle.
const fn resumable(state: RadioState) -> Option<RestingState> {
    match resting(state) {
        Some(RestingState::ScheduledReceiving { .. }) => Some(RestingState::Sleeping),
        other => other,
    }
}

const fn active_id(state: RadioState) -> Option<RequestId> {
    match state {
        RadioState::Transmitting { id, .. }
        | RadioState::EnergyScanning { id, .. }
        | RadioState::AssessingChannel { id, .. } => Some(id),
        RadioState::Disabled | RadioState::Resting(_) => None,
    }
}

/// The operation a cancellation may end in `state`: the active operation or
/// the open scheduled receive window.
const fn cancellable_id(state: RadioState) -> Option<RequestId> {
    match state {
        RadioState::Resting(RestingState::ScheduledReceiving { id, .. }) => Some(id),
        state => active_id(state),
    }
}

const fn required_tx_capability(mode: TxMode) -> RadioCapabilities {
    match mode {
        TxMode::Direct => RadioCapabilities::NONE,
        TxMode::ClearChannelAssessment => RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT,
        TxMode::CsmaCa { .. } => RadioCapabilities::CSMA_CA,
        TxMode::Scheduled { cca: false, .. } => RadioCapabilities::SCHEDULED_TRANSMIT,
        TxMode::Scheduled { cca: true, .. } => {
            RadioCapabilities::SCHEDULED_TRANSMIT.union(RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT)
        }
    }
}

const fn required_configuration_capability(configuration: Configuration) -> RadioCapabilities {
    match configuration {
        Configuration::Promiscuous(_) => RadioCapabilities::PROMISCUOUS,
        Configuration::AutomaticAcknowledgement(_) => RadioCapabilities::AUTOMATIC_ACKNOWLEDGEMENT,
        Configuration::PendingMode(_)
        | Configuration::AddPendingAddress(_)
        | Configuration::RemovePendingAddress(_)
        | Configuration::ResetPendingTable(_) => RadioCapabilities::SOURCE_MATCH,
        Configuration::Interface { setting, .. } => {
            RadioCapabilities::MULTI_PAN.union(match setting {
                InterfaceSetting::PendingMode(_)
                | InterfaceSetting::AddPendingAddress(_)
                | InterfaceSetting::RemovePendingAddress(_)
                | InterfaceSetting::ResetPendingTable(_) => RadioCapabilities::SOURCE_MATCH,
                InterfaceSetting::PanId(_)
                | InterfaceSetting::ShortAddress(_)
                | InterfaceSetting::ExtendedAddress(_)
                | InterfaceSetting::Enabled(_) => RadioCapabilities::NONE,
            })
        }
        Configuration::PanId(_)
        | Configuration::ShortAddress(_)
        | Configuration::ExtendedAddress(_)
        | Configuration::PanCoordinator(_) => RadioCapabilities::NONE,
    }
}

fn require_capability(
    capabilities: RadioCapabilities,
    command: CommandKind,
    required: RadioCapabilities,
) -> Result<(), CommandError> {
    if capabilities.contains(required) {
        Ok(())
    } else {
        Err(CommandError::Unsupported { command, required })
    }
}

fn require_id(expected: RequestId, actual: RequestId) -> Result<(), EventError> {
    if expected == actual {
        Ok(())
    } else {
        Err(EventError::RequestMismatch { expected, actual })
    }
}

fn require_channel(expected: Channel, actual: Channel) -> Result<(), EventError> {
    if expected == actual {
        Ok(())
    } else {
        Err(EventError::ChannelMismatch { expected, actual })
    }
}

#[cfg(test)]
mod tests;
