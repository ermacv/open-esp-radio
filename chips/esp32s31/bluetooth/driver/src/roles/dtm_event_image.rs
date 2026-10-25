//! DTM role, PHY and event-phase policy. Physical words remain in HAL.
use oer_esp32s31_hal::bluetooth::engine::{
    DtmItemConfiguration, DtmReceiveWait, SchedulerEventPriority,
};

/// DTM role shared by the CPU-owned link-state and scheduler-item formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRole {
    /// Repeated transmitter-test events use role image one.
    Transmitter,
    /// Repeated receiver-test events use role image two.
    Receiver,
}

/// Phase selecting the role-private receiver configuration for one DTM event.
///
/// The initial event establishes the complete receiver configuration. After
/// one completion has returned the private graph, the recurring vendor path
/// deliberately replaces it with the narrower re-use configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmReceiverEventPhase {
    /// First receiver event in one active test session.
    Initial,
    /// Receiver event prepared from a recycled active-session graph.
    Recurring,
}

/// PHY choice admitted by a transmitter DTM scheduler item.
///
/// These variants retain the role-specific meaning established by the DTM
/// command validator. They deliberately do not expose the controller field
/// image used by the private SRAM codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmSchedulerTransmitterPhy {
    /// LE 1M transmitter.
    Le1M,
    /// LE 2M transmitter.
    Le2M,
    /// LE Coded transmitter using S=8 coding.
    LeCodedS8,
    /// LE Coded transmitter using S=2 coding.
    LeCodedS2,
}

/// PHY choice admitted by a receiver DTM scheduler item.
///
/// The receiver command has no S=2 selector. Encoding that restriction in the
/// type prevents a transmitter-only choice from reaching the SRAM codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmSchedulerReceiverPhy {
    /// LE 1M receiver.
    Le1M,
    /// LE 2M receiver.
    Le2M,
    /// Generic LE Coded receiver.
    LeCoded,
}

/// Role and phase selecting one reviewed scheduler-item event transform.
///
/// Transmitter events have no phase-dependent scheduler-item configuration.
/// Nesting the receiver phase in the receiver variant prevents a caller from
/// attaching receiver-only phase semantics to a transmitter descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmSchedulerItemEventType {
    /// One transmitter-test scheduler item.
    Transmitter(DtmSchedulerTransmitterPhy),
    /// One receiver-test scheduler item in its explicit session phase.
    Receiver {
        /// Initial or recurring receiver configuration.
        phase: DtmReceiverEventPhase,
        /// Receiver-valid PHY retained without exposing its field image.
        phy: DtmSchedulerReceiverPhy,
    },
}

impl DtmSchedulerItemEventType {
    /// Return the DTM role selected by this event type.
    pub const fn role(self) -> DtmRole {
        match self {
            Self::Transmitter(_) => DtmRole::Transmitter,
            Self::Receiver { .. } => DtmRole::Receiver,
        }
    }

    /// Return the receiver phase, or `None` for a transmitter event.
    pub const fn receiver_phase(self) -> Option<DtmReceiverEventPhase> {
        match self {
            Self::Transmitter(_) => None,
            Self::Receiver { phase, .. } => Some(phase),
        }
    }

    pub(super) const fn configuration(self) -> DtmItemConfiguration {
        let (transmitter, code, wait) = match self {
            Self::Transmitter(phy) => (
                true,
                match phy {
                    DtmSchedulerTransmitterPhy::Le1M => 0,
                    DtmSchedulerTransmitterPhy::Le2M => 1,
                    DtmSchedulerTransmitterPhy::LeCodedS8 => 2,
                    DtmSchedulerTransmitterPhy::LeCodedS2 => 3,
                },
                None,
            ),
            Self::Receiver { phy, phase } => (
                false,
                match phy {
                    DtmSchedulerReceiverPhy::Le1M => 0,
                    DtmSchedulerReceiverPhy::Le2M => 1,
                    DtmSchedulerReceiverPhy::LeCoded => 3,
                },
                Some(match phase {
                    // SOURCE: pinned dtm_4.o initial and recurring receiver bodies.
                    DtmReceiverEventPhase::Initial => {
                        const { DtmReceiveWait::new(1, 15).expect("reviewed timing fits") }
                    }
                    DtmReceiverEventPhase::Recurring => {
                        const { DtmReceiveWait::new(1, 0).expect("reviewed timing fits") }
                    }
                }),
            ),
        };
        DtmItemConfiguration::new(
            transmitter,
            code,
            wait,
            const { SchedulerEventPriority::new(3).expect("reviewed priority fits") },
        )
        .expect("selected PHY fits")
    }
}
