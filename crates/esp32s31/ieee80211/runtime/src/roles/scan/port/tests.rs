use core::{
    future::{Future, ready},
    pin::pin,
    task::{Context, Poll},
};
use oer_esp32s31_ieee80211_sta::connection_coex::{
    ConnectionFrame, ConnectionFrameCoex, ReconnectFramePriority,
};
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_esp32s31_ieee80211_sta::scan::StaScanConfig;
use oer_ieee80211_sta_service::scan::StaScanBackend;

use oer_ieee80211_mac::scan::ScanObservation;

use oer_ieee80211_sta::scan::StaCandidateScanExit;
use oer_ieee80211_sta_service::scan::StaCandidateScanService;

use std::vec::Vec;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Prepare,
    Switch(u8),
    AdmissionOn,
    Start,
    ResumeMac,
    Probe(u8),
    Observe,
    AdmissionOff,
    StopMac,
}

#[derive(Default)]
struct Hardware {
    actions: Vec<Action>,
}

impl oer_esp32s31_ieee80211_mac::init::MacSnifferHardware for Hardware {
    fn configure_open_promiscuous_receive(&mut self) {
        self.actions.push(Action::AdmissionOn);
    }

    fn disable_open_promiscuous_receive(&mut self) {
        self.actions.push(Action::AdmissionOff);
    }
}

impl oer_esp32s31_ieee80211_mac::init::MacRuntimeStopHardware for Hardware {
    fn request_mac_runtime_stop(&mut self) {
        self.actions.push(Action::StopMac);
    }

    fn mac_runtime_active_state(&mut self) -> u8 {
        0
    }

    fn resume_mac_runtime(&mut self) {
        self.actions.push(Action::ResumeMac);
    }
}

struct Phy(u32);

impl ScanPhyPort<Hardware> for Phy {
    type Error = ();

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut Hardware,
        channel: u8,
        requested_dwell_millis: u16,
    ) -> impl Future<Output = Result<u16, Self::Error>> + 'a {
        hardware.actions.push(Action::Switch(channel));
        ready(Ok(requested_dwell_millis))
    }
    type Coex = NoCoex;

    fn connection_coex(&self) -> Self::Coex {
        NoCoex
    }
}

/// Coexistence without the reconnect policy: nothing is requested.
struct NoCoex;

impl ConnectionFrameCoex for NoCoex {
    fn connection_frame(
        &mut self,
        _frame: ConnectionFrame,
    ) -> impl Future<Output = Option<ReconnectFramePriority>> + '_ {
        ready(None)
    }
}

struct Receive(u32);

impl ScanReceivePort<Hardware> for Receive {
    type Error = ();

    fn prepare_initial(&mut self, hardware: &mut Hardware) -> Result<(), Self::Error> {
        hardware.actions.push(Action::Prepare);
        Ok(())
    }

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut Hardware,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        hardware.actions.push(Action::Start);
        ready(Ok(()))
    }

    fn observe_management<O, const RECORDS: usize>(
        &mut self,
        hardware: &mut Hardware,
        context: &mut ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<ScanRxProgress, Self::Error>
    where
        O: ScanFrameObserver,
    {
        let mac_stopped = hardware.actions.last() == Some(&Action::StopMac);
        hardware.actions.push(Action::Observe);
        if mac_stopped {
            return Ok(ScanRxProgress::default());
        }
        let mut beacon = [0_u8; 67];
        beacon[0] = 0x80;
        beacon[34] = 0x10;
        beacon[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        beacon[36..42].copy_from_slice(&[0, 4, b't', b'e', b's', b't']);
        beacon[42..45].copy_from_slice(&[3, 1, 11]);
        // WPA2-Personal: CCMP group and pairwise ciphers, PSK.
        beacon[45..67].copy_from_slice(&[
            48, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
        ]);
        assert_ne!(
            context.observe_management_frame(&beacon, -30, None),
            ScanObservation::Ignored
        );
        Ok(ScanRxProgress {
            completed_descriptors: 1,
            parsed_management_frames: 1,
            recycled_descriptors: 1,
            ..ScanRxProgress::default()
        })
    }

    fn park(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn prepare_next_channel(&mut self, hardware: &mut Hardware) -> Result<(), Self::Error> {
        hardware.actions.push(Action::Prepare);
        Ok(())
    }
}

struct Transmit(u32);

impl ScanTransmitPort<Hardware> for Transmit {
    type Error = ();

    fn begin_scan(&mut self) {}

    fn transmit_probe_request<'a, C: ConnectionFrameCoex + 'a>(
        &'a mut self,
        hardware: &'a mut Hardware,
        request: ScanProbeRequest<'a>,
        _coex: &'a mut C,
    ) -> impl Future<Output = Result<ScanProbeReport, Self::Error>> + 'a {
        hardware
            .actions
            .push(Action::Probe(request.current_channel.unwrap()));
        ready(Ok(ScanProbeReport::PassiveWithoutAttempt))
    }
}

/// Time that skips to each deadline, counting the one-millisecond dwell
/// ticks waited for.
#[derive(Default)]
struct DwellTimer {
    clock: oer_time_virtual::SkipClock,
    ticks: core::cell::Cell<u32>,
}

impl oer_time::Clock for DwellTimer {
    fn now(&self) -> oer_time::Instant {
        oer_time::Clock::now(&self.clock)
    }
}

impl crate::mac_clock::ReceptionClock for DwellTimer {
    fn snapshot(&self) -> Option<crate::mac_clock::MacClockSnapshot> {
        crate::mac_clock::MacClockStorage::<embassy_sync::blocking_mutex::raw::NoopRawMutex, _, _>::new(&self.clock)
            .start(crate::mac_clock::FixedCounter(0))
            .snapshot()
    }

    fn on_rf_wake(&self) {}

    fn counter_reading(&self) -> Option<(u32, oer_time::Instant)> {
        None
    }
}

impl oer_time::Timer for DwellTimer {
    async fn wait_until(&self, deadline: oer_time::Instant) {
        if deadline.checked_duration_since(oer_time::Clock::now(&self.clock))
            == Some(oer_time::Duration::from_millis(1))
        {
            self.ticks.set(self.ticks.get() + 1);
        }
        self.clock.wait_until(deadline).await;
    }
}

#[derive(Default)]
struct Observer(u32);

impl ScanFrameObserver for Observer {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _outcome: ScanObservation) {
        self.0 += 1;
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(core::task::Waker::noop());
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[test]
fn concrete_port_returns_every_owner_after_selected_candidate() {
    let mut table = ScanTable::<4>::new();
    let mut frame = [0_u8; 128];
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(7).unwrap());
    let port = ScanPort::new(
        ScanRadio::new(Phy(11), Hardware::default(), Receive(22), Transmit(33)),
        ScanStorage::new(&mut table, &mut frame, Observer::default(), &mut sequence),
        ScanStation::new(
            [7; 6],
            b"test",
            &[0x82, 0x84],
            oer_ieee80211_mac::security::StaSecurityPolicy::Wpa2Personal,
        )
        .with_descriptor_capacity(88),
        DwellTimer::default(),
    );
    let backend = StaScanBackend::new(StaScanConfig::new(2).unwrap());
    let mut service = StaCandidateScanService::new(backend);
    let exit = block_on(service.run(port, &[11]));
    let (owner, candidate) = match exit {
        StaCandidateScanExit::Selected {
            owner, candidate, ..
        } => (owner, candidate),
        StaCandidateScanExit::NoCandidate { .. } => {
            panic!("matching beacon must select one candidate")
        }
        StaCandidateScanExit::Failed { error, .. } => {
            panic!("running scan failed: {error:?}")
        }
        StaCandidateScanExit::Stopped { .. } => panic!("running scan stopped"),
        StaCandidateScanExit::InvalidPlan { error, .. } => {
            panic!("running scan plan failed: {error:?}")
        }
    };
    assert_eq!(candidate.channel, 11);

    let parts = owner.into_parts();
    assert_eq!(parts.phy.0, 11);
    assert_eq!(parts.rx.0, 22);
    assert_eq!(parts.tx.0, 33);
    assert_eq!(parts.timer.ticks.get(), 2);
    assert_eq!(parts.observer.0, 2);
    assert_eq!(
        parts.telemetry,
        ScanTelemetry {
            raw_frames: 2,
            ring_epochs: 2,
        }
    );
    assert_eq!(
        parts.hardware.actions,
        [
            Action::Prepare,
            Action::Switch(11),
            Action::AdmissionOn,
            Action::Start,
            Action::ResumeMac,
            Action::Probe(11),
            Action::Observe,
            Action::Observe,
            Action::AdmissionOff,
            Action::StopMac,
            Action::Observe,
        ]
    );
    assert_eq!(parts.sequence.peek(), SequenceNumber::new(8).unwrap());
    assert_eq!(parts.table.summary().records, 1);
    assert_eq!(parts.frame.len(), 128);
}

#[test]
fn standalone_scan_records_matching_bss_without_selecting_it() {
    let mut table = ScanTable::<4>::new();
    let mut frame = [0_u8; 128];
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(7).unwrap());
    let port = ScanPort::new(
        ScanRadio::new(Phy(11), Hardware::default(), Receive(22), Transmit(33)),
        ScanStorage::new(&mut table, &mut frame, Observer::default(), &mut sequence),
        ScanStation::new(
            [7; 6],
            b"test",
            &[0x82, 0x84],
            oer_ieee80211_mac::security::StaSecurityPolicy::Wpa2Personal,
        )
        .with_descriptor_capacity(88)
        .with_candidate_selection(false),
        DwellTimer::default(),
    );
    let backend = StaScanBackend::new(StaScanConfig::new(2).unwrap());
    let mut service = StaCandidateScanService::new(backend);
    let exit = block_on(service.run(port, &[11]));
    let owner = match exit {
        StaCandidateScanExit::NoCandidate { owner, .. } => owner,
        StaCandidateScanExit::Selected { .. } => {
            panic!("standalone scan must never select an association candidate")
        }
        StaCandidateScanExit::Failed { error, .. } => {
            panic!("standalone scan failed: {error:?}")
        }
        StaCandidateScanExit::Stopped { .. } => panic!("standalone scan stopped"),
        StaCandidateScanExit::InvalidPlan { error, .. } => {
            panic!("standalone scan plan failed: {error:?}")
        }
    };

    let parts = owner.into_parts();
    assert_eq!(parts.table.summary().records, 1);
    let record = &parts.table.records()[0];
    assert_eq!(&record.ssid[..usize::from(record.ssid_len)], b"test");
    assert_eq!(parts.table.records()[0].channel, 11);
    assert_eq!(parts.sequence.peek(), SequenceNumber::new(8).unwrap());
    assert_eq!(parts.hardware.actions.last(), Some(&Action::Observe));
    assert!(
        parts
            .hardware
            .actions
            .windows(2)
            .any(|actions| actions == [Action::AdmissionOff, Action::StopMac])
    );
}

#[test]
fn cancelling_an_unpolled_dwell_tick_leaves_time_and_counts_alone() {
    let timer = DwellTimer::default();
    let deadline = oer_time::Instant::from_micros(1_000);
    let wait = oer_time::Timer::wait_until(&timer, deadline);
    assert_eq!(oer_time::Clock::now(&timer), oer_time::Instant::EPOCH);
    assert_eq!(timer.ticks.get(), 0);
    drop(wait);
    assert_eq!(oer_time::Clock::now(&timer), oer_time::Instant::EPOCH);
    assert_eq!(timer.ticks.get(), 0);
    block_on(oer_time::Timer::wait_until(&timer, deadline));
    assert_eq!(oer_time::Clock::now(&timer), deadline);
    assert_eq!(timer.ticks.get(), 1);
}
