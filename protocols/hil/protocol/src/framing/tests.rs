use super::*;

/// Feeds `bytes` and returns the last complete frame as message `M`.
fn receive<M: crate::Message>(
    decoder: &mut FrameDecoder,
    bytes: &[u8],
) -> Option<Result<Envelope<M>, DecodeError>> {
    let mut last = None;
    decoder.feed(M::WIRE_KIND, bytes, |frame| {
        last = Some(frame.and_then(|frame| {
            frame
                .decode::<M>()
                .expect("the frame carries the expected type")
        }));
    });
    last
}
use crate::lifecycle::RequestReset;
use crate::{Envelope, WireKind};

fn command(sequence: u32) -> Envelope<RequestReset> {
    Envelope::new(0x1234_5678_9abc_def0, sequence, 42, sequence, RequestReset)
}

#[test]
fn round_trips_one_byte_at_a_time() {
    let expected = command(7);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    for byte in frame {
        if let Some(message) = receive(&mut decoder, core::slice::from_ref(byte)) {
            observed = Some(message.unwrap());
        }
    }
    assert_eq!(observed, Some(expected));
    assert_eq!(decoder.counters().frames, 1);
}

#[test]
fn wire_header_is_fixed_and_precedes_the_postcard_body() {
    let expected = command(7);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut raw = [0_u8; MAX_RAW_FRAME_BYTES];
    let decoded = cobs::decode(&frame[2..frame.len() - 1], &mut raw).unwrap();

    assert_eq!(&raw[..4], &WIRE_MAGIC);
    assert_eq!(raw[4], FRAMING_VERSION);
    assert_eq!(raw[5], WireKind::Command as u8);
    assert_eq!(
        u64::from_le_bytes(raw[6..14].try_into().unwrap()),
        expected.boot_id
    );
    assert_eq!(
        u32::from_le_bytes(raw[14..18].try_into().unwrap()),
        expected.message_sequence
    );
    assert_eq!(raw[KEY_RANGE], <RequestReset as crate::Message>::KEY.0);
    let payload_length = usize::from(u16::from_le_bytes(raw[LENGTH_RANGE].try_into().unwrap()));
    assert_eq!(decoded, WIRE_HEADER_BYTES + payload_length + CHECKSUM_BYTES);
    assert_eq!(
        postcard::from_bytes::<RequestReset>(
            &raw[WIRE_HEADER_BYTES..WIRE_HEADER_BYTES + payload_length]
        )
        .unwrap(),
        expected.body
    );
}

#[test]
fn a_frame_of_another_type_is_not_decoded_as_this_one() {
    let mut encoder = FrameEncoder::new();
    let frame = encoder
        .encode(&Envelope::new(7, 1, 0, 1, crate::base::GetBootStatus))
        .unwrap();
    let mut decoder = FrameDecoder::new();
    let mut seen = None;
    decoder.feed(WireKind::Command, frame, |result| {
        let frame = result.unwrap();
        assert_eq!(
            frame.key,
            <crate::base::GetBootStatus as crate::Message>::KEY
        );
        seen = Some(frame.decode::<RequestReset>().is_none());
    });
    assert_eq!(seen, Some(true));
    assert_eq!(decoder.counters().frames, 1);
}

#[test]
fn rejects_an_event_on_the_command_endpoint() {
    let expected = Envelope::new(7, 1, 0, 1, crate::base::Accepted);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive::<RequestReset>(&mut decoder, frame);
    assert_eq!(observed, Some(Err(DecodeError::MessageKind)));
    assert_eq!(decoder.counters().message_kind_errors, 1);
    assert_eq!(decoder.counters().deserialize_errors, 0);
}

#[test]
fn leading_delimiter_recovers_from_text_output() {
    let expected = command(9);
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    const NOISE: &[u8] = b"rom boot text\n";
    let mut input = [0_u8; MAX_WIRE_FRAME_BYTES + NOISE.len()];
    input[..NOISE.len()].copy_from_slice(NOISE);
    input[NOISE.len()..NOISE.len() + frame.len()].copy_from_slice(frame);

    let mut decoder = FrameDecoder::new();
    let mut observed = None;
    decoder.feed(
        WireKind::Command,
        &input[..NOISE.len() + frame.len()],
        |result| {
            if let Ok(frame) = result {
                observed = frame.decode::<RequestReset>().map(Result::unwrap);
            }
        },
    );
    assert_eq!(observed, Some(expected));
}

#[test]
fn rejects_checksum_corruption_and_recovers_for_next_frame() {
    let first = command(1);
    let second = command(2);
    let mut encoder = FrameEncoder::new();
    let mut damaged = [0_u8; MAX_WIRE_FRAME_BYTES];
    let first_frame = encoder.encode(&first).unwrap();
    damaged[..first_frame.len()].copy_from_slice(first_frame);
    let damaged_length = first_frame.len();
    damaged[damaged_length - 3] ^= 0x40;
    let second_frame = encoder.encode(&second).unwrap();

    let mut decoder = FrameDecoder::new();
    let mut errors = 0;
    decoder.feed(WireKind::Command, &damaged[..damaged_length], |result| {
        errors += usize::from(result.is_err());
    });
    let observed = receive(&mut decoder, second_frame).map(Result::unwrap);
    assert_eq!(errors, 1);
    assert_eq!(observed, Some(second));
}

#[test]
fn discards_overfull_noise_until_a_delimiter() {
    let expected = command(3);
    let mut decoder = FrameDecoder::new();
    decoder.feed(WireKind::Command, &[0], |_| {});
    decoder.feed(WireKind::Command, &[0x55; MAX_COBS_FRAME_BYTES + 4], |_| {});
    decoder.feed(WireKind::Command, &[0], |_| {});

    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let observed = receive(&mut decoder, frame).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
    assert_eq!(decoder.counters().overflows, 1);
}

#[test]
fn an_intact_frame_whose_payload_is_not_its_type_is_a_payload_error() {
    // Equal keys mean equal schemas, so only a defective sender produces
    // this: an intact frame whose body is no value of the type it names.
    let expected = Envelope::new(
        0x1234_5678_9abc_def0,
        9,
        42,
        9,
        crate::base::Rejected(crate::base::RejectReason::BootId),
    );
    let mut encoder = FrameEncoder::new();
    let frame = encoder.encode(&expected).unwrap();
    let encoded = &frame[2..frame.len() - 1];
    let mut raw = [0u8; 64];
    raw[..encoded.len()].copy_from_slice(encoded);
    let length = cobs::decode_in_place(&mut raw[..encoded.len()]).unwrap();
    let protected = length - CHECKSUM_BYTES;
    assert_eq!(
        protected,
        WIRE_HEADER_BYTES + 1,
        "a reject reason is one variant byte"
    );
    raw[WIRE_HEADER_BYTES] = 0x7f;
    let checksum = CRC32C.checksum(&raw[..protected]).to_le_bytes();
    raw[protected..length].copy_from_slice(&checksum);
    let mut reencoded = [0u8; 80];
    reencoded[1] = 0;
    let written = cobs::encode(&raw[..length], &mut reencoded[2..]);
    let frame = &reencoded[..written + 3];

    let mut decoder = FrameDecoder::new();
    let observed = receive::<crate::base::Rejected>(&mut decoder, frame);
    assert_eq!(observed, Some(Err(DecodeError::Payload)));
    assert_eq!(decoder.counters().frames, 1);
}

#[test]
fn a_message_serialized_by_its_producer_frames_like_an_encoded_one() {
    let expected = command(11);
    let mut encoder = FrameEncoder::new();
    let direct = encoder.encode(&expected).unwrap().to_vec();
    let outbound = Outbound::new(
        expected.message_sequence,
        expected.session_id,
        expected.request_id,
        &expected.body,
    )
    .unwrap();
    assert!(outbound.is::<RequestReset>());
    let queued = encoder
        .encode_outbound(expected.boot_id, &outbound)
        .unwrap()
        .to_vec();
    assert_eq!(queued, direct);
}

#[test]
fn invalid_internal_lengths_return_typed_errors() {
    let header = Header {
        kind: WireKind::Command,
        key: <RequestReset as crate::Message>::KEY,
        boot_id: 1,
        message_sequence: 2,
        session_id: 3,
        request_id: 4,
    };
    let mut encoder = FrameEncoder::new();
    for length in [MAX_POSTCARD_BYTES + 1, u16::MAX as usize, usize::MAX] {
        assert_eq!(encoder.frame(&header, length), Err(EncodeError::Serialize));
    }
    let mut decoder = FrameDecoder::new();
    assert_eq!(
        decoder.decode(WireKind::Command, usize::MAX),
        Err(DecodeError::TooShort)
    );
    let mut remaining = &[1, 2, 3][..];
    assert_eq!(take::<4>(&mut remaining), Err(DecodeError::TooShort));
    assert_eq!(remaining, &[1, 2, 3]);
}
