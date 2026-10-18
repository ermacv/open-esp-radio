//! Leaf comparisons: one vendor function and the compiled production probe
//! that runs its counterpart, over the product of reviewed argument
//! domains.
//!
//! Every case runs both sides over the whole radio register block retained
//! from one fill pattern, so every register bit the leaf reads takes both
//! values across the fills, and compares every register effect exactly and,
//! where the leaf returns one, the return word. A chip's scenario crate
//! declares its suites of leaves; the scratch objects live in the installed
//! chip's PHY layout.
use crate::harness::{Arg, Buffer, Input, Result, case, direct, filled, invalid, known, selection};
use crate::phy::layout::{layout, radio_aperture};
use crate::phy::{image_layout, select};
use crate::session::{Session, image_symbol_id, request};
use blobray_domain::{
    CallBinding, CallBoundary, CallCapture, CallDeclaration, CallRepetition, CallResponse,
    ComparisonVerdict, EffectDisposition, EffectPattern, EffectRule, EffectSelector, EffectValue,
    ExecutionCase, ExecutionEvidence, ExecutionGoal, ExecutionTarget, LinkRequest, MemoryPair,
    ModelStatus, RegionLifetime, SessionReset,
};
use oer_riscv_model::{ArtifactId, ObjectId, ObjectLocation};
use std::any::Any;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Input index of the compiled production probe ELF.
pub const PROBE_INPUT: usize = 2;
/// Input index of a suite's vendor firmware.
const FIRMWARE_INPUT: u64 = 3;
/// Radio register fills: all clear, all set and two alternating patterns.
pub const LEAF_FILLS: [u8; 4] = [0x00, 0xff, 0x5a, 0xa5];
/// Reads of one vendor-only register a leaf case may perform.
const VENDOR_ONLY_READS: u32 = 16;
/// Guest events one leaf case may record.
pub const LEAF_EVENTS: u32 = 1 << 10;

/// One leaf suite: its archives, pinned by artifact id (the first at
/// session input 0, the others after the ROM, the production probes and the
/// optional firmware), its ROM, its leaves and the chip-specific additions.
pub struct Suite {
    pub title: &'static str,
    /// Identifier of the suite in reviewed contract ids.
    pub id: &'static str,
    pub archives: &'static [&'static str],
    /// Artifact id of the ROM ELF at session input 1.
    pub rom: &'static str,
    /// Artifact id of the vendor firmware at session input 3 that supplies
    /// data and logging symbols code after a compared prefix references.
    pub firmware: Option<&'static str>,
    pub leaves: &'static [Leaf],
    /// Symbols the link declares absent: referenced only after compared
    /// prefixes.
    pub absent: &'static [&'static str],
    /// Further link roots in the first archive, beyond the leaves.
    pub roots: &'static [&'static [&'static str]],
    /// Builds the suite's vendor context from the linked session, such as
    /// tables the builders read; the builders receive it as
    /// [`Vendor::context`].
    pub prepare: Option<Prepare>,
    /// Further claims the suite's own sequences establish.
    pub claims: &'static [(&'static str, &'static str, &'static str)],
}

impl Suite {
    /// Session input of the suite archive at `index`.
    pub fn archive_input(&self, index: usize) -> u64 {
        if index == 0 {
            0
        } else {
            self.first_extra_input() + index as u64 - 1
        }
    }

    fn first_extra_input(&self) -> u64 {
        PROBE_INPUT as u64 + 1 + u64::from(self.firmware.is_some())
    }
}

/// Builds a suite's vendor context from its linked run.
pub type Prepare = fn(&mut LeafRun) -> Result<Box<dyn Any>>;

/// Private inputs and budget of a leaf suite.
pub struct LeafOptions {
    pub suite: &'static Suite,
    /// One path per suite archive, in the suite's order.
    pub archives: Vec<PathBuf>,
    pub rom: PathBuf,
    /// The suite's vendor firmware, when it names one.
    pub firmware: Option<PathBuf>,
    pub production: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub budget: crate::harness::Budget,
    pub patches: Vec<blobray_application::in_process::ImagePatch>,
}

/// Whether every device and call model of `case` on `side` completed.
fn all_complete(records: &[ExecutionEvidence], case: u32, side: bool) -> bool {
    records.iter().all(|r| match r {
        ExecutionEvidence::Model {
            case: c,
            replacement,
            observation,
        } if *c == case && *replacement == side => observation.status == ModelStatus::Complete,
        ExecutionEvidence::CallModel {
            case: c,
            replacement,
            observation,
        } if *c == case && *replacement == side => observation.status == ModelStatus::Complete,
        _ => true,
    })
}

/// Values one parameter takes: ABI words, six-byte addresses the leaf reads
/// through a pointer to a buffer at `layout().input`, or the address of an output
/// object in the region at `layout().output`, filled with `OUTPUT_FILL` and compared
/// after the leaf; a nullable output also takes the null pointer.
#[derive(Clone, Copy)]
pub enum Domain {
    Words(&'static [u32]),
    Addresses(&'static [[u8; 6]]),
    Output {
        offset: u32,
        length: u32,
        nullable: bool,
    },
}

impl Domain {
    fn len(self) -> usize {
        match self {
            Self::Words(values) => values.len(),
            Self::Addresses(values) => values.len(),
            Self::Output { nullable, .. } => 1 + usize::from(nullable),
        }
    }
}

/// A non-null output object of `length` bytes at the start of the region.
pub const fn output(length: u32) -> Domain {
    Domain::Output {
        offset: 0,
        length,
        nullable: false,
    }
}

/// Initial bytes of an output object: bytes a leaf leaves alone compare too.
pub const OUTPUT_FILL: u8 = 0xa5;
/// Source of the bytes a setup phase copies into a linked-image object.
const IMAGE_SOURCE: u32 = 0x3fff_b000;

/// One vendor leaf, its production probe and the argument domain of each
/// parameter, in ABI order; the cases are their product.
pub struct Leaf {
    pub vendor: &'static str,
    pub probe: &'static str,
    pub parameters: &'static [(&'static str, Domain)],
    pub returns: bool,
    /// Full fences production adds to order the leaf's register edge
    /// against surrounding memory and device accesses.
    pub ordering_fences: u32,
    /// Release fences production adds when it releases the shared-radio
    /// lease after the leaf's register transaction.
    pub release_fences: u32,
    /// Whether production may reject a case before it leases the radio, so
    /// the release fences occur only in cases that reach the hardware.
    pub release_optional: bool,
    /// Reviewed word writes production performs in place of vendor writes,
    /// each exactly once.
    pub replacements: &'static [Replacement],
    /// Registers the vendor reads for values production does not need,
    /// with their reason: production may omit those reads.
    pub vendor_reads: &'static [(u32, &'static str)],
    /// A bounded feature: the vendor side stops before calling this
    /// function, and only the prefix up to that call is compared.
    pub prefix_until: Option<&'static str>,
    /// Whether a tail call of `prefix_until` also ends the vendor side.
    pub prefix_tail: bool,
    /// Builds both sides' objects from the semantic probe words when the
    /// vendor reads its arguments from its own objects.
    pub vendor_abi: Option<VendorAbi>,
    /// The vendor function is a ROM symbol rather than a `libpp.a` root.
    pub rom: bool,
    /// Session input of the archive defining the vendor function.
    pub archive: u64,
    /// Object states the builder receives after the probe words: a further
    /// case dimension that reaches the probe only through the objects.
    pub states: &'static [u32],
    /// A dispatcher compared up to its selected callee: for the case words,
    /// the vendor callee and the production callee both sides stop before.
    /// Reaching another callee leaves the goal unmet; the first argument
    /// word at both stops must be the case's first word.
    pub dispatch: Option<Dispatch>,
    /// Vendor functions answered with a zero return and no other effect:
    /// assertions the compared domain never fails.
    pub quiet_calls: &'static [&'static str],
    /// Further reviewed rules of the leaf's effect contract, such as the
    /// polling a transport leaves to its hardware.
    pub rules: Option<fn() -> Vec<EffectRule>>,
    /// An expectation independent of both executions, checked against the
    /// vendor side of every case: the case words (probe words, then the
    /// object state) and what the vendor did.
    pub expect: Option<Expectation>,
}

/// What the vendor side of one leaf case did: its returned low word and its
/// ordered word MMIO effects.
pub struct Observed {
    pub returned: Option<u32>,
    pub effects: Vec<crate::evidence::PhyEffect>,
}

/// Checks one case against an independent expectation.
pub type Expectation = fn(&[u32], &Observed) -> std::result::Result<(), String>;

/// The vendor and production callees a dispatcher selects for the words.
pub type Dispatch = fn(&[u32]) -> DispatchTarget;

/// The callees a dispatcher selects, and the vendor callee's argument word
/// that carries the case's first word: the production callee takes it
/// first, while a vendor callee may take its context first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DispatchTarget {
    pub vendor: &'static str,
    pub production: &'static str,
    pub vendor_word: u16,
}

impl DispatchTarget {
    /// Both callees take the case's first word first.
    pub const fn first(vendor: &'static str, production: &'static str) -> Self {
        Self {
            vendor,
            production,
            vendor_word: 0,
        }
    }
}

/// Objects of one case: vendor argument words, initialized vendor and
/// production objects as (address, bytes), and vendor call models.
#[derive(Default)]
pub struct Objects {
    pub vendor_words: Vec<u32>,
    pub vendor: Vec<(u32, Vec<u8>)>,
    pub production: Vec<(u32, Vec<u8>)>,
    pub calls: Vec<CallDeclaration>,
    /// Object bytes both sides share and the relation compares after the
    /// leaf, as (address, length); each side receives the same objects.
    pub compared: Vec<(u32, u32)>,
    /// Vendor objects inside the linked image's data, as (address, bytes):
    /// setup phases copy them in with the captured ROM `memcpy`, and the
    /// compared phase keeps them warm.
    pub image: Vec<(u32, Vec<u8>)>,
    /// Register models both sides share, as (address, initial word): exact
    /// registers within the radio aperture whose initial value the case
    /// selects instead of the fill pattern.
    pub registers: Vec<(u32, u32)>,
    /// Registers both sides read as a finite sequence, as (address, runs):
    /// a read beyond the runs fails the case, so both sides must read each
    /// exactly as often as the runs allow.
    pub sequences: Vec<(u32, Vec<blobray_domain::ReadRun>)>,
    /// Further device models both sides share, ahead of the radio aperture,
    /// such as the analog command bank a transport drives.
    pub devices: Vec<blobray_domain::DeviceDeclaration>,
}

/// Builds a case's objects from the semantic probe words, followed by the
/// object state when the leaf has states.
pub type VendorAbi = fn(&[u32], &Vendor<'_>) -> Result<Objects>;

/// What a builder may read from the vendor side: linked-image or ROM symbol
/// addresses and data sections of the captured archive.
pub struct Vendor<'a> {
    pub resolve: &'a dyn Fn(&str) -> Result<u32>,
    /// Symbols the linked image defines; any other symbol is a ROM address.
    pub image: &'a BTreeMap<String, u32>,
    /// The suite's context from its `prepare` hook.
    pub context: &'a dyn Any,
    /// Symbol values of the vendor firmware, including the absolute
    /// addresses its linker scripts provide.
    pub firmware: &'a dyn Fn(&str) -> Result<u32>,
}

impl Vendor<'_> {
    /// The suite context, as the type its `prepare` hook built.
    pub fn context<T: 'static>(&self) -> Result<&T> {
        self.context
            .downcast_ref()
            .ok_or_else(|| invalid("the suite context has another type"))
    }

    pub fn symbol(&self, name: &str) -> Result<u32> {
        (self.resolve)(name)
    }

    /// The address of a symbol or uniquely named input section of the
    /// linked image.
    pub fn image_symbol(&self, name: &str) -> Result<u32> {
        self.image
            .get(name)
            .copied()
            .ok_or_else(|| invalid(format!("the linked image does not place {name}")))
    }

    /// The boundary of a call model answering `name`: captured code when the
    /// linked image defines it, an unmapped address otherwise.
    pub fn boundary(&self, name: &str) -> CallBoundary {
        call_boundary(self.image, name)
    }
}

pub fn call_boundary(image: &BTreeMap<String, u32>, name: &str) -> CallBoundary {
    if image.contains_key(name) {
        CallBoundary::CapturedCode
    } else {
        CallBoundary::Unmapped
    }
}

pub const fn leaf(
    vendor: &'static str,
    probe: &'static str,
    parameters: &'static [(&'static str, Domain)],
    returns: bool,
) -> Leaf {
    Leaf {
        vendor,
        probe,
        parameters,
        returns,
        ordering_fences: 0,
        release_fences: 0,
        release_optional: false,
        replacements: &[],
        vendor_reads: &[],
        prefix_until: None,
        prefix_tail: false,
        vendor_abi: None,
        rom: false,
        archive: 0,
        states: &[],
        dispatch: None,
        quiet_calls: &[],
        rules: None,
        expect: None,
    }
}

/// A dispatcher leaf compared up to the callee `select` names.
pub const fn dispatching(leaf: Leaf, select: Dispatch) -> Leaf {
    Leaf {
        dispatch: Some(select),
        ..leaf
    }
}

/// A leaf whose vendor `calls` return zero with no other effect.
pub const fn quiet(leaf: Leaf, calls: &'static [&'static str]) -> Leaf {
    Leaf {
        quiet_calls: calls,
        ..leaf
    }
}

/// A leaf whose vendor function is the ROM symbol of that name.
pub const fn rom(leaf: Leaf) -> Leaf {
    Leaf { rom: true, ..leaf }
}

/// A leaf whose vendor function the suite archive at session input `input`
/// defines.
pub const fn in_archive(leaf: Leaf, input: u64) -> Leaf {
    Leaf {
        archive: input,
        ..leaf
    }
}

/// A leaf whose production counterpart adds `fences` ordering fences.
pub const fn ordered(leaf: Leaf, fences: u32) -> Leaf {
    Leaf {
        ordering_fences: fences,
        ..leaf
    }
}

/// A leaf whose production counterpart releases its shared-radio lease
/// with `fences` release fences.
pub const fn released(leaf: Leaf, fences: u32) -> Leaf {
    Leaf {
        release_fences: fences,
        ..leaf
    }
}

/// A leaf whose production counterpart releases its shared-radio lease
/// with `fences` release fences in the cases it leases the radio at all: it
/// rejects invalid arguments before leasing.
pub const fn released_when_leased(leaf: Leaf, fences: u32) -> Leaf {
    Leaf {
        release_fences: fences,
        release_optional: true,
        ..leaf
    }
}

/// One reviewed write substitution: production writes `replacement` where
/// the vendor writes `vendor`, as (address, value) pairs, for `reason`.
#[derive(Clone, Copy)]
pub struct Replacement {
    pub vendor: (u32, u32),
    pub replacement: (u32, u32),
    pub reason: &'static str,
}

/// A leaf whose production counterpart performs `replacements`.
pub const fn replaced(leaf: Leaf, replacements: &'static [Replacement]) -> Leaf {
    Leaf {
        replacements,
        ..leaf
    }
}

/// A leaf whose vendor reads `registers` for values production does not
/// need; production may omit each read.
pub const fn vendor_reads(leaf: Leaf, registers: &'static [(u32, &'static str)]) -> Leaf {
    Leaf {
        vendor_reads: registers,
        ..leaf
    }
}

/// A leaf whose effect contract also carries the reviewed `rules`.
pub const fn ruled(leaf: Leaf, rules: fn() -> Vec<EffectRule>) -> Leaf {
    Leaf {
        rules: Some(rules),
        ..leaf
    }
}

/// A leaf whose vendor side every case checks against `expect`.
pub const fn expected(leaf: Leaf, expect: Expectation) -> Leaf {
    Leaf {
        expect: Some(expect),
        ..leaf
    }
}

/// A leaf whose objects also take each of `states`.
pub const fn stated(leaf: Leaf, states: &'static [u32]) -> Leaf {
    Leaf { states, ..leaf }
}

/// A leaf whose vendor reads its semantic arguments from objects `abi` builds.
pub const fn objects(leaf: Leaf, abi: VendorAbi) -> Leaf {
    Leaf {
        vendor_abi: Some(abi),
        ..leaf
    }
}

/// Initial bytes of each compared range, from the vendor objects covering it.
pub fn compared_bytes(objects: &[(u32, Vec<u8>)], compared: &[(u32, u32)]) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    for (address, length) in compared {
        let (start, object) = objects
            .iter()
            .find(|(start, object)| {
                *start <= *address && address + length <= start + object.len() as u32
            })
            .ok_or_else(|| invalid(format!("compared range {address:#x} has no object")))?;
        let offset = (address - start) as usize;
        bytes.extend_from_slice(&object[offset..offset + *length as usize]);
    }
    Ok(bytes)
}

/// Addresses of the named symbols of the linked image, including the
/// absolute companion definitions, and of the input sections its link map
/// places by section name: a local object without a symbol, such as a
/// static byte in `.bss.<name>`, is addressed by its section.
pub fn image_symbols(elf: &std::path::Path) -> Result<BTreeMap<String, u32>> {
    let bytes = std::fs::read(elf)?;
    let file = oer_elf::Elf::parse(&bytes)?;
    let mut symbols = BTreeMap::new();
    for symbol in file.symbols() {
        if let Ok(address) = u32::try_from(symbol.address)
            && !symbol.name.is_empty()
            && symbol.defined
        {
            symbols.entry(symbol.name.to_owned()).or_insert(address);
        }
    }
    if let Ok(map) = std::fs::read_to_string(elf.with_file_name(crate::state::LINK_MAP)) {
        for section in crate::state::link_map_sections(&map) {
            symbols.entry(section.name).or_insert(section.address);
        }
    }
    Ok(symbols)
}

/// A leaf compared only up to the vendor's call of `callee`.
pub const fn prefix(leaf: Leaf, callee: &'static str) -> Leaf {
    Leaf {
        prefix_until: Some(callee),
        ..leaf
    }
}

/// A leaf compared only up to the vendor's call or tail call of `callee`.
pub const fn tail_prefix(leaf: Leaf, callee: &'static str) -> Leaf {
    Leaf {
        prefix_until: Some(callee),
        prefix_tail: true,
        ..leaf
    }
}

/// Full-fence predecessor and successor sets: device input, output, memory
/// reads and writes.
const FULL_FENCE: u8 = 0xf;
/// Release-fence predecessor and successor sets: memory reads and writes
/// before memory writes.
const RELEASE_FENCE: (u8, u8) = (0x3, 0x1);

/// One case: its setup phases, the compared phase, the initial bytes of its
/// compared objects and its probe words.
pub type LeafCase = (Vec<ExecutionCase>, ExecutionCase, Vec<u8>, Vec<u32>);

/// Linked suite image with its captured roots and both execution targets.
pub struct LeafRun {
    pub session: Session,
    pub suite: &'static Suite,
    /// The suite context its `prepare` hook built.
    pub context: Box<dyn Any>,
    pub roots: BTreeMap<String, u32>,
    pub image_object: ObjectId,
    pub vendor: ExecutionTarget,
    pub production: ExecutionTarget,
}

impl std::ops::Deref for LeafRun {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}
impl std::ops::DerefMut for LeafRun {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

impl LeafRun {
    pub fn new(options: &LeafOptions) -> Result<Self> {
        let suite = options.suite;
        if options.archives.len() != suite.archives.len() {
            return Err(invalid("one path per suite archive"));
        }
        if options.firmware.is_some() != suite.firmware.is_some() {
            return Err(invalid("a firmware path exactly when the suite names one"));
        }
        let mut inputs = vec![
            Input {
                role: suite.archives[0],
                path: &options.archives[0],
                sha256: Some(crate::artifacts::sha256(suite.archives[0])?),
            },
            Input {
                role: "rom",
                path: &options.rom,
                sha256: Some(crate::artifacts::sha256(suite.rom)?),
            },
            Input {
                role: "production",
                path: &options.production,
                sha256: None,
            },
        ];
        if let (Some(id), Some(path)) = (suite.firmware, &options.firmware) {
            inputs.push(Input {
                role: "firmware",
                path,
                sha256: Some(crate::artifacts::sha256(id)?),
            });
        }
        for (id, path) in suite.archives.iter().zip(&options.archives).skip(1) {
            inputs.push(Input {
                role: id,
                path,
                sha256: Some(crate::artifacts::sha256(id)?),
            });
        }
        let archive_inputs: Vec<u64> = (0..suite.archives.len())
            .map(|index| suite.archive_input(index))
            .collect();
        let session = Session::start(
            &options.output,
            options.budget,
            &inputs,
            suite.title,
            &options.patches,
        )?;
        // The first leaf is the link entry; the others are further roots,
        // each selected in the archive that defines it.
        let leaves = suite.leaves;
        let mut vendors: Vec<(u64, &str)> = leaves
            .iter()
            .filter(|l| !l.rom)
            .map(|l| (l.archive, l.vendor))
            .filter(|(_, v)| *v != leaves[0].vendor)
            .chain(
                suite
                    .roots
                    .iter()
                    .flat_map(|roots| roots.iter())
                    .map(|root| (0, *root)),
            )
            .collect();
        vendors.sort_unstable();
        vendors.dedup();
        let roots = vendors
            .iter()
            .map(|(input, v)| select(&session, *input as usize, v))
            .collect::<Result<Vec<_>>>()?;
        let link = LinkRequest {
            companions: vec![],
            inputs: archive_inputs
                .iter()
                .map(|input| session.input_id(*input as usize))
                .collect::<Result<_>>()?,
            entry: select(&session, leaves[0].archive as usize, leaves[0].vendor)?,
            roots,
            layout: image_layout(),
            absent: suite.absent.iter().map(|n| (*n).to_owned()).collect(),
        };
        let mut candidates = vec![crate::chip().rom_input];
        if suite.firmware.is_some() {
            candidates.push(FIRMWARE_INPUT);
        }
        let linked = session.link(&link, &options.linker, leaves[0].vendor, &candidates)?;
        let (vendor, production) = session.targets(&linked.manifest.elf)?;
        let mut run = Self {
            suite,
            context: Box::new(()),
            image_object: ObjectId {
                artifact: linked.manifest.elf.clone(),
                location: ObjectLocation::Standalone,
            },
            roots: linked.roots,
            vendor,
            production,
            session,
        };
        if let Some(prepare) = suite.prepare {
            run.context = prepare(&mut run)?;
        }
        Ok(run)
    }

    /// The suite context its `prepare` hook built.
    pub fn context<T: 'static>(&self) -> Result<&T> {
        self.context
            .downcast_ref()
            .ok_or_else(|| invalid("the suite context has another type"))
    }

    /// Symbols of the linked image.
    pub fn image_symbols(&self) -> Result<BTreeMap<String, u32>> {
        image_symbols(&self.session.run.join("image/image.elf"))
    }

    /// Address of `name` in the linked image, or else in the ROM.
    pub fn symbol_address(&self, image: &BTreeMap<String, u32>, name: &str) -> Result<u32> {
        if let Some(address) = image.get(name) {
            return Ok(*address);
        }
        Ok(u32::try_from(
            crate::harness::symbol(
                &self.session.inventory,
                crate::chip().rom_input as usize,
                name,
            )?
            .value,
        )?)
    }

    /// Entry address of the vendor function of `leaf`.
    fn vendor_entry(&self, leaf: &Leaf) -> Result<u32> {
        if leaf.rom {
            Ok(u32::try_from(
                crate::harness::symbol(
                    &self.session.inventory,
                    crate::chip().rom_input as usize,
                    leaf.vendor,
                )?
                .value,
            )?)
        } else {
            Ok(self.roots[leaf.vendor])
        }
    }

    /// Exact code endpoint of the vendor function of `leaf`.
    fn vendor_endpoint(&self, leaf: &Leaf) -> Result<blobray_domain::CallEndpoint> {
        if leaf.rom {
            self.session
                .input_endpoint(crate::chip().rom_input, leaf.vendor)
        } else {
            self.session
                .image_endpoint(&self.image_object, leaf.vendor, self.roots[leaf.vendor])
        }
    }

    /// The reviewed contract of a leaf whose production adds ordering
    /// fences: exactly that many full fences, every other effect compared
    /// exactly.
    fn ordering_contract(&mut self, leaf: &Leaf) -> Result<ArtifactId> {
        let vendor = self.vendor_endpoint(leaf)?;
        let production = self.session.input_endpoint(2, leaf.probe)?;
        let write = |(address, value): (u32, u32)| EffectPattern {
            selector: EffectSelector::MmioWrite { address, width: 4 },
            value: EffectValue::Exact { value },
            preceded_by: None,
            occurrence: None,
            followed_by: None,
        };
        let mut rules: Vec<EffectRule> = leaf
            .replacements
            .iter()
            .map(|replacement| EffectRule {
                name: format!("replaced-write-{:08x}", replacement.vendor.0),
                vendor: Some(write(replacement.vendor)),
                replacement: Some(write(replacement.replacement)),
                disposition: EffectDisposition::Replaced,
                min_occurrences: 1,
                max_occurrences: 1,
                reason: replacement.reason.into(),
            })
            .collect();
        for (address, reason) in leaf.vendor_reads {
            let read = EffectPattern {
                selector: EffectSelector::MmioRead {
                    address: *address,
                    width: 4,
                },
                value: EffectValue::Any,
                preceded_by: None,
                occurrence: None,
                followed_by: None,
            };
            rules.push(EffectRule {
                name: format!("vendor-only-read-{address:08x}"),
                vendor: Some(read),
                replacement: Some(read),
                disposition: EffectDisposition::Omitted,
                min_occurrences: 0,
                max_occurrences: VENDOR_ONLY_READS,
                reason: (*reason).into(),
            });
        }
        if let Some(extra) = leaf.rules {
            rules.extend(extra());
        }
        if leaf.release_fences != 0 {
            rules.push(EffectRule {
                name: "lease-release-fence".into(),
                vendor: None,
                replacement: Some(EffectPattern {
                    selector: EffectSelector::Fence {
                        predecessor: RELEASE_FENCE.0,
                        successor: RELEASE_FENCE.1,
                    },
                    value: EffectValue::Any,
                    preceded_by: None,
                    occurrence: None,
                    followed_by: None,
                }),
                disposition: EffectDisposition::Added,
                min_occurrences: if leaf.release_optional {
                    0
                } else {
                    leaf.release_fences
                },
                max_occurrences: leaf.release_fences,
                reason: if leaf.release_optional {
                    "production releases its shared-radio lease after the register \
                    transaction, and leases the radio only for arguments that reach the \
                    hardware; the vendor serializes with a critical section answered as quiet \
                    calls"
                } else {
                    "production releases its shared-radio lease after the register \
                    transaction; the vendor serializes with a critical section answered as \
                    quiet calls"
                }
                .into(),
            });
        }
        if leaf.ordering_fences == 0 {
            return self.review_ordering(leaf, vendor, production, rules);
        }
        rules.push(EffectRule {
            name: "device-ordering-fence".into(),
            vendor: None,
            replacement: Some(EffectPattern {
                selector: EffectSelector::Fence {
                    predecessor: FULL_FENCE,
                    successor: FULL_FENCE,
                },
                value: EffectValue::Any,
                preceded_by: None,
                occurrence: None,
                followed_by: None,
            }),
            disposition: EffectDisposition::Added,
            min_occurrences: leaf.ordering_fences,
            max_occurrences: leaf.ordering_fences,
            reason: "production orders the register edge against surrounding memory and device \
                accesses; the vendor leaves ordering to its caller"
                .into(),
        });
        self.review_ordering(leaf, vendor, production, rules)
    }

    fn review_ordering(
        &mut self,
        leaf: &Leaf,
        vendor: blobray_domain::CallEndpoint,
        production: blobray_domain::CallEndpoint,
        rules: Vec<EffectRule>,
    ) -> Result<ArtifactId> {
        let applicability = "one register transaction over retained radio registers";
        self.session.review_effects(
            &format!("{}-effects", leaf.probe),
            &format!(
                "{}.{}.{}.effects",
                crate::chip().name,
                self.suite.id,
                leaf.vendor
            ),
            crate::phy::contracts::phy_contract(vendor, production, rules, applicability),
            "every register effect compares exactly; production adds ordering fences",
        )
    }

    /// Both sides of `leaf` for every argument combination, object state and
    /// fill.
    /// Each case carries the initial bytes of the objects it compares and
    /// its probe words.
    fn cases(&mut self, leaf: &Leaf) -> Result<Vec<LeafCase>> {
        let image_symbols = image_symbols(&self.session.run.join("image/image.elf"))?;
        let effects = if leaf.ordering_fences != 0
            || leaf.release_fences != 0
            || !leaf.replacements.is_empty()
            || !leaf.vendor_reads.is_empty()
            || leaf.rules.is_some()
        {
            Some(self.ordering_contract(leaf)?)
        } else {
            None
        };
        // Every combination of one value index per parameter.
        let mut combinations: Vec<Vec<usize>> = vec![vec![]];
        for (_, domain) in leaf.parameters {
            combinations = combinations
                .into_iter()
                .flat_map(|prefix| {
                    (0..domain.len()).map(move |index| {
                        let mut indices = prefix.clone();
                        indices.push(index);
                        indices
                    })
                })
                .collect();
        }
        let mut rows = vec![];
        for indices in &combinations {
            let (mut words, mut memory, mut arguments, mut label) =
                (vec![], vec![], vec![], String::new());
            // End of the output region the parameters address.
            let mut output: Option<u32> = None;
            for ((name, domain), index) in leaf.parameters.iter().zip(indices) {
                match domain {
                    Domain::Words(values) => {
                        let value = values[*index];
                        words.push(value);
                        arguments.push((*name, Arg::Word(Some(i64::from(value)))));
                        label.push_str(&format!("-{value:x}"));
                    }
                    Domain::Addresses(values) => {
                        let bytes = values[*index];
                        words.push(layout().input);
                        memory.push(known(layout().input, bytes.len() as u32, &bytes)?);
                        arguments.push((*name, Buffer::new(layout().input, bytes).into()));
                        label.push_str(&format!("-{bytes:02x?}"));
                    }
                    Domain::Output {
                        offset,
                        length,
                        nullable,
                    } => {
                        let address = if *nullable && *index == 0 {
                            0
                        } else {
                            layout().output + offset
                        };
                        words.push(address);
                        arguments.push((*name, Arg::Word(Some(i64::from(address)))));
                        output = Some(output.unwrap_or(0).max(offset + length));
                        label.push_str(&format!("-{address:x}"));
                    }
                }
            }
            let (output_memory, observe) = match output {
                Some(length) => (
                    vec![filled(layout().output, length, OUTPUT_FILL)?],
                    vec![selection(layout().output, length)],
                ),
                None => (vec![], vec![]),
            };
            let regions = |objects: &[(u32, Vec<u8>)]| {
                objects
                    .iter()
                    .map(|(address, bytes)| known(*address, bytes.len() as u32, bytes))
                    .collect::<Result<Vec<_>>>()
            };
            let rom = |name: &str| -> Result<u32> {
                if let Some(address) = image_symbols.get(name) {
                    return Ok(*address);
                }
                Ok(u32::try_from(
                    crate::harness::symbol(
                        &self.session.inventory,
                        crate::chip().rom_input as usize,
                        name,
                    )?
                    .value,
                )?)
            };
            let firmware = |name: &str| -> Result<u32> {
                Ok(u32::try_from(
                    crate::harness::symbol(&self.session.inventory, FIRMWARE_INPUT as usize, name)?
                        .value,
                )?)
            };
            let states: Vec<Option<u32>> = if leaf.states.is_empty() {
                vec![None]
            } else {
                leaf.states.iter().copied().map(Some).collect()
            };
            for state in states {
                let (
                    vendor_words,
                    mut vendor_memory,
                    production_memory,
                    vendor_calls,
                    compared,
                    initial,
                    image,
                    registers,
                    sequences,
                    case_devices,
                ) = match leaf.vendor_abi {
                    Some(abi) => {
                        let mut semantic = words.clone();
                        semantic.extend(state);
                        let objects = abi(
                            &semantic,
                            &Vendor {
                                resolve: &rom,
                                firmware: &firmware,
                                image: &image_symbols,
                                context: self.context.as_ref(),
                            },
                        )?;
                        let initial = compared_bytes(&objects.vendor, &objects.compared)?;
                        (
                            objects.vendor_words,
                            regions(&objects.vendor)?,
                            regions(&objects.production)?,
                            objects.calls,
                            objects.compared,
                            initial,
                            objects.image,
                            objects.registers,
                            objects.sequences,
                            objects.devices,
                        )
                    }
                    None => (
                        words.clone(),
                        memory.clone(),
                        vec![],
                        vec![],
                        vec![],
                        vec![],
                        vec![],
                        vec![],
                        vec![],
                        vec![],
                    ),
                };
                // Exact register models take precedence over the aperture.
                let devices = |fill: u8| {
                    let mut devices = case_devices.clone();
                    for (index, (address, runs)) in sequences.iter().enumerate() {
                        devices.push(crate::phy::layout::sequence_read(
                            &format!("case-sequence-{index}"),
                            *address,
                            runs.clone(),
                        ));
                    }
                    if !registers.is_empty() {
                        devices.push(crate::phy::layout::register_bank(
                            "case-registers",
                            "registers whose initial value the case selects",
                            registers.clone(),
                        ));
                    }
                    devices.push(radio_aperture(fill));
                    devices
                };
                let initial = [vec![OUTPUT_FILL; output.unwrap_or(0) as usize], initial].concat();
                let compared: Vec<_> = compared
                    .iter()
                    .map(|(address, length)| selection(*address, *length))
                    .collect();
                vendor_memory.extend(output_memory.clone());
                let observed = [observe.clone(), compared].concat();
                let state_label = state.map_or(String::new(), |state| format!("-s{state:x}"));
                for fill in LEAF_FILLS {
                    let mut vendor = direct(
                        self.vendor_entry(leaf)?,
                        &vendor_words,
                        vendor_memory.clone(),
                        devices(fill),
                        observed.clone(),
                    );
                    vendor.calls = vendor_calls.clone();
                    for name in leaf.quiet_calls {
                        // Names the suite declares absent share one unmapped
                        // address, which one declaration answers.
                        let (address, boundary) = if self.suite.absent.contains(name) {
                            (
                                blobray_domain::ABSENT_SYMBOL_ADDRESS,
                                CallBoundary::Unmapped,
                            )
                        } else {
                            (rom(name)?, call_boundary(&image_symbols, name))
                        };
                        if vendor.calls.iter().any(|c| c.binding.address == address) {
                            continue;
                        }
                        vendor.calls.push(CallDeclaration {
                            id: (*name).into(),
                            applicability: "a call the compared domain answers without effect"
                                .into(),
                            lifetime: RegionLifetime::Phase,
                            binding: CallBinding {
                                address,
                                boundary,
                                allow_tail: true,
                            },
                            argument_words: 1,
                            responses: vec![CallResponse {
                                return_words: [Some(0), None],
                                outputs: vec![],
                                allocation: None,
                                delay_micros: None,
                            }],
                            repetition: CallRepetition::Unbounded,
                        });
                    }
                    if let Some(callee) = leaf.prefix_until {
                        vendor.goal = ExecutionGoal::ObserveCall {
                            target: image_symbol_id(
                                &self.session.run.join("image/image.elf"),
                                &self.image_object,
                                callee,
                            )?,
                            include_tail: leaf.prefix_tail,
                        };
                    }
                    let mut production = self.session.probes.invoke(
                        leaf.probe,
                        arguments.clone(),
                        devices(fill),
                        observed.clone(),
                    )?;
                    if let Some(select) = leaf.dispatch {
                        let target = select(&words);
                        let (vendor_callee, production_callee) = (target.vendor, target.production);
                        let capture = |words: u16| CallCapture {
                            include_tail: true,
                            argument_words: words,
                            overrides: vec![],
                        };
                        vendor.goal = ExecutionGoal::ObserveCall {
                            target: image_symbol_id(
                                &self.session.run.join("image/image.elf"),
                                &self.image_object,
                                vendor_callee,
                            )?,
                            include_tail: true,
                        };
                        vendor.observe_calls = Some(capture(target.vendor_word + 1));
                        production.goal = ExecutionGoal::ObserveCall {
                            target: crate::harness::symbol(
                                &self.session.inventory,
                                PROBE_INPUT,
                                production_callee,
                            )?
                            .id
                            .clone(),
                            include_tail: true,
                        };
                        production.observe_calls = Some(capture(1));
                    }
                    production.memory.extend(output_memory.clone());
                    production.memory.extend(production_memory.clone());
                    production.arguments.resize(8, Some(0));
                    let name = format!("{}{label}{state_label}-{fill:02x}", leaf.vendor);
                    // Each image object is copied in by the captured ROM
                    // `memcpy`; production has no counterpart and copies
                    // nothing.
                    let memcpy = u32::try_from(
                        crate::harness::symbol(
                            &self.session.inventory,
                            crate::chip().rom_input as usize,
                            "memcpy",
                        )?
                        .value,
                    )?;
                    let mut setup = vec![];
                    for (index, (address, bytes)) in image.iter().enumerate() {
                        let length = bytes.len() as u32;
                        let mut phase = crate::harness::setup(
                            format!("{name}-image-{index}"),
                            direct(
                                memcpy,
                                &[*address, IMAGE_SOURCE, length],
                                vec![known(IMAGE_SOURCE, length, bytes)?],
                                vec![],
                                vec![],
                            ),
                            direct(
                                memcpy,
                                &[IMAGE_SOURCE, IMAGE_SOURCE, 0],
                                vec![],
                                vec![],
                                vec![],
                            ),
                            if index == 0 {
                                SessionReset::Cold
                            } else {
                                SessionReset::Warm
                            },
                        );
                        phase.stack_fill = Some(fill);
                        setup.push(phase);
                    }
                    let mut row = case(
                        name,
                        vendor,
                        Some(production),
                        if setup.is_empty() {
                            SessionReset::Cold
                        } else {
                            SessionReset::Warm
                        },
                        false,
                    );
                    row.stack_fill = Some(fill);
                    let relation = row.relation.as_mut().unwrap();
                    relation.returns.low = leaf.returns;
                    relation.effects = effects.clone();
                    relation.memory = (0..observed.len() as u16)
                        .map(|index| MemoryPair {
                            vendor: index,
                            replacement: index,
                        })
                        .collect();
                    let mut case_words = words.clone();
                    case_words.extend(state);
                    rows.push((setup, row, initial.clone(), case_words));
                }
            }
        }
        Ok(rows)
    }
}

/// Compare every leaf; each must MATCH in every case and record effects.
pub fn exercise(ctx: &mut LeafRun) -> Result<()> {
    for leaf in ctx.suite.leaves {
        let mut rows = vec![];
        let (mut initial, mut case_words, mut positions) = (vec![], vec![], vec![]);
        for (setup, row, bytes, words) in ctx.cases(leaf)? {
            rows.extend(setup);
            positions.push(rows.len() as u32);
            rows.push(row);
            initial.push(bytes);
            case_words.push(words);
        }
        let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
        let records = ctx
            .submit(
                leaf.probe,
                &request(&vendor, Some(&production), None, rows, LEAF_EVENTS),
                Some(ComparisonVerdict::Match),
            )?
            .records
            .clone();
        // Each case reads only its own records.
        let slices = crate::evidence::case_slices(&records);
        for (index, case) in positions.into_iter().enumerate() {
            let own = slices.get(&case).copied().unwrap_or_default();
            for side in [false, true] {
                if !all_complete(own, case, side) {
                    return Err(invalid(format!(
                        "{} case {case} did not complete",
                        leaf.vendor
                    )));
                }
            }
            if let Some(select) = leaf.dispatch {
                let expected = case_words[index][0];
                let vendor_word = select(&case_words[index]).vendor_word;
                for side in [false, true] {
                    let at = if side { 0 } else { vendor_word };
                    let argument = crate::evidence::events(own, case, side)
                        .iter()
                        .rev()
                        .find_map(|e| match e {
                            blobray_domain::ExecutionEvent::TransferArgument { word, value }
                                if *word == at =>
                            {
                                value.value()
                            }
                            _ => None,
                        });
                    if argument != Some(expected) {
                        return Err(invalid(format!(
                            "{} case {case}: side {side} reached its callee with {argument:?}",
                            leaf.vendor
                        )));
                    }
                }
                continue;
            }
            if let Some(expect) = leaf.expect {
                let observed = Observed {
                    returned: match crate::evidence::stop(own, case, false) {
                        blobray_domain::ExecutionStop::Returned { low, .. } => low,
                        _ => None,
                    },
                    effects: crate::evidence::phy_effects(&crate::evidence::events(
                        own, case, false,
                    )),
                };
                expect(&case_words[index], &observed).map_err(|error| {
                    invalid(format!(
                        "{} case {case} words {:x?}: {error}",
                        leaf.vendor, case_words[index]
                    ))
                })?;
            }
            // A leaf must act: a register effect, a write that changes an
            // object it is compared through, or a compared return word.
            let initial = &initial[index];
            if !leaf.returns
                && crate::evidence::phy_effects(&crate::evidence::events(own, case, false))
                    .is_empty()
                && (initial.is_empty() || crate::evidence::output(own, case, false) == *initial)
            {
                return Err(invalid(format!(
                    "{} case {case} has no register effect and changes no compared object",
                    leaf.vendor
                )));
            }
        }
    }
    Ok(())
}

/// Evidence claims: every leaf with its production probe.
pub fn claims(ctx: &LeafRun) -> Vec<(&'static str, &'static str, &'static str)> {
    ctx.suite
        .leaves
        .iter()
        .map(|l| (if l.rom { "rom" } else { "archive" }, l.vendor, l.probe))
        .chain(ctx.suite.claims.iter().copied())
        .collect()
}
