//! Shared-power membership and the reversible cold baseline.
use oer_radio_coex::RadioClient;

/// Platform references released around the cold-baseline restore.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Clock {
    AnalogI2c,
    Pll160m,
    Mpll,
}

/// A refused release leaves the exact reference held in its chip-owned slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseRefused;

/// Chip-owned atoms and references borrowed for one membership transition.
pub trait PowerPort {
    type Baseline: Copy;
    type RestoreError;
    fn capture(&self) -> Self::Baseline;
    fn restore(&mut self, baseline: Self::Baseline) -> Result<(), Self::RestoreError>;
    fn select_wifi_low_power_clock(&mut self);
    /// A refusal leaves the exact reference in its chip-owned slot.
    fn release(&mut self, clock: Clock) -> Result<(), ReleaseRefused>;
}

/// Entry additionally borrows the platform provider for the power sequence.
pub trait PowerEntryPort: PowerPort {
    fn power_up(&mut self) -> Result<(), crate::power::PowerError>;
}
/// Only the Wi-Fi entry binding grants this reset edge.
pub trait WifiPowerPort: PowerEntryPort {
    fn reset_wifi(&mut self);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerError<R> {
    AlreadyEntered,
    Power(crate::power::PowerError),
    Restore(R),
    Release(Clock),
}

/// Software membership, separate from the chip's affine public proofs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Clients(u8);
impl Clients {
    const fn bit(client: RadioClient) -> u8 {
        match client {
            RadioClient::Wifi => 1,
            RadioClient::Bluetooth => 2,
            RadioClient::Ieee802154 => 4,
        }
    }
    pub const fn holds(&self, client: RadioClient) -> bool {
        self.0 & Self::bit(client) != 0
    }
    pub const fn any(&self) -> bool {
        self.0 != 0
    }
    pub const fn only(&self, client: RadioClient) -> bool {
        self.0 == Self::bit(client)
    }
    pub fn enter(&mut self, client: RadioClient) {
        self.0 |= Self::bit(client);
    }
    pub fn leave(&mut self, client: RadioClient) {
        self.0 &= !Self::bit(client);
    }
}

/// Retained across power epochs, including failed entry and restore attempts.
pub struct PowerMembership<S> {
    clients: Clients,
    baseline: Option<S>,
    entered_since_boot: bool,
    wifi_reset_completed: bool,
}
impl<S> Default for PowerMembership<S> {
    fn default() -> Self {
        Self {
            clients: Clients::default(),
            baseline: None,
            entered_since_boot: false,
            wifi_reset_completed: false,
        }
    }
}
impl<S: Copy> PowerMembership<S> {
    pub const fn holds(&self, client: RadioClient) -> bool {
        self.clients.holds(client)
    }
    pub const fn any_client(&self) -> bool {
        self.clients.any()
    }
    fn prepare<P: PowerPort<Baseline = S>>(
        &mut self,
        client: RadioClient,
        port: &mut P,
    ) -> Result<(), PowerError<P::RestoreError>> {
        if self.clients.holds(client) {
            return Err(PowerError::AlreadyEntered);
        }
        if !self.clients.any() {
            if self.baseline.is_none() {
                self.baseline = Some(port.capture());
            }
            if !self.entered_since_boot {
                port.select_wifi_low_power_clock();
            }
        }
        Ok(())
    }
    fn finish<P: PowerEntryPort<Baseline = S>>(
        &mut self,
        client: RadioClient,
        port: &mut P,
    ) -> Result<(), PowerError<P::RestoreError>> {
        if !self.clients.any() {
            port.power_up().map_err(PowerError::Power)?;
            self.entered_since_boot = true;
        }
        self.clients.enter(client);
        Ok(())
    }
    pub fn enter<P: PowerEntryPort<Baseline = S>>(
        &mut self,
        client: RadioClient,
        port: &mut P,
    ) -> Result<(), PowerError<P::RestoreError>> {
        self.prepare(client, port)?;
        self.finish(client, port)
    }
    pub fn enter_wifi<P: WifiPowerPort<Baseline = S>>(
        &mut self,
        port: &mut P,
    ) -> Result<(), PowerError<P::RestoreError>> {
        self.prepare(RadioClient::Wifi, port)?;
        // SOURCE: IDF Wi-Fi initialization resets its MAC after initial PHY calibration.
        // Preserve baseline -> LP selection -> combined reset -> common power.
        if !self.wifi_reset_completed {
            port.reset_wifi();
        }
        self.finish(RadioClient::Wifi, port)?;
        self.wifi_reset_completed = true;
        Ok(())
    }
    pub fn exit<P: PowerPort<Baseline = S>>(
        &mut self,
        client: RadioClient,
        port: &mut P,
    ) -> Result<(), PowerError<P::RestoreError>> {
        if self.clients.only(client) {
            port.release(Clock::AnalogI2c)
                .map_err(|ReleaseRefused| PowerError::Release(Clock::AnalogI2c))?;
            if let Some(baseline) = self.baseline {
                port.restore(baseline).map_err(PowerError::Restore)?;
                self.baseline = None;
            }
            for clock in [Clock::Pll160m, Clock::Mpll] {
                port.release(clock)
                    .map_err(|ReleaseRefused| PowerError::Release(clock))?;
            }
        }
        self.clients.leave(client);
        Ok(())
    }
    /// Seed only software accounting in a native ownership model.
    #[doc(hidden)]
    #[cfg(any(test, feature = "test-support"))]
    pub fn hold_for_test(&mut self, client: RadioClient) {
        self.clients.enter(client);
    }
}

/// Client and external coexistence participant accounting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Participants(u8);
impl Participants {
    pub const fn count(&self) -> u32 {
        self.0.count_ones()
    }
    pub fn enter(&mut self, client: RadioClient) -> u32 {
        self.0 |= Clients::bit(client);
        self.count()
    }
    pub fn leave(&mut self, client: RadioClient) -> u32 {
        self.0 &= !Clients::bit(client);
        self.count()
    }
    pub fn enter_external(&mut self) -> u32 {
        self.0 |= 8;
        self.count()
    }
    pub fn leave_external(&mut self) -> u32 {
        self.0 &= !8;
        self.count()
    }
}
