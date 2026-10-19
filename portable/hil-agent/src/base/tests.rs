use oer_hil_protocol::base::{ImageKeyPage, sorted_keys};
use oer_hil_protocol::system::{InjectPanic, WatchdogTest, WatchdogTestMode};
use oer_hil_protocol::{Envelope, FrameEncoder, Key};

use super::*;

const BOOT: u64 = 42;

const SERVED: [Key; 5] = sorted_keys([
    GetImageKeys::KEY,
    GetBootStatus::KEY,
    GetPostMortemCheckpoints::KEY,
    GetLinkHealth::KEY,
    WatchdogTest::KEY,
]);

fn intake() -> Intake<'static> {
    Intake::new(BOOT, ImageKeySet::new(&SERVED), 0)
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
        answer(send::<_, NoRequests>(&mut intake, BOOT, 0, InjectPanic)),
        Answer::Rejected(RejectReason::Unsupported)
    ));
}

#[test]
fn the_image_serves_its_own_requests_outside_sessions() {
    let mut intake = intake();
    let request = WatchdogTest(WatchdogTestMode::Complete);
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
            Panic(InjectPanic),
        }
    }
    let mut intake = intake();
    match send::<_, Sessioned>(&mut intake, BOOT, 9, InjectPanic) {
        Request::Serve(identity, Sessioned::Panic(InjectPanic)) => {
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
