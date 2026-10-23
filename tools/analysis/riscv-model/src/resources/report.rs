//! Resource provenance shared by host analysis reports.
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnalysisProfile {
    BlobrayCli,
    ImageChecks,
}

/// Defaults are policy, independent of clocks and of the reporting adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationLimits {
    pub work_units: u64,
    pub working_memory_bytes: u64,
    pub timeout_ms: u64,
}
impl AnalysisProfile {
    pub const fn limits(self) -> OperationLimits {
        match self {
            Self::BlobrayCli => OperationLimits {
                work_units: DEFAULT_WORK_UNITS,
                working_memory_bytes: DEFAULT_WORKING_BYTES,
                timeout_ms: DEFAULT_TIMEOUT_MS,
            },
            Self::ImageChecks => OperationLimits {
                work_units: DEFAULT_WORK_UNITS,
                working_memory_bytes: 1024 << 20,
                timeout_ms: 300_000,
            },
        }
    }
}

/// `max(minimum, per_instruction * N + constant) * multiplier`.
/// N is the number of decoded instructions in the function.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalLimit {
    pub scope: LocalBudgetScope,
    pub minimum: u64,
    pub per_instruction: u64,
    pub constant: u64,
    pub multiplier: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisLimits {
    #[serde(flatten)]
    pub operation: OperationLimits,
    pub local: Vec<LocalLimit>,
    pub value_alternatives: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisUsage {
    pub work_units: u64,
    pub peak_admitted_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LimitReason {
    LocalBudget(LocalBudgetScope),
    ValueAlternatives,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitCount {
    pub reason: LimitReason,
    pub events: u64,
    pub functions: u64,
    pub results: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisLimited {
    pub events_by_reason: Vec<LimitCount>,
    pub functions: u64,
    pub results: u64,
}

/// Counts collected while producing one function's existing typed outcomes.
/// Merge a function only once; looking up its cached summary adds no event.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionLimits {
    counts: Vec<LimitCount>,
    results: u64,
}
impl FunctionLimits {
    fn count(&mut self, reason: LimitReason) -> &mut LimitCount {
        if let Some(i) = self.counts.iter().position(|c| c.reason == reason) {
            return &mut self.counts[i];
        }
        self.counts.push(LimitCount {
            reason,
            events: 0,
            functions: 1,
            results: 0,
        });
        self.counts.last_mut().expect("inserted limit count")
    }
    pub fn event(&mut self, reason: LimitReason) {
        self.count(reason).events += 1;
    }
    pub fn outcome<T>(&mut self, outcome: &SummaryOutcome<T>) {
        if let SummaryOutcome::LocalBudgetExceeded(budget) = outcome {
            self.event(LimitReason::LocalBudget(budget.scope));
        }
    }
    pub fn semantic_gap(&mut self, reason: SemanticGapReason) {
        if let Some(reason) = LimitReason::semantic_gap(reason) {
            self.event(reason);
        }
    }
    /// One output record, possibly carrying more than one reason.
    pub fn result(&mut self, reasons: &[LimitReason]) {
        if reasons.is_empty() {
            return;
        }
        self.results += 1;
        for (i, &reason) in reasons.iter().enumerate() {
            if !reasons[..i].contains(&reason) {
                self.count(reason).results += 1;
            }
        }
    }
}
impl LimitReason {
    pub fn semantic_gap(reason: SemanticGapReason) -> Option<Self> {
        match reason {
            SemanticGapReason::ExpressionLimit => {
                Some(Self::LocalBudget(LocalBudgetScope::FunctionExpressions))
            }
            SemanticGapReason::AlternativeLimit => Some(Self::ValueAlternatives),
            _ => None,
        }
    }
}
impl AnalysisLimited {
    pub fn function(&mut self, limits: &FunctionLimits) {
        if limits.counts.is_empty() {
            return;
        }
        self.functions += 1;
        self.results += limits.results;
        for count in &limits.counts {
            if let Some(total) = self
                .events_by_reason
                .iter_mut()
                .find(|c| c.reason == count.reason)
            {
                total.events += count.events;
                total.functions += 1;
                total.results += count.results;
            } else {
                self.events_by_reason.push(count.clone());
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisResources {
    pub profile: AnalysisProfile,
    pub limits: Box<AnalysisLimits>,
    pub usage: AnalysisUsage,
    pub limited: Box<AnalysisLimited>,
    /// Limits without a typed local outcome; absence of counts is not zero events.
    pub unaccounted: Vec<String>,
}
impl AnalysisResources {
    pub fn new(
        profile: AnalysisProfile,
        operation: OperationLimits,
        usage: AnalysisUsage,
        limited: AnalysisLimited,
    ) -> Self {
        let local = [
            LocalBudgetScope::ReadFlow,
            LocalBudgetScope::ArgumentSummary,
            LocalBudgetScope::SignatureAudit,
            LocalBudgetScope::FunctionExpressions,
        ]
        .into_iter()
        .map(|scope| {
            if scope == LocalBudgetScope::FunctionExpressions {
                LocalLimit {
                    scope,
                    minimum: 0,
                    per_instruction: 8,
                    constant: 32,
                    multiplier: 1,
                }
            } else {
                LocalLimit {
                    scope,
                    minimum: 100_000,
                    per_instruction: 512,
                    constant: 0,
                    multiplier: if scope == LocalBudgetScope::SignatureAudit {
                        16
                    } else {
                        1
                    },
                }
            }
        })
        .collect();
        Self {
            profile,
            limits: Box::new(AnalysisLimits {
                operation,
                local,
                value_alternatives: MAX_VALUE_ALTERNATIVES,
            }),
            usage,
            limited: Box::new(limited),
            unaccounted: [
                "C4: word slots, recursion stack bytes and offset range",
                "C5: dispatch slices, jump-table entries and indexed-address depth",
                "C6: word, field, type, DWARF-origin and recursion depth/steps/paths",
                "C7: postdominator function-instruction capacity",
                "C8: address enumeration and declared-word runs (inventory consumer)",
                "C14: function CFG/value work has only the operation guard",
                "C16: execution event capacity has only an operation error",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        }
    }
    pub fn header(&self) -> String {
        format!(
            "analysis_resources: {}\n",
            serde_json::to_string(self).expect("resource provenance is JSON")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn events_functions_and_output_records_are_independent() {
        let reason = LimitReason::LocalBudget(LocalBudgetScope::ReadFlow);
        let mut one = FunctionLimits::default();
        one.event(reason);
        one.event(reason);
        one.result(&[reason, reason]);
        one.result(&[reason]);
        let mut total = AnalysisLimited::default();
        total.function(&one);
        assert_eq!(
            (
                total.events_by_reason[0].events,
                total.functions,
                total.results
            ),
            (2, 1, 2)
        );
        let mut two = FunctionLimits::default();
        two.event(reason);
        total.function(&two);
        assert_eq!(
            (
                total.events_by_reason[0].events,
                total.functions,
                total.results
            ),
            (3, 2, 2)
        );
    }
    #[test]
    fn profiles_keep_the_existing_operation_limits() {
        assert_eq!(
            AnalysisProfile::BlobrayCli.limits(),
            OperationLimits {
                work_units: 1_000_000_000,
                working_memory_bytes: 256 << 20,
                timeout_ms: 900_000
            }
        );
        assert_eq!(
            AnalysisProfile::ImageChecks.limits(),
            OperationLimits {
                work_units: 1_000_000_000,
                working_memory_bytes: 1024 << 20,
                timeout_ms: 300_000
            }
        );
    }
}
