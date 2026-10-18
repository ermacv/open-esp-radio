//! Wire adaptation only. Production PHY owns checkpoints, composition owns
//! the deadline, and no diagnostic command can renew that deadline.
use oer_hil_protocol::{base::RejectReason, phy::PhyFaultCommand};

pub(super) fn control(
    command: PhyFaultCommand,
) -> Result<oer_hil_protocol::phy::FaultState, RejectReason> {
    #[cfg(feature = "phy-fault-injection")]
    {
        use oer_esp32s31_phy::fault_injection::{self as fault, Mode, Phase};
        use oer_hil_protocol::{
            phy::PhyFaultEvidence, phy::PhyFaultMode as WireMode, phy::PhyFaultPhase as WirePhase,
        };
        let accepted = match command {
            PhyFaultCommand::Arm(mode) => fault::arm(match mode {
                WireMode::BlockedPoll => Mode::BlockedPoll,
                WireMode::LostCompletion => Mode::LostCompletion,
                WireMode::Restoration => Mode::Restoration,
                WireMode::Cancelled => Mode::Cancelled,
            }),
            PhyFaultCommand::Status => true,
            // The transport commits release only after this acknowledgement
            // has actually been serialized, before the CPU can block.
            PhyFaultCommand::Release => fault::phase() == Phase::Reached,
        };
        if !accepted {
            return Err(RejectReason::InvalidState);
        }
        Ok(oer_hil_protocol::phy::FaultState(PhyFaultEvidence {
            phase: match if command == PhyFaultCommand::Release {
                Phase::Released
            } else {
                fault::phase()
            } {
                Phase::Idle => WirePhase::Idle,
                Phase::Armed => WirePhase::Armed,
                Phase::Reached => WirePhase::Reached,
                Phase::Released => WirePhase::Released,
                Phase::Cancelled => WirePhase::Cancelled,
            },
            reset_reason: crate::system::boot_evidence().reset_reason,
        }))
    }
    #[cfg(not(feature = "phy-fault-injection"))]
    {
        let _ = command;
        Err(RejectReason::InvalidState)
    }
}

pub(super) fn after_response(command: PhyFaultCommand, accepted: bool) {
    #[cfg(feature = "phy-fault-injection")]
    if accepted && command == PhyFaultCommand::Release {
        assert!(oer_esp32s31_phy::fault_injection::release());
    }
    #[cfg(not(feature = "phy-fault-injection"))]
    let _ = (command, accepted);
}
