//! The connected station's power-management agent.
//!
//! The power manager runs inside the connected control service, which owns
//! the MAC but neither the radio arbiter nor the Wi-Fi PHY membership. Its
//! coexistence requests and RF sleep and wake travel over one
//! [`StationPowerLink`] to [`run_station_power_agent`], which runs beside the
//! datapath with the [`RadioSystem`](oer_esp32s31_radio_runtime::RadioSystem). The agent also carries the station
//! TBTT interrupt, the coexistence phases and the Bluetooth preemption end
//! back to control. Control reads the coexistence state itself, from a
//! [`PowerCoexSource`], at every step, as the vendor reads it at every input.
//!
//! Control counts the commands it sent and the agent the commands it
//! performed. Until both agree, control holds every frame but its Null, so
//! nothing reaches the air before the RF wake or the air request the vendor
//! performs synchronously.

use core::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::RawMutex},
    channel::Channel,
    signal::Signal,
};
use oer_esp32s31_coex::{CoexClientRequest, CoexError, CoexEventId, timer_index};
use oer_esp32s31_ieee80211_sta::{
    connected_control::{ConnectedPowerCommand, POWER_COMMAND_CAPACITY, PowerCoexSnapshot},
    modem_sleep::{CoexPhaseView, PmCoexAction, PmCoexEvent},
};

#[cfg(target_arch = "riscv32")]
pub use agent::{StationRf, StationRfPower, finish_station_power, run_station_power_agent};

/// The coexistence arbiter's effects the power manager asks for: the radio
/// system under its lease.
pub trait CoexRadio {
    /// Program a Wi-Fi request of `request.event` on its policy timer.
    fn request_wifi_coex(&mut self, request: CoexClientRequest) -> Result<(), CoexError>;
    /// Withdraw the request of `event`.
    fn release_coex(&mut self, event: CoexEventId) -> Result<(), CoexError>;
    fn set_coex_interval(&mut self, interval: u32);
    fn restart_coex_phases(&mut self);
    fn set_coex_flexible_period(&mut self, period: u8);
}

/// Perform one coexistence effect of the power manager on `radio`, for the
/// direct station's power agent and the port station alike.
pub fn apply_coex_action(
    radio: &mut impl CoexRadio,
    action: PmCoexAction,
) -> Result<(), CoexError> {
    match action {
        // The vendor core programs only events with a policy timer; for the
        // others `coex_core_request` and `coex_core_release` return
        // `ESP_ERR_INVALID_ARG`, which power management ignores. On the S31
        // that leaves the slice request as the only effective one.
        //
        // SOURCE: complete pinned `libcoexist[coexist_core.o]::coex_core_request` and `libcoexist[coexist_core.o]::coex_core_timer_idx_get`.
        PmCoexAction::Request { event, .. } | PmCoexAction::Release(event)
            if timer_index(coex_event(event)).is_none() =>
        {
            Ok(())
        }
        PmCoexAction::Request {
            event,
            duration_micros,
        } => radio.request_wifi_coex(CoexClientRequest {
            event: coex_event(event),
            latency: 0,
            duration: duration_micros,
        }),
        PmCoexAction::Release(event) => radio.release_coex(coex_event(event)),
        PmCoexAction::SetInterval(interval) => {
            radio.set_coex_interval(interval);
            Ok(())
        }
        PmCoexAction::RestartPhases => {
            radio.restart_coex_phases();
            Ok(())
        }
        PmCoexAction::SetFlexiblePeriod(period) => {
            radio.set_coex_flexible_period(period);
            Ok(())
        }
    }
}

fn coex_event(event: PmCoexEvent) -> CoexEventId {
    CoexEventId::new(event.id()).expect("power management requests defined events")
}

/// Why the power agent stopped serving its station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationPowerFailure {
    /// The arbiter rejected a coexistence request or release.
    Coex(CoexError),
    /// The station's RF did not sleep or wake.
    Rf(StationRfPowerError),
}

/// Why the station's RF did not change its state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationRfPowerError {
    /// The RF slept already, or woke already.
    State,
    /// The shared PHY domain rejected the Wi-Fi client's release or return.
    Client(oer_esp32s31_phy::concurrent::ConcurrentPhyError),
    /// Closing RF after the last client failed and requires reset.
    Close,
    /// Waking closed RF failed.
    Prepare,
    /// The tracking due after the wake failed.
    Track,
}

/// The channel between one association's control service and its agent.
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-full-modem-sleep
pub struct StationPowerLink<M: RawMutex> {
    commands: Channel<M, ConnectedPowerCommand, POWER_COMMAND_CAPACITY>,
    sent: AtomicU32,
    performed: AtomicU32,
    /// Wakes control for an input, a performed command or a failure.
    wake: Signal<M, ()>,
    tbtt: AtomicBool,
    coex: BlockingMutex<M, Cell<Option<&'static (dyn PowerCoexSource + Sync)>>>,
    phase: BlockingMutex<M, Cell<Option<CoexPhaseView>>>,
    preemption: BlockingMutex<M, Cell<Option<Option<u64>>>>,
    failure: BlockingMutex<M, Cell<Option<StationPowerFailure>>>,
}

impl<M: RawMutex> Default for StationPowerLink<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// The coexistence state Wi-Fi's power management reads, without the
/// arbiter lease.
pub trait PowerCoexSource {
    fn power_coex(&self) -> PowerCoexSnapshot;
}

/// One association's use of a [`StationPowerLink`].
pub struct StationPowerBinding<'link, M: RawMutex> {
    pub(super) link: &'link StationPowerLink<M>,
}

impl<M: RawMutex> StationPowerLink<M> {
    /// Start one association on this link, reading the coexistence state
    /// from `coex`: forget the previous association's inputs and commands.
    pub fn bind(&self, coex: &'static (dyn PowerCoexSource + Sync)) -> StationPowerBinding<'_, M> {
        self.commands.clear();
        self.sent.store(0, Ordering::Release);
        self.performed.store(0, Ordering::Release);
        self.wake.reset();
        self.tbtt.store(false, Ordering::Release);
        self.phase.lock(|cell| cell.set(None));
        self.preemption.lock(|cell| cell.set(None));
        self.failure.lock(|cell| cell.set(None));
        self.coex.lock(|cell| cell.set(Some(coex)));
        StationPowerBinding { link: self }
    }

    pub const fn new() -> Self {
        Self {
            commands: Channel::new(),
            sent: AtomicU32::new(0),
            performed: AtomicU32::new(0),
            wake: Signal::new(),
            tbtt: AtomicBool::new(false),
            coex: BlockingMutex::new(Cell::new(None)),
            phase: BlockingMutex::new(Cell::new(None)),
            preemption: BlockingMutex::new(Cell::new(None)),
            failure: BlockingMutex::new(Cell::new(None)),
        }
    }

    /// Hand one command to the agent. Fails when the agent fell a whole
    /// queue behind.
    pub fn send(&self, command: ConnectedPowerCommand) -> Result<(), ConnectedPowerCommand> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                embassy_sync::channel::TrySendError::Full(command) => command,
            })?;
        self.sent.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// Whether a sent command waits for the agent.
    pub fn outstanding(&self) -> bool {
        self.sent.load(Ordering::Acquire) != self.performed.load(Ordering::Acquire)
    }

    /// Whether an input, or a failure, waits for control.
    pub fn has_input(&self) -> bool {
        self.tbtt.load(Ordering::Acquire)
            || self.phase.lock(Cell::get).is_some()
            || self.preemption.lock(Cell::get).is_some()
            || self.failure.lock(Cell::get).is_some()
    }

    /// The coexistence state now, once an association bound the link.
    pub fn coex(&self) -> Option<PowerCoexSnapshot> {
        self.coex.lock(Cell::get).map(PowerCoexSource::power_coex)
    }

    pub fn take_tbtt(&self) -> bool {
        self.tbtt.swap(false, Ordering::AcqRel)
    }

    pub fn take_phase(&self) -> Option<CoexPhaseView> {
        self.phase.lock(Cell::take)
    }

    pub fn take_preemption(&self) -> Option<Option<u64>> {
        self.preemption.lock(Cell::take)
    }

    pub fn failure(&self) -> Option<StationPowerFailure> {
        self.failure.lock(Cell::get)
    }

    /// Wait until the agent delivered an input or performed a command.
    pub async fn wait(&self) {
        self.wake.wait().await;
    }

    /// Stand in for the agent in host tests: take every sent command as
    /// performed and deliver `phase` and `tbtt`.
    #[cfg(test)]
    pub(crate) fn perform_for_test(
        &self,
        commands: &mut impl FnMut(ConnectedPowerCommand),
        phase: Option<CoexPhaseView>,
        tbtt: bool,
    ) {
        while let Ok(command) = self.commands.try_receive() {
            commands(command);
            self.performed.fetch_add(1, Ordering::AcqRel);
        }
        if let Some(phase) = phase {
            self.phase.lock(|cell| cell.set(Some(phase)));
        }
        if tbtt {
            self.tbtt.store(true, Ordering::Release);
        }
        self.wake.signal(());
    }
}

#[cfg(target_arch = "riscv32")]
mod agent {
    use core::{future::Future, sync::atomic::Ordering};

    use embassy_futures::select::{Either, Either3, select, select3};

    use oer_esp32s31_coex::{CoexClientRequest, CoexError, CoexEventId};
    use oer_esp32s31_hal::{shared_radio::PlatformClockProvider, types::MacPti};
    use oer_esp32s31_ieee80211_sta::{
        connected_control::{ConnectedPowerCommand, PowerCoexSnapshot},
        modem_sleep::{CoexPhaseView, CoexView},
    };
    use oer_esp32s31_radio_runtime::{
        CoexPreemptionEnd, RadioGuard, RadioSystem, WifiCoexViewCell,
    };

    use embassy_sync::blocking_mutex::raw::RawMutex;

    use oer_esp32s31_ieee80211::runtime::{WifiRfSleepError, WifiRoleOwner};
    use oer_esp32s31_phy::{
        ConcurrentRfError,
        concurrent::{ConcurrentAcquire, phy_ready},
    };

    use super::{PowerCoexSource, StationPowerFailure, StationPowerLink, StationRfPowerError};
    use crate::datapath::irq::EmbassyPowerIrqRuntime;

    impl<M: RawMutex> StationPowerLink<M> {
        fn fail(&self, failure: StationPowerFailure) {
            self.failure.lock(|cell| cell.set(Some(failure)));
            self.wake.signal(());
        }
    }

    /// The station's RF, through its role's PHY client.
    pub struct StationRf<'role, W, Membership> {
        role: &'role mut WifiRoleOwner<W, Membership>,
    }

    impl<'role, W, Membership> StationRf<'role, W, Membership> {
        pub fn new(role: &'role mut WifiRoleOwner<W, Membership>) -> Self {
            Self { role }
        }

        /// Whether the RF sleeps.
        pub fn asleep(&mut self) -> bool {
            self.role.context_mut().rf_asleep()
        }
    }

    impl<W, Membership, P, C: PlatformClockProvider, T: oer_time::Timer> StationRfPower<P, C, T>
        for StationRf<'_, W, Membership>
    {
        async fn sleep(
            &mut self,
            radio: &mut RadioGuard<'_, P, C, T>,
        ) -> Result<(), StationRfPowerError> {
            let last = self
                .role
                .context_mut()
                .suspend_rf(radio.lease())
                .map_err(rf_error)?;
            if last {
                match radio.close_phy_if_idle().await {
                    // A recoverable close keeps RF open and the domain unchanged.
                    Ok(_) | Err(ConcurrentRfError::Recoverable(_)) => {}
                    Err(_) => return Err(StationRfPowerError::Close),
                }
            }
            Ok(())
        }

        async fn wake(
            &mut self,
            radio: &mut RadioGuard<'_, P, C, T>,
        ) -> Result<(), StationRfPowerError> {
            // As `RadioGuard::resume_wifi`, the wake keeps no new
            // calibration cache of its own.
            let _prepared = radio
                .prepare_phy()
                .await
                .map_err(|_| StationRfPowerError::Prepare)?;
            let timer = radio.timer();
            let ready = phy_ready(radio.lease()).map_err(|_| StationRfPowerError::Prepare)?;
            let acquired = self
                .role
                .context_mut()
                .resume_rf(ready, timer)
                .map_err(rf_error)?;
            if acquired == ConcurrentAcquire::TrackingDue {
                radio
                    .track()
                    .await
                    .map_err(|_| StationRfPowerError::Track)?;
            }
            Ok(())
        }
    }

    fn rf_error(error: WifiRfSleepError) -> StationRfPowerError {
        match error {
            WifiRfSleepError::State => StationRfPowerError::State,
            WifiRfSleepError::Phy(error) => StationRfPowerError::Client(error),
        }
    }

    /// The owner of the station's RF membership, driven by the agent.
    pub trait StationRfPower<P, C: PlatformClockProvider, T: oer_time::Timer> {
        /// Put the station's RF to sleep, keeping its registration.
        fn sleep<'a>(
            &'a mut self,
            radio: &'a mut RadioGuard<'_, P, C, T>,
        ) -> impl Future<Output = Result<(), StationRfPowerError>> + 'a;

        /// Wake the station's RF.
        fn wake<'a>(
            &'a mut self,
            radio: &'a mut RadioGuard<'_, P, C, T>,
        ) -> impl Future<Output = Result<(), StationRfPowerError>> + 'a;
    }

    impl PowerCoexSource for WifiCoexViewCell {
        fn power_coex(&self) -> PowerCoexSnapshot {
            let view = self.get();
            PowerCoexSnapshot {
                view: CoexView {
                    active: view.active,
                    current_period: view.current_period,
                    flexible_period: view.flexible_period,
                    interval: view.interval,
                    phase0_share_percent: view.first_phase_share_percent,
                },
                beacon_pti: MacPti::new(u32::from(view.beacon_pti.value()))
                    .expect("coexistence priorities are four-bit values"),
            }
        }
    }

    impl<P, C: PlatformClockProvider, T: oer_time::Timer> super::CoexRadio for RadioGuard<'_, P, C, T> {
        fn request_wifi_coex(&mut self, request: CoexClientRequest) -> Result<(), CoexError> {
            RadioGuard::request_wifi_coex(self, request).map(|_| ())
        }

        fn release_coex(&mut self, event: CoexEventId) -> Result<(), CoexError> {
            RadioGuard::release_coex(self, event).map(|_| ())
        }

        fn set_coex_interval(&mut self, interval: u32) {
            RadioGuard::set_coex_interval(self, interval);
        }

        fn restart_coex_phases(&mut self) {
            RadioGuard::restart_coex_phases(self);
        }

        fn set_coex_flexible_period(&mut self, period: u8) {
            RadioGuard::set_coex_flexible_period(self, period);
        }
    }

    /// Serve one association's power link until the agent fails.
    ///
    /// Run it beside the connected datapath for the association's lifetime;
    /// dropping it between commands is safe, as the datapath's shutdown stops
    /// power management through the control core first.
    // CAPABILITY: coex-protocol-integration-and-lifetime-coexistence-power-management
    pub async fn run_station_power_agent<M, IM, P, C, T, R, K>(
        link: &StationPowerLink<M>,
        radio: &RadioSystem<P, C, T>,
        power_irq: &EmbassyPowerIrqRuntime<IM>,
        rf: &mut R,
        mac_clock: &K,
    ) -> StationPowerFailure
    where
        M: RawMutex,
        IM: RawMutex,
        C: PlatformClockProvider,
        T: oer_time::Timer,
        R: StationRfPower<P, C, T>,
        K: crate::mac_clock::ReceptionClock + ?Sized,
    {
        loop {
            let event = select3(
                link.commands.receive(),
                radio.wifi_coex_phase(),
                select(radio.wifi_coex_preemption_end(), power_irq.wait()),
            )
            .await;
            let mut guard = radio.lock().await;
            match event {
                Either3::First(command) => {
                    if let Err(failure) = perform(&mut guard, rf, mac_clock, command).await {
                        link.fail(failure);
                        return failure;
                    }
                    link.performed.fetch_add(1, Ordering::AcqRel);
                }
                Either3::Second(phase) => {
                    link.phase.lock(|cell| {
                        cell.set(Some(CoexPhaseView {
                            share_percent: phase.share_percent(),
                            wifi: phase.wifi(),
                        }))
                    });
                }
                Either3::Third(Either::First(end)) => {
                    let end = match end {
                        CoexPreemptionEnd::At(at) => Some(at.as_micros()),
                        CoexPreemptionEnd::Unknown => None,
                    };
                    link.preemption.lock(|cell| cell.set(Some(end)));
                }
                Either3::Third(Either::Second(observation)) => {
                    if !observation.sta_tbtt() {
                        continue;
                    }
                    link.tbtt.store(true, Ordering::Release);
                }
            }
            drop(guard);
            link.wake.signal(());
        }
    }

    /// Perform the commands control sent before the association stopped:
    /// the stop's air releases and RF wake.
    pub async fn finish_station_power<M, P, C, T, R, K>(
        link: &StationPowerLink<M>,
        radio: &RadioSystem<P, C, T>,
        rf: &mut R,
        mac_clock: &K,
    ) -> Result<(), StationPowerFailure>
    where
        M: RawMutex,
        C: PlatformClockProvider,
        T: oer_time::Timer,
        R: StationRfPower<P, C, T>,
        K: crate::mac_clock::ReceptionClock + ?Sized,
    {
        while let Ok(command) = link.commands.try_receive() {
            let mut guard = radio.lock().await;
            perform(&mut guard, rf, mac_clock, command).await?;
            link.performed.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }

    async fn perform<P, C, T, R, K>(
        radio: &mut RadioGuard<'_, P, C, T>,
        rf: &mut R,
        mac_clock: &K,
        command: ConnectedPowerCommand,
    ) -> Result<(), StationPowerFailure>
    where
        C: PlatformClockProvider,
        T: oer_time::Timer,
        R: StationRfPower<P, C, T>,
        K: crate::mac_clock::ReceptionClock + ?Sized,
    {
        match command {
            ConnectedPowerCommand::Coex(action) => {
                super::apply_coex_action(radio, action).map_err(StationPowerFailure::Coex)?;
            }
            ConnectedPowerCommand::RfSleep => {
                super::trace_counter(mac_clock, |mac_local_time, monotonic_micros| {
                    oer_trace::emit(&oer_ieee80211_trace::RfSleepEntered {
                        mac_local_time,
                        monotonic_micros,
                    })
                });
                rf.sleep(radio).await.map_err(StationPowerFailure::Rf)?
            }
            ConnectedPowerCommand::RfWake => {
                rf.wake(radio).await.map_err(StationPowerFailure::Rf)?;
                super::trace_counter(mac_clock, |mac_local_time, monotonic_micros| {
                    oer_trace::emit(&oer_ieee80211_trace::RfWoke {
                        mac_local_time,
                        monotonic_micros,
                    })
                });
                // The MAC local time's relation to the monotonic clock
                // across the sleep is not established: start a new
                // generation of it.
                mac_clock.on_rf_wake();
            }
        }
        Ok(())
    }
}

/// Trace the raw MAC counter beside the low 32 bits of monotonic time, the
/// evidence of whether the counter runs through RF sleep.
#[cfg(target_arch = "riscv32")]
fn trace_counter<K: crate::mac_clock::ReceptionClock + ?Sized>(
    mac_clock: &K,
    emit: impl FnOnce(u32, u32),
) {
    if let Some((counter, monotonic)) = mac_clock.counter_reading() {
        emit(counter, monotonic.as_micros() as u32);
    }
}

#[cfg(test)]
mod coex_tests {
    use std::vec::Vec;

    use super::*;

    /// A radio that records the effects it performs.
    #[derive(Default)]
    struct Recording {
        effects: Vec<&'static str>,
        requests: Vec<CoexClientRequest>,
        refuse: Option<CoexError>,
    }

    impl CoexRadio for Recording {
        fn request_wifi_coex(&mut self, request: CoexClientRequest) -> Result<(), CoexError> {
            if let Some(error) = self.refuse {
                return Err(error);
            }
            self.requests.push(request);
            self.effects.push("request");
            Ok(())
        }

        fn release_coex(&mut self, _event: CoexEventId) -> Result<(), CoexError> {
            self.effects.push("release");
            Ok(())
        }

        fn set_coex_interval(&mut self, _interval: u32) {
            self.effects.push("interval");
        }

        fn restart_coex_phases(&mut self) {
            self.effects.push("restart");
        }

        fn set_coex_flexible_period(&mut self, _period: u8) {
            self.effects.push("flexible");
        }
    }

    /// A power-management event with a policy timer, and one without.
    fn events() -> (PmCoexEvent, PmCoexEvent) {
        let mut timed = None;
        let mut untimed = None;
        for event in [
            PmCoexEvent::BeaconWindow,
            PmCoexEvent::Slice,
            PmCoexEvent::GroupTraffic,
        ] {
            match timer_index(coex_event(event)) {
                Some(_) => timed = timed.or(Some(event)),
                None => untimed = untimed.or(Some(event)),
            }
        }
        (timed.unwrap(), untimed.unwrap())
    }

    #[test]
    fn only_events_with_a_policy_timer_reach_the_arbiter() {
        let (timed, untimed) = events();
        let mut radio = Recording::default();
        for action in [
            PmCoexAction::Request {
                event: untimed,
                duration_micros: 10,
            },
            PmCoexAction::Release(untimed),
            PmCoexAction::Request {
                event: timed,
                duration_micros: 1_000,
            },
            PmCoexAction::Release(timed),
            PmCoexAction::SetInterval(102_400),
            PmCoexAction::RestartPhases,
            PmCoexAction::SetFlexiblePeriod(3),
        ] {
            assert_eq!(apply_coex_action(&mut radio, action), Ok(()));
        }
        assert_eq!(
            radio.effects,
            ["request", "release", "interval", "restart", "flexible"]
        );
        assert_eq!(radio.requests[0].event, coex_event(timed));
        assert_eq!(radio.requests[0].duration, 1_000);
        assert_eq!(radio.requests[0].latency, 0);
    }
}
