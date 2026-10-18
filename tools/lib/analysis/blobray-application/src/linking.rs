//! Captured-object linking in process. The host supplies the linker; the
//! application selects every input, checks every linker claim and keeps the
//! linked image in memory.
use crate::captured::visit_members;
use crate::in_process::{Executable, find};
use crate::*;
use std::path::PathBuf;

pub const LINK_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MEMBERS: usize = 4096;
/// Blockers one link reports before it stops.
const MAX_BLOCKERS: usize = 32;
/// Schema of `ImageManifest`.
const IMAGE_SCHEMA: u32 = 4;
mod companions;
mod evidence;
mod probe;
mod proposal;
pub use proposal::propose_companions;

#[derive(Clone, Debug)]
pub enum LinkInput {
    Object(String),
    Archive(Vec<String>),
}

/// Alias chosen by application, never an original untrusted filename.
#[derive(Clone, Debug)]
pub struct LinkMember {
    pub alias: String,
    pub object: ObjectId,
}

/// Treatment of names the selected closure leaves undefined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnresolvedSymbols {
    /// Image preparation: any unresolved name fails the link.
    Error,
    /// Companion proposal: unresolved names stay undefined in a trial image.
    Report,
}

/// Fixed semantic policy is identified by contract; adapters own flags and script dialect.
pub struct LinkInvocation<'a> {
    pub executable: &'a Path,
    pub identity: &'a LinkerIdentity,
    pub contract: LinkerContract,
    pub workspace: &'a LinkWorkspace,
    pub entry: &'a [u8],
    pub roots: Vec<Vec<u8>>,
    pub forced: Vec<String>,
    pub inputs: Vec<LinkInput>,
    pub members: Vec<LinkMember>,
    pub layout: ImageLayout,
    pub definitions: Vec<(String, u32)>,
    pub unresolved: UnresolvedSymbols,
}

/// The private directory of one link and the ELF extent its linker may
/// write. A workspace made by `new` removes its directory when dropped; a
/// nested one lives inside it.
pub struct LinkWorkspace {
    directory: PathBuf,
    _owned: Option<tempfile::TempDir>,
    elf_limit: u64,
}
impl LinkWorkspace {
    /// A new workspace below `parent`; the ELF may use the working capacity
    /// `memory` has left beyond the link's metadata.
    pub fn new(parent: &Path, memory: &WorkingMemory) -> Result<Self> {
        let capacity = memory.observation();
        Self::within(
            parent,
            capacity
                .limit_bytes
                .saturating_sub(capacity.reserved_bytes + LINK_METADATA_BYTES),
        )
    }
    fn within(parent: &Path, elf_limit: u64) -> Result<Self> {
        let owned = tempfile::Builder::new()
            .prefix(".link-")
            .tempdir_in(parent)
            .map_err(storage_io)?;
        Ok(Self {
            directory: owned.path().to_path_buf(),
            _owned: Some(owned),
            elf_limit,
        })
    }
    /// A new empty workspace at `path` below this one, with at most
    /// `elf_limit` bytes of ELF.
    fn nested(&self, path: &Path, elf_limit: u64) -> Result<Self> {
        let directory = self.directory().join(path);
        std::fs::create_dir_all(&directory).map_err(storage_io)?;
        Ok(Self {
            directory,
            _owned: None,
            elf_limit: elf_limit.min(self.elf_limit),
        })
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    /// The file a linker writes its ELF into, and the most bytes it may hold.
    pub fn elf_output(&self) -> Result<ElfOutput> {
        if self.elf_limit == 0 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "no working capacity remains for ELF validation",
            ));
        }
        Ok(ElfOutput {
            path: self.directory().join("image.elf"),
            maximum: self.elf_limit,
        })
    }
    pub fn materialize(&self, name: &str, bytes: &[u8]) -> Result<()> {
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(invalid("link workspace name must be a single component"));
        }
        std::fs::write(self.directory().join(name), bytes).map_err(storage_io)
    }
    /// Exercises the actual adapter, then independently validates RV32 output/evidence.
    pub fn probe(
        &self,
        host: &dyn LinkerHost,
        executable: &Path,
        identity: &LinkerIdentity,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        probe::check(self, host, executable, identity, control)
    }
}

/// The file a linker writes its ELF into.
pub struct ElfOutput {
    path: PathBuf,
    maximum: u64,
}
impl ElfOutput {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn maximum(&self) -> u64 {
        self.maximum
    }
    /// The ELF the linker wrote, within the maximum.
    pub fn finish(self) -> Result<Vec<u8>> {
        let length = std::fs::metadata(&self.path).map_err(storage_io)?.len();
        if length > self.maximum {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "linker ELF exceeds its admitted extent",
            ));
        }
        std::fs::read(&self.path).map_err(storage_io)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkOutput {
    Elf,
    Map,
    Extraction,
    Stderr,
}
pub trait LinkOutputSink {
    fn exited(&mut self, _code: Option<i32>, _signal: Option<i32>) {}
    fn write(
        &mut self,
        channel: LinkOutput,
        bytes: &[u8],
        control: &mut dyn RunControl,
    ) -> Result<()>;
    fn observe(&mut self, observation: LinkObservation, control: &mut dyn RunControl)
    -> Result<()>;
    /// Takes the ELF a linker wrote to its output file.
    fn elf(&mut self, bytes: Vec<u8>) -> Result<()>;
}
pub trait LinkerHost {
    fn identify(
        &self,
        executable: &Path,
        workspace: &LinkWorkspace,
        control: &mut dyn RunControl,
    ) -> Result<LinkerIdentity>;
    fn link(
        &self,
        request: &LinkInvocation<'_>,
        sink: &mut dyn LinkOutputSink,
        control: &mut dyn RunControl,
    ) -> Result<()>;
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
fn validate_request(request: &LinkRequest) -> Result<()> {
    request.layout.validate()?;
    if request.inputs.is_empty()
        || request.inputs.len() > 512
        || request.roots.len() >= MAX_IMAGE_ROOTS
    {
        return Err(invalid(format!(
            "link request needs 1..512 inputs and at most {} additional roots",
            MAX_IMAGE_ROOTS - 1
        )));
    }
    for (index, input) in request.inputs.iter().enumerate() {
        if request.inputs[..index].contains(input) {
            return Err(invalid("a link input is selected twice"));
        }
    }
    for root in std::iter::once(&request.entry).chain(&request.roots) {
        if !request.inputs.contains(&root.object.artifact) {
            return Err(invalid("root input is absent from ordered link inputs"));
        }
    }
    for (index, root) in request.roots.iter().enumerate() {
        if root == &request.entry || request.roots[..index].contains(root) {
            return Err(invalid("duplicate link root"));
        }
    }
    if request.absent.len() > MAX_ABSENT_SYMBOLS
        || request.absent.iter().enumerate().any(|(index, name)| {
            name.is_empty()
                || name.len() > 512
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'$'))
                || request.absent[..index].contains(name)
        })
    {
        return Err(invalid("absent names must be distinct linker identifiers"));
    }
    Ok(())
}

/// One object of a link input.
#[derive(Clone)]
struct Member {
    /// Position of the object's input in the request.
    input: usize,
    object: ObjectId,
    payload: Option<ArtifactId>,
    alias: String,
}

/// Every object of the link inputs, the definitions bound to names and the
/// resolved roots, with the reasons the link cannot proceed.
struct Collected {
    members: Vec<Member>,
    /// Companion then absent-name definitions of the request.
    definitions: Vec<(String, u32)>,
    roots: Vec<blobray_artifacts::LinkRootFacts>,
    blockers: Vec<String>,
}

fn block(blockers: &mut Vec<String>, message: impl Into<String>) -> Result<()> {
    if blockers.len() == MAX_BLOCKERS {
        return Err(Error::new(
            ErrorCode::ResourceLimited,
            format!("link blocker capacity exhausted ({MAX_BLOCKERS})"),
        ));
    }
    let mut message = message.into();
    truncate_message(&mut message);
    blockers.push(message);
    Ok(())
}

/// ELF records of one link member: companion names it must not define and
/// its diagnostics.
struct Scan<'a> {
    companions: &'a [(String, u32)],
    diagnostics: Vec<String>,
}
impl ElfSink for Scan<'_> {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn symbol(&mut self, r: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        companions::check_link_symbol(self.companions, r)
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, d: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        self.diagnostics.push(d.message.clone());
        Ok(())
    }
}

/// Every member of the request's inputs, in input order, presenting each
/// linkable member's bytes to `consume`.
fn collect(
    request: &LinkRequest,
    executables: &[Executable],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    mut consume: impl FnMut(&Member, &dyn ByteSource, &mut dyn RunControl) -> Result<()>,
) -> Result<Collected> {
    validate_request(request)?;
    let definitions = companions::resolve(request, executables, memory, control)?;
    let companions = &definitions[..request.companions.len()];
    let selectors: Vec<_> = std::iter::once(&request.entry)
        .chain(&request.roots)
        .cloned()
        .collect();
    let mut members = Vec::new();
    let mut blockers = Vec::new();
    let mut root_facts = Vec::new();
    for (input, artifact) in request.inputs.iter().enumerate() {
        let Ok(executable) = find(executables, artifact) else {
            block(&mut blockers, "a link input was not given")?;
            continue;
        };
        let container = visit_members(executable, memory, control, &mut |m, c| {
            if members.len() == MAX_MEMBERS {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    format!("link member capacity exhausted ({MAX_MEMBERS})"),
                ));
            }
            let mut scan = Scan {
                companions,
                diagnostics: Vec::new(),
            };
            let bytes = match &m.bytes {
                Ok(bytes) => Some(*bytes),
                Err(diagnostic) => {
                    scan.diagnostics.push(diagnostic.message.clone());
                    None
                }
            };
            let (payload, header) = match bytes {
                Some(bytes) => {
                    let (payload, header) = inspect_source(bytes, &m.id, memory, c, &mut scan)?;
                    (Some(payload), header)
                }
                None => (None, None),
            };
            for message in scan.diagnostics {
                block(&mut blockers, message)?;
            }
            let elf = header.is_some_and(|e| {
                e.bits == 32 && e.little_endian && e.machine == 243 && e.object_type == 1
            });
            if !elf || payload.is_none() {
                block(
                    &mut blockers,
                    "selected member is not a captured relocatable RV32 ELF",
                )?;
            }
            let ordinal = match m.id.location {
                ObjectLocation::Standalone => 0,
                ObjectLocation::ArchiveMember { ordinal } => ordinal,
            };
            let member = Member {
                input,
                object: m.id.clone(),
                payload,
                alias: format!("i{input}-m{ordinal}.o"),
            };
            if let (true, Some(bytes), Some(payload)) = (elf, bytes, &member.payload) {
                let selected: Vec<_> = selectors
                    .iter()
                    .filter(|s| s.object == m.id)
                    .cloned()
                    .collect();
                match blobray_artifacts::inspect_link_input(bytes, payload, &selected, memory, c) {
                    Ok(facts) => {
                        consume(&member, bytes, c)?;
                        root_facts.extend(facts);
                    }
                    Err(e) if e.code == ErrorCode::LinkBlocked => block(&mut blockers, e.message)?,
                    Err(e) => return Err(e),
                }
            }
            members.push(member);
            Ok(())
        })?;
        if let Some(framing) = container.framing {
            block(&mut blockers, framing.message)?;
        }
    }
    for root in &selectors {
        if !root_facts.iter().any(|r| &r.symbol == root) {
            block(
                &mut blockers,
                "selected root cannot be resolved to a supported executable occurrence",
            )?;
        }
    }
    for (i, root) in root_facts.iter().enumerate() {
        if root_facts[..i].iter().any(|r| r.name == root.name) {
            block(
                &mut blockers,
                "distinct roots have the same linker-visible name",
            )?;
        }
    }
    root_facts.sort_by_key(|r| selectors.iter().position(|s| s == &r.symbol).unwrap());
    Ok(Collected {
        members,
        definitions,
        roots: root_facts,
        blockers,
    })
}

/// Collect the request's members into `workspace`, failing with every
/// blocker when one exists.
fn materialize(
    request: &LinkRequest,
    executables: &[Executable],
    workspace: &LinkWorkspace,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Collected> {
    control.phase(RunPhase::Materialize)?;
    let found = collect(
        request,
        executables,
        memory,
        control,
        |member, source, c| workspace.materialize(&member.alias, &read_scratch(source, memory, c)?),
    )?;
    if !found.blockers.is_empty() {
        return Err(Error::new(
            ErrorCode::LinkBlocked,
            found.blockers.join("; "),
        ));
    }
    Ok(found)
}

/// The linker invocation of the collected members: roots' members are
/// forced, every other member of an archive is a lazy archive member.
fn invocation<'a>(
    request: &LinkRequest,
    found: &'a Collected,
    executable: &'a Path,
    identity: &'a LinkerIdentity,
    workspace: &'a LinkWorkspace,
    unresolved: UnresolvedSymbols,
) -> LinkInvocation<'a> {
    let mut forced = Vec::new();
    for root in &found.roots {
        let member = found
            .members
            .iter()
            .find(|m| m.object == root.symbol.object)
            .unwrap();
        if !forced.contains(&member.alias) {
            forced.push(member.alias.clone());
        }
    }
    let mut inputs = Vec::new();
    for input in 0..request.inputs.len() {
        let members: Vec<_> = found
            .members
            .iter()
            .filter(|m| m.input == input && !forced.contains(&m.alias))
            .collect();
        if members.is_empty() {
            continue;
        }
        if members[0].object.location == ObjectLocation::Standalone {
            inputs.push(LinkInput::Object(members[0].alias.clone()));
        } else {
            inputs.push(LinkInput::Archive(
                members.iter().map(|m| m.alias.clone()).collect(),
            ));
        }
    }
    LinkInvocation {
        executable,
        identity,
        workspace,
        contract: LinkerContract::ElfAnalysisLinkV1,
        entry: &found.roots[0].name,
        roots: found.roots.iter().skip(1).map(|r| r.name.clone()).collect(),
        forced,
        inputs,
        members: found
            .members
            .iter()
            .map(|m| LinkMember {
                alias: m.alias.clone(),
                object: m.object.clone(),
            })
            .collect(),
        layout: request.layout,
        definitions: found.definitions.clone(),
        unresolved,
    }
}

/// Everything one linker process wrote and claimed.
#[derive(Default)]
struct Outputs {
    exit_code: Option<i32>,
    signal: Option<i32>,
    elf: Vec<u8>,
    map: Vec<u8>,
    extraction: Vec<u8>,
    extractions: Vec<LinkObservation>,
    placements: Vec<LinkObservation>,
    exit_observations: u32,
    stderr: Vec<u8>,
    truncated: bool,
}
impl Outputs {
    fn diagnostics(&self) -> LinkerDiagnostics {
        LinkerDiagnostics {
            exit_code: self.exit_code,
            signal: self.signal,
            stderr_tail: self.stderr.clone(),
            stderr_truncated: self.truncated,
        }
    }
}
impl LinkOutputSink for Outputs {
    fn elf(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.elf = bytes;
        Ok(())
    }
    fn observe(
        &mut self,
        observation: LinkObservation,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        control.checkpoint(1)?;
        match &observation {
            LinkObservation::SectionPlacement { .. } => self.placements.push(observation),
            LinkObservation::ArchiveExtraction { .. } => self.extractions.push(observation),
            LinkObservation::ToolExit { code, signal } => {
                if *code != self.exit_code || *signal != self.signal || self.exit_observations != 0
                {
                    return Err(invalid("duplicate or inconsistent tool exit"));
                }
                self.exit_observations += 1;
            }
        }
        Ok(())
    }
    fn exited(&mut self, code: Option<i32>, signal: Option<i32>) {
        self.exit_code = code;
        self.signal = signal;
    }
    fn write(
        &mut self,
        channel: LinkOutput,
        bytes: &[u8],
        control: &mut dyn RunControl,
    ) -> Result<()> {
        control.bytes(bytes.len())?;
        match channel {
            LinkOutput::Elf => self.elf.extend_from_slice(bytes),
            LinkOutput::Map => self.map.extend_from_slice(bytes),
            LinkOutput::Extraction => self.extraction.extend_from_slice(bytes),
            LinkOutput::Stderr => {
                if self.stderr.len() + bytes.len() > 8192 {
                    self.truncated = true;
                    let remove = (self.stderr.len() + bytes.len() - 8192).min(self.stderr.len());
                    self.stderr.drain(..remove);
                }
                self.stderr
                    .extend_from_slice(&bytes[bytes.len().saturating_sub(8192)..]);
            }
        }
        Ok(())
    }
}

/// Run the linker, naming its diagnostics when it fails.
fn run(
    host: &dyn LinkerHost,
    invocation: &LinkInvocation<'_>,
    outputs: &mut Outputs,
    control: &mut dyn RunControl,
) -> Result<()> {
    control.phase(RunPhase::Link)?;
    if let Err(mut error) = host.link(invocation, outputs, control) {
        if error.code == ErrorCode::LinkFailed {
            error.message = format!(
                "{}: {}{}",
                error.message,
                String::from_utf8_lossy(&outputs.stderr),
                if outputs.truncated {
                    " [diagnostics truncated]"
                } else {
                    ""
                }
            );
            truncate_message(&mut error.message);
        }
        return Err(error);
    }
    Ok(())
}

/// One linked image: its manifest, its ELF and the linker's map.
pub struct LinkedImage {
    pub manifest: ImageManifest,
    pub elf: Executable,
    pub map: Vec<u8>,
}

/// Link the image `request` describes with the linker at `linker`, in a
/// temporary directory below `directory`. Every executable the request names
/// is among `executables`.
#[allow(clippy::too_many_arguments)]
pub fn link(
    request: &LinkRequest,
    executables: &[Executable],
    linker: &Path,
    host: &dyn LinkerHost,
    directory: &Path,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<LinkedImage> {
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
        UnresolvedSymbols::Error,
    );
    let mut outputs = Outputs::default();
    run(host, &invocation, &mut outputs, control)?;
    let (roots, mappings) = evidence::roots(&outputs, &found, &invocation, control)?;
    let validated = blobray_artifacts::validate_image(
        &outputs.elf.as_slice(),
        &request.layout,
        &roots,
        memory,
        control,
    )?;
    let linker_diagnostics = outputs.diagnostics();
    let elf = Executable::new(std::mem::take(&mut outputs.elf));
    Ok(LinkedImage {
        manifest: ImageManifest {
            schema: IMAGE_SCHEMA,
            request: request.clone(),
            contract: invocation.contract,
            linker: identity.clone(),
            abi: validated.abi,
            elf: elf.id().clone(),
            entry: roots[0].address,
            roots,
            segments: validated.segments,
            mappings,
            linker_diagnostics,
        },
        elf,
        map: outputs.map,
    })
}

#[cfg(test)]
mod root_limit_tests {
    use super::*;

    fn symbol(index: u64) -> SymbolId {
        SymbolId {
            object: ObjectId {
                artifact: ArtifactId::of_bytes(b"object"),
                location: ObjectLocation::Standalone,
            },
            table: SymbolTableKind::Static,
            table_section: 0,
            index,
        }
    }

    fn request(roots: u64) -> LinkRequest {
        LinkRequest {
            companions: vec![],
            inputs: vec![ArtifactId::of_bytes(b"object")],
            entry: symbol(0),
            roots: (1..=roots).map(symbol).collect(),
            layout: ImageLayout {
                code: ImageRegion {
                    start: 0x1000_0000,
                    length: 0x10_0000,
                },
                data: ImageRegion {
                    start: 0x2000_0000,
                    length: 0x10_0000,
                },
            },
            absent: vec![],
        }
    }

    #[test]
    fn absent_names_are_distinct_bounded_linker_identifiers() {
        let mut named = request(0);
        named.absent = vec!["putchar".into(), "puts".into()];
        validate_request(&named).unwrap();
        for absent in [
            vec!["putchar".to_owned(), "putchar".to_owned()],
            vec![String::new()],
            vec!["bad name".to_owned()],
            (0..=MAX_ABSENT_SYMBOLS).map(|i| format!("n{i}")).collect(),
        ] {
            named.absent = absent;
            assert_eq!(
                validate_request(&named).unwrap_err().code,
                ErrorCode::InvalidRequest
            );
        }
    }

    #[test]
    fn an_image_holds_the_entry_and_at_most_the_remaining_roots() {
        let additional = (MAX_IMAGE_ROOTS - 1) as u64;
        validate_request(&request(additional)).unwrap();
        assert_eq!(
            validate_request(&request(additional + 1)).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn inputs_are_distinct_and_hold_every_root() {
        let mut twice = request(0);
        twice.inputs.push(twice.inputs[0].clone());
        assert_eq!(
            validate_request(&twice).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        let mut foreign = request(0);
        foreign.entry.object.artifact = ArtifactId::of_bytes(b"another object");
        assert_eq!(
            validate_request(&foreign).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }
}
