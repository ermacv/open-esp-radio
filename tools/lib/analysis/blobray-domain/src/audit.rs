//! Final-image control-transfer policy. It is not proof about dynamic targets.
use crate::*;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForbiddenTargetRange {
    pub name: String,
    pub start: u32,
    pub end: u64,
}
impl ForbiddenTargetRange {
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty()
            || self.name.len() > 256
            || self.end <= u64::from(self.start)
            || self.end > (1u64 << 32)
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "invalid forbidden target range",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForbiddenTarget {
    pub section: u32,
    pub site: u32,
    pub target: u32,
    pub range: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TargetAuditSummary {
    pub sections: u64,
    pub executable_bytes: u64,
    pub embedded_data_bytes: u64,
    pub instructions: u64,
    pub unsupported_non_control: u64,
    pub unresolved_indirect: u64,
    pub coverage_gaps: u64,
    pub forbidden_targets: u64,
}

/// Auditable failures retain the exact site and a bounded encoding prefix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TargetAuditRecord {
    Forbidden {
        finding: ForbiddenTarget,
    },
    Gap {
        section: u32,
        site: u32,
        encoding: Vec<u8>,
        reason: TargetAuditGapReason,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetAuditGapReason {
    InstructionLength,
    UnsupportedEncoding,
}

/// Outcome of a check over its declared scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckVerdict {
    Pass,
    Fail,
    Inconclusive,
}
