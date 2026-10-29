use crate::lifecycle::Request as LifecycleRequest;
use oer_hil_protocol::base::{ImageKeyPage, sorted_keys};
use oer_hil_protocol::lifecycle::{RequestReset, ResetAccepted};

crate::requests! {
    enum WatchdogRequest (sessions = false) {
        Test(crate::test_messages::Probe),
    }
}
use crate::test_messages::{Probe, ProbeMode};
use oer_hil_protocol::{Envelope, FrameEncoder, Key};

use super::*;

const BOOT: u64 = 42;

const SERVED: [Key; 5] = sorted_keys([
    GetImageKeys::KEY,
    GetBootStatus::KEY,
    GetPostMortemCheckpoints::KEY,
    GetLinkHealth::KEY,
    Probe::KEY,
]);

fn intake() -> Intake<'static> {
    Intake::new(BOOT, ImageKeySet::new(&SERVED).unwrap(), 0)
}

fn send<M: Message, C: Requests>(
    intake: &mut Intake<'_>,
    boot: u64,
    session: u64,
    body: M,
) -> Request<C> {
    let mut encoder = FrameEncoder::new();
    let bytes = encoder
        .encode(&Envelope::new(boot, 1, session, 7, body))
        .unwrap();
    intake
        .receive::<C>(bytes, Sent::default())
        .expect("one complete request")
}

fn answer<C>(request: Request<C>) -> Answer {
    match request {
        Request::Answer(identity, answer) => {
            assert_eq!(identity.request_id, 7);
            answer
        }
        Request::Serve(..) => panic!("the base module answers this request"),
    }
}

#[test]
fn the_base_module_answers_its_requests() {
    let mut intake = intake();
    match answer(send::<_, NoRequests>(&mut intake, BOOT, 0, GetBootStatus)) {
        Answer::Boot => {}
        other => panic!("{other:?}"),
    }
    match answer(send::<_, NoRequests>(
        &mut intake,
        BOOT,
        0,
        GetImageKeys { first: 0 },
    )) {
        Answer::ImageKeys(first) => {
            let ImageKeyPage { total, keys, .. } = intake.image_keys().page(first);
            assert_eq!(total, 5);
            assert_eq!(keys.as_slice(), &SERVED);
        }
        other => panic!("{other:?}"),
    }
    match answer(send::<_, NoRequests>(&mut intake, BOOT, 0, GetLinkHealth)) {
        Answer::Link(link) => assert_eq!(link.rx_frames, 3),
        other => panic!("{other:?}"),
    }
}

#[test]
fn image_key_discovery_needs_no_boot_but_every_other_request_does() {
    let mut intake = intake();
    assert!(matches!(
        answer(send::<_, NoRequests>(
            &mut intake,
            0,
            0,
            oer_hil_protocol::base::GetHello
        )),
        Answer::Hello(_)
    ));
    assert!(matches!(
        answer(send::<_, NoRequests>(&mut intake, 41, 0, GetBootStatus)),
        Answer::Rejected(RejectReason::BootId)
    ));
}

#[test]
fn a_request_of_no_served_type_is_unsupported() {
    let mut intake = intake();
    assert!(matches!(
        answer(send::<_, NoRequests>(&mut intake, BOOT, 0, RequestReset)),
        Answer::Rejected(RejectReason::Unsupported)
    ));
}

#[test]
fn the_image_serves_its_own_requests_outside_sessions() {
    let mut intake = intake();
    let request = Probe(ProbeMode::Complete);
    match send::<_, WatchdogRequest>(&mut intake, BOOT, 0, request) {
        Request::Serve(identity, WatchdogRequest::Test(served)) => {
            assert_eq!((identity.request_id, served), (7, request));
        }
        Request::Answer(_, answer) => panic!("{answer:?}"),
    }
    assert!(matches!(
        answer(send::<_, WatchdogRequest>(&mut intake, BOOT, 3, request)),
        Answer::Rejected(RejectReason::InvalidState)
    ));
    assert!(matches!(
        answer(send::<_, WatchdogRequest>(
            &mut intake,
            BOOT,
            3,
            GetBootStatus
        )),
        Answer::Rejected(RejectReason::InvalidState)
    ));
}

#[test]
fn an_image_with_sessions_keeps_them() {
    crate::requests! {
        enum Sessioned (sessions = true) {
            Reset(RequestReset),
        }
    }
    let mut intake = intake();
    match send::<_, Sessioned>(&mut intake, BOOT, 9, RequestReset) {
        Request::Serve(identity, Sessioned::Reset(RequestReset)) => {
            assert_eq!(identity.session_id, 9);
        }
        _ => panic!("an image with sessions serves a request of one"),
    }
}

#[test]
fn every_request_of_one_chunk_is_handed_over_in_order() {
    let mut intake = intake();
    let mut encoder = FrameEncoder::new();
    let mut chunk = Vec::new();
    for request_id in [3, 4] {
        chunk.extend_from_slice(
            encoder
                .encode(&Envelope::new(BOOT, 1, 0, request_id, GetLinkHealth))
                .unwrap(),
        );
    }
    let mut seen = Vec::new();
    intake.receive_each::<NoRequests>(&chunk, Sent::default(), |request| {
        let Request::Answer(identity, Answer::Link(link)) = request else {
            panic!("a link health answer");
        };
        seen.push((identity.request_id, link.rx_frames));
    });
    assert_eq!(seen, [(3, 1), (4, 2)]);
}

crate::requests! {
    #[expect(dead_code, reason = "the test mounts the service for its keys alone")]
    enum Mounted (sessions = false) {
        Watchdog(Probe),
        #[cfg(any())]
        Absent(crate::test_messages::Switch),
    }
    publishes [oer_hil_protocol::lifecycle::ResetAccepted, #[cfg(any())] crate::test_messages::Switched]
}

crate::requests! {
    enum Sharing (sessions = false) {
        Reset(RequestReset),
    }
    publishes [oer_hil_protocol::lifecycle::ResetAccepted]
}

crate::image_keys! {
    MOUNTED = [Mounted, Sharing]
}

#[test]
fn a_service_reports_its_endpoints_their_responses_and_what_it_publishes() {
    use crate::test_messages::ProbeArmed;
    assert_eq!(
        Mounted::KEYS,
        [Probe::KEY, ProbeArmed::KEY, ResetAccepted::KEY]
    );
}

#[test]
fn lifecycle_support_does_not_advertise_image_diagnostics() {
    use crate::test_messages::{Marker, Probe, Switch};
    for key in [Marker::KEY, Switch::KEY, Probe::KEY] {
        assert!(!Base::KEYS.contains(&key));
        assert!(!LifecycleRequest::KEYS.contains(&key));
    }
    assert_eq!(
        LifecycleRequest::KEYS,
        [RequestReset::KEY, ResetAccepted::KEY]
    );
}

#[test]
fn reset_is_mounted_explicitly_and_rejects_foreign_boots_and_sessions() {
    let mut intake = intake();
    assert!(matches!(
        send::<_, LifecycleRequest>(&mut intake, BOOT, 0, RequestReset),
        Request::Serve(_, LifecycleRequest::Reset(RequestReset))
    ));
    for (boot, session, reason) in [
        (BOOT - 1, 0, RejectReason::BootId),
        (BOOT, 1, RejectReason::InvalidState),
    ] {
        assert!(matches!(
            answer(send::<_, LifecycleRequest>(&mut intake, boot, session, RequestReset)),
            Answer::Rejected(actual) if actual == reason
        ));
    }
}

#[test]
fn an_image_reports_the_base_module_and_each_shared_key_once() {
    use crate::test_messages::ProbeArmed;
    let mut expected: Vec<Key> = Base::KEYS.to_vec();
    expected.extend([
        Probe::KEY,
        ProbeArmed::KEY,
        RequestReset::KEY,
        ResetAccepted::KEY,
    ]);
    expected.sort();
    assert_eq!(MOUNTED.keys(), expected.as_slice());
    assert!(!MOUNTED.contains(crate::test_messages::Switch::KEY));
}
