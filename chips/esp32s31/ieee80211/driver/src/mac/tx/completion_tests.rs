use super::*;

fn ordinary(status: u8, detail: u8) -> Result<CompletionObservation, CompletionFault> {
    decode(CompletionRaw::ordinary(
        MacTxCompletionObservation::new_model(status, detail),
    ))
}

#[test]
fn rts_handler_preserves_all_detail_branches() {
    for detail in 0..=u8::MAX {
        let expected = match detail {
            1 | 3..=5 | 0xa0..=0xad => Ok(CompletionObservation::Observed(TxStatus::Collision)),
            0xc0 => Err(CompletionFault::SecurityKeyError),
            _ => Ok(CompletionObservation::NoEffect),
        };
        assert_eq!(ordinary(1, detail), expected, "RTS detail {detail:#x}");
    }
}

#[test]
fn hardware_trigger_metadata_does_not_change_the_dispatchers_zero_argument() {
    for status in [1, 4] {
        for trigger in [false, true] {
            let raw = MacTxCompletionObservation::new_model(status, 0xc0)
                .with_trigger_flow_model(trigger);
            assert_eq!(
                decode(CompletionRaw::ordinary(raw)),
                Err(CompletionFault::SecurityKeyError)
            );
        }
    }
}

#[test]
fn tx_error_decodes_every_hardware_detail_with_the_zero_dispatch_argument() {
    for detail in 0..=u8::MAX {
        let expected = match detail {
            0 => Ok(CompletionObservation::Observed(TxStatus::CtsTimeout)),
            1 | 3..=5 => Ok(CompletionObservation::Observed(TxStatus::Collision)),
            0xc0 => Err(CompletionFault::SecurityKeyError),
            _ => Ok(CompletionObservation::Observed(TxStatus::AckTimeout)),
        };
        assert_eq!(ordinary(4, detail), expected);
    }
}

#[test]
fn top_level_dispatch_rejects_only_statuses_outside_the_library_branches() {
    assert_eq!(
        ordinary(0, 0xff),
        Ok(CompletionObservation::Observed(TxStatus::Success))
    );
    assert_eq!(
        ordinary(2, 0xff),
        Ok(CompletionObservation::Observed(TxStatus::CtsTimeout))
    );
    assert_eq!(
        ordinary(5, 0xff),
        Ok(CompletionObservation::Observed(TxStatus::AckTimeout))
    );
    for status in core::iter::once(3).chain(6..=u8::MAX) {
        assert_eq!(
            ordinary(status, 0),
            Err(CompletionFault::InvalidStatus { status })
        );
    }
}

#[test]
fn no_effect_and_faults_never_construct_completed_metadata() {
    assert_eq!(
        TxCompletion::new_model(TxCookie(1), 1, 2),
        Ok(CompletionObservation::NoEffect)
    );
    assert_eq!(
        TxCompletion::new_model(TxCookie(1), 1, 0xc0),
        Err(CompletionFault::SecurityKeyError)
    );
    assert_eq!(
        TxCompletion::new_model(TxCookie(1), 4, 0xc0),
        Err(CompletionFault::SecurityKeyError)
    );
}

#[test]
fn generic_no_effect_does_not_transform_or_consume_an_owned_result() {
    let observation: CompletionObservation<core::convert::Infallible> =
        CompletionObservation::NoEffect;
    let mapped: CompletionObservation<TxCompletion> =
        observation.map(|impossible| match impossible {});
    assert_eq!(mapped, CompletionObservation::NoEffect);
}

#[test]
fn only_a_decoding_fault_quarantines_the_owner_exactly_once() {
    use core::cell::Cell;
    for (status, detail, quarantined) in
        [(0, 0, 0), (1, 2, 0), (1, 0xc0, 1), (4, 0xc0, 1), (3, 0, 1)]
    {
        let calls = Cell::new(0);
        let result = decode_or_quarantine(
            TxCookie(17),
            MacTxCompletionObservation::new_model(status, detail),
            || calls.set(calls.get() + 1),
        );
        assert_eq!(calls.get(), quarantined);
        assert_eq!(result.is_err(), quarantined != 0);
        if let Ok(CompletionObservation::Observed(completion)) = result {
            assert_eq!(completion.cookie, TxCookie(17));
        }
    }
}
