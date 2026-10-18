use core::{
    cell::Cell,
    future::Future,
    num::NonZeroU16,
    sync::atomic::{AtomicU8, Ordering},
    task::{Context, Poll, Waker},
};
use std::rc::Rc;

use embassy_futures::select::{Either, select};
use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::signal::Signal;
use oer_ieee80211_sta::station::StaReconnectPolicy;
use {
    oer_ieee80211_mac::channel::WifiChannel, oer_ieee80211_mac::station::association::Preference,
};

use oer_radio::wifi::{
    MonitorCapturePolicy, MonitorRequest, StationRequest, StationScanChannels, StationScanPolicy,
    StationSecurity, WIFI_SCAN_RESULT_CAPACITY, WifiMacAddress, WifiMonitorConfig,
    WifiRadioRestartRf, WifiScanReport, WifiScanRequest, WifiScanResult, WifiServicePlanningError,
    WifiServiceRequest, WifiSsid, WifiStartFailure, WifiStartReport, WifiStationConfig,
    WifiStopReport, WifiSupervisorConfiguration, WifiSupervisorPort,
};

use super::*;
use oer_radio::wifi::test_support::TEST_CAPABILITIES;

fn station_request() -> StationRequest {
    StationRequest::new(
        WifiSsid::new(b"mailbox").unwrap(),
        StationSecurity::wpa2_personal(b"password", b"mailbox").unwrap(),
        StaReconnectPolicy::new(2, 10, 100, 10).unwrap(),
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(10).unwrap(),
            Preference::Automatic,
        ),
    )
}

fn scan_request() -> WifiScanRequest {
    WifiScanRequest::new(
        StationScanChannels::CHANNELS_1_TO_13,
        NonZeroU16::new(10).unwrap(),
    )
}

fn run<F: Future>(future: F) -> F::Output {
    block_on(future)
}

fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

struct TestRoleControl<'a> {
    stop: &'a Signal<NoopRawMutex, ()>,
    stage: &'a AtomicU8,
}

impl WifiActiveRoleControl for TestRoleControl<'_> {
    fn request_stop(&mut self) {
        self.stage.store(1, Ordering::Release);
        self.stop.signal(());
    }
}

struct RestartControls {
    started: Signal<NoopRawMutex, ()>,
    release: Signal<NoopRawMutex, ()>,
    completed: Signal<NoopRawMutex, ()>,
}

impl RestartControls {
    const fn new() -> Self {
        Self {
            started: Signal::new(),
            release: Signal::new(),
            completed: Signal::new(),
        }
    }
}

struct FakeLocalEpochRunner {
    restart: Option<Rc<RestartControls>>,
    mode: FakeEpochMode,
}

#[derive(Clone, Copy)]
enum FakeEpochMode {
    Normal,
    RejectOnce,
    ActiveFault,
    LifecycleFault,
}

impl FakeLocalEpochRunner {
    const fn immediate() -> Self {
        Self {
            restart: None,
            mode: FakeEpochMode::Normal,
        }
    }

    fn with_restart(controls: Rc<RestartControls>) -> Self {
        Self {
            restart: Some(controls),
            mode: FakeEpochMode::Normal,
        }
    }

    fn with_mode(mode: FakeEpochMode) -> Self {
        Self {
            restart: None,
            mode,
        }
    }
}

impl WifiRoleEpochRunner<NoopRawMutex> for FakeLocalEpochRunner {
    type Stopped = Rc<Cell<u8>>;
    type Faulted = Rc<Cell<u8>>;
    type LifecycleFaulted = Rc<Cell<u8>>;
    type Error = &'static str;

    fn planning_error(&mut self, _error: WifiServicePlanningError) -> Self::Error {
        "planning"
    }

    fn fault_error(&mut self, _faulted: &Self::Faulted) -> Self::Error {
        "faulted"
    }

    fn lifecycle_fault_error(&mut self, _faulted: &Self::LifecycleFaulted) -> Self::Error {
        "lifecycle-faulted"
    }

    async fn run_epoch<'a>(
        &'a mut self,
        endpoint: &'a mut WifiSupervisorEndpoint<'_, NoopRawMutex, Self::Error>,
        stopped: Self::Stopped,
        service: WifiServiceRequest,
        generation: oer_radio::wifi::RadioSubsystemGeneration,
    ) -> WifiRoleEpochOutcome<Self::Stopped, Self::Faulted> {
        if matches!(self.mode, FakeEpochMode::RejectOnce) {
            self.mode = FakeEpochMode::Normal;
            let WifiServiceRequest::Station { request, .. } = service else {
                panic!("reject-once test only starts station")
            };
            endpoint
                .respond(WifiSupervisorResponse::Station(Err(
                    WifiStartFailure::rejected(request, "not-started"),
                )))
                .await;
            return WifiRoleEpochOutcome::NotStarted(stopped);
        }
        if matches!(self.mode, FakeEpochMode::ActiveFault) {
            endpoint
                .respond(WifiSupervisorResponse::Station(Err(
                    WifiStartFailure::faulted("faulted"),
                )))
                .await;
            return WifiRoleEpochOutcome::Faulted(stopped);
        }
        stopped.set(stopped.get() + 1);
        if service.scan_request().is_some() {
            endpoint
                .respond(WifiSupervisorResponse::Scan(Ok(WifiScanReport::new(
                    generation,
                    [WifiScanResult::EMPTY; WIFI_SCAN_RESULT_CAPACITY],
                    0,
                    0,
                    0,
                ))))
                .await;
            return WifiRoleEpochOutcome::Stopped(stopped);
        }
        let response = if service.station_request().is_some() {
            WifiSupervisorResponse::Station(Ok(WifiStartReport::new(generation)))
        } else if service.monitor_request().is_some() {
            WifiSupervisorResponse::Monitor(Ok(WifiStartReport::new(generation)))
        } else {
            return WifiRoleEpochOutcome::Faulted(stopped);
        };
        endpoint.respond(response).await;

        match endpoint.receive().await {
            WifiSupervisorCommand::Stop => {
                // In a real runner this response is emitted through
                // `finish_wifi_active_role` after classifying
                // the concrete owner-bearing task exit.
                endpoint
                    .respond(WifiSupervisorResponse::Stop(Ok(WifiStopReport::new(
                        generation,
                    ))))
                    .await;
                WifiRoleEpochOutcome::Stopped(stopped)
            }
            _ => WifiRoleEpochOutcome::Faulted(stopped),
        }
    }

    async fn restart_radio<'a>(
        &'a mut self,
        slot: &'a mut Option<Self::Stopped>,
    ) -> Result<WifiRadioRestartRf, Self::LifecycleFaulted> {
        let stopped = slot
            .take()
            .expect("test actor retains stopped owner between epochs");
        if matches!(self.mode, FakeEpochMode::LifecycleFault) {
            return Err(stopped);
        }
        if let Some(controls) = &self.restart {
            controls.started.signal(());
            controls.release.wait().await;
        }
        stopped.set(stopped.get() + 10);
        *slot = Some(stopped);
        if let Some(controls) = &self.restart {
            controls.completed.signal(());
        }
        Ok(WifiRadioRestartRf::ClosedAndWoken)
    }
}

#[test]
fn controller_and_supervisor_exchange_typed_station_completion() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();

    let application = async {
        radio
            .into_wifi()
            .start_station(station_request())
            .await
            .unwrap()
    };
    let supervisor = async {
        let WifiSupervisorCommand::StartStation(request) = endpoint.receive().await else {
            panic!("expected station request")
        };
        assert_eq!(request.ssid().as_bytes(), b"mailbox");
        endpoint
            .respond(WifiSupervisorResponse::Station(Ok(WifiStartReport::new(
                oer_radio::wifi::RadioSubsystemGeneration::INITIAL,
            ))))
            .await;
    };
    let (report, ()) = run(join(application, supervisor));
    assert_eq!(
        report.generation(),
        oer_radio::wifi::RadioSubsystemGeneration::INITIAL
    );
}

#[test]
fn stopped_dispatch_validates_before_returning_a_start_transaction() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES).with_station(
        WifiStationConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap()),
    );

    let application = async {
        radio
            .into_wifi()
            .start_station(station_request())
            .await
            .unwrap()
    };
    let supervisor = async {
        let dispatch = dispatch_wifi_stopped_command(
            &mut endpoint,
            configuration,
            oer_radio::wifi::RadioSubsystemGeneration::INITIAL,
            |_| (),
        )
        .await;
        let WifiStoppedDispatch::Start(service) = dispatch else {
            panic!("valid provisioned station request must be dispatched")
        };
        assert_eq!(
            service.station_request().unwrap().ssid().as_bytes(),
            b"mailbox"
        );
        endpoint
            .respond(WifiSupervisorResponse::Station(Ok(WifiStartReport::new(
                oer_radio::wifi::RadioSubsystemGeneration::INITIAL.next(),
            ))))
            .await;
    };
    let (report, ()) = run(join(application, supervisor));
    assert_eq!(report.generation().value(), 1);
}

#[test]
fn stopped_dispatch_rejects_unprovisioned_start_without_moving_a_request() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, WifiServicePlanningError>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES);

    let application = async { radio.into_wifi().start_station(station_request()).await };
    let supervisor = async {
        let dispatch = dispatch_wifi_stopped_command(
            &mut endpoint,
            configuration,
            oer_radio::wifi::RadioSubsystemGeneration::INITIAL,
            |error| error,
        )
        .await;
        assert!(matches!(dispatch, WifiStoppedDispatch::Handled));
    };
    let (result, ()) = run(join(application, supervisor));
    match result {
        Err(oer_radio::wifi::WifiRoleStartFailure::Rejected {
            wifi: _,
            request,
            error: WifiSupervisorError::Service(WifiServicePlanningError::StationNotProvisioned),
        }) => assert_eq!(request.ssid().as_bytes(), b"mailbox"),
        _ => panic!("planning rejection must return the exact station request"),
    }
}

#[test]
fn supervisor_actor_keeps_a_non_send_owner_across_role_epochs() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES)
        .with_station(WifiStationConfig::new(
            WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap(),
        ))
        .with_standalone_scan()
        .with_standalone_monitor();
    let owner = Rc::new(Cell::new(0));
    let observed_owner = Rc::clone(&owner);
    let (radio, task) = match prepare_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::immediate(),
        owner,
    ) {
        Ok(prepared) => prepared,
        Err(_) => panic!("fresh supervisor resources must prepare once"),
    };

    let application = async {
        let scan = radio.into_wifi().scan(scan_request()).await.unwrap();
        let scan_generation = scan.report.generation().value();
        let station = scan.wifi.start_station(station_request()).await.unwrap();
        let station_generation = station.generation().value();
        let wifi = station.stop().await.unwrap();
        let monitor = wifi
            .start_monitor(MonitorRequest::new(
                WifiChannel::mhz20(6).unwrap(),
                WifiMonitorConfig::normalized(),
            ))
            .await
            .unwrap();
        let monitor_generation = monitor.generation().value();
        let _wifi = monitor.stop().await.unwrap();
        (scan_generation, station_generation, monitor_generation)
    };
    let Either::First(generations) = run(select(application, task.run()));
    assert_eq!(generations, (1, 2, 3));
    assert_eq!(observed_owner.get(), 3);
}

#[test]
fn idle_restart_runs_in_the_owner_actor_and_advances_generation() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES).with_station(
        WifiStationConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap()),
    );
    let owner = Rc::new(Cell::new(0));
    let observed_owner = Rc::clone(&owner);
    let (radio, task) = prepare_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::immediate(),
        owner,
    )
    .unwrap_or_else(|_| panic!("fresh supervisor must prepare"));

    let application = async {
        let (wifi, restart) = radio.into_wifi().restart_radio().await.unwrap();
        assert_eq!(restart.rf(), WifiRadioRestartRf::ClosedAndWoken);
        let station = wifi.start_station(station_request()).await.unwrap();
        (restart.generation().value(), station.generation().value())
    };
    let Either::First(generations) = run(select(application, task.run()));
    assert_eq!(generations, (1, 2));
    assert_eq!(observed_owner.get(), 11);
}

#[test]
fn not_started_keeps_the_epoch_generation_and_exact_stopped_owner() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES).with_station(
        WifiStationConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap()),
    );
    let owner = Rc::new(Cell::new(0));
    let observed_owner = Rc::clone(&owner);
    let (radio, task) = prepare_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::with_mode(FakeEpochMode::RejectOnce),
        owner,
    )
    .unwrap_or_else(|_| panic!("fresh supervisor must prepare"));

    let application = async {
        let mut port = radio.into_wifi().into_port();
        match port.start_station(station_request()).await {
            Err(WifiStartFailure::Rejected {
                request,
                error: WifiSupervisorError::Service("not-started"),
            }) => assert_eq!(request.ssid().as_bytes(), b"mailbox"),
            _ => panic!("NotStarted must retain the exact rejected request"),
        }
        let started = port.start_station(station_request()).await.unwrap();
        assert_eq!(started.generation().value(), 1);
        port.stop().await.unwrap();
        assert_eq!(observed_owner.get(), 1);
        assert_eq!(Rc::strong_count(&observed_owner), 2);
    };
    assert!(matches!(
        run(select(application, task.run())),
        Either::First(())
    ));
}

#[test]
fn role_fault_and_lifecycle_fault_keep_distinct_owner_frontiers() {
    for mode in [FakeEpochMode::ActiveFault, FakeEpochMode::LifecycleFault] {
        let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
        let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES).with_station(
            WifiStationConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap()),
        );
        let owner = Rc::new(Cell::new(7));
        let observed_owner = Rc::clone(&owner);
        let (radio, task) = prepare_wifi_supervisor(
            &resources,
            configuration,
            FakeLocalEpochRunner::with_mode(mode),
            owner,
        )
        .unwrap_or_else(|_| panic!("fresh supervisor must prepare"));

        let application = async {
            let mut port = radio.into_wifi().into_port();
            let expected = match mode {
                FakeEpochMode::ActiveFault => {
                    assert!(matches!(
                        port.start_station(station_request()).await,
                        Err(WifiStartFailure::Faulted {
                            error: WifiSupervisorError::Service("faulted")
                        })
                    ));
                    "faulted"
                }
                FakeEpochMode::LifecycleFault => {
                    assert_eq!(
                        port.restart_radio().await,
                        Err(WifiSupervisorError::Service("lifecycle-faulted"))
                    );
                    "lifecycle-faulted"
                }
                _ => unreachable!(),
            };
            assert_eq!(
                port.stop().await,
                Err(WifiSupervisorError::Service(expected))
            );
            assert_eq!(observed_owner.get(), 7);
            assert_eq!(Rc::strong_count(&observed_owner), 2);
        };
        assert!(matches!(
            run(select(application, task.run())),
            Either::First(())
        ));
    }
}

#[test]
fn supervisor_prepare_failure_returns_runner_and_stopped_owner() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES);
    let (_radio, _task) = prepare_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::immediate(),
        Rc::new(Cell::new(1)),
    )
    .unwrap_or_else(|_| panic!("fresh supervisor resources must prepare once"));

    let retained = Rc::new(Cell::new(7));
    let failure = match prepare_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::immediate(),
        Rc::clone(&retained),
    ) {
        Ok(_) => panic!("one static control domain cannot prepare two actors"),
        Err(failure) => failure,
    };
    assert_eq!(failure.error(), WifiSupervisorControlError::InUse);
    let (returned_configuration, _runner, returned_owner) = failure.into_parts();
    assert_eq!(returned_configuration, configuration);
    assert!(Rc::ptr_eq(&returned_owner, &retained));
}

#[test]
fn cancelled_restart_caller_does_not_cancel_the_owner_actor() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES).with_station(
        WifiStationConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap()),
    );
    let owner = Rc::new(Cell::new(0));
    let observed_owner = Rc::clone(&owner);
    let controls = Rc::new(RestartControls::new());
    let (radio, task) = prepare_wifi_supervisor(
        &resources,
        configuration,
        FakeLocalEpochRunner::with_restart(Rc::clone(&controls)),
        owner,
    )
    .unwrap_or_else(|_| panic!("fresh supervisor must prepare"));

    let application = async {
        let mut port = radio.into_wifi().into_port();
        let restart = port.restart_radio();
        assert!(matches!(
            select(restart, controls.started.wait()).await,
            Either::Second(())
        ));
        controls.release.signal(());
        controls.completed.wait().await;

        let station = port.start_station(station_request()).await.unwrap();
        station.generation().value()
    };
    let Either::First(generation) = run(select(application, task.run()));
    assert_eq!(generation, 2);
    assert_eq!(observed_owner.get(), 11);
}

#[test]
fn disappearing_supervisor_wakes_an_inflight_controller() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let application = async { radio.into_wifi().start_station(station_request()).await };
    let supervisor = async {
        let _ = endpoint.receive().await;
        drop(endpoint);
    };
    let (result, ()) = run(join(application, supervisor));
    assert!(matches!(
        result,
        Err(oer_radio::wifi::WifiRoleStartFailure::Faulted {
            error: WifiSupervisorError::SupervisorUnavailable
        })
    ));
}

#[test]
fn supervisor_drop_does_not_overwrite_a_ready_completion() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let mut port = radio.into_wifi().into_port();
    let mut operation = core::pin::pin!(port.start_station(station_request()));
    assert!(poll_once(operation.as_mut()).is_pending());
    {
        let mut receive = core::pin::pin!(endpoint.receive());
        assert!(matches!(
            poll_once(receive.as_mut()),
            Poll::Ready(WifiSupervisorCommand::StartStation(_))
        ));
    }
    {
        let mut respond = core::pin::pin!(endpoint.respond(WifiSupervisorResponse::Station(Ok(
            WifiStartReport::new(oer_radio::wifi::RadioSubsystemGeneration::INITIAL.next(),)
        )),));
        assert!(poll_once(respond.as_mut()).is_ready());
    }
    drop(endpoint);
    match poll_once(operation.as_mut()) {
        Poll::Ready(Ok(report)) => assert_eq!(report.generation().value(), 1),
        _ => panic!("the ready completion must survive a full-queue Drop"),
    }
}

#[test]
fn cancelled_transport_future_cannot_poison_the_next_command() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let mut port = radio.into_wifi().into_port();
    let cancel = Signal::<NoopRawMutex, ()>::new();
    let cancelled = Signal::<NoopRawMutex, ()>::new();

    let application = async {
        let first = port.start_station(station_request());
        assert!(matches!(
            select(first, cancel.wait()).await,
            Either::Second(())
        ));
        cancelled.signal(());

        let result = port
            .start_monitor(MonitorRequest::new(
                WifiChannel::mhz20(6).unwrap(),
                WifiMonitorConfig::normalized(),
            ))
            .await;
        assert!(result.is_ok());
    };
    let supervisor = async {
        assert!(matches!(
            endpoint.receive().await,
            WifiSupervisorCommand::StartStation(_)
        ));
        cancel.signal(());
        cancelled.wait().await;
        endpoint
            .respond(WifiSupervisorResponse::Station(Ok(WifiStartReport::new(
                oer_radio::wifi::RadioSubsystemGeneration::INITIAL,
            ))))
            .await;

        assert!(matches!(
            endpoint.receive().await,
            WifiSupervisorCommand::StartMonitor(_)
        ));
        endpoint
            .respond(WifiSupervisorResponse::Monitor(Ok(WifiStartReport::new(
                oer_radio::wifi::RadioSubsystemGeneration::INITIAL.next(),
            ))))
            .await;
    };

    run(join(application, supervisor));
}

#[test]
fn cancellation_before_first_poll_publishes_no_command() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let mut port = radio.into_wifi().into_port();
    drop(port.start_station(station_request()));
    assert!(resources.no_pending_command_for_test());

    let mut monitor = core::pin::pin!(port.start_monitor(MonitorRequest::new(
        WifiChannel::mhz20(6).unwrap(),
        WifiMonitorConfig::normalized(),
    )));
    assert!(poll_once(monitor.as_mut()).is_pending());
    {
        let mut receive = core::pin::pin!(endpoint.receive());
        assert!(matches!(
            poll_once(receive.as_mut()),
            Poll::Ready(WifiSupervisorCommand::StartMonitor(_))
        ));
    }
    {
        let mut respond = core::pin::pin!(endpoint.respond(WifiSupervisorResponse::Monitor(Ok(
            WifiStartReport::new(oer_radio::wifi::RadioSubsystemGeneration::INITIAL,)
        )),));
        assert!(poll_once(respond.as_mut()).is_ready());
    }
    assert!(matches!(poll_once(monitor.as_mut()), Poll::Ready(Ok(_))));
    assert!(resources.no_pending_command_for_test());
}

#[test]
fn reconciliation_applies_backpressure_without_duplicate_publication() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let mut port = radio.into_wifi().into_port();
    {
        let mut first = core::pin::pin!(port.start_station(station_request()));
        assert!(poll_once(first.as_mut()).is_pending());
        let mut receive = core::pin::pin!(endpoint.receive());
        assert!(matches!(
            poll_once(receive.as_mut()),
            Poll::Ready(WifiSupervisorCommand::StartStation(_))
        ));
    }
    let mut next = core::pin::pin!(port.start_monitor(MonitorRequest::new(
        WifiChannel::mhz20(11).unwrap(),
        WifiMonitorConfig::normalized(),
    )));
    assert!(poll_once(next.as_mut()).is_pending());
    assert!(resources.no_pending_command_for_test());

    {
        let mut respond = core::pin::pin!(endpoint.respond(WifiSupervisorResponse::Station(Ok(
            WifiStartReport::new(oer_radio::wifi::RadioSubsystemGeneration::INITIAL,)
        )),));
        assert!(poll_once(respond.as_mut()).is_ready());
    }
    assert!(poll_once(next.as_mut()).is_pending());
    {
        let mut receive = core::pin::pin!(endpoint.receive());
        match poll_once(receive.as_mut()) {
            Poll::Ready(WifiSupervisorCommand::StartMonitor(request)) => {
                assert_eq!(request.channel().primary(), 11)
            }
            _ => panic!("only the next typed command may be published"),
        }
    }
    assert!(resources.no_pending_command_for_test());
    assert!(poll_once(next.as_mut()).is_pending());
    assert!(resources.no_pending_command_for_test());

    {
        let mut respond = core::pin::pin!(endpoint.respond(WifiSupervisorResponse::Monitor(Ok(
            WifiStartReport::new(oer_radio::wifi::RadioSubsystemGeneration::INITIAL.next(),)
        )),));
        assert!(poll_once(respond.as_mut()).is_ready());
    }
    assert!(matches!(poll_once(next.as_mut()), Poll::Ready(Ok(_))));
    assert!(resources.no_pending_command_for_test());
}

#[test]
fn cancelled_completion_then_supervisor_drop_rejects_unpublished_request() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let mut port = radio.into_wifi().into_port();

    {
        let mut first = core::pin::pin!(port.start_station(station_request()));
        assert!(poll_once(first.as_mut()).is_pending());
        let mut receive = core::pin::pin!(endpoint.receive());
        assert!(matches!(
            poll_once(receive.as_mut()),
            Poll::Ready(WifiSupervisorCommand::StartStation(_))
        ));
    }

    let request = MonitorRequest::new(
        WifiChannel::mhz20(6).unwrap(),
        WifiMonitorConfig::normalized(),
    );
    let mut second = core::pin::pin!(port.start_monitor(request));
    assert!(poll_once(second.as_mut()).is_pending());
    drop(endpoint);

    match poll_once(second.as_mut()) {
        Poll::Ready(Err(WifiStartFailure::Rejected {
            request,
            error: WifiSupervisorError::SupervisorUnavailable,
        })) => assert_eq!(request.channel().primary(), 6),
        Poll::Ready(_) => panic!("unpublished request must retain its identity"),
        Poll::Pending => panic!("dead supervisor cannot complete a newly published command"),
    }
    assert!(resources.no_pending_command_for_test());
}

#[test]
fn ready_cancelled_completion_then_supervisor_drop_rejects_next_request() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let mut port = radio.into_wifi().into_port();

    {
        let mut first = core::pin::pin!(port.start_station(station_request()));
        assert!(poll_once(first.as_mut()).is_pending());
        let mut receive = core::pin::pin!(endpoint.receive());
        assert!(matches!(
            poll_once(receive.as_mut()),
            Poll::Ready(WifiSupervisorCommand::StartStation(_))
        ));
    }
    {
        let mut respond = core::pin::pin!(endpoint.respond(WifiSupervisorResponse::Station(Ok(
            WifiStartReport::new(oer_radio::wifi::RadioSubsystemGeneration::INITIAL,)
        )),));
        assert!(poll_once(respond.as_mut()).is_ready());
    }
    drop(endpoint);

    let mut next = core::pin::pin!(port.start_monitor(MonitorRequest::new(
        WifiChannel::mhz20(11).unwrap(),
        WifiMonitorConfig::normalized(),
    )));
    match poll_once(next.as_mut()) {
        Poll::Ready(Err(WifiStartFailure::Rejected {
            request,
            error: WifiSupervisorError::SupervisorUnavailable,
        })) => assert_eq!(request.channel().primary(), 11),
        Poll::Ready(_) => panic!("ready old completion cannot become the next response"),
        Poll::Pending => panic!("dead supervisor cannot complete the next command"),
    }
    assert!(resources.no_pending_command_for_test());
}

#[test]
fn cancelling_reconciliation_does_not_publish_a_later_request_after_drop() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let mut port = radio.into_wifi().into_port();

    {
        let mut first = core::pin::pin!(port.start_station(station_request()));
        assert!(poll_once(first.as_mut()).is_pending());
        let mut receive = core::pin::pin!(endpoint.receive());
        assert!(matches!(
            poll_once(receive.as_mut()),
            Poll::Ready(WifiSupervisorCommand::StartStation(_))
        ));
    }
    {
        let mut reconciling = core::pin::pin!(port.start_monitor(MonitorRequest::new(
            WifiChannel::mhz20(6).unwrap(),
            WifiMonitorConfig::normalized(),
        )));
        assert!(poll_once(reconciling.as_mut()).is_pending());
    }
    drop(endpoint);

    let mut later = core::pin::pin!(port.start_monitor(MonitorRequest::new(
        WifiChannel::mhz20(11).unwrap(),
        WifiMonitorConfig::normalized(),
    )));
    match poll_once(later.as_mut()) {
        Poll::Ready(Err(WifiStartFailure::Rejected {
            request,
            error: WifiSupervisorError::SupervisorUnavailable,
        })) => assert_eq!(request.channel().primary(), 11),
        Poll::Ready(_) => panic!("cancelled reconciliation must retain later request"),
        Poll::Pending => panic!("dead supervisor cannot complete a later command"),
    }
    assert!(resources.no_pending_command_for_test());
}

#[test]
fn monitor_policy_remains_an_owned_typed_command() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let request = MonitorRequest::new(
        WifiChannel::mhz20(11).unwrap(),
        WifiMonitorConfig::normalized(),
    )
    .with_capture_policy(MonitorCapturePolicy::truncate_at(
        NonZeroU16::new(256).unwrap(),
    ));
    let application = async { radio.into_wifi().start_monitor(request).await };
    let supervisor = async {
        let WifiSupervisorCommand::StartMonitor(request) = endpoint.receive().await else {
            panic!("expected monitor request")
        };
        assert_eq!(request.channel().primary(), 11);
        assert_eq!(request.capture_policy().snapshot_length(), Some(256));
        endpoint
            .respond(WifiSupervisorResponse::Monitor(Ok(WifiStartReport::new(
                oer_radio::wifi::RadioSubsystemGeneration::INITIAL,
            ))))
            .await;
    };
    let (result, ()) = run(join(application, supervisor));
    assert!(result.is_ok());
}

#[test]
fn active_role_stop_is_acknowledged_only_after_owner_future_returns() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let stop = Signal::<NoopRawMutex, ()>::new();
    let stage = AtomicU8::new(0);
    let mut control = TestRoleControl {
        stop: &stop,
        stage: &stage,
    };

    let application = async {
        let mut port = radio.into_wifi().into_port();
        port.stop().await.unwrap()
    };
    let supervisor = async {
        let role = async {
            stop.wait().await;
            stage.store(2, Ordering::Release);
            "returned-owner"
        };
        let exit = drive_wifi_active_role(&mut endpoint, &mut control, role, |_| {
            panic!("no start command expected")
        })
        .await;
        assert!(exit.stop_requested());
        assert_eq!(stage.load(Ordering::Acquire), 2);

        let frontier = finish_wifi_active_role(
            &mut endpoint,
            oer_radio::wifi::RadioSubsystemGeneration::INITIAL,
            exit,
            WifiRoleFrontier::<_, ()>::Stopped,
            |_| unreachable!("the test role returned a stopped owner"),
        )
        .await;
        assert!(matches!(
            frontier,
            WifiRoleFrontier::Stopped("returned-owner")
        ));
    };
    let (report, ()) = run(join(application, supervisor));
    assert_eq!(
        report.generation(),
        oer_radio::wifi::RadioSubsystemGeneration::INITIAL
    );
    assert_eq!(stage.load(Ordering::Acquire), 2);
}

#[test]
fn simultaneous_role_and_command_readiness_prefers_the_pinned_owner() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let mut port = radio.into_wifi().into_port();
    let mut stop = core::pin::pin!(port.stop());
    assert!(poll_once(stop.as_mut()).is_pending());

    let stop_signal = Signal::<NoopRawMutex, ()>::new();
    let stage = AtomicU8::new(0);
    let mut control = TestRoleControl {
        stop: &stop_signal,
        stage: &stage,
    };
    let owner = Rc::new(Cell::new(9));
    let role = async { Rc::clone(&owner) };
    let mut role = core::pin::pin!(role);
    let exit = {
        let mut drive = core::pin::pin!(drive_wifi_active_role_pinned(
            &mut endpoint,
            &mut control,
            role.as_mut(),
            |_| unreachable!("the ready role wins before command dispatch"),
        ));
        match poll_once(drive.as_mut()) {
            Poll::Ready(exit) => exit,
            Poll::Pending => panic!("the ready pinned role must win this poll"),
        }
    };
    assert!(!exit.stop_requested());
    assert_eq!(stage.load(Ordering::Acquire), 0);
    let frontier = run(finish_wifi_active_role(
        &mut endpoint,
        oer_radio::wifi::RadioSubsystemGeneration::INITIAL,
        exit,
        WifiRoleFrontier::<_, ()>::Stopped,
        |_| unreachable!(),
    ));
    let WifiRoleFrontier::Stopped(returned) = frontier else {
        panic!("role returned its exact owner")
    };
    assert!(Rc::ptr_eq(&returned, &owner));
    assert!(poll_once(stop.as_mut()).is_pending());

    let mut dispatch = core::pin::pin!(dispatch_wifi_stopped_command(
        &mut endpoint,
        WifiSupervisorConfiguration::new(TEST_CAPABILITIES),
        oer_radio::wifi::RadioSubsystemGeneration::INITIAL,
        |_| unreachable!(),
    ));
    assert!(matches!(
        poll_once(dispatch.as_mut()),
        Poll::Ready(WifiStoppedDispatch::Handled)
    ));
    match poll_once(stop.as_mut()) {
        Poll::Ready(Ok(report)) => assert_eq!(report.generation().value(), 0),
        _ => panic!("stopped dispatch must answer the still-queued stop"),
    }
}

#[test]
fn faulted_role_never_produces_a_successful_stop_response() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let stop = Signal::<NoopRawMutex, ()>::new();
    let stage = AtomicU8::new(0);
    let mut control = TestRoleControl {
        stop: &stop,
        stage: &stage,
    };

    let application = async {
        let mut port = radio.into_wifi().into_port();
        port.stop().await
    };
    let supervisor = async {
        let role = async {
            stop.wait().await;
            "retained-faulted-owner"
        };
        let exit = drive_wifi_active_role(&mut endpoint, &mut control, role, |_| {
            unreachable!("no start command expected")
        })
        .await;
        finish_wifi_active_role(
            &mut endpoint,
            oer_radio::wifi::RadioSubsystemGeneration::INITIAL,
            exit,
            WifiRoleFrontier::<(), _>::Faulted,
            |owner| {
                assert_eq!(*owner, "retained-faulted-owner");
                "faulted"
            },
        )
        .await
    };

    let (result, frontier) = run(join(application, supervisor));
    assert_eq!(result, Err(WifiSupervisorError::Service("faulted")));
    assert!(matches!(
        frontier,
        WifiRoleFrontier::Faulted("retained-faulted-owner")
    ));
}

#[test]
fn active_role_rejects_a_new_start_with_the_untouched_request() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let stop = Signal::<NoopRawMutex, ()>::new();
    let finish = Signal::<NoopRawMutex, ()>::new();
    let stage = AtomicU8::new(0);
    let mut control = TestRoleControl {
        stop: &stop,
        stage: &stage,
    };

    let application = async { radio.into_wifi().start_station(station_request()).await };
    let supervisor = async {
        let role = async {
            finish.wait().await;
            "returned-owner"
        };
        let exit = drive_wifi_active_role(&mut endpoint, &mut control, role, |kind| {
            assert_eq!(kind, WifiStartKind::Station);
            finish.signal(());
            "already-running"
        })
        .await;
        assert_eq!(exit.output(), &"returned-owner");
        assert!(!exit.stop_requested());
    };

    let (result, ()) = run(join(application, supervisor));
    match result {
        Err(oer_radio::wifi::WifiRoleStartFailure::Rejected {
            wifi: _,
            request,
            error: WifiSupervisorError::Service("already-running"),
        }) => assert_eq!(request.ssid().as_bytes(), b"mailbox"),
        _ => panic!("active role must return the exact rejected request"),
    }
}

#[test]
fn active_role_identifies_a_rejected_restart() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, &'static str>::new();
    let (radio, mut endpoint) = resources.split().unwrap();
    let stop = Signal::<NoopRawMutex, ()>::new();
    let finish = Signal::<NoopRawMutex, ()>::new();
    let stage = AtomicU8::new(0);
    let mut control = TestRoleControl {
        stop: &stop,
        stage: &stage,
    };

    let application = async {
        match radio.into_wifi().restart_radio().await {
            Ok(_) => panic!("an active role cannot restart the radio"),
            Err(error) => error,
        }
    };
    let supervisor = async {
        let role = async {
            finish.wait().await;
            "returned-owner"
        };
        let exit = drive_wifi_active_role(&mut endpoint, &mut control, role, |kind| {
            assert_eq!(kind, WifiStartKind::WholeRadioRestart);
            finish.signal(());
            "already-running"
        })
        .await;
        assert_eq!(exit.output(), &"returned-owner");
        assert!(!exit.stop_requested());
    };

    let (error, ()) = run(join(application, supervisor));
    assert_eq!(error, WifiSupervisorError::Service("already-running"));
}

#[test]
fn mailbox_storage_cannot_be_split_twice() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let _ = resources.split().unwrap();
    assert!(matches!(
        resources.split(),
        Err(WifiSupervisorControlError::InUse)
    ));
}

#[test]
fn supervisor_drop_does_not_reopen_split_storage() {
    let resources = WifiSupervisorControlResources::<NoopRawMutex, ()>::new();
    let (_radio, endpoint) = resources.split().unwrap();
    drop(endpoint);
    assert!(matches!(
        resources.split(),
        Err(WifiSupervisorControlError::InUse)
    ));
}
