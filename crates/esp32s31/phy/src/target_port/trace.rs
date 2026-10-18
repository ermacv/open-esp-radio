//! Target failures as [`oer_phy_trace`] faults, and the poison snapshot.

use oer_phy_trace::{Fault, FaultStage, PoisonedBy};

use super::{TargetPhyParamTrackingError, TargetPhyRegisterError};
use crate::{
    PhyParamTrackingRunError, PhyRegisterRunError, PhyState, PhyTargetPortError,
    state::client::PhyClientSnapshot,
};

const fn fault(stage: FaultStage, detail: u16) -> Fault {
    Fault { stage, detail }
}

/// A target-port failure; `detail` is the variant.
pub(crate) const fn port_fault(error: PhyTargetPortError) -> Fault {
    match error {
        PhyTargetPortError::HardwareEdgeTimedOut => fault(FaultStage::Hardware, 0),
        PhyTargetPortError::HardwareCapabilityUnavailable => fault(FaultStage::Hardware, 1),
        PhyTargetPortError::HardwareInvariant => fault(FaultStage::Hardware, 2),
        PhyTargetPortError::RfOperationLimit => fault(FaultStage::Transition, 3),
        PhyTargetPortError::UnexpectedBinding => fault(FaultStage::Transition, 4),
        PhyTargetPortError::RegistrationEpochMismatch => fault(FaultStage::EpochMismatch, 5),
    }
}

/// A tracking failure; port failures keep their port detail, the others
/// are numbered from 0x100.
pub(crate) const fn tracking_fault(error: &TargetPhyParamTrackingError) -> Fault {
    match error {
        TargetPhyParamTrackingError::EpochMismatch => fault(FaultStage::EpochMismatch, 0x100),
        TargetPhyParamTrackingError::Run(PhyParamTrackingRunError::Port(error)) => {
            port_fault(*error)
        }
        TargetPhyParamTrackingError::Run(PhyParamTrackingRunError::Transition(_)) => {
            fault(FaultStage::Transition, 0x101)
        }
        TargetPhyParamTrackingError::Run(PhyParamTrackingRunError::ParentEdgeLimit) => {
            fault(FaultStage::Transition, 0x102)
        }
        TargetPhyParamTrackingError::MissingCompletedOwner => {
            fault(FaultStage::MissingOwner, 0x300)
        }
    }
}

/// A registration failure; port failures keep their port detail, the
/// others are numbered from 0x400.
pub(crate) const fn register_fault(error: &TargetPhyRegisterError) -> Fault {
    match error {
        TargetPhyRegisterError::Run(PhyRegisterRunError::Port(error)) => port_fault(*error),
        TargetPhyRegisterError::Run(PhyRegisterRunError::Lowering { .. }) => {
            fault(FaultStage::Transition, 0x400)
        }
        TargetPhyRegisterError::Run(PhyRegisterRunError::Transition(_)) => {
            fault(FaultStage::Transition, 0x401)
        }
        TargetPhyRegisterError::Run(PhyRegisterRunError::Radio(_)) => {
            fault(FaultStage::Hardware, 0x402)
        }
        TargetPhyRegisterError::MissingCompletedModelOwner => {
            fault(FaultStage::MissingOwner, 0x403)
        }
    }
}

/// Record a poison with its snapshot. The PHY clock and power domain must
/// be on: the snapshot samples the PBus and analog-I2C host state. It takes
/// no lock and does not wait.
#[cfg_attr(not(feature = "trace"), allow(unused_variables))]
pub(crate) fn record_poison(
    by: PoisonedBy,
    fault: Fault,
    slot: oer_phy_trace::Slot,
    clients: PhyClientSnapshot,
    state: &PhyState,
    platform_clocks: oer_esp32s31_hal::power::PlatformClockHolds,
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
) {
    #[cfg(feature = "trace")]
    {
        let bus = oer_esp32s31_hal::phy::pbus::observe_bus(registers);
        oer_phy_trace::record_poison(&oer_phy_trace::PhySnapshot {
            poison: oer_phy_trace::Poison { by, fault },
            slot,
            clients: crate::trace::clients(clients),
            temperatures: crate::trace::temperatures(state),
            bus: oer_phy_trace::BusRead::Read(oer_phy_trace::BusState {
                pbus_busy: bus.pbus_busy,
                analog_i2c_busy: bus.analog_i2c_busy,
                pbus_results: bus.pbus_results,
            }),
            platform_clocks: crate::trace::platform_clocks(platform_clocks),
        });
    }
}
