//! An interrupt-table source stays silent until its owner enables it and
//! after its owner disables it again.

use oer_hil_workload::{context::Context, require_keys};
use std::{path::Path, time::Duration};

use oer_hil_protocol::system::{ProbeSourceGate, SourceGate, SourceGated};

use crate::Result;
use oer_hil_link::SerialCapture;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn run(probes: u8, output: &Path, context: &Context<'_>) -> Result<()> {
    context.with_capture(output, |capture| probe(capture, context, probes))?;
    eprintln!("interrupt-table=PASS probes={probes}");
    Ok(())
}

fn probe(capture: &SerialCapture, context: &Context<'_>, probes: u8) -> Result<()> {
    require_keys::<SourceGate>(capture)?;
    for probe in 1..=probes {
        let gated = capture.request(0, ProbeSourceGate, PROBE_TIMEOUT)?;
        context.results.observe(format!("probe-{probe:03}"), &gated);
        validate(&gated).map_err(|error| format!("probe {probe}: {error}"))?;
    }
    Ok(())
}

/// The handler ran exactly once, while its source was enabled.
fn validate(gated: &SourceGated) -> std::result::Result<(), String> {
    if gated.before_enable != 0 {
        return Err(format!(
            "the handler ran {} times before its source was enabled",
            gated.before_enable
        ));
    }
    if gated.enabled != 1 {
        return Err(format!(
            "the handler ran {} times for one pending alarm once its source was enabled",
            gated.enabled
        ));
    }
    if gated.after_disable != 0 {
        return Err(format!(
            "the handler ran {} times after its source was disabled",
            gated.after_disable
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handler_runs_only_while_its_source_is_enabled() {
        let gated = SourceGated {
            before_enable: 0,
            enabled: 1,
            after_disable: 0,
        };
        assert!(validate(&gated).is_ok());
        for leak in [
            SourceGated {
                before_enable: 1,
                ..gated
            },
            SourceGated {
                enabled: 0,
                ..gated
            },
            SourceGated {
                after_disable: 1,
                ..gated
            },
        ] {
            assert!(validate(&leak).is_err(), "{leak:?}");
        }
    }
}
