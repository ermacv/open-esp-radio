//! Library queries share outcome accounting before projecting function facts.
use super::*;
use crate::{call_arguments, field, references};

type Emit<'e, T> = dyn FnMut(&T, &mut dyn RunControl) -> Result<()> + 'e;

/// Coverage of the whole query, independent of which functions matched.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QuerySummary {
    pub limited: AnalysisLimited,
    pub functions: u64,
    pub partial: u64,
    pub partial_causes: PartialCauses,
    pub blocked: u64,
    pub gaps: u64,
}

/// One selected function, or a function whose unknown behavior stays visible.
#[derive(Clone, Debug)]
pub enum FunctionResult<T> {
    Matched(T),
    Blocked {
        function: LibraryFunction,
        error: Error,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldSummary {
    pub analysis: QuerySummary,
    pub unknown_addresses: u64,
}

/// References from a function or candidate pointer storage. Neither storage
/// variant proves a call. All results are borrowed only during emission.
#[derive(Clone, Debug)]
pub enum CallerResult {
    Function(FunctionResult<references::Caller>),
    Stored(references::Stored),
}

#[derive(Clone, Debug, Default)]
pub struct CallersSummary {
    pub analysis: QuerySummary,
    pub scan: references::ScanSummary,
}

/// Count every outcome once, including functions with no selected result.
fn query(
    inputs: &[Executable],
    assumptions: Assumptions<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    visit: &mut Visit<'_>,
) -> Result<QuerySummary> {
    let mut summary = QuerySummary::default();
    summary.limited = analyze_library(
        inputs,
        assumptions,
        decoder,
        memory,
        control,
        &mut |outcome, c| {
            match &outcome {
                LibraryOutcome::Analyzed(analyzed) => {
                    summary.functions += 1;
                    summary.partial += u64::from(!analyzed.complete());
                    summary.partial_causes.add(analyzed.partial_causes(c)?);
                }
                LibraryOutcome::Blocked { .. } => summary.blocked += 1,
                LibraryOutcome::Gap { .. } => summary.gaps += 1,
            }
            visit(outcome, c)
        },
    )?;
    Ok(summary)
}

/// Accesses whose root/path ends at `offset`, optionally selecting the width.
#[allow(clippy::too_many_arguments)]
pub fn field_accesses(
    inputs: &[Executable],
    assumptions: Assumptions<'_>,
    offset: i64,
    width: Option<u8>,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    emit: &mut Emit<'_, FunctionResult<field::FieldAccessFunction>>,
) -> Result<FieldSummary> {
    let mut unknown_addresses = 0;
    let analysis = query(
        inputs,
        assumptions,
        decoder,
        memory,
        control,
        &mut |outcome, c| {
            match outcome {
                LibraryOutcome::Analyzed(analyzed) => {
                    unknown_addresses += field::unknown_addresses(analyzed.records);
                    let facts =
                        FunctionIndex::new(analyzed.records, RecordPolicy::Dictionary, memory, c)?;
                    let accesses = field::field_accesses(&facts, offset, width, c)?;
                    if !accesses.is_empty() {
                        emit(
                            &FunctionResult::Matched(field::FieldAccessFunction {
                                function: analyzed.function.clone(),
                                accesses,
                            }),
                            c,
                        )?;
                    }
                }
                LibraryOutcome::Blocked { function, error } => emit(
                    &FunctionResult::Blocked {
                        function: function.clone(),
                        error: error.clone(),
                    },
                    c,
                )?,
                LibraryOutcome::Gap { .. } => {}
            }
            Ok(())
        },
    )?;
    Ok(FieldSummary {
        analysis,
        unknown_addresses,
    })
}

/// Integer argument values at exact and may call sites of `symbols`.
#[allow(clippy::too_many_arguments)]
pub fn call_arguments(
    inputs: &[Executable],
    assumptions: Assumptions<'_>,
    symbols: &[String],
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    emit: &mut Emit<'_, FunctionResult<call_arguments::CallingFunction>>,
) -> Result<QuerySummary> {
    let targets = references::Targets::capture(inputs, symbols, memory, control)?;
    query(
        inputs,
        assumptions,
        decoder,
        memory,
        control,
        &mut |outcome, c| {
            match outcome {
                LibraryOutcome::Analyzed(analyzed) => {
                    let facts =
                        FunctionIndex::new(analyzed.records, RecordPolicy::Dictionary, memory, c)?;
                    let sites = call_arguments::call_sites(&facts, symbols, &targets, c)?;
                    if !sites.is_empty() {
                        emit(
                            &FunctionResult::Matched(call_arguments::CallingFunction {
                                function: analyzed.function.clone(),
                                sites,
                            }),
                            c,
                        )?;
                    }
                }
                LibraryOutcome::Blocked { function, error } => emit(
                    &FunctionResult::Blocked {
                        function: function.clone(),
                        error: error.clone(),
                    },
                    c,
                )?,
                LibraryOutcome::Gap { .. } => {}
            }
            Ok(())
        },
    )
}

/// Relocations, exact/may transfers and candidate pointer storage for `symbols`.
#[allow(clippy::too_many_arguments)]
pub fn callers(
    inputs: &[Executable],
    assumptions: Assumptions<'_>,
    symbols: &[String],
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    emit: &mut Emit<'_, CallerResult>,
) -> Result<CallersSummary> {
    let targets = references::Targets::capture(inputs, symbols, memory, control)?;
    let mut stored = references::StoredScan::new(memory, control)?;
    stored.scan_data(inputs, &targets, memory, control)?;
    let analysis = query(
        inputs,
        assumptions,
        decoder,
        memory,
        control,
        &mut |outcome, c| {
            match outcome {
                LibraryOutcome::Analyzed(analyzed) => {
                    let references =
                        references::call_sites(analyzed.records, symbols, &targets, c)?;
                    stored.materialized(analyzed.function, analyzed.records, &targets, c)?;
                    if !references.is_empty() {
                        emit(
                            &CallerResult::Function(FunctionResult::Matched(references::Caller {
                                function: analyzed.function.clone(),
                                references,
                            })),
                            c,
                        )?;
                    }
                }
                LibraryOutcome::Blocked { function, error } => emit(
                    &CallerResult::Function(FunctionResult::Blocked {
                        function: function.clone(),
                        error: error.clone(),
                    }),
                    c,
                )?,
                LibraryOutcome::Gap { .. } => {}
            }
            Ok(())
        },
    )?;
    for reference in stored.references {
        emit(&CallerResult::Stored(reference), control)?;
    }
    Ok(CallersSummary {
        analysis,
        scan: stored.summary,
    })
}

#[cfg(test)]
mod tests;
