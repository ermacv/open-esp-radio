use super::*;
use crate::sequence::seq;

const SOURCE: [u8; 6] = [0x02, 0x00, 0x00, 0x12, 0x34, 0x56];

#[test]
fn encodes_wildcard_probe_request() {
    let mut output = [0xa5; 64];
    let length = ProbeRequest {
        destination: BROADCAST_ADDRESS,
        source: SOURCE,
        bssid: BROADCAST_ADDRESS,
        sequence_number: seq(0x123),
        ssid: b"",
        supported_rates: &[0x82, 0x84, 0x8b, 0x96],
    }
    .encode(&mut output)
    .unwrap();

    assert_eq!(length, 32);
    assert_eq!(&output[0..2], &[0x40, 0x00]);
    assert_eq!(&output[2..4], &[0, 0]);
    assert_eq!(&output[4..10], &[0xff; 6]);
    assert_eq!(&output[10..16], &SOURCE);
    assert_eq!(&output[16..22], &[0xff; 6]);
    assert_eq!(&output[22..24], &[0x30, 0x12]);
    assert_eq!(&output[24..32], &[0, 0, 1, 4, 0x82, 0x84, 0x8b, 0x96]);
    assert_eq!(output[32], 0xa5);
}

#[test]
fn encodes_directed_request_and_extended_rates() {
    let rates = [
        0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
    ];
    let mut output = [0; 64];
    let length = ProbeRequest {
        destination: BROADCAST_ADDRESS,
        source: SOURCE,
        bssid: BROADCAST_ADDRESS,
        sequence_number: seq(0),
        ssid: b"open",
        supported_rates: &rates,
    }
    .encode(&mut output)
    .unwrap();

    assert_eq!(length, 46);
    assert_eq!(&output[24..30], &[0, 4, b'o', b'p', b'e', b'n']);
    assert_eq!(
        &output[30..40],
        &[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]
    );
    assert_eq!(&output[40..46], &[50, 4, 0x30, 0x48, 0x60, 0x6c]);
}

#[test]
fn rejects_invalid_inputs_before_touching_output() {
    let mut output = [0xa5; 31];
    let error = ProbeRequest {
        destination: BROADCAST_ADDRESS,
        source: SOURCE,
        bssid: BROADCAST_ADDRESS,
        sequence_number: seq(0),
        ssid: b"",
        supported_rates: &[0x82, 0x84, 0x8b, 0x96],
    }
    .encode(&mut output)
    .unwrap_err();
    assert_eq!(error, ProbeRequestError::OutputTooSmall { required: 32 });
    assert_eq!(output, [0xa5; 31]);

    assert_eq!(
        ProbeRequest {
            destination: BROADCAST_ADDRESS,
            source: SOURCE,
            bssid: BROADCAST_ADDRESS,
            sequence_number: seq(0),
            ssid: &[0; MAX_SSID_LEN + 1],
            supported_rates: &[0x82],
        }
        .encode(&mut [0; 64]),
        Err(ProbeRequestError::SsidTooLong)
    );
    assert_eq!(
        ProbeRequest {
            destination: BROADCAST_ADDRESS,
            source: SOURCE,
            bssid: BROADCAST_ADDRESS,
            sequence_number: seq(0),
            ssid: b"",
            supported_rates: &[],
        }
        .encode(&mut [0; 64]),
        Err(ProbeRequestError::NoSupportedRates)
    );
}
