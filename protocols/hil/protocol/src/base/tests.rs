use super::*;
use crate::{FrameDecoder, FrameEncoder, Key, WireKind};

/// Encodes `message` and returns what a decoder sees of it.
fn transmit<M: Message + core::fmt::Debug + PartialEq>(message: &Envelope<M>) -> Envelope<M> {
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(message).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed(M::WIRE_KIND, bytes, |frame| {
        observed = frame.unwrap().decode::<M>().map(Result::unwrap);
    });
    observed.expect("one frame of this type")
}

/// The header of `message` as the device's dispatcher sees it.
fn header_of<M: Message>(message: &Envelope<M>, check: impl FnOnce(&Frame<'_>)) {
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(message).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut check = Some(check);
    decoder.feed(M::WIRE_KIND, bytes, |frame| {
        (check.take().unwrap())(&frame.unwrap());
    });
    assert!(check.is_none(), "one frame");
}

#[test]
fn an_unknown_boot_allows_only_session_free_hello_discovery() {
    header_of(&Envelope::new(0, 1, 0, 1, GetHello), |frame| {
        assert_eq!(validate_target(frame, 42), Ok(()));
        assert_eq!(validate_target(frame, 0), Err(RejectReason::BootId));
    });
    header_of(&Envelope::new(0, 1, 9, 1, GetHello), |frame| {
        assert_eq!(validate_target(frame, 42), Err(RejectReason::BootId));
    });
    for request in [0, 41] {
        header_of(&Envelope::new(request, 1, 0, 1, GetBootStatus), |frame| {
            assert_eq!(validate_target(frame, 42), Err(RejectReason::BootId));
        });
    }
    header_of(
        &Envelope::new(0, 1, 0, 1, GetImageKeys { first: 0 }),
        |frame| assert_eq!(validate_target(frame, 42), Err(RejectReason::BootId)),
    );
    header_of(&Envelope::new(42, 1, 0, 1, GetBootStatus), |frame| {
        assert_eq!(validate_target(frame, 42), Ok(()));
    });
}

#[test]
fn a_reply_keeps_the_request_identity() {
    header_of(&Envelope::new(42, 7, 3, 11, GetBootStatus), |request| {
        let reply = reply(42, 99, request, Rejected(RejectReason::Unsupported));
        assert_eq!(
            (
                reply.boot_id,
                reply.message_sequence,
                reply.session_id,
                reply.request_id
            ),
            (42, 99, 3, 11)
        );
    });
}

#[test]
fn boot_evidence_round_trips() {
    for reset_reason in [
        ResetReason::Other,
        ResetReason::Software,
        ResetReason::MainWatchdog1,
    ] {
        let expected = Envelope::new(
            7,
            2,
            0,
            3,
            BootEvidence {
                reset_reason,
                raw_reset_reason: 3,
                post_mortem: None,
                platform_panic: None,
            },
        );
        assert_eq!(transmit(&expected), expected);
    }
    let request = Envelope::new(7, 2, 0, 3, GetBootStatus);
    assert_eq!(transmit(&request), request);
}

/// The largest post-mortem summary and a full checkpoint page fit one frame.
#[test]
fn the_largest_post_mortem_fits_a_frame() {
    extern crate std;
    use std::string::String;
    let text = |bytes: usize| String::from("x").repeat(bytes);
    let hart = HartState {
        responded: true,
        mepc: u32::MAX,
        ra: u32::MAX,
        sp: u32::MAX,
        mcause: u32::MAX,
        mstatus: u32::MAX,
    };
    let faults = [
        Fault::Hang(HangFault {
            detected_uptime_ms: u32::MAX,
            stalled_executors: u8::MAX,
            harts: [hart; 2],
            samples: [u32::MAX; 16],
            stalled_task: Some(TaskStall {
                slot: TaskSlot::SessionEvidence,
                pending_ms: u32::MAX,
            }),
        }),
        Fault::Panic(PanicFault {
            file: text(48).as_str().try_into().unwrap(),
            line: u32::MAX,
            message: text(96).as_str().try_into().unwrap(),
        }),
    ];
    for fault in faults {
        let boot = BootEvidence {
            reset_reason: ResetReason::CpuLockup,
            raw_reset_reason: u8::MAX,
            post_mortem: Some(PostMortemSummary {
                boot_count: u32::MAX,
                checkpoints: u8::MAX,
                fault: Some(fault),
            }),
            platform_panic: Some(PlatformPanic {
                hart: u8::MAX,
                in_interrupt: true,
                interrupted_pc: Some(u32::MAX),
                file: text(48).as_str().try_into().unwrap(),
                line: u32::MAX,
                column: u32::MAX,
            }),
        };
        let expected = Envelope::new(u64::MAX, u32::MAX, u64::MAX, u32::MAX, boot);
        assert_eq!(transmit(&expected), expected);
    }
    let mut checkpoints = heapless::Vec::new();
    for _ in 0..POST_MORTEM_CHECKPOINT_PAGE {
        checkpoints
            .push(Checkpoint {
                name: text(CHECKPOINT_NAME_BYTES).as_str().try_into().unwrap(),
                arg: u32::MAX,
                uptime_ms: u32::MAX,
                hart: u8::MAX,
            })
            .unwrap();
    }
    let page = PostMortemCheckpoints {
        first: u8::MAX,
        checkpoints,
    };
    let expected = Envelope::new(u64::MAX, u32::MAX, u64::MAX, u32::MAX, page);
    assert_eq!(transmit(&expected), expected);
}

const SERVED: [Key; 4] = sorted_keys([
    GetBootStatus::KEY,
    GetImageKeys::KEY,
    GetLinkHealth::KEY,
    GetPostMortemCheckpoints::KEY,
]);

#[test]
fn capabilities_page_out_the_same_sorted_keys_every_time() {
    let image_keys = ImageKeySet::new(&SERVED);
    assert!(
        SERVED
            .windows(2)
            .all(|pair| pair[0].to_u64() < pair[1].to_u64())
    );
    let page = image_keys.page(0);
    assert_eq!((page.first, page.total), (0, 4));
    assert_eq!(page.keys.as_slice(), &SERVED);
    assert_eq!(image_keys.page(0), page);
    assert!(image_keys.page(4).keys.is_empty());
    assert!(image_keys.contains(GetLinkHealth::KEY));
    assert!(!image_keys.contains(crate::system::WatchdogTest::KEY));
    let hello = image_keys.hello(0);
    assert_eq!(hello.keys, 4);
    assert_eq!(hello.keys_digest, digest(&SERVED));
    assert_ne!(
        hello.keys_digest,
        digest(&SERVED[..3]),
        "the digest names the set"
    );
}

#[test]
fn a_full_capability_page_fits_a_frame() {
    let keys: [Key; IMAGE_KEYS_PER_PAGE] =
        core::array::from_fn(|index| Key((index as u64).to_le_bytes()));
    let page = ImageKeySet::new(&keys).page(0);
    assert_eq!(page.keys.len(), IMAGE_KEYS_PER_PAGE);
    let expected = Envelope::new(u64::MAX, u32::MAX, u64::MAX, u32::MAX, page);
    assert_eq!(transmit(&expected), expected);
}

#[test]
fn a_request_and_its_reply_travel_in_opposite_directions() {
    assert_eq!(GetBootStatus::WIRE_KIND, WireKind::Command);
    assert_eq!(BootEvidence::WIRE_KIND, WireKind::Event);
    assert_eq!(Rejected::WIRE_KIND, WireKind::Event);
}
