//! The beacon-stamp probe's result line, and nothing else.

/// The probe's verdict as its result line states it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeResult {
    /// `OER-RESULT: pass beacon-stamp`: every round ran and its records
    /// were printed. It decides nothing about the Timestamp.
    Pass,
    /// `OER-RESULT: fail beacon-stamp <reason>`: the run ended early, for
    /// the reason it names.
    Fail(String),
}

/// The probe's result in `line`; `Ok(None)` for a line without a result
/// marker, an error for a result line that is not the beacon-stamp probe's.
pub fn parse(line: &str) -> Result<Option<ProbeResult>, String> {
    const MARKER: &str = "OER-RESULT: ";
    let Some(start) = line.find(MARKER) else {
        return Ok(None);
    };
    let result = line
        .get(start + MARKER.len()..)
        .unwrap_or_default()
        .trim_end();
    if result == "pass beacon-stamp" {
        return Ok(Some(ProbeResult::Pass));
    }
    if let Some(reason) = result.strip_prefix("fail beacon-stamp ")
        && !reason.is_empty()
    {
        return Ok(Some(ProbeResult::Fail(reason.to_owned())));
    }
    Err(format!(
        "a result line that is not the beacon-stamp probe's: `{result}`"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_beacon_stamp_pass_passes() {
        assert_eq!(
            parse("OER-RESULT: pass beacon-stamp"),
            Ok(Some(ProbeResult::Pass))
        );
        assert_eq!(
            parse("[1.2] OER-RESULT: pass beacon-stamp\r"),
            Ok(Some(ProbeResult::Pass))
        );
        assert!(parse("OER-RESULT: pass rx-chain").is_err());
        assert!(parse("OER-RESULT: pass beacon-stamp extra").is_err());
        assert!(parse("OER-RESULT: pass").is_err());
    }

    #[test]
    fn a_fail_keeps_its_reason() {
        assert_eq!(
            parse("OER-RESULT: fail beacon-stamp error=completion-timeout"),
            Ok(Some(ProbeResult::Fail(
                "error=completion-timeout".to_owned()
            )))
        );
        assert!(parse("OER-RESULT: fail beacon-stamp").is_err());
        assert!(parse("OER-RESULT: fail boot-smoke x").is_err());
    }

    #[test]
    fn other_lines_are_no_result() {
        assert_eq!(parse("STAMP rec n=0 frames=1"), Ok(None));
        assert_eq!(parse(""), Ok(None));
    }
}
