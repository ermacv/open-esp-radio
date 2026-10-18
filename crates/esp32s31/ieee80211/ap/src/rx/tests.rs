use oer_esp32s31_ieee80211_mac::rx::{PUBLIC_HEADER_SIZE, RxSegment};

use oer_ieee80211_mac::ccmp::{CcmpPacketNumber, CcmpRxReplayState};

use super::*;

const AP: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];
const OTHER_PEER: [u8; 6] = [2, 0, 0, 0, 0, 4];
const DESTINATION: [u8; 6] = [2, 0, 0, 0, 0, 3];
const TAIL: usize = 0x38;
const LENGTH_SHIFT: u32 = 14;
const BIT_30: u32 = 1 << 30;
const BIT_31: u32 = 1 << 31;

#[derive(Default)]
struct Sink {
    ethernet: std::vec::Vec<std::vec::Vec<u8>>,
}

impl ApRxSink for Sink {
    fn publish(&mut self, event: ApRxEvent<'_>) {
        let mut frame = std::vec![0; event.frame.length()];
        event.frame.copy_to(&mut frame).unwrap();
        self.ethernet.push(frame);
    }
}

fn config() -> ApRxConfig {
    ApRxConfig {
        access_point: AP,
        ingress: RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        security: LinkProtection::Ccmp,
    }
}

fn open_config() -> ApRxConfig {
    ApRxConfig {
        security: LinkProtection::Open,
        ..config()
    }
}

fn duplicate_owner(association_id: u16, epoch: u32) -> ApRxDuplicateOwner {
    ApRxDuplicateOwner::new(association_id, epoch).unwrap()
}

fn segment(storage: &[u8; 192], descriptor_word0: u32) -> RxSegment<'_> {
    RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0,
        buffer: storage,
        next_descriptor_address: 0,
    }
}

fn open_fragment(
    storage: &mut [u8; 192],
    sequence: u16,
    fragment: u8,
    more_fragments: bool,
    address3: [u8; 6],
    payload: &[u8],
) -> u32 {
    let mpdu_length = 24 + payload.len();
    let signal_length = mpdu_length + 4;
    storage.fill(0);
    storage[0x1f] = 1;
    storage[TAIL..TAIL + 4].copy_from_slice(
        &(((signal_length + 4) as u32) << 16 | signal_length as u32).to_le_bytes(),
    );
    let frame = &mut storage[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + mpdu_length];
    let mut frame_control = 0x0108_u16;
    if more_fragments {
        frame_control |= 0x0400;
    }
    frame[..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&AP);
    frame[10..16].copy_from_slice(&PEER);
    frame[16..22].copy_from_slice(&address3);
    frame[22..24].copy_from_slice(&((sequence << 4) | u16::from(fragment)).to_le_bytes());
    frame[24..].copy_from_slice(payload);
    192 | (((PUBLIC_HEADER_SIZE + signal_length) as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31
}

#[expect(
    clippy::too_many_arguments,
    reason = "the test vector keeps every independently mutated 802.11/CCMP field explicit"
)]
fn protected_fragment(
    storage: &mut [u8; 192],
    sequence: u16,
    fragment: u8,
    more_fragments: bool,
    retry: bool,
    packet_number: u64,
    address3: [u8; 6],
    payload: &[u8],
) -> u32 {
    let mpdu_length = 24 + 8 + payload.len() + 8;
    let signal_length = mpdu_length + 4;
    storage.fill(0);
    storage[0x1f] = 1;
    storage[TAIL..TAIL + 4].copy_from_slice(
        &(((signal_length + 4) as u32) << 16 | signal_length as u32).to_le_bytes(),
    );
    let frame = &mut storage[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + mpdu_length];
    let mut frame_control = 0x4108_u16;
    if more_fragments {
        frame_control |= 0x0400;
    }
    if retry {
        frame_control |= 0x0800;
    }
    frame[..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&AP);
    frame[10..16].copy_from_slice(&PEER);
    frame[16..22].copy_from_slice(&address3);
    frame[22..24].copy_from_slice(&((sequence << 4) | u16::from(fragment)).to_le_bytes());
    frame[24..32].copy_from_slice(
        &CcmpHeader::new(
            CcmpPacketNumber::new(packet_number).unwrap(),
            CcmpKeyId::PAIRWISE,
        )
        .encode(),
    );
    frame[32..32 + payload.len()].copy_from_slice(payload);
    192 | (((PUBLIC_HEADER_SIZE + signal_length) as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31
}

#[test]
fn ordinary_pairwise_fast_path_matches_general_dispatch_and_duplicate_state() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2, 3, 4];
    let mut storage = [0_u8; 192];
    let descriptor =
        protected_fragment(&mut storage, 17, 0, false, false, 3, DESTINATION, &payload);
    let owner = duplicate_owner(1, 9).with_key_generation(4);
    let mut general = ApRxDispatcher::new(config());
    let mut fast = ApRxDispatcher::new(config());
    let mut general_sink = Sink::default();
    let mut fast_sink = Sink::default();

    let general_result = general.dispatch_at(
        segment(&storage, descriptor),
        oer_time::Instant::from_micros(1),
        |_| ApRxAdmission::authorized(owner),
        &mut general_sink,
    );
    let fast_result = fast
        .try_dispatch_ordinary_pairwise(
            segment(&storage, descriptor),
            |_| ApRxAdmission::authorized(owner),
            &mut fast_sink,
        )
        .expect("ordinary pairwise path is available");

    assert_eq!(fast_result, general_result);
    assert_eq!(fast_sink.ethernet, general_sink.ethernet);
    assert_eq!(
        fast_result,
        ApRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );

    storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
    assert_eq!(
        general.dispatch_at(
            segment(&storage, descriptor),
            oer_time::Instant::from_micros(2),
            |_| ApRxAdmission::authorized(owner),
            &mut general_sink,
        ),
        ApRxDispatch::Duplicate
    );
    assert_eq!(
        fast.try_dispatch_ordinary_pairwise(
            segment(&storage, descriptor),
            |_| ApRxAdmission::authorized(owner),
            &mut fast_sink,
        ),
        Some(ApRxDispatch::Duplicate)
    );
}

#[test]
fn retransmission_is_removed_as_a_duplicate_before_replay_detection() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2, 3, 4];
    let mut storage = [0_u8; 192];
    let owner = duplicate_owner(1, 11).with_key_generation(6);
    let replayed = CcmpReplayError::Replayed {
        packet_number: CcmpPacketNumber::new(7).unwrap(),
        highest: CcmpPacketNumber::new(7).unwrap(),
    };
    let mut general = ApRxDispatcher::new(config());
    let mut fast = ApRxDispatcher::new(config());
    let mut sink = Sink::default();

    let accepted = protected_fragment(&mut storage, 36, 0, false, false, 7, DESTINATION, &payload);
    for dispatcher in [&mut general, &mut fast] {
        assert!(matches!(
            dispatcher.dispatch_at(
                segment(&storage, accepted),
                oer_time::Instant::from_micros(1),
                |_| ApRxAdmission::authorized(owner),
                &mut sink,
            ),
            ApRxDispatch::Data { .. }
        ));
    }

    // The peer missed our ACK and retransmits the same MPDU and PN.
    let retry = protected_fragment(&mut storage, 36, 0, false, true, 7, DESTINATION, &payload);
    assert_eq!(
        general.dispatch_at(
            segment(&storage, retry),
            oer_time::Instant::from_micros(2),
            |_| ApRxAdmission::replayed(owner, replayed),
            &mut sink,
        ),
        ApRxDispatch::Duplicate
    );
    assert_eq!(
        fast.try_dispatch_ordinary_pairwise(
            segment(&storage, retry),
            |_| ApRxAdmission::replayed(owner, replayed),
            &mut sink,
        ),
        Some(ApRxDispatch::Duplicate)
    );

    // Without Retry, or with another Sequence Control, a reused PN is a replay.
    for descriptor in [
        protected_fragment(&mut storage, 36, 0, false, false, 7, DESTINATION, &payload),
        protected_fragment(&mut storage, 37, 0, false, true, 7, DESTINATION, &payload),
    ] {
        assert_eq!(
            general.dispatch_at(
                segment(&storage, descriptor),
                oer_time::Instant::from_micros(3),
                |_| ApRxAdmission::replayed(owner, replayed),
                &mut sink,
            ),
            ApRxDispatch::Rejected(ApRxError::Replay(replayed))
        );
    }
    // A rejected MPDU never becomes the accepted retry fingerprint.
    let retry = protected_fragment(&mut storage, 37, 0, false, true, 7, DESTINATION, &payload);
    assert_eq!(
        fast.try_dispatch_ordinary_pairwise(
            segment(&storage, retry),
            |_| ApRxAdmission::replayed(owner, replayed),
            &mut sink,
        ),
        Some(ApRxDispatch::Rejected(ApRxError::Replay(replayed)))
    );
    assert_eq!(sink.ethernet.len(), 2);
}

#[test]
fn ordinary_pairwise_fallback_preserves_fragment_clock() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2, 3, 4];
    let mut storage = [0_u8; 192];
    let descriptor =
        protected_fragment(&mut storage, 18, 0, false, false, 4, DESTINATION, &payload);
    let owner = duplicate_owner(1, 10).with_key_generation(5);
    let mut general = ApRxDispatcher::new(config());
    let mut fast = ApRxDispatcher::new(config());
    // Model the exceptional state left after any earlier fragment. The
    // typed ordinary leaf must decline without mutation; the caller then
    // enters the complete graph with its fragment clock.
    general.fragment_admission_active = true;
    fast.fragment_admission_active = true;
    let mut general_sink = Sink::default();
    let mut fast_sink = Sink::default();

    let mut admit = |_: ApRxAdmissionRequest| ApRxAdmission::authorized(owner);
    let general_result = general.dispatch_at(
        segment(&storage, descriptor),
        oer_time::Instant::from_micros(77),
        &mut admit,
        &mut general_sink,
    );
    assert_eq!(
        fast.try_dispatch_ordinary_pairwise(
            segment(&storage, descriptor),
            |_| ApRxAdmission::authorized(owner),
            &mut fast_sink,
        ),
        None
    );
    let fast_result = fast.dispatch_at(
        segment(&storage, descriptor),
        oer_time::Instant::from_micros(77),
        &mut admit,
        &mut fast_sink,
    );

    assert_eq!(fast_result, general_result);
    assert_eq!(fast_sink.ethernet, general_sink.ethernet);
    assert_eq!(
        fast_result,
        ApRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
}

#[test]
fn open_ap_reassembly_requires_live_peer_admission_and_copying_publication() {
    let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];
    let final_payload = [3, 4, 5];
    let mut first_storage = [0_u8; 192];
    let first_descriptor = open_fragment(
        &mut first_storage,
        0x123,
        0,
        true,
        DESTINATION,
        &first_payload,
    );
    let mut final_storage = [0_u8; 192];
    let final_descriptor = open_fragment(
        &mut final_storage,
        0x123,
        1,
        false,
        DESTINATION,
        &final_payload,
    );
    let mut dispatcher = ApRxDispatcher::new(open_config());
    let mut sink = Sink::default();

    assert!(!dispatcher.may_publish_in_place(segment(&first_storage, first_descriptor)));
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&first_storage, first_descriptor),
            oer_time::Instant::from_micros(10),
            |_| ApRxAdmission::unauthorized(),
            &mut sink,
        ),
        ApRxDispatch::Unauthorized
    );
    assert_eq!(dispatcher.clear_open_fragmentation(), 0);

    assert_eq!(
        dispatcher.dispatch_at(
            segment(&first_storage, first_descriptor),
            oer_time::Instant::from_micros(11),
            |_| ApRxAdmission::authorized(duplicate_owner(1, 1)),
            &mut sink,
        ),
        ApRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        }
    );
    first_storage[PUBLIC_HEADER_SIZE + 1] &= !0x04;
    first_storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&first_storage, first_descriptor),
            oer_time::Instant::from_micros(12),
            |_| ApRxAdmission::authorized(duplicate_owner(1, 1)),
            &mut sink,
        ),
        ApRxDispatch::Rejected(ApRxError::Fragment(
            OpenDataFragmentError::MoreFragmentsMismatch
        ))
    );
    first_storage[PUBLIC_HEADER_SIZE + 1] &= !0x08;
    first_storage[PUBLIC_HEADER_SIZE + 1] |= 0x04;
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&final_storage, final_descriptor),
            oer_time::Instant::from_micros(13),
            |_| ApRxAdmission::authorized(duplicate_owner(1, 2)),
            &mut sink,
        ),
        ApRxDispatch::Rejected(ApRxError::Fragment(OpenDataFragmentError::Orphan {
            fragment_number: 1
        }))
    );
    assert_eq!(dispatcher.clear_open_fragmentation(), 0);
    assert!(sink.ethernet.is_empty());
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&first_storage, first_descriptor),
            oer_time::Instant::from_micros(14),
            |_| ApRxAdmission::authorized(duplicate_owner(1, 2)),
            &mut sink,
        ),
        ApRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        }
    );
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&final_storage, final_descriptor),
            oer_time::Instant::from_micros(15),
            |_| ApRxAdmission::authorized(duplicate_owner(1, 2)),
            &mut sink,
        ),
        ApRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(sink.ethernet.len(), 1);
    assert_eq!(&sink.ethernet[0][..6], &DESTINATION);
    assert_eq!(&sink.ethernet[0][6..12], &PEER);
    assert_eq!(&sink.ethernet[0][12..14], &0x0800_u16.to_be_bytes());
    assert_eq!(&sink.ethernet[0][14..], &[1, 2, 3, 4, 5]);

    let _ = dispatcher.dispatch_at(
        segment(&first_storage, first_descriptor),
        oer_time::Instant::from_micros(20),
        |_| ApRxAdmission::authorized(duplicate_owner(1, 2)),
        &mut sink,
    );
    assert!(dispatcher.forget_peer(PEER));
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&final_storage, final_descriptor),
            oer_time::Instant::from_micros(21),
            |_| ApRxAdmission::authorized(duplicate_owner(1, 2)),
            &mut sink,
        ),
        ApRxDispatch::Rejected(ApRxError::Fragment(OpenDataFragmentError::Orphan {
            fragment_number: 1
        }))
    );
}

#[test]
fn ap_ccmp_fragments_commit_each_pn_after_exact_bounded_admission() {
    let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];
    let final_payload = [3, 4, 5];
    let mut first_storage = [0_u8; 192];
    let first_descriptor = protected_fragment(
        &mut first_storage,
        7,
        0,
        true,
        false,
        3,
        DESTINATION,
        &first_payload,
    );
    let mut final_storage = [0_u8; 192];
    let final_descriptor = protected_fragment(
        &mut final_storage,
        7,
        1,
        false,
        false,
        4,
        DESTINATION,
        &final_payload,
    );
    let mut dispatcher = ApRxDispatcher::new(config());
    let mut sink = Sink::default();
    let owner = duplicate_owner(1, 1).with_key_generation(7);
    let replay = core::cell::RefCell::new(CcmpRxReplayState::default());
    let pending = core::cell::RefCell::new(None);
    let mut admit = |request: ApRxAdmissionRequest| match request.operation() {
        ApRxAdmissionOperation::AuthorizeFragment => ApRxAdmission::authorized(owner),
        ApRxAdmissionOperation::PrepareFragment => {
            let header = request.ccmp_header().unwrap();
            match replay
                .borrow()
                .prepare(request.lane(), header.packet_number())
            {
                Ok(candidate) => {
                    pending.replace(Some(candidate));
                    ApRxAdmission::prepared(ApRxPreparedReplay {
                        peer: request.peer(),
                        lane: request.lane(),
                        ccmp_header: header,
                        owner,
                        candidate: ApRxPreparedCandidate::Model,
                    })
                }
                Err(error) => ApRxAdmission::rejected(ApRxError::Replay(error)),
            }
        }
        ApRxAdmissionOperation::CommitFragment(_) => {
            let candidate = pending
                .borrow_mut()
                .take()
                .expect("one prepared replay candidate");
            match replay.borrow_mut().commit(candidate) {
                Ok(()) => ApRxAdmission::authorized(owner),
                Err(error) => ApRxAdmission::rejected(ApRxError::Replay(error)),
            }
        }
        ApRxAdmissionOperation::Ordinary => {
            panic!("fragment path cannot request ordinary admission")
        }
    };

    assert_eq!(
        dispatcher.dispatch_at(
            segment(&first_storage, first_descriptor),
            oer_time::Instant::from_micros(1),
            &mut admit,
            &mut sink,
        ),
        ApRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        }
    );
    first_storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&first_storage, first_descriptor),
            oer_time::Instant::from_micros(2),
            &mut admit,
            &mut sink,
        ),
        ApRxDispatch::Duplicate
    );
    first_storage[PUBLIC_HEADER_SIZE + 32] ^= 0x01;
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&first_storage, first_descriptor),
            oer_time::Instant::from_micros(3),
            &mut admit,
            &mut sink,
        ),
        ApRxDispatch::Rejected(ApRxError::Fragment(
            OpenDataFragmentError::RetryPayloadMismatch { fragment_number: 0 }
        ))
    );
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&final_storage, final_descriptor),
            oer_time::Instant::from_micros(4),
            &mut admit,
            &mut sink,
        ),
        ApRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(sink.ethernet.len(), 1);
    assert_eq!(&sink.ethernet[0][14..], &[1, 2, 3, 4, 5]);

    final_storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&final_storage, final_descriptor),
            oer_time::Instant::from_micros(5),
            &mut admit,
            &mut sink,
        ),
        ApRxDispatch::Duplicate
    );
    assert_eq!(sink.ethernet.len(), 1);
    assert!(pending.borrow().is_none());
    assert_eq!(
        replay.borrow().highest(CcmpReplayLane::NonQos),
        CcmpPacketNumber::new(4)
    );
}

#[test]
fn protected_retry_cannot_turn_an_ordinary_mpdu_into_a_fragment_train() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let mut ordinary_storage = [0_u8; 192];
    let ordinary_descriptor = protected_fragment(
        &mut ordinary_storage,
        7,
        0,
        false,
        false,
        3,
        DESTINATION,
        &payload,
    );
    let mut retry_first_storage = [0_u8; 192];
    let retry_first_descriptor = protected_fragment(
        &mut retry_first_storage,
        7,
        0,
        true,
        true,
        4,
        DESTINATION,
        &payload,
    );
    let mut colliding_final_storage = [0_u8; 192];
    let colliding_final_descriptor = protected_fragment(
        &mut colliding_final_storage,
        7,
        1,
        false,
        false,
        5,
        DESTINATION,
        &[2],
    );
    let mut new_first_storage = [0_u8; 192];
    let new_first_descriptor = protected_fragment(
        &mut new_first_storage,
        8,
        0,
        true,
        true,
        4,
        DESTINATION,
        &payload,
    );
    let mut new_final_storage = [0_u8; 192];
    let new_final_descriptor = protected_fragment(
        &mut new_final_storage,
        8,
        1,
        false,
        false,
        5,
        DESTINATION,
        &[2],
    );
    let owner = duplicate_owner(1, 1).with_key_generation(7);
    let replay = core::cell::RefCell::new(CcmpRxReplayState::default());
    let pending = core::cell::RefCell::new(None);
    let mut admit = |request: ApRxAdmissionRequest| match request.operation() {
        ApRxAdmissionOperation::Ordinary => {
            let header = request.ccmp_header().expect("protected ordinary request");
            let candidate = {
                replay
                    .borrow()
                    .prepare(request.lane(), header.packet_number())
            };
            match candidate.and_then(|candidate| replay.borrow_mut().commit(candidate)) {
                Ok(()) => ApRxAdmission::authorized(owner),
                Err(error) => ApRxAdmission::rejected(ApRxError::Replay(error)),
            }
        }
        ApRxAdmissionOperation::AuthorizeFragment => ApRxAdmission::authorized(owner),
        ApRxAdmissionOperation::PrepareFragment => {
            let header = request.ccmp_header().expect("protected fragment request");
            match replay
                .borrow()
                .prepare(request.lane(), header.packet_number())
            {
                Ok(candidate) => {
                    pending.replace(Some(candidate));
                    ApRxAdmission::prepared(ApRxPreparedReplay {
                        peer: request.peer(),
                        lane: request.lane(),
                        ccmp_header: header,
                        owner,
                        candidate: ApRxPreparedCandidate::Model,
                    })
                }
                Err(error) => ApRxAdmission::rejected(ApRxError::Replay(error)),
            }
        }
        ApRxAdmissionOperation::CommitFragment(_) => {
            let candidate = pending
                .borrow_mut()
                .take()
                .expect("one prepared replay candidate");
            match replay.borrow_mut().commit(candidate) {
                Ok(()) => ApRxAdmission::authorized(owner),
                Err(error) => ApRxAdmission::rejected(ApRxError::Replay(error)),
            }
        }
    };
    let mut dispatcher = ApRxDispatcher::new(config());
    let mut sink = Sink::default();

    assert_eq!(
        dispatcher.dispatch_at(
            segment(&ordinary_storage, ordinary_descriptor),
            oer_time::Instant::from_micros(1),
            &mut admit,
            &mut sink,
        ),
        ApRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(sink.ethernet.len(), 1);

    assert_eq!(
        dispatcher.dispatch_at(
            segment(&retry_first_storage, retry_first_descriptor),
            oer_time::Instant::from_micros(2),
            &mut admit,
            &mut sink,
        ),
        ApRxDispatch::Duplicate
    );
    assert_eq!(dispatcher.fragments.active_contexts(), 0);
    assert!(pending.borrow().is_none());
    assert_eq!(
        replay.borrow().highest(CcmpReplayLane::NonQos),
        CcmpPacketNumber::new(3),
        "duplicate admission must precede replay prepare and commit"
    );

    assert_eq!(
        dispatcher.dispatch_at(
            segment(&colliding_final_storage, colliding_final_descriptor),
            oer_time::Instant::from_micros(3),
            &mut admit,
            &mut sink,
        ),
        ApRxDispatch::Rejected(ApRxError::Fragment(OpenDataFragmentError::Orphan {
            fragment_number: 1
        }))
    );
    assert_eq!(dispatcher.fragments.active_contexts(), 0);
    assert_eq!(sink.ethernet.len(), 1);

    // A retried fragment zero absent from ordinary history remains a
    // legitimate new train and owns its normal per-fragment replay edges.
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&new_first_storage, new_first_descriptor),
            oer_time::Instant::from_micros(4),
            &mut admit,
            &mut sink,
        ),
        ApRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        }
    );
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&new_final_storage, new_final_descriptor),
            oer_time::Instant::from_micros(5),
            &mut admit,
            &mut sink,
        ),
        ApRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(sink.ethernet.len(), 2);
    assert_eq!(
        replay.borrow().highest(CcmpReplayLane::NonQos),
        CcmpPacketNumber::new(5)
    );
}

#[test]
fn open_retry_cannot_turn_an_ordinary_mpdu_into_a_fragment_train() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let mut ordinary_storage = [0_u8; 192];
    let ordinary_descriptor =
        open_fragment(&mut ordinary_storage, 7, 0, false, DESTINATION, &payload);
    let owner = duplicate_owner(1, 1);
    let mut dispatcher = ApRxDispatcher::new(open_config());
    let mut sink = Sink::default();

    assert_eq!(
        dispatcher.dispatch_at(
            segment(&ordinary_storage, ordinary_descriptor),
            oer_time::Instant::from_micros(1),
            |_| ApRxAdmission::authorized(owner),
            &mut sink,
        ),
        ApRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );

    ordinary_storage[PUBLIC_HEADER_SIZE + 1] |= 0x04 | 0x08;
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&ordinary_storage, ordinary_descriptor),
            oer_time::Instant::from_micros(2),
            |_| ApRxAdmission::authorized(owner),
            &mut sink,
        ),
        ApRxDispatch::Duplicate
    );
    assert_eq!(dispatcher.clear_open_fragmentation(), 0);

    let mut final_storage = [0_u8; 192];
    let final_descriptor = open_fragment(&mut final_storage, 7, 1, false, DESTINATION, &[2]);
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&final_storage, final_descriptor),
            oer_time::Instant::from_micros(3),
            |_| ApRxAdmission::authorized(owner),
            &mut sink,
        ),
        ApRxDispatch::Rejected(ApRxError::Fragment(OpenDataFragmentError::Orphan {
            fragment_number: 1
        }))
    );

    let mut invalid_first_storage = [0_u8; 192];
    let invalid_first_descriptor =
        open_fragment(&mut invalid_first_storage, 8, 0, true, DESTINATION, &[0; 9]);
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&invalid_first_storage, invalid_first_descriptor),
            oer_time::Instant::from_micros(4),
            |_| ApRxAdmission::authorized(owner),
            &mut sink,
        ),
        ApRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        }
    );
    let mut invalid_final_storage = [0_u8; 192];
    let invalid_final_descriptor =
        open_fragment(&mut invalid_final_storage, 8, 1, false, DESTINATION, &[2]);
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&invalid_final_storage, invalid_final_descriptor),
            oer_time::Instant::from_micros(5),
            |_| ApRxAdmission::authorized(owner),
            &mut sink,
        ),
        ApRxDispatch::Rejected(ApRxError::Fragment(OpenDataFragmentError::InvalidLlcSnap))
    );

    invalid_first_storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch_at(
            segment(&invalid_first_storage, invalid_first_descriptor),
            oer_time::Instant::from_micros(6),
            |_| ApRxAdmission::authorized(owner),
            &mut sink,
        ),
        ApRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        },
        "failed fragment trains do not poison ordinary duplicate history"
    );
    assert_eq!(dispatcher.clear_open_fragmentation(), 1);
    assert_eq!(sink.ethernet.len(), 1);
}

#[test]
fn admits_only_authorized_peer_and_suppresses_its_retry() {
    const HEADER: usize = 24;
    const PAYLOAD: [u8; 4] = [1, 2, 3, 4];
    const MPDU: usize = HEADER + 8 + 8 + PAYLOAD.len() + 8;
    const SIGNAL: usize = MPDU + 4;
    let mut storage = [0_u8; 192];
    storage[0x1f] = 1;
    storage[TAIL..TAIL + 4]
        .copy_from_slice(&(((SIGNAL + 4) as u32) << 16 | SIGNAL as u32).to_le_bytes());
    let frame = &mut storage[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + MPDU];
    frame[..2].copy_from_slice(&0x4108_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&AP);
    frame[10..16].copy_from_slice(&PEER);
    frame[16..22].copy_from_slice(&DESTINATION);
    frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
    frame[HEADER..HEADER + 8].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    frame[HEADER + 8..HEADER + 16].copy_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0]);
    frame[HEADER + 16..HEADER + 20].copy_from_slice(&PAYLOAD);
    let descriptor_word0 =
        192 | (((PUBLIC_HEADER_SIZE + SIGNAL) as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31;
    let mut dispatcher = ApRxDispatcher::new(config());
    assert_eq!(
        dispatcher.reorder_key(segment(&storage, descriptor_word0)),
        None,
        "legacy data does not enter a BlockAck sequence space"
    );
    let mut sink = Sink::default();
    assert_eq!(
        dispatcher.dispatch_protected(segment(&storage, descriptor_word0), |_| None, &mut sink,),
        ApRxDispatch::Unauthorized
    );
    assert_eq!(
        dispatcher.dispatch_protected(
            segment(&storage, descriptor_word0),
            |candidate| (candidate == PEER).then_some(duplicate_owner(1, 1)),
            &mut sink,
        ),
        ApRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(&sink.ethernet[0][..6], &DESTINATION);
    assert_eq!(&sink.ethernet[0][6..12], &PEER);
    assert_eq!(&sink.ethernet[0][14..], &PAYLOAD);

    storage[PUBLIC_HEADER_SIZE + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch_protected(
            segment(&storage, descriptor_word0),
            |candidate| (candidate == PEER).then_some(duplicate_owner(1, 1)),
            &mut sink,
        ),
        ApRxDispatch::Duplicate
    );

    storage[PUBLIC_HEADER_SIZE + 10..PUBLIC_HEADER_SIZE + 16].copy_from_slice(&OTHER_PEER);
    assert_eq!(
        dispatcher.dispatch_protected(
            segment(&storage, descriptor_word0),
            |candidate| match candidate {
                PEER => Some(duplicate_owner(1, 1)),
                OTHER_PEER => Some(duplicate_owner(2, 1)),
                _ => None,
            },
            &mut sink,
        ),
        ApRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
}

#[test]
fn reused_pairwise_pn_is_rejected_before_publication_even_with_a_new_sequence() {
    const HEADER: usize = 24;
    const PAYLOAD: [u8; 4] = [1, 2, 3, 4];
    const MPDU: usize = HEADER + 8 + 8 + PAYLOAD.len() + 8;
    const SIGNAL: usize = MPDU + 4;
    let mut storage = [0_u8; 192];
    storage[0x1f] = 1;
    storage[TAIL..TAIL + 4]
        .copy_from_slice(&(((SIGNAL + 4) as u32) << 16 | SIGNAL as u32).to_le_bytes());
    let frame = &mut storage[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + MPDU];
    frame[..2].copy_from_slice(&0x4108_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&AP);
    frame[10..16].copy_from_slice(&PEER);
    frame[16..22].copy_from_slice(&DESTINATION);
    frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
    frame[HEADER..HEADER + 8].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    frame[HEADER + 8..HEADER + 16].copy_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0]);
    frame[HEADER + 16..HEADER + 20].copy_from_slice(&PAYLOAD);
    let descriptor_word0 =
        192 | (((PUBLIC_HEADER_SIZE + SIGNAL) as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31;
    let mut dispatcher = ApRxDispatcher::new(config());
    let mut sink = Sink::default();
    let mut replay = CcmpRxReplayState::default();
    let mut admit = |request: ApRxAdmissionRequest| {
        if matches!(
            request.operation(),
            ApRxAdmissionOperation::AuthorizeFragment
        ) {
            return ApRxAdmission::authorized(duplicate_owner(1, 1));
        }
        let header = request
            .ccmp_header()
            .expect("WPA2 dispatch carries one parsed CCMP header");
        match replay.prepare(request.lane(), header.packet_number()) {
            Ok(candidate) => match replay.commit(candidate) {
                Ok(()) => ApRxAdmission::authorized(duplicate_owner(1, 1)),
                Err(error) => ApRxAdmission::rejected(ApRxError::Replay(error)),
            },
            Err(error) => ApRxAdmission::rejected(ApRxError::Replay(error)),
        }
    };

    assert_eq!(
        dispatcher.dispatch(segment(&storage, descriptor_word0), &mut admit, &mut sink,),
        ApRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    storage[PUBLIC_HEADER_SIZE + 22..PUBLIC_HEADER_SIZE + 24]
        .copy_from_slice(&0x4560_u16.to_le_bytes());
    let pn3 = CcmpPacketNumber::new(3).unwrap();
    assert_eq!(
        dispatcher.dispatch(segment(&storage, descriptor_word0), &mut admit, &mut sink,),
        ApRxDispatch::Rejected(ApRxError::Replay(CcmpReplayError::Replayed {
            packet_number: pn3,
            highest: pn3,
        })),
    );
    assert_eq!(sink.ethernet.len(), 1);

    storage[PUBLIC_HEADER_SIZE + HEADER + 3] = 0x60;
    assert_eq!(
        dispatcher.dispatch(segment(&storage, descriptor_word0), &mut admit, &mut sink,),
        ApRxDispatch::Rejected(ApRxError::PairwiseKeyId(1)),
    );
    assert_eq!(sink.ethernet.len(), 1);
}

#[test]
fn duplicate_slots_are_reclaimed_across_close_reassociation_and_stop() {
    let mut dispatcher = ApRxDispatcher::new(config());
    let first = duplicate_owner(1, 1);
    assert!(
        !dispatcher
            .duplicate_filter(PEER, first)
            .is_duplicate(false, 0x1230, None)
    );
    assert!(
        dispatcher
            .duplicate_filter(PEER, first)
            .is_duplicate(true, 0x1230, None)
    );

    assert!(dispatcher.forget_peer(PEER));
    assert!(
        !dispatcher
            .duplicate_filter(PEER, first)
            .is_duplicate(true, 0x1230, None)
    );

    // Same-address reassociation retains its AID but owns a new epoch.
    let reassociated = duplicate_owner(1, 2);
    assert!(
        !dispatcher
            .duplicate_filter(PEER, reassociated)
            .is_duplicate(true, 0x1230, None)
    );

    // Churn through every bounded AID and then reuse one. No stale peer
    // can consume capacity because an AID selects its exact slot.
    for association_id in 1..=AP_MAX_CLIENTS as u16 {
        let mut peer = PEER;
        peer[5] = u8::try_from(association_id).unwrap();
        let owner = duplicate_owner(association_id, 10 + u32::from(association_id));
        assert!(!dispatcher.duplicate_filter(peer, owner).is_duplicate(
            false,
            association_id << 4,
            None
        ));
    }
    assert_eq!(
        dispatcher.duplicates.iter().flatten().count(),
        AP_MAX_CLIENTS
    );
    assert!(
        !dispatcher
            .duplicate_filter(OTHER_PEER, duplicate_owner(1, 99))
            .is_duplicate(true, 0x1230, None)
    );

    dispatcher.reset(config());
    assert!(dispatcher.duplicates.iter().all(Option::is_none));
    assert!(
        !dispatcher
            .duplicate_filter(PEER, duplicate_owner(1, 100))
            .is_duplicate(true, 0x1230, None)
    );
}
