//! An image's result line, and nothing else: the beacon-stamp probe's
//! (`beacon-stamp`) or the responder's (`ack-responder`).

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

/// Image `image`'s result in `line`; `Ok(None)` for a line without a
/// result marker, an error for a result line of another image.
pub fn parse(line: &str, image: &str) -> Result<Option<ProbeResult>, String> {
    const MARKER: &str = "OER-RESULT: ";
    let Some(start) = line.find(MARKER) else {
        return Ok(None);
    };
    let result = line
        .get(start + MARKER.len()..)
        .unwrap_or_default()
        .trim_end();
    if result.strip_prefix("pass ") == Some(image) {
        return Ok(Some(ProbeResult::Pass));
    }
    if let Some(reason) = result
        .strip_prefix("fail ")
        .and_then(|rest| rest.strip_prefix(image))
        .and_then(|rest| rest.strip_prefix(' '))
        && !reason.is_empty()
    {
        return Ok(Some(ProbeResult::Fail(reason.to_owned())));
    }
    Err(format!("a result line that is not {image}'s: `{result}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_beacon_stamp_pass_passes() {
        assert_eq!(
            parse("OER-RESULT: pass beacon-stamp", "beacon-stamp"),
            Ok(Some(ProbeResult::Pass))
        );
        assert_eq!(
            parse("[1.2] OER-RESULT: pass beacon-stamp\r", "beacon-stamp"),
            Ok(Some(ProbeResult::Pass))
        );
        assert!(parse("OER-RESULT: pass rx-chain", "beacon-stamp").is_err());
        assert!(parse("OER-RESULT: pass beacon-stamp extra", "beacon-stamp").is_err());
        assert!(parse("OER-RESULT: pass", "beacon-stamp").is_err());
    }

    #[test]
    fn a_fail_keeps_its_reason() {
        assert_eq!(
            parse(
                "OER-RESULT: fail beacon-stamp error=completion-timeout",
                "beacon-stamp"
            ),
            Ok(Some(ProbeResult::Fail(
                "error=completion-timeout".to_owned()
            )))
        );
        assert!(parse("OER-RESULT: fail beacon-stamp", "beacon-stamp").is_err());
        assert!(parse("OER-RESULT: fail boot-smoke x", "beacon-stamp").is_err());
    }

    #[test]
    fn the_responder_has_its_own_result_line() {
        assert_eq!(
            parse("OER-RESULT: pass ack-responder", "ack-responder"),
            Ok(Some(ProbeResult::Pass))
        );
        assert!(parse("OER-RESULT: pass ack-responder", "beacon-stamp").is_err());
        assert!(parse("OER-RESULT: pass beacon-stampx", "beacon-stamp").is_err());
    }

    #[test]
    fn other_lines_are_no_result() {
        assert_eq!(parse("STAMP rec n=0 frames=1", "beacon-stamp"), Ok(None));
        assert_eq!(parse("", "beacon-stamp"), Ok(None));
    }
}
