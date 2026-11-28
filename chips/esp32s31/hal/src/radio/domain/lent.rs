//! The PHY's registers and books, lent from the radio domain for the length
//! of one operation. A loan is a borrow: letting it go gives nothing back,
//! because nothing left the domain.
use super::*;
use crate::radio::coex::{PHY_GRANT_PROTECT_EVENT, PhyGrantProtect};

/// The PHY's registers and its route state (registration epoch and
/// calibration restore slots), lent from the domain.
///
/// ```compile_fail,E0499
/// use oer_chips_esp32s31_hal::{power::PlatformClockBackend, radio::domain::RadioDomain};
/// fn kept<B: PlatformClockBackend>(radio: &mut RadioDomain<B>) {
///     let phy = radio.phy_hardware();
///     let _also = radio.phy_hardware();
///     drop(phy);
/// }
/// ```
pub struct PhyHardware<'a> {
    pub(crate) registers: &'a mut crate::radio::phy::RadioPhyRegisters,
    pub(crate) restore: &'a mut PhyRouteState,
}

impl PhyHardware<'_> {
    /// The same loan, for a shorter borrow.
    pub fn reborrow(&mut self) -> PhyHardware<'_> {
        PhyHardware {
            registers: self.registers,
            restore: self.restore,
        }
    }
}

/// The PHY's registers with the coexistence grant-protect request, lent
/// from the domain.
pub struct PhyLoan<'a> {
    hardware: PhyHardware<'a>,
    grant_timer: &'a mut oer_chips_esp32s31_pac::coex::PhyGrantProtectTimer,
    grant_protected: &'a mut bool,
}

impl<'a> PhyLoan<'a> {
    /// The registers alone.
    pub fn hardware_mut(&mut self) -> &mut PhyHardware<'a> {
        &mut self.hardware
    }

    /// The registers beside the grant-protect request.
    pub fn hardware_and_grant(&mut self) -> (&mut PhyHardware<'a>, PhyGrantProtect<'_>) {
        let pti = CoexPtiTable::VENDOR.pti(PHY_GRANT_PROTECT_EVENT);
        (
            &mut self.hardware,
            PhyGrantProtect::new(
                crate::radio::modem::CoexGrantTimerRegisters::new(self.grant_timer),
                pti,
                self.grant_protected,
            ),
        )
    }
}

impl<B: PlatformClockBackend> RadioDomain<B> {
    /// Lend the PHY's registers and route state.
    pub fn phy_hardware(&mut self) -> PhyHardware<'_> {
        let state = &mut self.state;
        PhyHardware {
            registers: state.registers.radio_phy_mut(),
            restore: &mut state.phy,
        }
    }

    /// Lend the PHY's registers with the grant-protect request.
    pub fn phy_loan(&mut self) -> PhyLoan<'_> {
        let SharedRadioState {
            registers,
            phy,
            phy_grant_protected,
            ..
        } = &mut self.state;
        PhyLoan {
            hardware: PhyHardware {
                registers: &mut registers.radio_phy,
                restore: phy,
            },
            grant_timer: &mut registers.coexistence.phy_grant_protect_timer,
            grant_protected: phy_grant_protected,
        }
    }
}
