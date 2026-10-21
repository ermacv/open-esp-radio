//! Corpus call graph and memoized summaries. Value analysis is unchanged.
use super::*;
use crate::library::LibraryOutcome;
use oer_analysis_blobray_analysis::registers::{
    ArgumentProgram, ArgumentSummaries, ArgumentSummary,
};

struct Entry<'m> {
    program: Option<ArgumentProgram<'m>>,
    summary: Option<ArgumentSummary<'m>>,
    links: AdmittedVec<'m, (u64, Option<usize>)>,
    discovery: usize,
    low: usize,
    active: bool,
}

pub(crate) struct ArgumentCorpus<'f, 'm> {
    pub(super) catalog: &'f Callees<'m>,
    memory: &'m WorkingMemory,
    entries: AdmittedVec<'m, Entry<'m>>,
    invalid_inputs: AdmittedVec<'m, u64>,
    invalid_objects: AdmittedVec<'m, ObjectId>,
    ledger: Ledger<'m>,
}

pub(super) struct Calls<'s, 'm> {
    entries: &'s [Entry<'m>],
    links: AdmittedVec<'m, (u64, Option<usize>)>,
}
impl ArgumentSummaries for Calls<'_, '_> {
    fn summary(&self, site: u64) -> Option<&ArgumentSummary<'_>> {
        let index = self.links.binary_search_by_key(&site, |link| link.0).ok()?;
        self.entries
            .get(self.links.get(index)?.1?)?
            .summary
            .as_ref()
    }
}

impl<'f, 'm> ArgumentCorpus<'f, 'm> {
    pub fn new(
        catalog: &'f Callees<'m>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut entries = AdmittedVec::new(memory);
        for _ in 0..catalog.len() {
            c.checkpoint(1)?;
            entries.push(
                Entry {
                    program: None,
                    summary: None,
                    links: AdmittedVec::new(memory),
                    discovery: usize::MAX,
                    low: 0,
                    active: false,
                },
                c.position(),
            )?;
        }
        Ok(Self {
            catalog,
            memory,
            entries,
            invalid_inputs: AdmittedVec::new(memory),
            invalid_objects: AdmittedVec::new(memory),
            ledger: Ledger::new(memory, c)?,
        })
    }

    fn entry(&self, id: usize) -> Result<&Entry<'m>> {
        self.entries.get(id).ok_or_else(integrity)
    }
    fn entry_mut(&mut self, id: usize) -> Result<&mut Entry<'m>> {
        self.entries.get_mut(id).ok_or_else(integrity)
    }

    pub fn capture(
        &mut self,
        outcome: LibraryOutcome<'_>,
        decoder: &dyn FunctionSemantics,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let analyzed = match outcome {
            LibraryOutcome::Analyzed(analyzed) => analyzed,
            LibraryOutcome::Gap { input, object, .. } => {
                if let Some(object) = object {
                    self.ledger
                        .admit(object.artifact.allocated_bytes() as usize, c)?;
                    self.invalid_objects.push(object.clone(), c.position())?;
                } else {
                    self.invalid_inputs.push(input, c.position())?;
                }
                return Ok(());
            }
            LibraryOutcome::Blocked { .. } => return Ok(()),
        };
        // Opaque calls have an explicit May boundary here. AlternativeLimit
        // affects value expressions, not the instruction CFG. Other gaps can
        // hide flow, so no summary may narrow a caller through them.
        if !analyzed.coverage.complete() {
            return Ok(());
        }
        for record in analyzed.records {
            c.checkpoint(1)?;
            if matches!(record, FunctionRecord::Gap { .. })
                || matches!(record, FunctionRecord::SemanticGap { reason, .. }
                if !matches!(reason, SemanticGapReason::OpaqueCall | SemanticGapReason::AlternativeLimit))
            {
                return Ok(());
            }
        }
        let Some(id) = self.catalog.function_id(analyzed.function, c)? else {
            return Ok(());
        };
        let result: Result<()> = (|| {
            let facts = Facts::new(analyzed.records, self.memory, c)?;
            let program = ArgumentProgram::capture(&facts, decoder, self.memory, c)?;
            let mut sites = callees::CallSites::new(&facts, self.memory);
            let mut links = AdmittedVec::new(self.memory);
            for site in program.calls() {
                let target = sites.at(site, analyzed.function, self.catalog, c)?;
                let target = match target.and_then(|target| target.function) {
                    Some(target) => self.catalog.function_id(&target, c)?,
                    None => None,
                };
                links.push((site, target), c.position())?;
            }
            self.entry_mut(id)?.links = links;
            self.entry_mut(id)?.program = Some(program);
            Ok(())
        })();
        match result {
            Err(error)
                if matches!(error.code, ErrorCode::ResourceLimited | ErrorCode::TimedOut) =>
            {
                Ok(())
            }
            result => result,
        }
    }

    /// Iterative Tarjan visits SCCs in callee-first order. Entire recursive
    /// components retain no summary, including members discovered before the
    /// back edge. Every other recipe is solved once and then released.
    pub fn solve(&mut self, c: &mut dyn RunControl) -> Result<()> {
        for id in 0..self.entries.len() {
            let (input, object) = self.catalog.owner(id)?;
            if self.invalid_inputs.contains(&input) || self.invalid_objects.contains(object) {
                self.entry_mut(id)?.program = None;
                self.entry_mut(id)?.links.truncate(0);
            }
        }
        let mut sequence = 0;
        let mut stack = AdmittedVec::new(self.memory);
        let mut frames = AdmittedVec::new(self.memory);
        let mut component = AdmittedVec::new(self.memory);
        for root in 0..self.entries.len() {
            if self.entry(root)?.discovery != usize::MAX {
                continue;
            }
            frames.push((root, 0usize), c.position())?;
            while let Some((id, cursor)) = frames.last().copied() {
                c.checkpoint(1)?;
                if self.entry_mut(id)?.discovery == usize::MAX {
                    self.entry_mut(id)?.discovery = sequence;
                    self.entry_mut(id)?.low = sequence;
                    self.entry_mut(id)?.active = true;
                    sequence += 1;
                    stack.push(id, c.position())?;
                }
                if let Some((_, target)) = self.entry(id)?.links.get(cursor).copied() {
                    frames.last_mut().ok_or_else(integrity)?.1 += 1;
                    if let Some(target) = target {
                        if self.entry_mut(target)?.discovery == usize::MAX {
                            frames.push((target, 0), c.position())?;
                        } else if self.entry(target)?.active {
                            self.entry_mut(id)?.low =
                                self.entry(id)?.low.min(self.entry(target)?.discovery);
                        }
                    }
                    continue;
                }
                frames.pop();
                if let Some((parent, _)) = frames.last().copied() {
                    self.entry_mut(parent)?.low = self.entry(parent)?.low.min(self.entry(id)?.low);
                }
                if self.entry(id)?.low != self.entry(id)?.discovery {
                    continue;
                }
                component.truncate(0);
                loop {
                    let member = stack.pop().ok_or_else(integrity)?;
                    self.entry_mut(member)?.active = false;
                    component.push(member, c.position())?;
                    if member == id {
                        break;
                    }
                }
                let recursive = component.len() > 1
                    || self.entry(id)?.links.iter().any(|link| link.1 == Some(id));
                for member in component.iter().copied() {
                    let program = self.entry_mut(member)?.program.take();
                    if recursive {
                        continue;
                    }
                    if let Some(program) = program {
                        let empty = AdmittedVec::new(self.memory);
                        let links = std::mem::replace(&mut self.entry_mut(member)?.links, empty);
                        let calls = Calls {
                            entries: &self.entries,
                            links,
                        };
                        let summary = program.summarize(&calls, self.memory, c)?;
                        self.entry_mut(member)?.summary = summary;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn calls(
        &self,
        sites: &mut callees::CallSites<'_, '_, 'm>,
        caller: &LibraryFunction,
        c: &mut dyn RunControl,
    ) -> Result<Calls<'_, 'm>> {
        let mut links = AdmittedVec::new(self.memory);
        // CallInputs includes tail transfers and is already sorted by site.
        for record in sites.records() {
            if let FunctionRecord::CallInputs { offset, .. } = record {
                c.checkpoint(1)?;
                let target = sites.at(*offset, caller, self.catalog, c)?;
                let target = match target.and_then(|target| target.function) {
                    Some(target)
                        if !self.invalid_inputs.contains(&target.input)
                            && !self.invalid_objects.contains(&target.symbol.object) =>
                    {
                        self.catalog.function_id(&target, c)?
                    }
                    _ => None,
                };
                links.push((*offset, target), c.position())?;
            }
        }
        links.sort_unstable_by_key(|link| link.0);
        Ok(Calls {
            entries: &self.entries,
            links,
        })
    }
}
fn integrity() -> Error {
    Error::new(ErrorCode::Integrity, "invalid argument-summary graph")
}
