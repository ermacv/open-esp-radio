use core::cell::Cell;
use core::future::{Future, ready};
use oer_ieee80211_mac::security::SaePwe;
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_mac::station::AssociationResponse;
use oer_ieee80211_sta::join::{
    association::{StaAssociationAttempt, StaAssociationFailure},
    authentication::{StaAuthenticationAttempt, StaAuthenticationFailure},
    sae::{StaSaeFailure, StaSaeTransmission},
};

use super::*;
use crate::test_support::block_on;
use oer_time::Clock;

const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Authentication,
    Association,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestError {
    ReceiveAlreadyStarted,
    ReceiveNotStarted,
}

struct Backend {
    phase: Phase,
    receive_live: bool,
    auth_response_poll: Option<u32>,
    association_response_poll: Option<u32>,
    auth_polls: u32,
    association_polls: u32,
    auth_attempts: [Option<StaAuthenticationAttempt>; 3],
    auth_attempt_count: usize,
    association_attempts: [Option<StaAssociationAttempt>; 7],
    association_attempt_count: usize,
    starts: u16,
    stops: u16,
    /// The access point side of an SAE exchange, and the frames it sends
    /// at the next receive service.
    sae_access_point: Option<oer_ieee80211_rsn::sae::SaeCommit>,
    sae_keys: Option<oer_ieee80211_rsn::sae::SaeKeys>,
    sae_replies: std::vec::Vec<std::vec::Vec<u8>>,
    sae_sequences: std::vec::Vec<SequenceNumber>,
}

impl Backend {
    const fn new(auth_response_poll: Option<u32>, association_response_poll: Option<u32>) -> Self {
        Self {
            phase: Phase::Idle,
            receive_live: false,
            auth_response_poll,
            association_response_poll,
            auth_polls: 0,
            association_polls: 0,
            auth_attempts: [None; 3],
            auth_attempt_count: 0,
            association_attempts: [None; 7],
            association_attempt_count: 0,
            starts: 0,
            stops: 0,
            sae_access_point: None,
            sae_keys: None,
            sae_replies: std::vec::Vec::new(),
            sae_sequences: std::vec::Vec::new(),
        }
    }
}

impl StaJoinBackend for Backend {
    type Error = TestError;

    fn start_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        let result = if self.receive_live {
            Err(TestError::ReceiveAlreadyStarted)
        } else {
            self.receive_live = true;
            self.starts += 1;
            Ok(())
        };
        ready(result)
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        let result = if self.receive_live {
            self.receive_live = false;
            self.phase = Phase::Idle;
            self.stops += 1;
            Ok(())
        } else {
            Err(TestError::ReceiveNotStarted)
        };
        ready(result)
    }

    fn transmit_open_authentication(
        &mut self,
        attempt: StaAuthenticationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        self.phase = Phase::Authentication;
        self.auth_attempts[self.auth_attempt_count] = Some(attempt);
        self.auth_attempt_count += 1;
        ready(Ok(()))
    }

    fn transmit_association(
        &mut self,
        attempt: StaAssociationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        self.phase = Phase::Association;
        self.association_attempts[self.association_attempt_count] = Some(attempt);
        self.association_attempt_count += 1;
        ready(Ok(()))
    }

    fn transmit_sae_authentication<'a>(
        &'a mut self,
        sequence_number: SequenceNumber,
        transmission: &'a StaSaeTransmission,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        use oer_ieee80211_rsn::sae::SaeCommitValues;
        self.sae_sequences.push(sequence_number);
        let Some(access_point) = self.sae_access_point.as_ref() else {
            // A silent access point.
            return ready(Ok(()));
        };
        let _ = self.sae_access_point.as_ref();
        if transmission.transaction == 1 {
            let station = SaeCommitValues::parse(transmission.body(), false).unwrap();
            self.sae_keys = Some(access_point.process(station).unwrap());
            let mut body = [0; oer_ieee80211_rsn::sae::SAE_COMMIT_LEN];
            access_point
                .values()
                .encode(None, false, &mut body)
                .unwrap();
            self.sae_replies.push(sae_frame(1, &body));
        } else {
            let keys = self.sae_keys.as_ref().unwrap();
            assert_eq!(keys.verify_peer_confirm(transmission.body()), Ok(1));
            self.sae_replies.push(sae_frame(2, &keys.own_confirm(1)));
        }
        ready(Ok(()))
    }

    fn service_receive<'a, O>(
        &'a mut self,
        observer: &'a mut O,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        O: StaJoinRxObserver + 'a,
    {
        let result = if !self.receive_live {
            Err(TestError::ReceiveNotStarted)
        } else {
            match self.phase {
                Phase::Authentication => {
                    self.auth_polls += 1;
                    if self.auth_response_poll == Some(self.auth_polls) {
                        let _ = observer.observe_completed(Some(&authentication_response(0)));
                    }
                }
                Phase::Association => {
                    self.association_polls += 1;
                    if self.association_response_poll == Some(self.association_polls) {
                        let _ = observer.observe_completed(Some(&association_response(0)));
                    }
                }
                Phase::Idle => {
                    if !self.sae_replies.is_empty() {
                        let frame = self.sae_replies.remove(0);
                        let _ = observer.observe_completed(Some(&frame));
                    }
                }
            }
            Ok(())
        };
        ready(result)
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

fn authentication_response(status_code: u16) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
    frame[28..30].copy_from_slice(&status_code.to_le_bytes());
    frame
}

fn association_response(status_code: u16) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x0010_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&0x0431_u16.to_le_bytes());
    frame[26..28].copy_from_slice(&status_code.to_le_bytes());
    frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
    frame
}

#[test]
fn successful_join_uses_typed_sequences_and_leaves_association_rx_live() {
    let backend = Backend::new(Some(1), Some(2));
    let mut runner = StaJoinRunner::new(backend, TestTimer::default());
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0x123).unwrap());

    assert_eq!(
        block_on(runner.authenticate(LOCAL, BSSID, &mut sequence)),
        Ok(StaAuthenticationSuccess {
            attempt: 1,
            total_received_frames: 1,
        })
    );
    assert_eq!(
        block_on(runner.associate(LOCAL, BSSID, LinkProtection::Ccmp, &mut sequence,)),
        Ok(StaAssociationSuccess {
            response: AssociationResponse {
                capability_info: 0x0431,
                status_code: 0,
                association_id: 42,
                ht_capability: false,
                he_capability: false,
                he_operation: false,
                wmm: false,
                wmm_parameters: None,
                association_comeback_tu: None,
            },
            total_received_frames: 1,
        })
    );

    assert_eq!(
        runner.backend().auth_attempts[0].unwrap().sequence_number,
        SequenceNumber::new(0x123).unwrap()
    );
    assert_eq!(
        runner.backend().association_attempts[0]
            .unwrap()
            .sequence_number,
        SequenceNumber::new(0x124).unwrap()
    );
    assert!(runner.backend().receive_live);
    assert_eq!(runner.backend().starts, 2);
    assert_eq!(runner.backend().stops, 1);
}

#[test]
fn authentication_timeout_is_three_exact_one_second_epochs() {
    let backend = Backend::new(None, None);
    let mut runner = StaJoinRunner::new(backend, TestTimer::default());
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());

    assert_eq!(
        block_on(runner.authenticate(LOCAL, BSSID, &mut sequence)),
        Err(StaJoinError::AuthenticationFailed {
            attempts: 3,
            failure: StaAuthenticationFailure::Timeout,
            total_received_frames: 0,
        })
    );
    assert_eq!(runner.backend().auth_attempt_count, 3);
    assert_eq!(runner.backend().starts, 3);
    assert_eq!(runner.backend().stops, 3);
    assert!(!runner.backend().receive_live);
    assert_eq!(runner.timer.now().as_micros(), 3_000_000);
    assert_eq!(runner.timer.waits.get(), 3_000);
}

#[test]
fn association_timeout_sends_seven_requests_and_stops_rx_at_1000_ms() {
    let backend = Backend::new(None, None);
    let mut runner = StaJoinRunner::new(backend, TestTimer::default());
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(7).unwrap());

    assert_eq!(
        block_on(runner.associate(LOCAL, BSSID, LinkProtection::Ccmp, &mut sequence,)),
        Err(StaJoinError::AssociationFailed {
            failure: StaAssociationFailure::Timeout,
            total_received_frames: 0,
        })
    );
    assert_eq!(runner.backend().association_attempt_count, 7);
    assert_eq!(
        runner
            .backend()
            .association_attempts
            .map(|attempt| attempt.unwrap().offset.as_micros() / 1_000),
        [0, 160, 320, 480, 640, 800, 960]
    );
    assert_eq!(runner.timer.now().as_micros(), 1_000_000);
    assert_eq!(runner.timer.waits.get(), 1_000);
    assert!(!runner.backend().receive_live);
    assert_eq!(runner.backend().starts, 1);
    assert_eq!(runner.backend().stops, 1);
}

#[test]
fn association_response_on_exact_deadline_wins_before_timeout() {
    let backend = Backend::new(None, Some(1_000));
    let mut runner = StaJoinRunner::new(backend, TestTimer::default());
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());

    assert!(block_on(runner.associate(LOCAL, BSSID, LinkProtection::Ccmp, &mut sequence,)).is_ok());
    assert_eq!(runner.timer.now().as_micros(), 1_000_000);
    assert!(runner.backend().receive_live);
    assert_eq!(runner.backend().stops, 0);
}

fn sae_frame(transaction: u16, body: &[u8]) -> std::vec::Vec<u8> {
    let mut frame = std::vec![0_u8; 30];
    frame[0] = 0xb0;
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&3_u16.to_le_bytes());
    frame[26..28].copy_from_slice(&transaction.to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

#[test]
fn an_sae_exchange_returns_the_access_point_pmk() {
    use oer_ieee80211_rsn::sae::{SaeCommit, SaePasswordElement};
    let commit = |local, peer, seed| {
        SaeCommit::new(
            SaePasswordElement::hunting_and_pecking(b"correct horse", local, peer).unwrap(),
            [seed; 32],
            [seed + 1; 32],
        )
        .unwrap()
    };
    let mut backend = Backend::new(None, None);
    backend.sae_access_point = Some(commit(BSSID, LOCAL, 0x40));
    let mut runner = StaJoinRunner::new(backend, TestTimer::default());
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0x10).unwrap());
    let pmk = block_on(runner.authenticate_sae(
        StaSaeAuthentication::new(
            LOCAL,
            BSSID,
            commit(LOCAL, BSSID, 0x20),
            SaePwe::HuntingAndPecking,
        ),
        &mut sequence,
    ))
    .unwrap();
    let (backend, _) = runner.into_parts();
    assert_eq!(pmk.pmk, backend.sae_keys.as_ref().unwrap().pmk);
    // The commit and the confirm each take a sequence number.
    assert_eq!(
        backend.sae_sequences,
        [
            SequenceNumber::new(0x10).unwrap(),
            SequenceNumber::new(0x11).unwrap()
        ]
    );
    assert_eq!((backend.starts, backend.stops), (1, 1));
}

#[test]
fn an_unanswered_sae_commit_times_out_after_four_seconds() {
    use oer_ieee80211_rsn::sae::{SaeCommit, SaePasswordElement};
    let commit = SaeCommit::new(
        SaePasswordElement::hunting_and_pecking(b"correct horse", LOCAL, BSSID).unwrap(),
        [0x20; 32],
        [0x21; 32],
    )
    .unwrap();
    let mut runner = StaJoinRunner::new(Backend::new(None, None), TestTimer::default());
    let mut sequence = StaSequenceCounter::new(SequenceNumber::new(0).unwrap());
    let result = block_on(runner.authenticate_sae(
        StaSaeAuthentication::new(LOCAL, BSSID, commit, SaePwe::HuntingAndPecking),
        &mut sequence,
    ));
    assert_eq!(
        result.err(),
        Some(StaJoinError::SaeFailed(StaSaeFailure::Timeout))
    );
    let (backend, timer) = runner.into_parts();
    assert_eq!(timer.now().as_micros(), 4_000_000);
    assert_eq!(backend.sae_sequences.len(), 1);
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
    block_on(timer.wait_until(deadline));
    assert_eq!(timer.now(), deadline);
    assert_eq!(timer.waits.get(), 1);
}
