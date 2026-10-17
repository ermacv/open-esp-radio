use core::cell::Cell;
use core::future::{Future, ready};

use oer_ieee80211_mac::security::rsn::Akm;
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_rsn::{
    OwnedEapolFrame, Pmk, PtkContext,
    aes::RsnUnwrappedKeyData,
    frames::{OwnedRsnIe, RsnGtk, RsnPlainKeyData},
    runner::RsnRxProgress,
    supplicant::{RSN_STA_MESSAGE1_TIMEOUT, RSN_STA_MESSAGE3_TIMEOUT},
};

use super::*;
use oer_ieee80211_mac::station::StaSequenceCounter;
use oer_time::Clock;

/// Receive polls before each response timeout wins.
const MESSAGE1_POLLS: u32 =
    (RSN_STA_MESSAGE1_TIMEOUT.as_micros() / crate::runner::RX_POLL_INTERVAL.as_micros()) as u32;
const MESSAGE3_POLLS: u32 =
    (RSN_STA_MESSAGE3_TIMEOUT.as_micros() / crate::runner::RX_POLL_INTERVAL.as_micros()) as u32;

const LOCAL: [u8; 6] = [1; 6];
const AP: [u8; 6] = [2; 6];
const SNONCE: [u8; 32] = [3; 32];
const ANONCE: [u8; 32] = [4; 32];
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

fn seq(value: u16) -> SequenceNumber {
    SequenceNumber::new(value).unwrap()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestError {
    ReceiveNotLive,
}

struct Backend {
    receive_live: bool,
    message1_poll: Option<u32>,
    repeated_message1_poll: Option<u32>,
    message3_poll: Option<u32>,
    message1_polls: u32,
    message3_polls: u32,
    restarts: u16,
    stops: u16,
    transmissions: u16,
    last_sequence: Option<SequenceNumber>,
    inject_peer_attacks: bool,
}

impl Backend {
    const fn new(
        message1_poll: Option<u32>,
        repeated_message1_poll: Option<u32>,
        message3_poll: Option<u32>,
    ) -> Self {
        Self {
            receive_live: true,
            message1_poll,
            repeated_message1_poll,
            message3_poll,
            message1_polls: 0,
            message3_polls: 0,
            restarts: 0,
            stops: 0,
            transmissions: 0,
            last_sequence: None,
            inject_peer_attacks: false,
        }
    }

    const fn with_peer_attack_sequence(mut self) -> Self {
        self.inject_peer_attacks = true;
        self
    }

    fn message1() -> OwnedEapolFrame<512> {
        let frame = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 7, ANONCE).unwrap();
        OwnedEapolFrame::try_copy(RsnInterface::Station, AP, frame.as_bytes()).unwrap()
    }

    fn message3_with_replay(replay_counter: u64) -> OwnedEapolFrame<512> {
        let pmk = Pmk::derive(b"password", b"ssid").unwrap();
        let ptk = pmk.derive_ptk(
            Akm::Psk,
            PtkContext {
                authenticator_address: AP,
                supplicant_address: LOCAL,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            },
        );
        let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
        let gtk = RsnGtk::new(
            oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
            false,
            &[0x5a; 16],
        );
        let plain = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
        let frame = RsnTxFrame::<512>::message3(
            Akm::Psk,
            LOCAL,
            replay_counter,
            ANONCE,
            [7, 6, 5, 4, 3, 2, 1, 0],
            plain.as_bytes(),
        )
        .unwrap()
        .authenticate(&ptk);
        OwnedEapolFrame::try_copy(RsnInterface::Station, AP, frame.as_bytes()).unwrap()
    }

    fn message3() -> OwnedEapolFrame<512> {
        Self::message3_with_replay(8)
    }

    fn bad_mic_message3() -> OwnedEapolFrame<512> {
        let valid = Self::message3();
        let mut bytes = [0_u8; 512];
        let len = valid.as_bytes().len();
        bytes[..len].copy_from_slice(valid.as_bytes());
        bytes[81] ^= 1;
        OwnedEapolFrame::try_copy(RsnInterface::Station, AP, &bytes[..len]).unwrap()
    }

    fn unsupported_message() -> OwnedEapolFrame<512> {
        let frame = RsnTxFrame::<512>::message4(Akm::Psk, LOCAL, 8).unwrap();
        OwnedEapolFrame::try_copy(RsnInterface::Station, AP, frame.as_bytes()).unwrap()
    }

    fn peer_attack(poll: u32) -> Option<OwnedEapolFrame<512>> {
        match poll {
            1 => Some(Self::bad_mic_message3()),
            2 => Some(Self::message3_with_replay(7)),
            3 => Some(Self::unsupported_message()),
            _ => None,
        }
    }
}

impl RsnHandshakeBackend for Backend {
    type Error = TestError;

    fn service_receive(&mut self) -> impl Future<Output = Result<RsnRxProgress, Self::Error>> + '_ {
        let result = if !self.receive_live {
            Err(TestError::ReceiveNotLive)
        } else if self.restarts == 0 {
            self.message1_polls += 1;
            if self.message1_poll == Some(self.message1_polls) {
                Ok(RsnRxProgress::eapol(1, Self::message1()))
            } else {
                Ok(RsnRxProgress::drained(0))
            }
        } else {
            self.message3_polls += 1;
            if self.inject_peer_attacks
                && let Some(frame) = Self::peer_attack(self.message3_polls)
            {
                Ok(RsnRxProgress::eapol(1, frame))
            } else if self.message3_poll == Some(self.message3_polls) {
                Ok(RsnRxProgress::eapol(1, Self::message3()))
            } else if self.repeated_message1_poll == Some(self.message3_polls) {
                Ok(RsnRxProgress::eapol(1, Self::message1()))
            } else {
                Ok(RsnRxProgress::drained(0))
            }
        };
        ready(result)
    }

    fn restart_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        self.restarts += 1;
        ready(Ok(()))
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        let result = if self.receive_live {
            self.receive_live = false;
            self.stops += 1;
            Ok(())
        } else {
            Err(TestError::ReceiveNotLive)
        };
        ready(result)
    }

    fn transmit_message2(
        &mut self,
        frame: &RsnTxFrame<512>,
        sequence_number: SequenceNumber,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        assert_eq!(
            frame.key_frame().message(),
            EapolKeyMessage::PairwiseMessage2
        );
        self.transmissions += 1;
        self.last_sequence = Some(sequence_number);
        ready(Ok(()))
    }
}

#[derive(Debug, Default)]
struct TestTimer {
    clock: oer_time_virtual::SkipClock,
    waits: Cell<u32>,
}

impl Clock for TestTimer {
    fn now(&self) -> Instant {
        self.clock.now()
    }
}

impl Timer for TestTimer {
    async fn wait_until(&self, deadline: Instant) {
        self.waits.set(self.waits.get() + 1);
        self.clock.wait_until(deadline).await;
    }
}

fn config<'a>(pmk: &'a Pmk) -> RsnHandshakeConfig<'a> {
    RsnHandshakeConfig {
        local: LOCAL,
        authenticator: AP,
        supplicant_nonce: SNONCE,
        association_security_ies: &RSN,
        authenticator_rsn_ie: &RSN,
        authenticator_rsnxe: &[],
        pmk,
    }
}

struct IdentityUnwrap;

impl AsyncRsnKeyUnwrap for IdentityUnwrap {
    type Error = ();

    fn unwrap_key_data<'a>(
        &'a mut self,
        _kek: &'a [u8; 16],
        encrypted: &'a [u8],
    ) -> impl Future<Output = Result<RsnUnwrappedKeyData, Self::Error>> + 'a {
        ready(RsnUnwrappedKeyData::try_copy(encrypted).map_err(|_| ()))
    }
}

fn pending_key_install() -> RsnPendingKeyInstall {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let backend = Backend::new(Some(1), None, Some(1));
    let mut runner = RsnHandshakeRunner::new(backend, TestTimer::default(), IdentityUnwrap);
    let mut sequence = StaSequenceCounter::new(seq(0x123));
    embassy_futures::block_on(runner.run(config(&pmk), &mut || sequence.take())).unwrap()
}

#[test]
fn message3_wait_ignores_bad_mic_wrong_replay_and_unsupported_frames() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let backend = Backend::new(Some(1), None, Some(4)).with_peer_attack_sequence();
    let mut runner = RsnHandshakeRunner::new(backend, TestTimer::default(), IdentityUnwrap);
    let mut sequence = StaSequenceCounter::new(seq(0x123));

    let pending = embassy_futures::block_on(runner.run(config(&pmk), &mut || sequence.take()))
        .expect("untrusted peer rejects must not abort the live join");
    assert_eq!(pending.completed_frames(), 5);
    assert_eq!(pending.message2_transmissions(), 1);
    assert_eq!(pending.request().replay_counter(), 8);
    assert!(!runner.backend().receive_live);
    assert_eq!(runner.backend().stops, 1);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeyBackendError {
    Install,
    Transmit,
}

struct KeyBackend {
    fail_install: bool,
    fail_transmit: bool,
    installs: u8,
    transmissions: u8,
    rollbacks: u8,
}

impl KeyBackend {
    const fn new(fail_install: bool, fail_transmit: bool) -> Self {
        Self {
            fail_install,
            fail_transmit,
            installs: 0,
            transmissions: 0,
            rollbacks: 0,
        }
    }
}

impl RsnKeyInstallBackend for KeyBackend {
    type Error = KeyBackendError;
    type InstalledKeys = u8;

    fn install_keys(
        &mut self,
        request: &RsnStaKeyInstallRequest,
    ) -> Result<Self::InstalledKeys, Self::Error> {
        assert_eq!(request.replay_counter(), 8);
        assert_eq!(request.pairwise().peer(), &AP);
        if self.fail_install {
            return Err(KeyBackendError::Install);
        }
        self.installs += 1;
        Ok(0xa5)
    }

    fn rollback_keys(&mut self, keys: Self::InstalledKeys) -> Result<(), Self::Error> {
        assert_eq!(keys, 0xa5);
        self.rollbacks += 1;
        Ok(())
    }

    fn transmit_message4<'a>(
        &'a mut self,
        frame: &'a RsnTxFrame<512>,
        keys: &'a mut Self::InstalledKeys,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        assert_eq!(*keys, 0xa5);
        assert_eq!(
            frame.key_frame().message(),
            EapolKeyMessage::PairwiseMessage4
        );
        assert_eq!(frame.key_frame().replay_counter(), 8);
        self.transmissions += 1;
        ready(if self.fail_transmit {
            Err(KeyBackendError::Transmit)
        } else {
            Ok(())
        })
    }
}

#[test]
fn key_install_runner_publishes_m4_and_returns_typed_key_ownership() {
    let mut runner = RsnKeyInstallRunner::new(KeyBackend::new(false, false));
    let established = embassy_futures::block_on(runner.run(pending_key_install())).unwrap();

    assert_eq!(established.metadata().replay_counter, 8);
    assert_eq!(established.metadata().group_key_id.get(), 2);
    assert_eq!(established.metadata().completed_frames, 2);
    assert_eq!(established.metadata().message2_transmissions, 1);
    assert_eq!(established.into_parts().0, 0xa5);
    assert_eq!(runner.backend().installs, 1);
    assert_eq!(runner.backend().transmissions, 1);
    assert_eq!(runner.backend().rollbacks, 0);
}

#[test]
fn message4_tx_failure_rolls_back_both_published_keys() {
    let mut runner = RsnKeyInstallRunner::new(KeyBackend::new(false, true));

    assert!(matches!(
        embassy_futures::block_on(runner.run(pending_key_install())),
        Err(RsnKeyInstallError::Failed(RsnKeyInstallFailure::Transmit(
            KeyBackendError::Transmit
        )))
    ));
    assert_eq!(runner.backend().installs, 1);
    assert_eq!(runner.backend().transmissions, 1);
    assert_eq!(runner.backend().rollbacks, 1);
}

#[test]
fn atomic_install_failure_does_not_attempt_rollback_or_message4() {
    let mut runner = RsnKeyInstallRunner::new(KeyBackend::new(true, false));

    assert!(matches!(
        embassy_futures::block_on(runner.run(pending_key_install())),
        Err(RsnKeyInstallError::Install(KeyBackendError::Install))
    ));
    assert_eq!(runner.backend().installs, 0);
    assert_eq!(runner.backend().transmissions, 0);
    assert_eq!(runner.backend().rollbacks, 0);
}

#[test]
fn message1_timeout_is_exact_and_stops_the_live_ring() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let backend = Backend::new(None, None, None);
    let mut runner = RsnHandshakeRunner::new(
        backend,
        TestTimer::default(),
        oer_ieee80211_rsn::aes::RsnSoftwareAes::new(),
    );
    let mut sequence = StaSequenceCounter::new(seq(0x123));

    assert!(matches!(
        embassy_futures::block_on(runner.run(config(&pmk), &mut || sequence.take())),
        Err(RsnHandshakeError::Timeout {
            wait: RsnStaResponseWait::Message1,
            elapsed: RSN_STA_MESSAGE1_TIMEOUT,
            completed_frames: 0,
        })
    ));
    assert_eq!(runner.timer.now().as_micros(), 3_000_000);
    assert_eq!(runner.timer.waits.get(), MESSAGE1_POLLS);
    assert!(!runner.backend().receive_live);
    assert_eq!(runner.backend().stops, 1);
}

#[test]
fn peer_message1_sends_m2_once_but_never_retries_it_on_local_timeout() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let backend = Backend::new(Some(1), None, None);
    let mut runner = RsnHandshakeRunner::new(
        backend,
        TestTimer::default(),
        oer_ieee80211_rsn::aes::RsnSoftwareAes::new(),
    );
    let mut sequence = StaSequenceCounter::new(seq(0x123));

    assert!(matches!(
        embassy_futures::block_on(runner.run(config(&pmk), &mut || sequence.take())),
        Err(RsnHandshakeError::Timeout {
            wait: RsnStaResponseWait::Message3,
            elapsed: RSN_STA_MESSAGE3_TIMEOUT,
            completed_frames: 1,
        })
    ));
    assert_eq!(runner.backend().restarts, 1);
    assert_eq!(runner.backend().transmissions, 1);
    assert_eq!(runner.backend().last_sequence, Some(seq(0x123)));
    assert_eq!(sequence.peek(), seq(0x124));
    assert_eq!(runner.timer.now().as_micros(), 6_001_000);
    assert_eq!(
        runner.timer.waits.get(),
        MESSAGE1_POLLS.min(1) + MESSAGE3_POLLS
    );
}

#[test]
fn repeated_peer_message1_is_the_only_message2_refresh_source() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let backend = Backend::new(Some(1), Some(100), None);
    let mut runner = RsnHandshakeRunner::new(
        backend,
        TestTimer::default(),
        oer_ieee80211_rsn::aes::RsnSoftwareAes::new(),
    );
    let mut sequence = StaSequenceCounter::new(seq(7));

    assert!(matches!(
        embassy_futures::block_on(runner.run(config(&pmk), &mut || sequence.take())),
        Err(RsnHandshakeError::Timeout {
            wait: RsnStaResponseWait::Message3,
            ..
        })
    ));
    assert_eq!(runner.backend().transmissions, 2);
    assert_eq!(sequence.peek(), seq(9));
    assert_eq!(runner.timer.now().as_micros(), 6_001_000);
}

#[test]
fn message1_on_exact_deadline_is_serviced_before_timeout() {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let backend = Backend::new(Some(MESSAGE1_POLLS), None, None);
    let mut runner = RsnHandshakeRunner::new(
        backend,
        TestTimer::default(),
        oer_ieee80211_rsn::aes::RsnSoftwareAes::new(),
    );
    let mut sequence = StaSequenceCounter::new(seq(0));

    assert!(matches!(
        embassy_futures::block_on(runner.run(config(&pmk), &mut || sequence.take())),
        Err(RsnHandshakeError::Timeout {
            wait: RsnStaResponseWait::Message3,
            ..
        })
    ));
    assert_eq!(runner.backend().transmissions, 1);
    assert_eq!(runner.timer.now().as_micros(), 9_000_000);
}

#[test]
fn cancelling_an_unpolled_wait_does_not_count_or_advance_it() {
    let timer = TestTimer::default();
    let deadline = Instant::from_micros(1_000);
    let wait = timer.wait_until(deadline);
    assert_eq!(timer.now(), Instant::EPOCH);
    assert_eq!(timer.waits.get(), 0);
    drop(wait);
    assert_eq!(timer.now(), Instant::EPOCH);
    assert_eq!(timer.waits.get(), 0);
    embassy_futures::block_on(timer.wait_until(deadline));
    assert_eq!(timer.now(), deadline);
    assert_eq!(timer.waits.get(), 1);
}
