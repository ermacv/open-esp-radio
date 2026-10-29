//! The HIL AES scenario: the RPC image unwraps RFC 3394 key data with the
//! product hardware engine; the host checks every answer against the RFC's
//! known answer and against its own reference wrap, which shares no code
//! with the image's.
#![forbid(unsafe_code)]

use std::{process::ExitCode, time::Duration};

use oer_hil_agent::base::Service as _;
use oer_hil_harness::{Case, Cx, Failure, Scenario, check};
use oer_services_aes::{MAX_WRAPPED, UnwrapError, UnwrapKey, Unwrapped};

mod reference;

const SCENARIO: Scenario = Scenario {
    name: "aes",
    image: "esp32s31-rpc",
    services: &[oer_services_aes::Request::KEYS],
};

const CASES: [Case; 4] = [
    Case {
        name: "rfc3394-known-answer",
        run: known_answer,
    },
    Case {
        name: "reference-wraps",
        run: reference_wraps,
    },
    Case {
        name: "integrity-refused",
        run: integrity_refused,
    },
    Case {
        name: "length-refused",
        run: length_refused,
    },
];

/// One unwrap: 2 to 4 blocks of six rounds, each a scheduled hardware block.
const UNWRAP_TIMEOUT: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    oer_hil_harness::main(&SCENARIO, &CASES)
}

/// The device's answer to unwrapping `wrapped` under `kek`.
fn unwrap(cx: &mut Cx<'_>, kek: [u8; 16], wrapped: &[u8]) -> Result<Unwrapped, Failure> {
    let wrapped = heapless::Vec::from_slice(wrapped)
        .map_err(|_| Failure::Incomplete(format!("{} bytes exceed the request", wrapped.len())))?;
    cx.call(UnwrapKey { kek, wrapped }, UNWRAP_TIMEOUT)
}

/// RFC 3394 §4.1: 128 bits of key data under a 128-bit KEK.
fn known_answer(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let answer = unwrap(
        cx,
        reference::RFC3394_4_1_KEK,
        &reference::RFC3394_4_1_WRAPPED,
    )?;
    check(
        answer
            == Unwrapped::Plaintext(
                heapless::Vec::from_slice(&reference::RFC3394_4_1_KEY_DATA).unwrap_or_default(),
            ),
        format!("RFC 3394 §4.1 unwrapped to {answer:?}"),
    )
}

/// Key data of 128, 192 and 256 bits under several KEKs, wrapped on the host.
fn reference_wraps(cx: &mut Cx<'_>) -> Result<(), Failure> {
    for (index, (kek, data)) in reference::cases().into_iter().enumerate() {
        let wrapped = reference::wrap(&kek, &data);
        let answer = unwrap(cx, kek, &wrapped)?;
        let expected = Unwrapped::Plaintext(heapless::Vec::from_slice(&data).unwrap_or_default());
        check(
            answer == expected,
            format!(
                "case {index}: {} bytes of key data unwrapped to {answer:?}",
                data.len()
            ),
        )?;
    }
    Ok(())
}

/// A changed bit anywhere fails the integrity check (RFC 3394 §2.2.3).
fn integrity_refused(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let mut wrapped = reference::RFC3394_4_1_WRAPPED;
    for bit in [0, 70, 8 * wrapped.len() - 1] {
        wrapped[bit / 8] ^= 1 << (bit % 8);
        let answer = unwrap(cx, reference::RFC3394_4_1_KEK, &wrapped)?;
        wrapped[bit / 8] ^= 1 << (bit % 8);
        check(
            answer == Unwrapped::Refused(UnwrapError::IntegrityCheckFailed),
            format!("bit {bit} flipped: {answer:?}"),
        )?;
    }
    Ok(())
}

/// Input that is not 64-bit blocks with two blocks of key data (RFC 3394 §2).
fn length_refused(cx: &mut Cx<'_>) -> Result<(), Failure> {
    for length in [0, 8, 16, 20, MAX_WRAPPED - 1] {
        let answer = unwrap(cx, reference::RFC3394_4_1_KEK, &vec![0xa6; length])?;
        check(
            answer == Unwrapped::Refused(UnwrapError::InvalidLength),
            format!("{length} bytes: {answer:?}"),
        )?;
    }
    Ok(())
}
