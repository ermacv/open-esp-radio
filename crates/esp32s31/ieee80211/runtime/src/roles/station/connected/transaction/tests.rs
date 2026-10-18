use core::future::ready;

use crate::{
    datapath::irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime},
    roles::station::StationControlResources,
};

use embassy_futures::block_on;

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_time_virtual::SkipClock;

use std::{cell::Cell, rc::Rc};

use super::*;

struct TestRoute(Rc<Cell<bool>>);

impl MacInterruptRoute for TestRoute {
    type Platform = ();
    type Setup = u32;
    type Error = u8;

    fn activate(
        &mut self,
        _platform: &Self::Platform,
        setup: Self::Setup,
        _event_mask: oer_esp32s31_hal::types::MacInterruptMask,
    ) -> Result<(), (Self::Error, Self::Setup)> {
        let _ = setup;
        Ok(())
    }

    fn quiesce(&mut self, _platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
        assert!(self.0.get(), "RX admission must close before IRQ quiesce");
        Ok(19)
    }
}

struct RetryRoute {
    attempts: Rc<Cell<u8>>,
    admission_closed: Rc<Cell<bool>>,
}

impl MacInterruptRoute for RetryRoute {
    type Platform = ();
    type Setup = u32;
    type Error = u8;

    fn activate(
        &mut self,
        _platform: &Self::Platform,
        _setup: Self::Setup,
        _event_mask: oer_esp32s31_hal::types::MacInterruptMask,
    ) -> Result<(), (Self::Error, Self::Setup)> {
        Ok(())
    }

    fn quiesce(&mut self, _platform: &Self::Platform) -> Result<Self::Setup, Self::Error> {
        assert!(
            self.admission_closed.get(),
            "RX admission must close before every IRQ quiesce attempt"
        );
        let attempt = self.attempts.get();
        self.attempts.set(attempt + 1);
        if attempt == 0 { Err(9) } else { Ok(19) }
    }
}

struct TestRunner {
    network: u32,
    services: u16,
    admission_closed: Rc<Cell<bool>>,
    clock: SkipClock,
}

impl ConnectedEpochRunnerOwner for TestRunner {
    type Network = u32;
    type Services = u16;

    fn into_connected_epoch_parts(self) -> (Self::Network, Self::Services) {
        (self.network, self.services)
    }
}

impl ConnectedStationRunner<NoopRawMutex> for TestRunner {
    type Error = u8;
    type Clock = SkipClock;

    fn clock(&self) -> &SkipClock {
        &self.clock
    }

    fn run_station_epoch<'a>(
        &'a mut self,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<Output = ConnectedStationExit<Self::Error>> + 'a {
        ready(ConnectedStationExit::Disconnected(
            oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason::BeaconLoss,
        ))
    }

    fn close_station_rx_admission(&mut self) {
        assert!(!self.admission_closed.replace(true));
    }

    fn station_rx_frontend_quiescent(&mut self) -> bool {
        self.admission_closed.get()
    }
}

struct OrderObserver(Rc<Cell<u8>>);

impl ConnectedRunObserver for OrderObserver {
    async fn observe<'a, F>(&'a mut self, future: F) -> F::Output
    where
        F: Future + 'a,
    {
        assert_eq!(self.0.get(), 0);
        self.0.set(1);
        let output = future.await;
        self.0.set(2);
        output
    }
}

#[test]
fn transaction_classifies_the_live_exit_before_revealing_parked_owners() {
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let admission_closed = Rc::new(Cell::new(false));
    let mut interrupt = InterruptEpoch::new(TestRoute(admission_closed.clone()), 18, &mac, &power);
    interrupt
        .activate(&(), oer_esp32s31_hal::types::MacInterruptMask::COLD_RX)
        .expect("test interrupt epoch activates");
    let control = StationControlResources::<NoopRawMutex>::new();
    let (_controller, mut receiver) = control.split().expect("fresh control owner");
    let order = Rc::new(Cell::new(0));
    let mut observer = OrderObserver(order.clone());
    let stopped = block_on(run_and_quiesce_esp32s31_connected_epoch(
        interrupt,
        &(),
        TestRunner {
            network: 29,
            services: 31,
            admission_closed: admission_closed.clone(),

            clock: SkipClock::new(),
        },
        &mut receiver,
        &mut observer,
        |exit, runner| {
            assert!(matches!(
                exit,
                ConnectedStationExit::Disconnected(
                    oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason::BeaconLoss
                )
            ));
            assert_eq!(runner.services, 31);
            assert_eq!(order.get(), 2);
            order.set(3);
            37_u8
        },
    ))
    .unwrap_or_else(|_| panic!("ready transaction must quiesce"));

    assert_eq!(order.get(), 3);
    assert_eq!(stopped.exit, 37);
    assert_eq!(stopped.quiesced.network, 29);
    assert_eq!(stopped.quiesced.services, 31);
    assert!(stopped.quiesced.interrupt.is_active());
    assert!(admission_closed.get());
}

#[test]
fn logical_park_does_not_call_physical_route_quiesce() {
    let mac = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let power = EmbassyPowerIrqRuntime::<NoopRawMutex>::new();
    let route_attempts = Rc::new(Cell::new(0));
    let admission_closed = Rc::new(Cell::new(false));
    let mut interrupt = InterruptEpoch::new(
        RetryRoute {
            attempts: route_attempts.clone(),
            admission_closed: admission_closed.clone(),
        },
        18,
        &mac,
        &power,
    );
    interrupt
        .activate(&(), oer_esp32s31_hal::types::MacInterruptMask::COLD_RX)
        .expect("test interrupt epoch activates");
    let control = StationControlResources::<NoopRawMutex>::new();
    let (_controller, mut receiver) = control.split().expect("fresh control owner");
    let order = Rc::new(Cell::new(0));
    let mut observer = OrderObserver(order.clone());
    let stopped = block_on(run_and_quiesce_esp32s31_connected_epoch(
        interrupt,
        &(),
        TestRunner {
            network: 29,
            services: 31,
            admission_closed: admission_closed.clone(),

            clock: SkipClock::new(),
        },
        &mut receiver,
        &mut observer,
        |_exit, _runner| {
            order.set(3);
            37_u8
        },
    ))
    .unwrap_or_else(|_| panic!("logical park must not call the route quiesce hook"));

    assert_eq!(stopped.exit, 37);
    assert_eq!(route_attempts.get(), 0);
    assert_eq!(
        order.get(),
        3,
        "classified exit must remain stable after IRQ failure"
    );
    assert_eq!(stopped.quiesced.network, 29);
    assert_eq!(stopped.quiesced.services, 31);
    assert!(stopped.quiesced.interrupt.is_active());
    assert_eq!(order.get(), 3);
}
