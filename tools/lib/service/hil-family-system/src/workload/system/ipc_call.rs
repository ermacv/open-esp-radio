//! esp-hal's inter-processor call reaches each core, core 0 included: the
//! call's line enters through the platform's own vector-table entry.

use oer_hil_workload::{context::Context, require_keys};
use std::{path::Path, time::Duration};

use oer_hil_protocol::system::{CallAcrossCores, CoresCalled, IpcCall};

use crate::Result;
use oer_hil_link::SerialCapture;

const CALL_TIMEOUT: Duration = Duration::from_secs(5);

pub fn run(calls: u8, output: &Path, context: &Context<'_>) -> Result<()> {
    context.with_capture(output, |capture| call(capture, context, calls))?;
    eprintln!("ipc-call=PASS calls={calls}");
    Ok(())
}

fn call(capture: &SerialCapture, context: &Context<'_>, calls: u8) -> Result<()> {
    require_keys::<IpcCall>(capture)?;
    for call in 1..=calls {
        let called = capture.request(0, CallAcrossCores, CALL_TIMEOUT)?;
        context.results.observe(format!("call-{call:03}"), &called);
        validate(&called).map_err(|error| format!("call {call}: {error}"))?;
    }
    Ok(())
}

/// Each posted function ran, on the core it was posted to.
fn validate(called: &CoresCalled) -> std::result::Result<(), String> {
    for (route, ran_on, target) in [
        ("core 0 to core 0", called.core0_to_core0, 0),
        ("core 0 to core 1", called.core0_to_core1, 1),
        ("core 1 to core 0", called.core1_to_core0, 0),
    ] {
        match ran_on {
            Some(core) if core == target => {}
            Some(core) => return Err(format!("the call from {route} ran on core {core}")),
            None => return Err(format!("the call from {route} never ran")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_must_run_on_its_target_core() {
        let called = CoresCalled {
            core0_to_core0: Some(0),
            core0_to_core1: Some(1),
            core1_to_core0: Some(0),
        };
        assert!(validate(&called).is_ok());
        let lost = CoresCalled {
            core1_to_core0: None,
            ..called
        };
        assert!(
            validate(&lost)
                .unwrap_err()
                .contains("core 1 to core 0 never ran")
        );
        let misrouted = CoresCalled {
            core0_to_core1: Some(0),
            ..called
        };
        assert!(validate(&misrouted).is_err());
    }
}
