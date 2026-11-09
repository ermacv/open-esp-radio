//! The rx-rebind probe's result line, and nothing else.

/// The probe's verdict as its result line states it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeResult {
    /// `OER-RESULT: pass rx-rebind`: no detected violation at full coverage.
    Pass,
    /// `OER-RESULT: fail rx-rebind <reason>`: a violation, an insufficient
    /// experiment or an error, as the reason names it.
    Fail(String),
}

/// The probe's result in `line`; `Ok(None)` for a line without a result
/// marker, an error for a result line that is not the rx-rebind probe's.
pub fn parse(line: &str) -> Result<Option<ProbeResult>, String> {
    const MARKER: &str = "OER-RESULT: ";
    let Some(start) = line.find(MARKER) else {
        return Ok(None);
    };
    let result = line
        .get(start + MARKER.len()..)
        .unwrap_or_default()
        .trim_end();
    if result == "pass rx-rebind" {
        return Ok(Some(ProbeResult::Pass));
    }
    if let Some(reason) = result.strip_prefix("fail rx-rebind ")
        && !reason.is_empty()
    {
        return Ok(Some(ProbeResult::Fail(reason.to_owned())));
    }
    Err(format!(
        "a result line that is not the rx-rebind probe's: `{result}`"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_rx_rebind_pass_passes() {
        assert_eq!(
            parse("OER-RESULT: pass rx-rebind"),
            Ok(Some(ProbeResult::Pass))
        );
        assert_eq!(
            parse("[1.2] OER-RESULT: pass rx-rebind\r"),
            Ok(Some(ProbeResult::Pass))
        );
        assert!(parse("OER-RESULT: pass rx-chain").is_err());
        assert!(parse("OER-RESULT: pass rx-rebind extra").is_err());
        assert!(parse("OER-RESULT: pass").is_err());
    }

    #[test]
    fn a_fail_keeps_its_reason() {
        assert_eq!(
            parse("OER-RESULT: fail rx-rebind insufficient-experiment=rebind-coverage"),
            Ok(Some(ProbeResult::Fail(
                "insufficient-experiment=rebind-coverage".to_owned()
            )))
        );
        assert!(parse("OER-RESULT: fail rx-rebind").is_err());
        assert!(parse("OER-RESULT: fail boot-smoke x").is_err());
    }

    #[test]
    fn other_lines_are_no_result() {
        assert_eq!(parse("SEG 0 arm=fixed frames=1"), Ok(None));
        assert_eq!(parse(""), Ok(None));
    }
}
