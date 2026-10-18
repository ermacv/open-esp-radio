//! Classify errors as scenario or infrastructure failures without changing the
//! public run-record schema.

use oer_tool_data_hil_run_bundle_format::run::Failure;
use oer_tool_data_hil_run_bundle_format::run::FailureKind;

pub fn classify(error: &(dyn std::error::Error + 'static)) -> Failure {
    let mut cause = Some(error);
    let mut kind = FailureKind::Scenario;
    while let Some(error) = cause {
        if error.is::<oer_tool_service_hil_link::error::LinkError>()
            || error.is::<oer_tool_service_hil_lab::Error>()
            || error.is::<std::io::Error>()
            || error.is::<oer_tool_base_process::Cancelled>()
            || error.is::<oer_tool_base_process::owned::DeadlineExceeded>()
        {
            kind = FailureKind::Infrastructure;
            break;
        }
        cause = error.source();
    }
    Failure::new(kind, error.to_string())
}
