//! Bluetooth's shared-domain steps, serialized with Wi-Fi by the one actor.

use oer_esp32s31_bluetooth_driver::radio::RadioController;
use oer_esp32s31_hal::{
    bluetooth::{ClockedOwner, ColdOwner, PoweredOwner},
    domain::{BluetoothClient, CoexAbsent, Coexisting, CommonPower, PlatformClockProvider},
};
use oer_esp32s31_phy::{
    PhyTargetObserver,
    bluetooth_client::{
        BluetoothPhyMembership, bluetooth_rf_ready, join_bluetooth, leave_bluetooth,
    },
    concurrent::{PhyClientAccess, phy_ready},
};
use oer_esp32s31_radio_client::{
    DomainHealth, Failed, Failure, LpClock, PhyPreparation, RcSlow,
    bluetooth::{BluetoothJoin, BluetoothJoined, BluetoothLeft, Installation, Permission},
};

use super::Esp32s31Radio;
use crate::{Exit, PhyDomain};

/// A Bluetooth installation attached to the existing shared backend.
/// Wi-Fi-only compositions do not contain this state or link its code.
pub struct BluetoothAttachment<H> {
    controller: H,
    cold: Option<ColdOwner>,
    powered: Option<PoweredOwner>,
    clocked: Option<ClockedOwner>,
    coex: Option<Coex>,
    power: Option<CommonPower<BluetoothClient>>,
    membership: Option<BluetoothPhyMembership>,
    installation: Option<Installation>,
    configuration: Option<BluetoothJoin>,
}

enum Coex {
    Absent(CoexAbsent<BluetoothClient>),
    Present(Coexisting<BluetoothClient>),
}

impl<H> BluetoothAttachment<H> {
    pub fn new(cold: ColdOwner, coex: CoexAbsent<BluetoothClient>, controller: H) -> Self {
        Self {
            controller,
            cold: Some(cold),
            powered: None,
            clocked: None,
            coex: Some(Coex::Absent(coex)),
            power: None,
            membership: None,
            installation: None,
            configuration: None,
        }
    }

    /// Only between actor run borrows: hand over runtime or park its stopped
    /// owner. This exposes no shared-domain registers or PHY membership.
    pub fn controller_mut(&mut self) -> &mut H {
        &mut self.controller
    }

    /// Consume a cold attachment only after normal actor leave/reset.
    #[allow(clippy::result_large_err)]
    pub fn into_stopped(mut self) -> Result<(ColdOwner, CoexAbsent<BluetoothClient>, H), Self> {
        if !self.stopped() {
            return Err(self);
        }
        let cold = self.cold.take().expect("checked stopped partition");
        let Some(Coex::Absent(absent)) = self.coex.take() else {
            unreachable!("checked stopped coexistence")
        };
        Ok((cold, absent, self.controller))
    }
    fn stopped(&self) -> bool {
        self.installation.is_none()
            && self.membership.is_none()
            && self.power.is_none()
            && self.cold.is_some()
            && matches!(self.coex, Some(Coex::Absent(_)))
    }

    /// Initial operation and its already rounded provider power.
    pub const fn configuration(&self) -> Option<BluetoothJoin> {
        self.configuration
    }

    pub const fn permission(&self) -> Option<Permission> {
        match self.configuration {
            Some(configuration) => Some(configuration.permission()),
            None => None,
        }
    }
}

impl<C, T, O, H> crate::BluetoothBackend<Esp32s31Radio<C, T, O>, RcSlow> for BluetoothAttachment<H>
where
    C: PlatformClockProvider,
    T: oer_cooperate::CooperativeTimer,
    O: PhyTargetObserver + Clone,
    H: RadioController<C::Backend>,
{
    fn attached(&mut self, shared: &mut Esp32s31Radio<C, T, O>) {
        shared.bluetooth_attached = true;
    }
    fn detached(&mut self, shared: &mut Esp32s31Radio<C, T, O>) {
        shared.bluetooth_attached = false;
    }
    fn is_stopped(&self, _: &Esp32s31Radio<C, T, O>) -> bool {
        self.stopped()
    }
    fn admits(&self, _shared: &Esp32s31Radio<C, T, O>, _join: &BluetoothJoin) -> bool {
        self.cold.is_some() && self.controller.can_prepare() && self.membership.is_none()
    }

    async fn enter(
        &mut self,
        shared: &mut Esp32s31Radio<C, T, O>,
        join: BluetoothJoin,
        phy: PhyPreparation,
        _lp: &LpClock<RcSlow>,
    ) -> Result<BluetoothJoined, Failed<BluetoothJoin>> {
        let failed = || Failed {
            returned: join,
            cause: Failure::Poisoned,
        };
        crate::boundary::shared(shared.track_tick())
            .await
            .map_err(|_| failed())?;
        let cold = self.cold.take().ok_or_else(failed)?;
        let (powered, power) = match cold.power_up(&mut shared.radio, &shared.clocks) {
            Ok(owners) => owners,
            Err(error) => {
                self.cold = Some(error.into_owner());
                return Err(failed());
            }
        };
        self.power = Some(power);
        let clocked = match powered.enable_clocks(&mut shared.radio, &shared.clocks) {
            Ok(clocked) => clocked,
            Err(error) => {
                self.powered = Some(error.into_owner());
                return Err(failed());
            }
        };
        // All later errors retain common power and every controller owner.
        let installation = match self.controller.prepare(clocked) {
            Ok(installation) => Installation(installation),
            Err(error) => {
                self.clocked = error.returned;
                return Err(failed());
            }
        };
        self.installation = Some(installation);
        let coex = self.coex.take().ok_or_else(failed)?;
        let Coex::Absent(absent) = coex else {
            self.coex = Some(coex);
            return Err(failed());
        };
        let (present, _) = shared.radio.enter_coex(absent);
        self.coex = Some(Coex::Present(present));
        let task = self.controller.task_owner().ok_or_else(failed)?;
        let ready = phy_ready(&mut shared.radio).map_err(|_| failed())?;
        let PhyClientAccess::Available(ready) = ready.client::<BluetoothPhyMembership>() else {
            return Err(failed());
        };
        let joined = join_bluetooth(ready, task, &shared.timer).map_err(|_| failed())?;
        self.membership = Some(joined.membership);
        self.configuration = Some(join);
        Ok(BluetoothJoined { installation, phy })
    }

    async fn track(
        &mut self,
        shared: &mut Esp32s31Radio<C, T, O>,
        installation: Installation,
    ) -> Result<Installation, Failed<Installation>> {
        let failed = || Failed {
            returned: installation,
            cause: Failure::Poisoned,
        };
        if self.installation != Some(installation) {
            return Err(failed());
        }
        // Re-evaluate due tracking, including work already serviced for Wi-Fi.
        crate::boundary::shared(shared.track_tick())
            .await
            .map_err(|_| failed())?;
        let member = self.membership.as_ref().ok_or_else(failed)?;
        let ready = bluetooth_rf_ready(&mut shared.radio, member).map_err(|_| failed())?;
        self.controller
            .initialize_phy(ready)
            .map_err(|_| failed())?;
        Ok(installation)
    }

    async fn exit(
        &mut self,
        shared: &mut Esp32s31Radio<C, T, O>,
        installation: Installation,
    ) -> (BluetoothLeft, Exit) {
        let poisoned = || {
            (
                BluetoothLeft { stopped: false },
                Exit {
                    last: false,
                    health: DomainHealth::Poisoned,
                },
            )
        };
        if self.installation != Some(installation) || !self.controller.is_retired(installation.0) {
            return poisoned();
        }
        if crate::boundary::shared(shared.track_tick()).await.is_err() {
            return poisoned();
        }
        let Some(task) = self.controller.task_owner() else {
            return poisoned();
        };
        let Some(member) = self.membership.take() else {
            return poisoned();
        };
        let last = match leave_bluetooth(&mut shared.radio, task, member) {
            Ok(last) => last,
            Err(error) => {
                self.membership = Some(error.into_membership());
                return poisoned();
            }
        };
        let clocked = match self.controller.reset(&mut shared.radio) {
            Ok(clocked) => clocked,
            Err(_) => return poisoned(),
        };
        let powered = match clocked.disable_clocks(&mut shared.radio) {
            Ok(powered) => powered,
            Err(error) => {
                self.clocked = Some(error.into_owner());
                return poisoned();
            }
        };
        let Some(Coex::Present(present)) = self.coex.take() else {
            self.powered = Some(powered);
            return poisoned();
        };
        let (absent, _) = shared.radio.leave_coex(present);
        let Some(power) = self.power.take() else {
            self.coex = Some(Coex::Absent(absent));
            self.powered = Some(powered);
            return poisoned();
        };
        match powered.power_down(&mut shared.radio, power, &absent) {
            Ok(cold) => {
                self.cold = Some(cold);
                self.coex = Some(Coex::Absent(absent));
                self.installation = None;
                self.configuration = None;
                (
                    BluetoothLeft { stopped: true },
                    Exit {
                        last,
                        health: DomainHealth::Healthy,
                    },
                )
            }
            Err(error) => {
                let (powered, power) = error.into_owner();
                self.powered = Some(powered);
                self.power = Some(power);
                self.coex = Some(Coex::Absent(absent));
                poisoned()
            }
        }
    }
}
