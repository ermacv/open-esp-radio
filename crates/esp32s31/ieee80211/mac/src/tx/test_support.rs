//! Checked semantic completion fixtures for TX unit tests.

use super::{CompletionObservation, TxCompletion, TxCookie};

pub(crate) fn model_completion(cookie: TxCookie, status: u8, detail: u8) -> TxCompletion {
    match TxCompletion::new_model(cookie, status, detail).expect("valid model observation") {
        CompletionObservation::Observed(completion) => completion,
        CompletionObservation::NoEffect => panic!("fixture needs an observed completion"),
    }
}
