//! Bluetooth's shared-domain steps, serialized with Wi-Fi by the one actor.

use core::convert::Infallible;

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
    bluetooth::{
        Bluetooth, BluetoothJoin, BluetoothJoined, BluetoothLeft, Installation, Permission,
    },
    wifi::Wifi,
};

use super::Esp32s31Radio;
use crate::{ClientBackend, CloseFault, Exit, Fault, LastClientLeft, PhyDomain};

/// A Bluetooth installation attached to the existing shared backend.
/// Wi-Fi-only compositions do not contain this state or link its code.
pub struct WithBluetooth<C: PlatformClockProvider, T, O, H> {
    shared: Esp32s31Radio<C, T, O>,
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

impl<C: PlatformClockProvider, T, O, H> WithBluetooth<C, T, O, H> {
    pub fn new(
        shared: Esp32s31Radio<C, T, O>,
        cold: ColdOwner,
        coex: CoexAbsent<BluetoothClient>,
        controller: H,
    ) -> Self {
        Self {
            shared,
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

    /// Detach a stopped installation without replacing the common RF owner.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn into_stopped(
        mut self,
    ) -> Result<
        (
            Esp32s31Radio<C, T, O>,
            ColdOwner,
            CoexAbsent<BluetoothClient>,
            H,
        ),
        Self,
    > {
        if self.installation.is_some()
            || self.membership.is_some()
            || self.power.is_some()
            || self.cold.is_none()
            || !matches!(self.coex, Some(Coex::Absent(_)))
        {
            return Err(self);
        }
        let cold = self.cold.take().expect("checked stopped partition");
        let Some(Coex::Absent(absent)) = self.coex.take() else {
            unreachable!("checked stopped coexistence")
        };
        Ok((self.shared, cold, absent, self.controller))
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

impl<C, T, O, H> PhyDomain for WithBluetooth<C, T, O, H>
where
    C: PlatformClockProvider,
    T: oer_time::Timer,
    O: PhyTargetObserver + Clone,
{
    async fn prepare(&mut self) -> Result<PhyPreparation, Fault> {
        self.shared.prepare().await
    }
    async fn close_rf(&mut self, last: LastClientLeft) -> Result<(), CloseFault> {
        self.shared.close_rf(last).await
    }
    fn tracking_deadline(&self) -> Option<oer_time::Instant> {
        self.shared.tracking_deadline()
    }
    async fn track_tick(&mut self) -> Result<(), Fault> {
        self.shared.track_tick().await?;
        self.shared.wifi.tracking_due = false;
        Ok(())
    }
}

impl<C, T, O, H> ClientBackend<Wifi, RcSlow> for WithBluetooth<C, T, O, H>
where
    C: PlatformClockProvider,
    T: oer_time::Timer,
    O: PhyTargetObserver + Clone,
{
    async fn enter(
        &mut self,
        join: <Wifi as oer_esp32s31_radio_client::Client>::Join,
        phy: PhyPreparation,
        lp: &LpClock<RcSlow>,
    ) -> Result<
        <Wifi as oer_esp32s31_radio_client::Client>::Joined,
        Failed<<Wifi as oer_esp32s31_radio_client::Client>::JoinReturned>,
    > {
        if self.track_tick().await.is_err() {
            return Err(Failed {
                returned: <Wifi as oer_esp32s31_radio_client::Client>::join_returned(join),
                cause: Failure::Poisoned,
            });
        }
        self.shared.enter(join, phy, lp).await
    }
    async fn track(
        &mut self,
        track: <Wifi as oer_esp32s31_radio_client::Client>::Track,
    ) -> Result<
        <Wifi as oer_esp32s31_radio_client::Client>::Tracked,
        Failed<<Wifi as oer_esp32s31_radio_client::Client>::TrackReturned>,
    > {
        if self.track_tick().await.is_err() {
            return Err(Failed {
                returned: track.wifi,
                cause: Failure::Poisoned,
            });
        }
        self.shared.track(track).await
    }
    async fn retune(
        &mut self,
        retune: <Wifi as oer_esp32s31_radio_client::Client>::Retune,
    ) -> Result<
        <Wifi as oer_esp32s31_radio_client::Client>::Retuned,
        Failed<<Wifi as oer_esp32s31_radio_client::Client>::RetuneReturned>,
    > {
        if self.track_tick().await.is_err() {
            return Err(Failed {
                returned: retune.wifi,
                cause: Failure::Poisoned,
            });
        }
        self.shared.retune(retune).await
    }
    async fn exit(
        &mut self,
        leave: <Wifi as oer_esp32s31_radio_client::Client>::Leave,
    ) -> (<Wifi as oer_esp32s31_radio_client::Client>::Left, Exit) {
        if self.track_tick().await.is_err() {
            return (
                oer_esp32s31_radio_client::wifi::WifiLeft {
                    wifi: oer_esp32s31_radio_client::wifi::WifiReturned::Clocked(leave.wifi),
                },
                Exit {
                    last: false,
                    health: DomainHealth::Poisoned,
                },
            );
        }
        self.shared.exit(leave).await
    }
}

impl<C, T, O, H> ClientBackend<Bluetooth, RcSlow> for WithBluetooth<C, T, O, H>
where
    C: PlatformClockProvider,
    T: oer_time::Timer,
    O: PhyTargetObserver + Clone,
    H: RadioController<C::Backend>,
{
    fn admits(&self, _join: &BluetoothJoin) -> bool {
        self.cold.is_some() && self.controller.can_prepare() && self.membership.is_none()
    }

    async fn enter(
        &mut self,
        join: BluetoothJoin,
        phy: PhyPreparation,
        _lp: &LpClock<RcSlow>,
    ) -> Result<BluetoothJoined, Failed<BluetoothJoin>> {
        let failed = || Failed {
            returned: join,
            cause: Failure::Poisoned,
        };
        self.track_tick().await.map_err(|_| failed())?;
        let cold = self.cold.take().ok_or_else(failed)?;
        let (powered, power) = match cold.power_up(&mut self.shared.radio, &self.shared.clocks) {
            Ok(owners) => owners,
            Err(error) => {
                self.cold = Some(error.into_owner());
                return Err(failed());
            }
        };
        self.power = Some(power);
        let clocked = match powered.enable_clocks(&mut self.shared.radio, &self.shared.clocks) {
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
        let (present, _) = self.shared.radio.enter_coex(absent);
        self.coex = Some(Coex::Present(present));
        let task = self.controller.task_owner().ok_or_else(failed)?;
        let ready = phy_ready(&mut self.shared.radio).map_err(|_| failed())?;
        let PhyClientAccess::Available(ready) = ready.client::<BluetoothPhyMembership>() else {
            return Err(failed());
        };
        let joined = join_bluetooth(ready, task, &self.shared.timer).map_err(|_| failed())?;
        self.membership = Some(joined.membership);
        self.configuration = Some(join);
        Ok(BluetoothJoined { installation, phy })
    }

    async fn track(
        &mut self,
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
        self.track_tick().await.map_err(|_| failed())?;
        let member = self.membership.as_ref().ok_or_else(failed)?;
        let ready = bluetooth_rf_ready(&mut self.shared.radio, member).map_err(|_| failed())?;
        self.controller
            .initialize_phy(ready)
            .map_err(|_| failed())?;
        Ok(installation)
    }

    async fn retune(&mut self, never: Infallible) -> Result<Infallible, Failed<Infallible>> {
        match never {}
    }

    async fn exit(&mut self, installation: Installation) -> (BluetoothLeft, Exit) {
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
        if self.track_tick().await.is_err() {
            return poisoned();
        }
        let Some(task) = self.controller.task_owner() else {
            return poisoned();
        };
        let Some(member) = self.membership.take() else {
            return poisoned();
        };
        let last = match leave_bluetooth(&mut self.shared.radio, task, member) {
            Ok(last) => last,
            Err(error) => {
                self.membership = Some(error.into_membership());
                return poisoned();
            }
        };
        let clocked = match self.controller.reset(&mut self.shared.radio) {
            Ok(clocked) => clocked,
            Err(_) => return poisoned(),
        };
        let powered = match clocked.disable_clocks(&mut self.shared.radio) {
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
        let (absent, _) = self.shared.radio.leave_coex(present);
        let Some(power) = self.power.take() else {
            self.coex = Some(Coex::Absent(absent));
            self.powered = Some(powered);
            return poisoned();
        };
        match powered.power_down(&mut self.shared.radio, power, &absent) {
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
