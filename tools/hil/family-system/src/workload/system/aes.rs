//! Reference generation and verdicts live on the host. The target only serves
//! individual KeyUnwrap requests, returning the same engine after each one.
use crate::Result;
use oer_crypto::{
    Aes128Key, KeyData, KeyUnwrapRequest,
    conformance::{RFC3394_4_1_CIPHERTEXT, RFC3394_4_1_KEK},
};
use oer_hil_protocol::system::{AesKeyData, AesKeyUnwrap, AesKeyUnwrapped, AesUnwrapError};
use oer_hil_scenario::workload::{context::Context, require_keys};
use oer_ieee80211_cipher::aes::{SoftwareAes128, SoftwareKeyUnwrap, software_aes128_key_wrap};
use std::{path::Path, time::Duration};

const LENGTHS: [usize; 8] = [16, 24, 32, 40, 64, 120, 248, 504];

fn cases() -> Result<Vec<AesKeyUnwrap>> {
    let mut requests = vec![AesKeyUnwrap {
        operation: 1,
        kek: RFC3394_4_1_KEK,
        wrapped: RFC3394_4_1_CIPHERTEXT.as_slice().try_into()?,
    }];
    for cycle in 0..32u8 {
        for length in LENGTHS {
            let kek = core::array::from_fn(|i| cycle.wrapping_mul(17).wrapping_add(i as u8));
            let plain: Vec<_> = (0..length)
                .map(|i| (i as u8).wrapping_mul(29).wrapping_add(cycle))
                .collect();
            let wrapped = software_aes128_key_wrap(&kek, &plain)
                .map_err(|e| format!("reference wrap: {e:?}"))?;
            for corrupt in [false, true, false] {
                let mut wrapped: AesKeyData = wrapped.as_bytes().try_into()?;
                if corrupt {
                    let last = wrapped.last_mut().ok_or("empty wrapped reference")?;
                    *last ^= 1;
                }
                requests.push(AesKeyUnwrap {
                    operation: u32::try_from(requests.len() + 1)?,
                    kek,
                    wrapped,
                });
            }
        }
    }
    Ok(requests)
}

fn reference(request: &AesKeyUnwrap) -> Result<std::result::Result<AesKeyData, AesUnwrapError>> {
    let wrapped = KeyData::<512>::try_copy(&request.wrapped).map_err(|_| "reference capacity")?;
    let response = SoftwareKeyUnwrap::new(SoftwareAes128).unwrap_now(KeyUnwrapRequest::new(
        Aes128Key::copy_from(&request.kek),
        wrapped,
    ));
    match response.plaintext() {
        Ok(bytes) => Ok(Ok(bytes.try_into()?)),
        Err(oer_crypto::KeyUnwrapError::IntegrityCheckFailed) => {
            Ok(Err(AesUnwrapError::IntegrityCheckFailed))
        }
        Err(error) => Err(format!("unexpected software reference error: {error:?}").into()),
    }
}

fn matches(
    request: &AesKeyUnwrap,
    expected: &std::result::Result<AesKeyData, AesUnwrapError>,
    actual: &AesKeyUnwrapped,
    successes: u32,
) -> bool {
    actual.operation == request.operation
        && actual.completed == request.operation
        && actual.succeeded == successes
        && actual.refused == request.operation - successes
        && actual.result == *expected
}

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    context.with_capture(output, |capture| {
        require_keys::<AesKeyUnwrap>(capture)?;
        let mut successes = 0;
        let mut last = None;
        for request in cases()? {
            let expected = reference(&request)?;
            successes += u32::from(expected.is_ok());
            let actual = capture.request(0, request.clone(), Duration::from_secs(5))?;
            if !matches(&request, &expected, &actual, successes) {
                context.results.observe(
                    "aes-key-unwrap-first-mismatch",
                    &serde_json::json!({
                        "request": request, "expected": expected, "actual": actual,
                    }),
                );
                return Err(
                    format!("product AES mismatch at operation {}", request.operation).into(),
                );
            }
            last = Some(actual);
        }
        context.results.observe("aes-key-unwrap-counters", &last);
        context
            .results
            .claim("aes-key-unwrap", &["AES-192/256 KEKs", "Wi-Fi traffic"]);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_crypto::conformance::RFC3394_4_1_KEY_DATA;
    #[test]
    fn reference_covers_rfc_gtk_lengths_rekey_and_recovery_after_every_refusal() {
        let cases = cases().unwrap();
        assert_eq!(cases.len(), 769);
        assert_eq!(
            reference(&cases[0]).unwrap().unwrap().as_slice(),
            RFC3394_4_1_KEY_DATA
        );
        let mut successes = 0;
        for (index, case) in cases.iter().enumerate() {
            let result = reference(case).unwrap();
            assert_eq!(result.is_err(), index > 0 && (index - 1) % 3 == 1);
            successes += u32::from(result.is_ok());
            let mut actual = AesKeyUnwrapped {
                operation: case.operation,
                completed: case.operation,
                succeeded: successes,
                refused: case.operation - successes,
                result: result.clone(),
            };
            assert!(matches(case, &result, &actual, successes));
            actual.result = Err(AesUnwrapError::Unavailable);
            assert!(!matches(case, &result, &actual, successes));
        }
        assert_eq!(successes, 513);
    }
}
