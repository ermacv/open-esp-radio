//! Verdicts over a scenario's typed findings.

use serde::{Deserialize, Serialize};

/// At least one finding. A failed verdict without a finding cannot be
/// made, nor read.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    try_from = "Vec<F>",
    into = "Vec<F>",
    bound(
        serialize = "F: Clone + Serialize",
        deserialize = "F: Deserialize<'de>"
    )
)]
pub struct Findings<F>(Vec<F>);

/// An empty list where findings are required.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NoFinding;

impl core::fmt::Display for NoFinding {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("a failed verdict has at least one finding")
    }
}

impl std::error::Error for NoFinding {}

impl<F> TryFrom<Vec<F>> for Findings<F> {
    type Error = NoFinding;

    fn try_from(findings: Vec<F>) -> Result<Self, NoFinding> {
        if findings.is_empty() {
            Err(NoFinding)
        } else {
            Ok(Self(findings))
        }
    }
}

impl<F> From<Findings<F>> for Vec<F> {
    fn from(findings: Findings<F>) -> Self {
        findings.0
    }
}

impl<F> Findings<F> {
    pub fn as_slice(&self) -> &[F] {
        &self.0
    }
}

/// A scenario's verdict over one trace, with its own finding type.
/// Infrastructure failures are not verdicts: a run that could not record
/// its trace has none.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "kebab-case",
    tag = "verdict",
    bound(
        serialize = "F: Clone + Serialize",
        deserialize = "F: Deserialize<'de>"
    )
)]
pub enum Verdict<F> {
    Pass,
    Fail { findings: Findings<F> },
}

impl<F> Verdict<F> {
    /// `Pass` exactly when there is no finding.
    pub fn from_findings(findings: Vec<F>) -> Self {
        match Findings::try_from(findings) {
            Ok(findings) => Self::Fail { findings },
            Err(NoFinding) => Self::Pass,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    enum Finding {
        Order,
    }

    #[test]
    fn no_finding_is_a_pass_and_a_fail_has_one() {
        assert_eq!(Verdict::<Finding>::from_findings(Vec::new()), Verdict::Pass);
        let failed = Verdict::from_findings(vec![Finding::Order]);
        let Verdict::Fail { findings } = &failed else {
            panic!("failed")
        };
        assert_eq!(findings.as_slice(), [Finding::Order]);
        let json = serde_json::to_string(&failed).unwrap();
        assert_eq!(
            serde_json::from_str::<Verdict<Finding>>(&json).unwrap(),
            failed
        );
    }

    #[test]
    fn a_failed_verdict_without_findings_cannot_be_read() {
        assert!(
            serde_json::from_str::<Verdict<Finding>>(r#"{"verdict":"fail","findings":[]}"#)
                .is_err()
        );
    }
}
