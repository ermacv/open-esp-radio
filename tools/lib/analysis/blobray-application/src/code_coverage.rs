//! Vendor coverage of the root closures that in-process executions exercise.
use crate::execution::{LoadedSegment, Sources};
use crate::*;
use blobray_analysis::closure::{ClosureInput, CodeMemory, code_closure};
use std::collections::{BTreeMap, BTreeSet};

/// Executable segments of one target, ascending by address.
pub(crate) struct Code<'s, 'a> {
    pub segments: Vec<&'s LoadedSegment<'a>>,
}
impl CodeMemory for Code<'_, '_> {
    fn code(&self, address: u32, bytes: &mut [u8; 4]) -> Option<usize> {
        let index = self
            .segments
            .partition_point(|s| s.address <= address)
            .checked_sub(1)?;
        let segment = self.segments[index];
        let offset = (address - segment.address) as usize;
        if offset >= segment.memory_size {
            return None;
        }
        // The zero-filled tail beyond the file bytes is executable zero bytes.
        let available = (segment.memory_size - offset).min(4);
        for (i, byte) in bytes.iter_mut().take(available).enumerate() {
            *byte = segment.bytes.get(offset + i).copied().unwrap_or(0);
        }
        Some(available)
    }
}

/// The vendor code executions reached, their roots and closure boundaries.
#[derive(Default)]
struct Reached {
    instructions: BTreeSet<u32>,
    /// Observed (taken, fallthrough) per branch site.
    branches: BTreeMap<u32, (bool, bool)>,
    /// Observed targets of executed indirect transfers, by site.
    transfers: BTreeMap<u32, BTreeSet<u32>>,
    roots: BTreeSet<u32>,
    boundaries: BTreeSet<u32>,
}

impl Reached {
    /// Add the vendor coverage of one execution of `request` and its closure
    /// roots and boundaries; `goals` are the request's resolved goals.
    fn add(
        &mut self,
        request: &ExecutionRequest,
        coverage: ExecutionCoverage,
        goals: &[[Option<ResolvedExecutionGoal>; 2]],
        c: &mut dyn RunControl,
    ) -> Result<()> {
        for pc in coverage.instructions {
            c.checkpoint(1)?;
            self.instructions.insert(pc);
        }
        for transfer in coverage.transfers {
            c.checkpoint(1)?;
            self.transfers
                .entry(transfer.site)
                .or_default()
                .insert(transfer.target);
        }
        for branch in coverage.branches {
            c.checkpoint(1)?;
            let seen = self.branches.entry(branch.site).or_default();
            seen.0 |= branch.taken;
            seen.1 |= branch.fallthrough;
        }
        for (case, goal) in request.cases.iter().zip(goals.iter()) {
            c.checkpoint(1)?;
            let vendor = &case.vendor;
            self.roots.insert(vendor.entry);
            self.boundaries
                .extend(vendor.calls.iter().map(|call| call.binding.address));
            match goal[0] {
                Some(ResolvedExecutionGoal::ReachSymbol { address })
                | Some(ResolvedExecutionGoal::ObserveCall { address, .. }) => {
                    self.boundaries.insert(address);
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The merged vendor coverage of one execution's records.
fn vendor_coverage<'r>(
    records: impl IntoIterator<Item = &'r ExecutionEvidence>,
) -> Result<ExecutionCoverage> {
    let mut coverage = ExecutionCoverage::default();
    let mut found = false;
    for record in records {
        if let ExecutionEvidence::Coverage {
            replacement: false,
            coverage: vendor,
        } = record
        {
            found = true;
            coverage
                .instructions
                .extend_from_slice(&vendor.instructions);
            coverage.branches.extend_from_slice(&vendor.branches);
            coverage.transfers.extend_from_slice(&vendor.transfers);
        }
    }
    if !found {
        return Err(Error::new(ErrorCode::Integrity, "vendor coverage missing"));
    }
    Ok(coverage)
}

/// One vendor target shared by every selected execution.
fn same_target(target: &mut Option<ExecutionTarget>, request: &ExecutionRequest) -> Result<()> {
    match target {
        None => *target = Some(request.vendor.clone()),
        Some(first) if *first == request.vendor => {}
        Some(_) => {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "code coverage executions differ in their vendor target",
            ));
        }
    }
    Ok(())
}

fn function_coverage(
    function: &ClosureFunction,
    name: Option<String>,
    reached: &Reached,
    c: &mut dyn RunControl,
) -> Result<FunctionCodeCoverage> {
    c.checkpoint((function.blocks.len() + function.branches.len()) as u64 + 1)?;
    let uncovered_blocks: Vec<u32> = function
        .blocks
        .iter()
        .copied()
        .filter(|block| !reached.instructions.contains(block))
        .collect();
    let mut uncovered_directions = Vec::new();
    for site in &function.branches {
        let (taken, fallthrough) = reached.branches.get(site).copied().unwrap_or_default();
        for (seen, direction) in [(taken, true), (fallthrough, false)] {
            if !seen {
                uncovered_directions.push(BranchDirection {
                    site: *site,
                    taken: direction,
                });
            }
        }
    }
    let total = |n: usize, uncovered: usize| CoverageCount {
        reached: (n - uncovered) as u64,
        total: n as u64,
    };
    Ok(FunctionCodeCoverage {
        entry: function.entry,
        name,
        blocks: total(function.blocks.len(), uncovered_blocks.len()),
        directions: total(2 * function.branches.len(), uncovered_directions.len()),
        uncovered_blocks,
        uncovered_directions,
        callees: function.callees.clone(),
        modeled: function.modeled.clone(),
        unresolved: function.unresolved.clone(),
        followed: function.followed.clone(),
        gaps: function.gaps.clone(),
    })
}

/// The vendor coverage of in-memory executions (caller identity, request and
/// records), which share one vendor target whose executables `executables`
/// supplies. Symbol goals resolve in those executables.
pub fn report_in_process(
    executions: &[(ArtifactId, &ExecutionRequest, &[ExecutionEvidence])],
    executables: &[crate::in_process::Executable],
    semantics: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<CodeCoverageReport> {
    let mut target = None;
    let mut reached = Reached::default();
    let mut ids = Vec::new();
    for (identity, request, records) in executions {
        same_target(&mut target, request)?;
        let goals: Vec<[Option<ResolvedExecutionGoal>; 2]> = request
            .cases
            .iter()
            .map(|case| {
                Ok([
                    Some(crate::in_process::resolve_goal(
                        &case.vendor.goal,
                        &request.vendor,
                        executables,
                        memory,
                        c,
                    )?),
                    None,
                ])
            })
            .collect::<Result<_>>()?;
        reached.add(request, vendor_coverage(records.iter())?, &goals, c)?;
        ids.push(identity.clone());
    }
    let target =
        target.ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "no executions selected"))?;
    let sources = Sources::from_executables(&[&target], executables, memory, c)?;
    let mut names: BTreeMap<u32, String> = BTreeMap::new();
    for id in &target.executables {
        let executable = crate::in_process::find(executables, id)?;
        for (address, name) in oer_riscv_program::code_symbols(&executable.bytes(), memory, c)? {
            // The first name in order identifies a start with several names.
            names.entry(address).or_insert(name);
        }
    }
    build(ids, &target, reached, &sources, names, semantics, memory, c)
}

#[allow(clippy::too_many_arguments)]
fn build(
    ids: Vec<ArtifactId>,
    target: &ExecutionTarget,
    reached: Reached,
    sources: &Sources<'_>,
    names: BTreeMap<u32, String>,
    semantics: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<CodeCoverageReport> {
    let mut segments: Vec<_> = sources
        .target_segments(target)
        .filter(|s| s.flags & 1 != 0)
        .collect();
    segments.sort_by_key(|s| s.address);
    let starts: BTreeSet<u32> = names.keys().copied().collect();
    let roots: Vec<u32> = reached.roots.iter().copied().collect();
    let closure = code_closure(
        &ClosureInput {
            roots: &roots,
            boundaries: &reached.boundaries,
            function_starts: &starts,
            observed: &reached.transfers,
            code: &Code { segments },
            semantics,
        },
        memory,
        c,
    )?;
    let by_entry: BTreeMap<u32, &ClosureFunction> = closure.iter().map(|f| (f.entry, f)).collect();
    let mut functions = Vec::new();
    for function in &closure {
        functions.push(function_coverage(
            function,
            names.get(&function.entry).cloned(),
            &reached,
            c,
        )?);
    }
    let mut root_reports = Vec::new();
    for root in &roots {
        let mut members = BTreeSet::new();
        let mut pending = vec![*root];
        while let Some(entry) = pending.pop() {
            c.checkpoint(1)?;
            if members.insert(entry) {
                pending.extend(by_entry[&entry].callees.iter().copied());
            }
        }
        let blocks: BTreeSet<u32> = members
            .iter()
            .flat_map(|e| by_entry[e].blocks.iter().copied())
            .collect();
        let sites: BTreeSet<u32> = members
            .iter()
            .flat_map(|e| by_entry[e].branches.iter().copied())
            .collect();
        let directions = sites
            .iter()
            .map(|site| {
                let (taken, fallthrough) = reached.branches.get(site).copied().unwrap_or_default();
                u64::from(taken) + u64::from(fallthrough)
            })
            .sum();
        root_reports.push(RootCoverage {
            entry: *root,
            name: names.get(root).cloned(),
            functions: members.into_iter().collect(),
            blocks: CoverageCount {
                reached: blocks
                    .iter()
                    .filter(|b| reached.instructions.contains(b))
                    .count() as u64,
                total: blocks.len() as u64,
            },
            directions: CoverageCount {
                reached: directions,
                total: 2 * sites.len() as u64,
            },
        });
    }
    // Disjoint union of every function's decoded code.
    let mut ranges: Vec<[u32; 2]> = closure
        .iter()
        .flat_map(|f| f.code.iter().copied())
        .collect();
    ranges.sort_unstable();
    let mut code: Vec<[u32; 2]> = Vec::new();
    for range in ranges {
        match code.last_mut() {
            Some(last) if last[1] >= range[0] => last[1] = last[1].max(range[1]),
            _ => code.push(range),
        }
    }
    let outside = reached
        .instructions
        .iter()
        .filter(|pc| {
            let index = code.partition_point(|range| range[0] <= **pc);
            index == 0 || **pc >= code[index - 1][1]
        })
        .count() as u64;
    Ok(CodeCoverageReport {
        executions: ids,
        roots: root_reports,
        functions,
        outside,
    })
}
