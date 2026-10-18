//! Driver verification inside the calling process.
//!
//! The caller supplies the request, the executables its targets name by
//! content and the effect contracts and layout projections its relations
//! select. Contracts and projections are reviewed outside Blobray and selected
//! by the digest of their canonical encoding. Records stay in memory: no
//! project, content store or journal participates. A symbol goal resolves in
//! the executable whose content is its object.
//!
//! The vendor side of a request can execute once and be reused by requests
//! that differ only in their replacement side, such as the same request over
//! patched production images; only the replacement then executes. Reuse
//! yields the records of a full execution.
pub use crate::code_coverage::report_in_process as coverage;
pub use crate::dependence::ObservedInstructions;
use crate::execution::{Resolved, Sources, VendorSide, run_resolved};
use crate::*;
use std::sync::Arc;

/// A static ELF executable with the content identity requests name it by,
/// computed once when the executable is made.
#[derive(Clone, Debug)]
pub struct Executable {
    bytes: Arc<[u8]>,
    id: ArtifactId,
}
impl Executable {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        let bytes = bytes.into();
        Self {
            id: ArtifactId::of_bytes(&bytes),
            bytes,
        }
    }
    pub fn id(&self) -> &ArtifactId {
        &self.id
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Checkpoints between wall-clock deadline checks: rare enough to stay off
/// the interpreter's cost, frequent enough that waiting on an external tool,
/// one checkpoint per bounded poll, notices the deadline within seconds.
const DEADLINE_INTERVAL: u32 = 256;

/// Work-unit and wall-clock limits of one in-process operation, and the
/// position it reached, which a working-memory failure reports.
pub struct Limits {
    units: u64,
    limit: u64,
    deadline: std::time::Instant,
    checks: u32,
    position: RunPosition,
}
impl Limits {
    pub fn new(max_work_units: u64, timeout: std::time::Duration) -> Self {
        Self {
            units: 0,
            limit: max_work_units,
            deadline: std::time::Instant::now() + timeout,
            checks: 0,
            position: RunPosition::default(),
        }
    }
}
impl RunControl for Limits {
    fn position(&self) -> RunPosition {
        self.position
    }
    fn set_position(&mut self, position: RunPosition) {
        self.position = position;
    }
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.units = self.units.saturating_add(units);
        if self.units > self.limit {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "work budget exhausted",
            ));
        }
        self.checks += 1;
        if self.checks == DEADLINE_INTERVAL {
            self.checks = 0;
            if std::time::Instant::now() > self.deadline {
                return Err(Error::new(ErrorCode::ResourceLimited, "deadline exceeded"));
            }
        }
        Ok(())
    }
}

/// The executable among `executables` whose content is `id`.
pub(crate) fn find<'e>(executables: &'e [Executable], id: &ArtifactId) -> Result<&'e Executable> {
    executables.iter().find(|e| e.id == *id).ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidRequest,
            "the request names an executable that was not given",
        )
    })
}

fn digest(value: &impl serde::Serialize) -> Result<ArtifactId> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| Error::new(ErrorCode::InvalidRequest, e.to_string()))?;
    Ok(ArtifactId::of_bytes(&bytes))
}

/// The identity a relation selects an effect contract reviewed outside
/// Blobray by: the digest of its canonical encoding.
pub fn effect_contract_id(contract: &EffectContract) -> Result<ArtifactId> {
    digest(contract)
}

/// The identity a relation selects a layout projection reviewed outside
/// Blobray by: the digest of its canonical encoding.
pub fn projection_id(projection: &LayoutProjection) -> Result<ArtifactId> {
    digest(projection)
}

/// Vendor observations of one request's cases and the vendor coverage over
/// them, from `vendor`. They are reused only by requests with the same key.
pub struct VendorResults {
    key: ArtifactId,
    cases: Vec<ExecutionObservation>,
    coverage: Vec<ExecutionCoverage>,
}

/// The key of the vendor side of `input` under `executor`: everything the
/// vendor observations depend on when no replacement case blocks them.
fn vendor_key(input: &InProcessComparison<'_>, executor: &dyn Executor) -> Result<ArtifactId> {
    let request = input.request;
    digest(&serde_json::json!({
        "schema": EXECUTION_SCHEMA,
        "environment": crate::EXECUTION_ENVIRONMENT,
        "executor": executor.identity(),
        "target": &request.vendor,
        "max_events": request.max_events,
        "cases": request
            .cases
            .iter()
            .map(|case| (case.reset, case.stack_fill, &case.vendor))
            .collect::<Vec<_>>(),
    }))
}

/// Bytes of the loaded replacement image at `address` replaced in every
/// replacement session, such as a single-point mutant of compiled code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImagePatch {
    pub address: u32,
    pub original: Vec<u8>,
    pub replacement: Vec<u8>,
}

pub struct InProcessComparison<'a> {
    pub request: &'a ExecutionRequest,
    /// Every executable the request's targets name, in any order; others
    /// are ignored.
    pub executables: &'a [Executable],
    pub effects: &'a [EffectContract],
    pub projections: &'a [LayoutProjection],
    /// Vendor results of this request's vendor side, from `vendor`.
    pub vendor_results: Option<&'a VendorResults>,
    /// Semantics of the replacement ISA, to report which executed replacement
    /// instructions the compared observations depend on.
    pub dependence: Option<&'a dyn FunctionSemantics>,
    /// Patches of the loaded replacement image; no binary is rebuilt.
    pub patches: &'a [ImagePatch],
}

pub struct InProcessResult {
    pub records: Vec<ExecutionEvidence>,
    pub verdict: Option<ComparisonVerdict>,
    pub complete: bool,
    /// Whether the given vendor results replaced executing the vendor side.
    pub vendor_reused: bool,
    /// Executed and observed replacement instructions, when requested.
    pub observed: Option<ObservedInstructions>,
}

/// One run of a request.
struct Run {
    records: Vec<ExecutionEvidence>,
    verdict: Option<ComparisonVerdict>,
    complete: bool,
    vendor_independent: bool,
    observed: Option<ObservedInstructions>,
}

fn unsupported(what: &str) -> Error {
    Error::new(
        ErrorCode::InvalidRequest,
        format!("in-process verification does not support {what}"),
    )
}

/// The physical boundary of `goal`: a symbol goal names a code symbol of an
/// executable of `target`, which `executables` supplies.
pub(crate) fn resolve_goal(
    goal: &ExecutionGoal,
    target: &ExecutionTarget,
    executables: &[Executable],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<ResolvedExecutionGoal> {
    let (symbol, include_tail) = match goal {
        ExecutionGoal::Return => return Ok(ResolvedExecutionGoal::Return),
        ExecutionGoal::ReachSymbol { target } => (target, None),
        ExecutionGoal::ObserveCall {
            target,
            include_tail,
        } => (target, Some(*include_tail)),
    };
    if !target.maps(&symbol.object) || symbol.table != SymbolTableKind::Static {
        return Err(unsupported(
            "symbol goals outside the static table of a target executable",
        ));
    }
    let executable = find(executables, &symbol.object.artifact)?;
    let address = oer_riscv_program::code_symbol_at(
        &executable.bytes(),
        symbol.table_section,
        symbol.index,
        memory,
        control,
    )?;
    Ok(match include_tail {
        None => ResolvedExecutionGoal::ReachSymbol { address },
        Some(include_tail) => ResolvedExecutionGoal::ObserveCall {
            address,
            include_tail,
        },
    })
}

/// Validate `input`, then run its request with the vendor side from `vendor`.
fn run(
    input: &InProcessComparison<'_>,
    vendor: &mut VendorSide<'_>,
    executor: &dyn Executor,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Run> {
    let request = input.request;
    request.validate()?;
    let effects = input
        .effects
        .iter()
        .map(|contract| {
            contract.validate()?;
            Ok(ResolvedEffectContract {
                id: effect_contract_id(contract)?,
                contract: contract.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let projections = input
        .projections
        .iter()
        .map(|projection| {
            projection.validate()?;
            Ok(ResolvedProjection {
                id: projection_id(projection)?,
                projection: projection.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut goals = Vec::with_capacity(request.cases.len());
    for (phase, case) in request.cases.iter().enumerate() {
        control.checkpoint(1)?;
        let mut resolved = [None; 2];
        for (side, (invocation, target)) in std::iter::once((&case.vendor, &request.vendor))
            .chain(case.replacement.as_ref().zip(request.replacement.as_ref()))
            .enumerate()
        {
            resolved[side] = Some(resolve_goal(
                &invocation.goal,
                target,
                input.executables,
                memory,
                control,
            )?);
        }
        if let Some(relation) = &case.relation {
            if let Some(selected) = selected_effect_contract(Some(relation), &effects)? {
                selected.contract.validate_use(request, phase)?;
            }
            if let Some(selected) = selected_projection(Some(relation), &projections)? {
                selected.projection.validate_use(request, phase)?;
            }
        }
        goals.push(resolved);
    }
    let mut targets = Vec::new();
    if matches!(vendor, VendorSide::Execute(_)) {
        targets.push(&request.vendor);
    }
    targets.extend(&request.replacement);
    let sources = Sources::from_executables(&targets, input.executables, memory, control)?;
    // Dependence needs the replacement's executable segments and function starts.
    let replacement = match (input.dependence, &request.replacement) {
        (Some(semantics), Some(target)) => {
            let mut segments: Vec<_> = sources
                .target_segments(target)
                .filter(|s| s.flags & 1 != 0)
                .collect();
            segments.sort_by_key(|s| s.address);
            let mut starts = std::collections::BTreeSet::new();
            for id in &target.executables {
                let executable = find(input.executables, id)?;
                for (address, _) in
                    oer_riscv_program::code_symbols(&executable.bytes(), memory, control)?
                {
                    starts.insert(address);
                }
            }
            Some((crate::code_coverage::Code { segments }, starts, semantics))
        }
        _ => None,
    };
    let mut analyzer = replacement.as_ref().map(|(code, starts, semantics)| {
        crate::dependence::Analyzer::new(code, starts, *semantics)
    });
    let record_steps = analyzer.is_some();
    let mut sink = |log, c: &mut dyn RunControl| match &mut analyzer {
        Some(analyzer) => analyzer.session(log, memory, c),
        None => Ok(()),
    };
    let reused = matches!(vendor, VendorSide::Reuse { .. });
    let mut records = Vec::new();
    let outcome = run_resolved(
        &Resolved {
            request,
            goals: &goals,
            projections: &projections,
            effects: &effects,
            sources: &sources,
            patches: input.patches,
        },
        vendor,
        record_steps.then_some(&mut sink as &mut crate::execution::StepSink<'_, '_>),
        executor,
        memory,
        &mut |record, _| {
            records.push(record);
            Ok(())
        },
        control,
    )?;
    // An independent structural check of what execution and comparison
    // recorded; a MATCH never rests on the comparison alone. A reuse that
    // stopped where the vendor side came to depend on the replacement
    // recorded only a prefix, which `verify` discards.
    if !reused || outcome.vendor_independent {
        blobray_verification::validate_records(
            &blobray_verification::RecordedRun {
                request,
                effects: &effects,
                projections: &projections,
                records: &records,
                verdict: outcome.verdict,
                complete: outcome.complete,
            },
            control,
        )?;
    }
    let observed = analyzer.map(|a| a.result);
    Ok(Run {
        records,
        verdict: outcome.verdict,
        complete: outcome.complete,
        vendor_independent: outcome.vendor_independent,
        observed,
    })
}

/// Execute the vendor side of every case of `input.request` once, for reuse
/// by `verify` of requests with the same vendor side.
pub fn vendor(
    input: &InProcessComparison<'_>,
    executor: &dyn Executor,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<VendorResults> {
    let mut request = input.request.clone();
    request.replacement = None;
    request.binding = None;
    for case in &mut request.cases {
        case.replacement = None;
        case.relation = None;
    }
    let mut cases = Vec::new();
    let records = run(
        &InProcessComparison {
            request: &request,
            vendor_results: None,
            dependence: None,
            ..*input
        },
        &mut VendorSide::Execute(Some(&mut cases)),
        executor,
        memory,
        control,
    )?
    .records;
    Ok(VendorResults {
        key: vendor_key(input, executor)?,
        cases,
        coverage: records
            .into_iter()
            .filter_map(|record| match record {
                ExecutionEvidence::Coverage {
                    replacement: false,
                    coverage,
                } => Some(coverage),
                _ => None,
            })
            .collect(),
    })
}

/// Execute and compare every case of `input.request`, reusing
/// `input.vendor_results` unless a replacement case blocks the vendor side.
pub fn verify(
    input: &InProcessComparison<'_>,
    executor: &dyn Executor,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<InProcessResult> {
    if let Some(reused) = input.vendor_results {
        if reused.key != vendor_key(input, executor)?
            || reused.cases.len() != input.request.cases.len()
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "vendor results belong to another vendor side",
            ));
        }
        let run = run(
            input,
            &mut VendorSide::Reuse {
                cases: &reused.cases,
                coverage: &reused.coverage,
            },
            executor,
            memory,
            control,
        )?;
        // A replacement case that blocked the vendor side makes the vendor
        // observations depend on it; such a request executes fully.
        if run.vendor_independent {
            return Ok(InProcessResult {
                records: run.records,
                verdict: run.verdict,
                complete: run.complete,
                vendor_reused: true,
                observed: run.observed,
            });
        }
    }
    let run = run(
        input,
        &mut VendorSide::Execute(None),
        executor,
        memory,
        control,
    )?;
    Ok(InProcessResult {
        records: run.records,
        verdict: run.verdict,
        complete: run.complete,
        vendor_reused: false,
        observed: run.observed,
    })
}
