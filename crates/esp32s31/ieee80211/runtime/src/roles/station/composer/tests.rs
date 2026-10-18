use core::num::NonZeroU16;
use oer_time_virtual::SkipClock;

use oer_ieee80211_mac::sequence::SequenceNumber;

use crate::roles::station::StationControlResources;

use embassy_futures::block_on;

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use {
    oer_ieee80211_mac::scan::ScanRecord, oer_ieee80211_mac::station::StaTxSequenceCounters,
    oer_ieee80211_mac::station::association::Preference,
};

use oer_ieee80211_sta::request::{StationScanChannels, StationScanPolicy, WifiSsid};

use oer_ieee80211_rsn::Pmk;

use super::*;

fn discovery() -> StationDiscovery {
    StationDiscovery::new(
        WifiSsid::new(b"ssid").expect("test SSID is valid"),
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(20).expect("test scan dwell is nonzero"),
            Preference::Automatic,
        ),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FakeError {
    RefreshContract,
}

struct FakePort {
    clock: SkipClock,
}

#[derive(Default)]
struct FakeObserver {
    started: Option<StationServicePhaseKind>,
    finished: bool,
}

impl<'security> StationEnginePort<'security, NoopRawMutex> for FakePort {
    type Runtime = u8;
    type InitialHardware = u16;
    type InitialScanRx = i16;
    type RxFrontier = u32;
    type Network = u64;
    type Disconnected = u128;
    type Reconnected = usize;
    type Connected = ();
    type Error = FakeError;
    type Fault = core::convert::Infallible;
    type Timer = SkipClock;

    fn timer(&self) -> &SkipClock {
        &self.clock
    }

    fn run_initial_scan<'a>(
        &'a mut self,
        _phase: StationInitialScanPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::InitialScanRx,
            Self::Network,
        >,
        _discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<
        Output = StationInitialScanExit<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
            StationEngineOwner<'security, NoopRawMutex, Self>,
            Self::Error,
        >,
    > + 'a
    where
        'security: 'a,
    {
        async { panic!("refresh-contract test must not enter initial scan") }
    }

    fn run_initial_join<'a>(
        &'a mut self,
        _phase: StationInitialJoinPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
        >,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<
        Output = StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            StationEngineOwner<'security, NoopRawMutex, Self>,
            Self::Error,
        >,
    > + 'a
    where
        'security: 'a,
    {
        async { panic!("refresh-contract test must not enter initial") }
    }

    fn run_running_scan<'a>(
        &'a mut self,
        _phase: StationRunningScanPhase<'security, Self::Runtime, Self::Disconnected>,
        _discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<
        Output = StationRunningScanExit<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
            StationEngineOwner<'security, NoopRawMutex, Self>,
            Self::Error,
        >,
    > + 'a
    where
        'security: 'a,
    {
        async { panic!("missing refresh must be rejected before scan") }
    }

    fn run_reconnected<'a>(
        &'a mut self,
        _phase: StationReconnectedPhase<'security, Self::Runtime, Self::Reconnected, Self::Network>,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<
        Output = StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            StationEngineOwner<'security, NoopRawMutex, Self>,
            Self::Error,
        >,
    > + 'a
    where
        'security: 'a,
    {
        async { panic!("refresh-contract test must not enter reconnect") }
    }

    fn run_connected<'a>(
        &'a mut self,
        _phase: StationConnectedPhase<'security, Self::Runtime, Self::Connected>,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<
        Output = StaAttemptOutcome<StationEngineOwner<'security, NoopRawMutex, Self>, Self::Error>,
    > + 'a
    where
        'security: 'a,
    {
        async { panic!("refresh-contract test must not enter connected") }
    }

    fn candidate_refresh_contract_error(&mut self) -> Self::Error {
        FakeError::RefreshContract
    }
}

impl<'security> StationEngineObserver<'security, NoopRawMutex, FakePort> for FakeObserver {
    fn attempt_started(&mut self, _context: StaAttemptContext, phase: StationServicePhaseKind) {
        self.started = Some(phase);
    }

    fn attempt_finished<'a>(
        &'a mut self,
        _context: StaAttemptContext,
        _outcome: &'a StaAttemptOutcome<
            StationEngineOwner<'security, NoopRawMutex, FakePort>,
            FakeError,
        >,
    ) -> impl Future<Output = ()> + 'a
    where
        'security: 'a,
    {
        async move {
            self.finished = true;
        }
    }
}

#[derive(Default)]
struct ScanTransitionPort {
    initial_joined: bool,
    reconnected: bool,
    clock: SkipClock,
}

impl<'security> StationEnginePort<'security, NoopRawMutex> for ScanTransitionPort {
    type Runtime = u8;
    type InitialHardware = u16;
    type InitialScanRx = i16;
    type RxFrontier = u32;
    type Network = u64;
    type Disconnected = u128;
    type Reconnected = usize;
    type Connected = ();
    type Error = FakeError;
    type Fault = core::convert::Infallible;
    type Timer = SkipClock;

    fn timer(&self) -> &SkipClock {
        &self.clock
    }

    fn run_initial_scan<'a>(
        &'a mut self,
        phase: StationInitialScanPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::InitialScanRx,
            Self::Network,
        >,
        _discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<
        Output = StationInitialScanExit<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
            StationEngineOwner<'security, NoopRawMutex, Self>,
            Self::Error,
        >,
    > + 'a
    where
        'security: 'a,
    {
        async move {
            let (runtime, hardware, scan_rx, network, identity, security) = phase.into_parts();
            assert_eq!(scan_rx, -17);
            let candidate = ScanRecord {
                bssid: [0x02, 0, 0, 0, 0, 6],
                channel: 6,
                rssi: -42,
                ..ScanRecord::EMPTY
            };
            StationInitialScanExit::join_ready(
                runtime,
                hardware,
                41,
                network,
                identity.select(candidate),
                security,
            )
        }
    }

    fn run_initial_join<'a>(
        &'a mut self,
        phase: StationInitialJoinPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
        >,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<
        Output = StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            StationEngineOwner<'security, NoopRawMutex, Self>,
            Self::Error,
        >,
    > + 'a
    where
        'security: 'a,
    {
        async move {
            let (runtime, hardware, receive, network, station, security) = phase.into_parts();
            assert_eq!((hardware, receive, network), (11, 41, 13));
            assert_eq!(station.access_point.channel, 6);
            self.initial_joined = true;
            StationJoinExit::complete(StaAttemptOutcome::Stopped {
                owner: StationServiceOwner::new(
                    runtime,
                    StationServicePhase::InitialJoin {
                        hardware,
                        receive,
                        network,
                        station,
                    },
                    security,
                ),
            })
        }
    }

    fn run_running_scan<'a>(
        &'a mut self,
        phase: StationRunningScanPhase<'security, Self::Runtime, Self::Disconnected>,
        _discovery: StationDiscovery,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<
        Output = StationRunningScanExit<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
            StationEngineOwner<'security, NoopRawMutex, Self>,
            Self::Error,
        >,
    > + 'a
    where
        'security: 'a,
    {
        async move {
            let (runtime, disconnected, station, security) = phase.into_parts();
            assert_eq!(disconnected, 19);
            StationRunningScanExit::join_ready(runtime, 23, 29, station, security)
        }
    }

    fn run_reconnected<'a>(
        &'a mut self,
        phase: StationReconnectedPhase<'security, Self::Runtime, Self::Reconnected, Self::Network>,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<
        Output = StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            StationEngineOwner<'security, NoopRawMutex, Self>,
            Self::Error,
        >,
    > + 'a
    where
        'security: 'a,
    {
        async move {
            let (runtime, epoch, network, station, security) = phase.into_parts();
            assert_eq!((epoch, network), (23, 29));
            self.reconnected = true;
            StationJoinExit::complete(StaAttemptOutcome::Stopped {
                owner: StationServiceOwner::new(
                    runtime,
                    StationServicePhase::Reconnected {
                        epoch,
                        network,
                        station,
                    },
                    security,
                ),
            })
        }
    }

    fn run_connected<'a>(
        &'a mut self,
        _phase: StationConnectedPhase<'security, Self::Runtime, Self::Connected>,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<
        Output = StaAttemptOutcome<StationEngineOwner<'security, NoopRawMutex, Self>, Self::Error>,
    > + 'a
    where
        'security: 'a,
    {
        async { panic!("transition test does not enter connected") }
    }

    fn candidate_refresh_contract_error(&mut self) -> Self::Error {
        FakeError::RefreshContract
    }
}

static TEST_PMKSA: oer_esp32s31_ieee80211_sta::attempt::StaSharedPmksa =
    oer_esp32s31_ieee80211_sta::attempt::StaSharedPmksa::new();

#[test]
fn phase_owner_returns_runtime_target_and_security_without_reconstruction() {
    let pmk = Pmk::derive(b"password", b"ssid").expect("test WPA2 input is valid");
    let sequences = StaTxSequenceCounters::new(SequenceNumber::new(9).unwrap());
    let station = StaAttemptStation {
        station_address: [2, 0, 0, 0, 0, 1],
        access_point: ScanRecord::EMPTY,
        association_preference: Preference::Automatic,
        security: oer_ieee80211_mac::security::StaSecurityPolicy::Wpa2Personal,
    };
    let owner = StationServiceOwner::new(
        7_u8,
        StationServicePhase::<u16, i16, u32, u64, u128, usize, ()>::InitialJoin {
            hardware: 11,
            receive: 12,
            network: 13,
            station,
        },
        StaAttemptSecurity::new(
            oer_esp32s31_ieee80211_sta::attempt::StaPersonalCredentials::wpa2(
                pmk,
                oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                || 7,
                &TEST_PMKSA,
            ),
            [0x5a; 32],
            sequences,
            oer_esp32s31_ieee80211_sta::wpa2::Wpa2Message4Protection::PairwiseCcmp,
        ),
    );
    let (runtime, phase, security) = owner.into_parts();
    assert_eq!(runtime, 7);
    let StationServicePhase::InitialJoin {
        hardware,
        receive,
        network,
        station: returned_station,
    } = phase
    else {
        panic!("initial owner must remain initial");
    };
    assert_eq!((hardware, receive, network), (11, 12, 13));
    assert_eq!(returned_station.station_address, station.station_address);
    assert_eq!(
        security
            .wpa2_material()
            .expect("test owner retains WPA2 material")
            .1,
        [0x5a; 32]
    );
    assert_eq!(
        security.sequences.peek_non_qos(),
        SequenceNumber::new(9).unwrap()
    );
}

#[test]
fn common_engine_rejects_running_scan_without_refresh_before_port_entry() {
    let pmk = Pmk::derive(b"password", b"ssid").expect("test WPA2 input is valid");
    let sequences = StaTxSequenceCounters::new(SequenceNumber::new(3).unwrap());
    let station = StaAttemptStation {
        station_address: [2, 0, 0, 0, 0, 2],
        access_point: ScanRecord::EMPTY,
        association_preference: Preference::Automatic,
        security: oer_ieee80211_mac::security::StaSecurityPolicy::Wpa2Personal,
    };
    let owner = StationServiceOwner::new(
        7_u8,
        StationServicePhase::<u16, i16, u32, u64, u128, usize, ()>::RunningScan {
            disconnected: 19,
            station,
        },
        StaAttemptSecurity::new(
            oer_esp32s31_ieee80211_sta::attempt::StaPersonalCredentials::wpa2(
                pmk,
                oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                || 7,
                &TEST_PMKSA,
            ),
            [0x33; 32],
            sequences,
            oer_esp32s31_ieee80211_sta::wpa2::Wpa2Message4Protection::PairwiseCcmp,
        ),
    );
    let control = StationControlResources::<NoopRawMutex>::new();
    let (_controller, mut receiver) = control.split().expect("fresh control domain splits");
    let mut runner = StationEngine::with_observer(
        FakePort {
            clock: SkipClock::new(),
        },
        discovery(),
        FakeObserver::default(),
    );
    let outcome = block_on(runner.run_attempt(
        owner,
        StaAttemptContext {
            generation: 1,
            attempt: 2,
            refresh_candidate: false,
        },
        &mut receiver,
    ));
    let StaAttemptOutcome::Failed { owner, failure } = outcome else {
        panic!("missing refresh must be a finite contract failure");
    };
    assert_eq!(failure.stage, StaLifecycleStage::CandidateSelection);
    assert_eq!(failure.disposition, StaFailureDisposition::Terminal);
    assert_eq!(failure.error, FakeError::RefreshContract);
    assert!(matches!(
        owner.phase,
        StationServicePhase::RunningScan {
            disconnected: 19,
            ..
        }
    ));
    let (_port, _discovery, observer) = runner.into_parts();
    assert_eq!(observer.started, Some(StationServicePhaseKind::RunningScan));
    assert!(observer.finished);
}

#[test]
fn common_engine_selects_candidate_before_dispatching_initial_join() {
    let pmk = Pmk::derive(b"password", b"ssid").expect("test WPA2 input is valid");
    let sequences = StaTxSequenceCounters::new(SequenceNumber::new(5).unwrap());
    let identity = StaIdentity {
        station_address: [2, 0, 0, 0, 0, 4],
        association_preference: Preference::Automatic,
        security: oer_ieee80211_mac::security::StaSecurityPolicy::Wpa2Personal,
    };
    let owner = StationServiceOwner::new(
        7_u8,
        StationServicePhase::<u16, i16, u32, u64, u128, usize, ()>::InitialScan {
            hardware: 11,
            receive: -17,
            network: 13,
            identity,
        },
        StaAttemptSecurity::new(
            oer_esp32s31_ieee80211_sta::attempt::StaPersonalCredentials::wpa2(
                pmk,
                oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                || 7,
                &TEST_PMKSA,
            ),
            [0x55; 32],
            sequences,
            oer_esp32s31_ieee80211_sta::wpa2::Wpa2Message4Protection::PairwiseCcmp,
        ),
    );
    let control = StationControlResources::<NoopRawMutex>::new();
    let (_controller, mut receiver) = control.split().expect("fresh control domain splits");
    let mut runner = StationEngine::new(ScanTransitionPort::default(), discovery());
    let outcome = block_on(runner.run_attempt(
        owner,
        StaAttemptContext {
            generation: 0,
            attempt: 1,
            refresh_candidate: true,
        },
        &mut receiver,
    ));
    let StaAttemptOutcome::Advanced { owner } = outcome else {
        panic!("initial scan must return the next finite phase");
    };
    let StationServicePhase::InitialJoin { ref station, .. } = owner.phase else {
        panic!("candidate selection must precede initial join");
    };
    assert_eq!(station.station_address, identity.station_address);
    assert_eq!(station.access_point.channel, 6);
    assert!(!runner.port().initial_joined);
    let outcome = block_on(runner.run_attempt(
        owner,
        StaAttemptContext {
            generation: 0,
            attempt: 1,
            refresh_candidate: true,
        },
        &mut receiver,
    ));
    assert!(matches!(outcome, StaAttemptOutcome::Stopped { .. }));
    assert!(runner.into_port().initial_joined);
}

#[test]
fn common_engine_dispatches_join_ready_scan_owner_to_reconnected_phase() {
    let pmk = Pmk::derive(b"password", b"ssid").expect("test WPA2 input is valid");
    let sequences = StaTxSequenceCounters::new(SequenceNumber::new(4).unwrap());
    let station = StaAttemptStation {
        station_address: [2, 0, 0, 0, 0, 3],
        access_point: ScanRecord::EMPTY,
        association_preference: Preference::Automatic,
        security: oer_ieee80211_mac::security::StaSecurityPolicy::Wpa2Personal,
    };
    let owner = StationServiceOwner::new(
        7_u8,
        StationServicePhase::<u16, i16, u32, u64, u128, usize, ()>::RunningScan {
            disconnected: 19,
            station,
        },
        StaAttemptSecurity::new(
            oer_esp32s31_ieee80211_sta::attempt::StaPersonalCredentials::wpa2(
                pmk,
                oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                || 7,
                &TEST_PMKSA,
            ),
            [0x44; 32],
            sequences,
            oer_esp32s31_ieee80211_sta::wpa2::Wpa2Message4Protection::PairwiseCcmp,
        ),
    );
    let control = StationControlResources::<NoopRawMutex>::new();
    let (_controller, mut receiver) = control.split().expect("fresh control domain splits");
    let mut runner = StationEngine::new(ScanTransitionPort::default(), discovery());
    let outcome = block_on(runner.run_attempt(
        owner,
        StaAttemptContext {
            generation: 2,
            attempt: 1,
            refresh_candidate: true,
        },
        &mut receiver,
    ));
    let StaAttemptOutcome::Advanced { owner } = outcome else {
        panic!("running scan must return the next finite phase");
    };
    assert!(matches!(
        owner.phase,
        StationServicePhase::Reconnected {
            epoch: 23,
            network: 29,
            ..
        }
    ));
    assert!(!runner.port().reconnected);
    let outcome = block_on(runner.run_attempt(
        owner,
        StaAttemptContext {
            generation: 2,
            attempt: 1,
            refresh_candidate: true,
        },
        &mut receiver,
    ));
    assert!(matches!(outcome, StaAttemptOutcome::Stopped { .. }));
    assert!(runner.into_port().reconnected);
}

#[test]
fn running_scan_completion_prepares_reconnect_only_for_a_selected_candidate() {
    let sequences = StaTxSequenceCounters::new(SequenceNumber::new(0).unwrap());
    let pmk = Pmk::derive(b"password", b"ssid").expect("test WPA2 input is valid");
    let security = StaAttemptSecurity::new(
        oer_esp32s31_ieee80211_sta::attempt::StaPersonalCredentials::wpa2(
            pmk,
            oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            || 7,
            &TEST_PMKSA,
        ),
        [0; 32],
        sequences,
        oer_esp32s31_ieee80211_sta::wpa2::Wpa2Message4Protection::PairwiseCcmp,
    );
    let original = StaAttemptStation {
        station_address: [2, 0, 0, 0, 0, 8],
        access_point: ScanRecord::EMPTY,
        association_preference: Preference::Automatic,
        security: oer_ieee80211_mac::security::StaSecurityPolicy::Wpa2Personal,
    };
    let candidate = ScanRecord {
        bssid: [0x02, 1, 2, 3, 4, 9],
        ..original.access_point
    };
    let exit = complete_esp32s31_station_running_scan::<_, _, _, _, _, _, core::convert::Infallible>(
        7_u8,
        11_u16,
        original,
        security,
        StationRunningScanCompletion::<FakeError>::Selected(candidate),
        |disconnected| (u32::from(disconnected), usize::from(disconnected) + 1),
        |_runtime, _disconnected, _station, _security| -> u64 {
            panic!("selected candidate must not restore the running-scan owner")
        },
    );
    match exit {
        StationRunningScanExit::JoinReady(phase) => {
            let (runtime, epoch, network, station, _security) = phase.into_parts();
            assert_eq!(runtime, 7);
            assert_eq!(epoch, 12);
            assert_eq!(network, 11);
            assert_eq!(station.access_point.bssid, candidate.bssid);
        }
        StationRunningScanExit::Complete(_) => panic!("expected reconnect frontier"),
    }
}
