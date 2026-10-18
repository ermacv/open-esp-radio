//! Trial links propose companions; they never select or keep an image.
use super::*;

/// Defined functions and data objects of one candidate executable, by name:
/// (name index, local binding, symbol).
struct Definitions<'a> {
    names: &'a [String],
    found: Vec<(usize, bool, SymbolId)>,
}
impl ElfSink for Definitions<'_> {
    fn symbol(&mut self, r: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(1)?;
        // STT_OBJECT or STT_FUNC in a static table.
        if !matches!(r.symbol_type, 1 | 2)
            || r.raw_section == 0
            || r.id.table != SymbolTableKind::Static
        {
            return Ok(());
        }
        let Some(name) = r.name.as_deref() else {
            return Ok(());
        };
        if let Some(index) = self.names.iter().position(|n| n.as_bytes() == name) {
            self.found.push((index, r.binding == 0, r.id.clone()));
        }
        Ok(())
    }
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}

/// Link `request` with unresolved names permitted, then resolve each name the
/// closure leaves undefined against the static executables `candidates` in
/// their explicit order. A name resolves to the first candidate that defines
/// it. Within that candidate a single global or weak definition is taken; a
/// local one only when no global or weak one exists. Several definitions of
/// the chosen binding, or none in any candidate, leave the name unresolved.
#[allow(clippy::too_many_arguments)]
pub fn propose_companions(
    request: &LinkRequest,
    candidates: &[ArtifactId],
    executables: &[Executable],
    linker: &Path,
    host: &dyn LinkerHost,
    directory: &Path,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<CompanionProposal> {
    if candidates.is_empty()
        || candidates.len() > 16
        || candidates.iter().any(|c| request.inputs.contains(c))
        || candidates
            .iter()
            .enumerate()
            .any(|(i, c)| candidates[..i].contains(c))
    {
        return Err(invalid(
            "companion candidates must be 1-16 distinct executables outside the link inputs",
        ));
    }
    let _fixed = memory.reserve(LINK_METADATA_BYTES, control.position())?;
    let workspace = LinkWorkspace::new(directory, memory)?;
    let identity = host.identify(linker, &workspace, control)?;
    let found = materialize(request, executables, &workspace, memory, control)?;
    let invocation = invocation(
        request,
        &found,
        linker,
        &identity,
        &workspace,
        UnresolvedSymbols::Report,
    );
    let mut outputs = Outputs::default();
    run(host, &invocation, &mut outputs, control)?;
    let names = blobray_artifacts::undefined_names(&outputs.elf.as_slice(), memory, control)?;
    // (candidate position, name index, local binding, symbol)
    let mut found = Vec::new();
    for (position, candidate) in candidates.iter().enumerate() {
        let executable = find(executables, candidate)?;
        let object = ObjectId {
            artifact: executable.id().clone(),
            location: ObjectLocation::Standalone,
        };
        let mut scan = Definitions {
            names: &names,
            found: Vec::new(),
        };
        let bytes: &[u8] = executable.bytes();
        let (_, header) = inspect_source(&bytes, &object, memory, control, &mut scan)?;
        if header.is_some_and(|e| {
            e.object_type == 2 && e.bits == 32 && e.machine == 243 && e.little_endian
        }) {
            found.extend(
                scan.found
                    .into_iter()
                    .map(|(index, local, symbol)| (position, index, local, symbol)),
            );
        }
    }
    let mut proposal = CompanionProposal {
        resolved: Vec::new(),
        unresolved: Vec::new(),
    };
    for (index, name) in names.iter().enumerate() {
        control.checkpoint(1)?;
        let first = found
            .iter()
            .filter(|(_, i, _, _)| *i == index)
            .map(|(position, _, _, _)| *position)
            .min();
        let in_first: Vec<_> = found
            .iter()
            .filter(|(position, i, _, _)| *i == index && Some(*position) == first)
            .collect();
        let local = in_first.iter().all(|(_, _, local, _)| *local);
        let selected: Vec<_> = in_first
            .into_iter()
            .filter(|(_, _, is_local, _)| *is_local == local)
            .collect();
        match selected.as_slice() {
            [(_, _, _, symbol)] => proposal.resolved.push(ProposedCompanion {
                name: name.clone(),
                symbol: symbol.clone(),
            }),
            _ => proposal.unresolved.push(name.clone()),
        }
    }
    Ok(proposal)
}
