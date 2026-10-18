//! Bounded adapter conformance scenario; the output uses the production validators.
use super::*;
#[path = "probe_bytes.rs"]
mod bytes;

pub(super) fn check(
    workspace: &LinkWorkspace,
    host: &dyn LinkerHost,
    executable: &Path,
    identity: &LinkerIdentity,
    control: &mut dyn RunControl,
) -> Result<()> {
    let mut previous = None;
    for attempt in 0..2 {
        let probe = workspace.nested(
            &Path::new(&format!("capability-{attempt}")).join("probe"),
            65536,
        )?;
        let fingerprint = check_once(&probe, host, executable, identity, control)?;
        if previous.as_ref().is_some_and(|p| p != &fingerprint) {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "linker capability probe is not deterministic",
            ));
        }
        previous = Some(fingerprint);
    }
    Ok(())
}
fn check_once(
    probe: &LinkWorkspace,
    host: &dyn LinkerHost,
    executable: &Path,
    identity: &LinkerIdentity,
    control: &mut dyn RunControl,
) -> Result<Vec<ArtifactId>> {
    let memory = WorkingMemory::new(1024 * 1024)?; // Covered by the caller's fixed 8 MiB metadata reservation.
    let members: Vec<_> = [("entry.o", bytes::ENTRY), ("helper.o", bytes::HELPER)]
        .into_iter()
        .enumerate()
        .map(|(i, (alias, bytes))| {
            probe.materialize(alias, bytes)?;
            let payload = ArtifactId::of_bytes(bytes);
            Ok(Member {
                input: i,
                object: ObjectId {
                    artifact: payload.clone(),
                    location: ObjectLocation::Standalone,
                },
                payload: Some(payload),
                alias: alias.into(),
            })
        })
        .collect::<Result<_>>()?;
    let selection = SymbolId {
        object: members[0].object.clone(),
        table: SymbolTableKind::Static,
        table_section: 6,
        index: 4,
    };
    let roots = blobray_artifacts::inspect_link_input(
        &bytes::ENTRY,
        members[0].payload.as_ref().unwrap(),
        &[selection],
        &memory,
        control,
    )?;
    let found = Collected {
        members,
        definitions: Vec::new(),
        roots,
        blockers: Vec::new(),
    };
    let layout = ImageLayout {
        code: ImageRegion {
            start: 0x10000000,
            length: 65536,
        },
        data: ImageRegion {
            start: 0x20000000,
            length: 65536,
        },
    };
    let request = LinkInvocation {
        unresolved: UnresolvedSymbols::Error,
        executable,
        identity,
        contract: LinkerContract::ElfAnalysisLinkV1,
        workspace: probe,
        entry: b"entry",
        roots: Vec::new(),
        forced: vec!["entry.o".into()],
        inputs: vec![LinkInput::Archive(vec!["helper.o".into()])],
        members: found
            .members
            .iter()
            .map(|m| LinkMember {
                alias: m.alias.clone(),
                object: m.object.clone(),
            })
            .collect(),
        layout,
        definitions: Vec::new(),
    };
    let mut output = Outputs::default();
    let checked = (|| {
        host.link(
            &request,
            &mut ProbeSink {
                output: &mut output,
                bytes: [0; 4],
                records: 0,
            },
            control,
        )?;
        let (roots, _) = evidence::roots(&output, &found, &request, control)?;
        if output.elf.len() > 65536 {
            return Err(invalid("probe output exceeds 64 KiB"));
        }
        let bytes = &output.elf;
        blobray_artifacts::validate_image(&bytes.as_slice(), &layout, &roots, &memory, control)?;
        let mut facts = Facts::default();
        blobray_artifacts::inspect_payload(
            bytes,
            &found.members[0].object,
            &memory,
            control,
            &mut facts,
        )?;
        if facts.relocations < 2 || facts.unused || !facts.text {
            return Err(invalid(
                "linker probe did not retain relocations, preserve code extent and collect unused sections",
            ));
        }
        let observations = serde_json::to_vec(&(&output.extractions, &output.placements))
            .map_err(|e| invalid(e.to_string()))?;
        let mut fingerprint = Vec::new();
        for bytes in [&output.elf, &output.map, &output.extraction, &observations] {
            if bytes.len() > 65536 {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "probe artifact exceeds 64 KiB",
                ));
            }
            fingerprint.push(ArtifactId::of_bytes_controlled(bytes, control)?);
        }
        fingerprint.push(ArtifactId::of_bytes(&output.stderr));
        Ok(fingerprint)
    })();
    checked.map_err(|mut e: Error| {
        if matches!(
            e.code,
            ErrorCode::LinkFailed
                | ErrorCode::LinkBlocked
                | ErrorCode::InvalidRequest
                | ErrorCode::Integrity
        ) {
            e.code = ErrorCode::Incompatible;
            e.message = format!("linker lacks ElfAnalysisLinkV1 capability: {}", e.message);
        }
        e
    })
}
#[derive(Default)]
struct Facts {
    relocations: usize,
    unused: bool,
    text: bool,
}
impl ElfSink for Facts {
    fn section(&mut self, r: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        if r.name.as_deref() == Some(b".text") {
            self.text = r.size == 20;
        }
        Ok(())
    }
    fn symbol(&mut self, r: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        self.unused |= r.name.as_deref() == Some(b"unused");
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        self.relocations += 1;
        Ok(())
    }
    fn diagnostic(&mut self, r: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Err(invalid(r.message.clone()))
    }
}

struct ProbeSink<'a> {
    output: &'a mut Outputs,
    bytes: [usize; 4],
    records: usize,
}
impl LinkOutputSink for ProbeSink<'_> {
    fn exited(&mut self, code: Option<i32>, signal: Option<i32>) {
        self.output.exited(code, signal);
    }
    fn write(
        &mut self,
        channel: LinkOutput,
        bytes: &[u8],
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let slot = match channel {
            LinkOutput::Elf => 0,
            LinkOutput::Map => 1,
            LinkOutput::Extraction => 2,
            LinkOutput::Stderr => 3,
        };
        if self.bytes[slot] + bytes.len() > 65536 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "capability probe output exceeds 64 KiB",
            ));
        }
        self.bytes[slot] += bytes.len();
        self.output.write(channel, bytes, control)
    }
    fn observe(&mut self, r: LinkObservation, control: &mut dyn RunControl) -> Result<()> {
        if self.records == 128 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "capability probe observation capacity exhausted",
            ));
        }
        self.records += 1;
        self.output.observe(r, control)
    }
    fn elf(&mut self, bytes: Vec<u8>) -> Result<()> {
        if bytes.len() > 65536 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "capability probe ELF exceeds 64 KiB",
            ));
        }
        self.output.elf(bytes)
    }
}
