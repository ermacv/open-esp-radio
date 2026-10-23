//! Resource provenance shared by host analysis reports.
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnalysisProfile {
    BlobrayCli,
    ImageChecks,
    RegisterInventory,
    SignatureCorpus,
    VendorScenario,
    VendorCopies,
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
            Self::RegisterInventory | Self::SignatureCorpus => OperationLimits {
                work_units: DEFAULT_WORK_UNITS,
                working_memory_bytes: DEFAULT_WORKING_BYTES,
                timeout_ms: 3_600_000,
            },
            Self::VendorScenario => OperationLimits {
                work_units: 2_000_000_000,
                working_memory_bytes: 256 << 20,
                timeout_ms: 600_000,
            },
            Self::VendorCopies => OperationLimits {
                work_units: 1_000_000_000_000,
                working_memory_bytes: 8192 << 20,
                timeout_ms: 1_200_000,
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
    pub zero_for_empty: bool,
}

impl LocalLimit {
    pub const fn of(scope: LocalBudgetScope) -> Self {
        let mut limit = Self {
            scope,
            minimum: 100_000,
            per_instruction: 512,
            constant: 0,
            multiplier: 1,
            zero_for_empty: false,
        };
        match scope {
            LocalBudgetScope::SignatureAudit => limit.multiplier = 16,
            LocalBudgetScope::FunctionExpressions => {
                limit.minimum = 0;
                limit.per_instruction = 8;
                limit.constant = 32;
                limit.zero_for_empty = true;
            }
            LocalBudgetScope::ReadFlow | LocalBudgetScope::ArgumentSummary => {}
        }
        limit
    }

    /// Work allowances saturate on overflow; expression admission uses
    /// `checked_limit` to preserve its operation error on capacity overflow.
    pub fn limit(self, instructions: u64) -> u64 {
        self.checked_limit(instructions).unwrap_or(u64::MAX)
    }

    pub fn checked_limit(self, instructions: u64) -> Option<u64> {
        if self.zero_for_empty && instructions == 0 {
            return Some(0);
        }
        self.per_instruction
            .checked_mul(instructions)?
            .checked_add(self.constant)?
            .max(self.minimum)
            .checked_mul(self.multiplier)
    }
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
    pub fn value_events(events: [u64; 2]) -> Self {
        let mut limits = Self::default();
        for (reason, events) in [
            LimitReason::LocalBudget(LocalBudgetScope::FunctionExpressions),
            LimitReason::ValueAlternatives,
        ]
        .into_iter()
        .zip(events)
        {
            if events != 0 {
                limits.count(reason).events = events;
            }
        }
        limits
    }

    pub fn merge(&mut self, other: &Self) {
        for count in &other.counts {
            self.count(count.reason).events += count.events;
        }
    }

    fn count(&mut self, reason: LimitReason) -> &mut LimitCount {
        if let Some(i) = self.counts.iter().position(|c| c.reason == reason) {
            return self.counts.get_mut(i).expect("existing limit count");
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
    /// One output record, possibly carrying more than one reason.
    pub fn result(&mut self, reasons: &[LimitReason]) {
        if reasons.is_empty() {
            return;
        }
        self.results += 1;
        for (i, &reason) in reasons.iter().enumerate() {
            if !reasons.iter().take(i).any(|&earlier| earlier == reason) {
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
    /// Combine independent inputs, whose function identities do not overlap.
    pub fn merge(&mut self, other: &Self) {
        self.functions += other.functions;
        self.results += other.results;
        for count in &other.events_by_reason {
            if let Some(total) = self
                .events_by_reason
                .iter_mut()
                .find(|c| c.reason == count.reason)
            {
                total.events += count.events;
                total.functions += count.functions;
                total.results += count.results;
            } else {
                self.events_by_reason.push(count.clone());
            }
        }
    }

    pub fn result(&mut self, reason: LimitReason) {
        self.results += 1;
        if let Some(count) = self
            .events_by_reason
            .iter_mut()
            .find(|c| c.reason == reason)
        {
            count.results += 1;
        } else {
            self.events_by_reason.push(LimitCount {
                reason,
                events: 0,
                functions: 0,
                results: 1,
            });
        }
    }

    /// Replace output counts after filtering, retaining whole-operation events.
    pub fn project_results(&mut self, projected: &Self) {
        self.results = projected.results;
        for count in &mut self.events_by_reason {
            count.results = projected
                .events_by_reason
                .iter()
                .find(|c| c.reason == count.reason)
                .map_or(0, |c| c.results);
        }
    }

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

/// Limits without typed local provenance, documented in the analysis architecture.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnaccountedLimit {
    WordSlots,
    RecursionStackBytes,
    RecursionStackOffsets,
    ValueAlternativeConstruction,
    ReturnedConstantAlternatives,
    DispatchSlices,
    DispatchTableEntries,
    SweepTableEntries,
    IndexedAddressDepth,
    WordSteps,
    WordPaths,
    WordCopyExtent,
    WordReadChain,
    WordAddressDepth,
    WordExpressionAllowance,
    FieldDepth,
    NamedFieldDepth,
    FunctionPointerFieldDepth,
    TypedefChain,
    DwarfOriginChain,
    RecursionInstructions,
    RecursionPending,
    RecursionSeeds,
    RecursionSteps,
    PostdominatorInstructions,
    CheckedAddressIndices,
    InventoryEnumeratedIndices,
    DeclaredWordRuns,
    TrapEntrySteps,
    TrapEntryPaths,
    FunctionWork,
    ExecutionEvents,
}
impl UnaccountedLimit {
    pub const ALL: &[Self] = &[
        Self::WordSlots,
        Self::RecursionStackBytes,
        Self::RecursionStackOffsets,
        Self::ValueAlternativeConstruction,
        Self::ReturnedConstantAlternatives,
        Self::DispatchSlices,
        Self::DispatchTableEntries,
        Self::SweepTableEntries,
        Self::IndexedAddressDepth,
        Self::WordSteps,
        Self::WordPaths,
        Self::WordCopyExtent,
        Self::WordReadChain,
        Self::WordAddressDepth,
        Self::WordExpressionAllowance,
        Self::FieldDepth,
        Self::NamedFieldDepth,
        Self::FunctionPointerFieldDepth,
        Self::TypedefChain,
        Self::DwarfOriginChain,
        Self::RecursionInstructions,
        Self::RecursionPending,
        Self::RecursionSeeds,
        Self::RecursionSteps,
        Self::PostdominatorInstructions,
        Self::CheckedAddressIndices,
        Self::InventoryEnumeratedIndices,
        Self::DeclaredWordRuns,
        Self::TrapEntrySteps,
        Self::TrapEntryPaths,
        Self::FunctionWork,
        Self::ExecutionEvents,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisResources {
    pub profile: AnalysisProfile,
    pub limits: Box<AnalysisLimits>,
    pub usage: AnalysisUsage,
    pub limited: Box<AnalysisLimited>,
    /// Limits without a typed local outcome; absence of counts is not zero events.
    pub unaccounted: Vec<UnaccountedLimit>,
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
        .map(LocalLimit::of)
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
            unaccounted: UnaccountedLimit::ALL.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reported_local_formulas_preserve_the_existing_budgets() {
        let report = AnalysisResources::new(
            AnalysisProfile::BlobrayCli,
            AnalysisProfile::BlobrayCli.limits(),
            AnalysisUsage::default(),
            AnalysisLimited::default(),
        );
        for instructions in [0, 1, 195, 196, 8192] {
            for &reported in &report.limits.local {
                let work = 100_000.max(instructions * 512);
                let expected = match reported.scope {
                    LocalBudgetScope::ReadFlow | LocalBudgetScope::ArgumentSummary => work,
                    LocalBudgetScope::SignatureAudit => work * 16,
                    LocalBudgetScope::FunctionExpressions if instructions == 0 => 0,
                    LocalBudgetScope::FunctionExpressions => instructions * 8 + 32,
                };
                assert_eq!(reported.limit(instructions), expected);
                assert_eq!(reported, LocalLimit::of(reported.scope));
            }
        }
    }

    #[test]
    fn formula_data_controls_evaluation_and_overflow_stays_explicit() {
        let formula = LocalLimit {
            scope: LocalBudgetScope::ReadFlow,
            minimum: 3,
            per_instruction: 5,
            constant: 7,
            multiplier: 2,
            zero_for_empty: false,
        };
        assert_eq!(formula.checked_limit(2), Some(34));
        for scope in [
            LocalBudgetScope::ReadFlow,
            LocalBudgetScope::ArgumentSummary,
            LocalBudgetScope::SignatureAudit,
            LocalBudgetScope::FunctionExpressions,
        ] {
            let formula = LocalLimit::of(scope);
            assert_eq!(formula.checked_limit(u64::MAX), None);
            assert_eq!(formula.limit(u64::MAX), u64::MAX);
        }
    }

    #[test]
    fn unaccounted_limits_are_typed_stable_identifiers() {
        let encoded = serde_json::to_value(UnaccountedLimit::ALL).expect("unaccounted JSON");
        let decoded: Vec<UnaccountedLimit> =
            serde_json::from_value(encoded.clone()).expect("typed limits");
        assert_eq!(decoded, UnaccountedLimit::ALL);
        assert!(encoded.as_array().expect("limit list").iter().all(|name| {
            name.as_str().is_some_and(|name| {
                name.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
            })
        }));
        assert_eq!(
            serde_json::to_string(&UnaccountedLimit::WordSlots).expect("word slots"),
            "\"word-slots\""
        );
        assert!(serde_json::from_str::<UnaccountedLimit>("\"unknown-limit\"").is_err());
    }

    #[test]
    fn events_functions_and_output_records_are_independent() {
        let reason = LimitReason::LocalBudget(LocalBudgetScope::ReadFlow);
        let mut one = FunctionLimits::default();
        let outcome = SummaryOutcome::<()>::LocalBudgetExceeded(LocalBudgetExceeded {
            scope: LocalBudgetScope::ReadFlow,
            limit: 100_000,
        });
        one.outcome(&outcome);
        one.outcome(&outcome);
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
        for profile in [AnalysisProfile::RegisterInventory, AnalysisProfile::SignatureCorpus] {
            assert_eq!(profile.limits(), OperationLimits {
                work_units: 1_000_000_000,
                working_memory_bytes: 256 << 20,
                timeout_ms: 3_600_000,
            });
        }
        assert_eq!(AnalysisProfile::VendorScenario.limits(), OperationLimits {
            work_units: 2_000_000_000,
            working_memory_bytes: 256 << 20,
            timeout_ms: 600_000,
        });
        assert_eq!(AnalysisProfile::VendorCopies.limits(), OperationLimits {
            work_units: 1_000_000_000_000,
            working_memory_bytes: 8192 << 20,
            timeout_ms: 1_200_000,
        });
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

    #[test]
    fn resource_document_round_trips_and_rejects_unknown_fields() {
        let resources = AnalysisResources::new(
            AnalysisProfile::BlobrayCli,
            AnalysisProfile::BlobrayCli.limits(),
            AnalysisUsage::default(),
            AnalysisLimited::default(),
        );
        let value = serde_json::to_value(&resources).expect("resource document");
        assert_eq!(
            serde_json::from_value::<AnalysisResources>(value.clone()).expect("round trip"),
            resources
        );
        for path in [vec![], vec!["limits"], vec!["usage"], vec!["limited"]] {
            let mut invalid = value.clone();
            let mut object = &mut invalid;
            for key in path {
                object = &mut object[key];
            }
            object
                .as_object_mut()
                .expect("resource object")
                .insert("unexpected".into(), true.into());
            assert!(serde_json::from_value::<AnalysisResources>(invalid).is_err());
        }
    }

    #[test]
    fn filtering_changes_only_output_counts() {
        let reason = LimitReason::LocalBudget(LocalBudgetScope::ReadFlow);
        let mut function = FunctionLimits::default();
        function.event(reason);
        function.result(&[reason]);
        function.result(&[reason]);
        let mut all = AnalysisLimited::default();
        all.function(&function);
        let mut selected = AnalysisLimited::default();
        selected.result(reason);
        all.project_results(&selected);
        assert_eq!((all.functions, all.results), (1, 1));
        assert_eq!(
            (
                all.events_by_reason[0].events,
                all.events_by_reason[0].results
            ),
            (1, 1)
        );
    }
}
