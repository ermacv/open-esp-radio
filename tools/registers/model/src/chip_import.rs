//! Import vendor blocks into block types and a chip root
//! ([`crate::chip_model`]): `cargo registers import`.
//!
//! The import reads the pinned vendor SVD for geometry and the pinned
//! ESP-IDF register headers for the facts; where they disagree the header
//! wins and the import report records the disagreement. Per register, the
//! header states the offset, the fields and their bits, each field's access
//! class, its default and the reserved bits; the SVD names the registers and
//! the fields at the same bits (a field the header names otherwise takes the
//! header's name) and states the reset of the bits the header leaves out. A
//! register the SVD lacks comes from its header alone; one the header lacks
//! keeps the SVD's fields as read-only, and none at all where the header's
//! device layout reserves its word. A block the SVD lacks is imported from
//! its header alone.
//!
//! The strictness overlay of the old soc path (`model/strictness.toml`)
//! contributes what no header line states, each with the lines it cites:
//! command registers, FIFO and data ports, read side effects, the fields a
//! layout defines inside a wider header field (`define-fields`), the fields
//! a vendor driver writes where the header keeps them to debugging or calls
//! them reserved, and enumerated values. Its other entries restate the
//! header; the report lists every one that disagrees with it.
//!
//! Every `sources` entry the import writes is an evidence ID of the catalog
//! it writes beside the root: one per pinned file (the SVD, each header)
//! and one per cited line (`<FILE>_L<line>`, its description holding the
//! `esp-idf:<path>:<line>` locator).

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

mod emit;
mod header;
mod merge;
mod svd;

use crate::chip_model::FieldClass;
pub use emit::type_file_name;

/// A typed import failure: the input or entity, and why.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{context}: {reason}")]
pub struct ImportError {
    pub context: String,
    pub reason: String,
}

impl ImportError {
    pub fn new(context: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            reason: reason.into(),
        }
    }
}

/// The schema of an import spec.
pub const SPEC_SCHEMA: u32 = 1;

/// What to import and where to write it (`registers/import.toml`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ImportSpec {
    pub schema: u32,
    pub chip: String,
    /// The pinned artifact ID of the vendor SVD.
    pub svd: String,
    /// The pins source of the ESP-IDF files.
    pub esp_idf: String,
    /// The strictness overlay whose facts move, relative to the spec.
    #[serde(default)]
    pub overlay: Option<String>,
    /// The chip root to write, relative to the spec.
    pub root: String,
    /// The directory of the type files, relative to the root.
    pub types_directory: String,
    /// The evidence catalog to write, relative to the spec.
    pub evidence: String,
    /// The `interrupts.h` of the chip.
    pub interrupts: String,
    /// The `reg_base.h` files whose macros place the instances.
    pub bases: Vec<String>,
    pub types: Vec<TypeSpec>,
}

/// One block type and its instances.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TypeSpec {
    pub name: String,
    pub description: String,
    /// The `*_reg.h` of the block.
    pub registers: String,
    /// The `*_struct.h` whose device layout reserves words.
    #[serde(default)]
    pub layout: Option<String>,
    pub instances: Vec<InstanceSpec>,
}

/// One instance: its SVD peripheral (none for a block the SVD lacks) and
/// the `reg_base.h` macro of its base.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct InstanceSpec {
    pub name: String,
    #[serde(default)]
    pub svd: Option<String>,
    pub base: String,
    /// The instance's own `*_reg.h` where it is not the type's.
    #[serde(default)]
    pub registers: Option<String>,
    #[serde(default)]
    pub layout: Option<String>,
}

impl ImportSpec {
    pub fn parse(text: &str) -> Result<Self, ImportError> {
        let spec: Self = toml_edit::de::from_str(text)
            .map_err(|error| ImportError::new("import spec", error.to_string()))?;
        if spec.schema != SPEC_SCHEMA {
            return Err(ImportError::new(
                "import spec",
                format!("requires schema = {SPEC_SCHEMA}, found {}", spec.schema),
            ));
        }
        let mut types = BTreeSet::new();
        let mut instances = BTreeSet::new();
        for block in &spec.types {
            if !types.insert(block.name.as_str()) {
                return Err(ImportError::new(
                    "import spec",
                    format!("type {} is listed twice", block.name),
                ));
            }
            if block.instances.is_empty() {
                return Err(ImportError::new(
                    format!("import spec type {}", block.name),
                    "has no instance",
                ));
            }
            for instance in &block.instances {
                if !instances.insert(instance.name.as_str()) {
                    return Err(ImportError::new(
                        "import spec",
                        format!("instance {} is listed twice", instance.name),
                    ));
                }
            }
        }
        Ok(spec)
    }

    /// Every ESP-IDF file the import reads, the overlay's cited lines aside.
    pub fn idf_files(&self) -> BTreeSet<String> {
        let mut files = BTreeSet::from([self.interrupts.clone()]);
        files.extend(self.bases.iter().cloned());
        for block in &self.types {
            files.insert(block.registers.clone());
            files.extend(block.layout.iter().cloned());
            for instance in &block.instances {
                files.extend(instance.registers.iter().cloned());
                files.extend(instance.layout.iter().cloned());
            }
        }
        files
    }
}

/// A pinned file: its pins source, repository, revision, path and SHA-256.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pin {
    pub source: String,
    pub repository: String,
    pub revision: String,
    pub path: String,
    pub sha256: String,
}

/// The pinned files the import reads.
pub trait PinnedFiles {
    /// The vendor SVD and its pin.
    fn svd(&self) -> Result<(String, Pin), ImportError>;
    /// The ESP-IDF file at `path` (relative to the checkout) and its pin.
    fn idf(&self, path: &str) -> Result<(String, Pin), ImportError>;
}

/// The files an import writes, each as text, and its report.
#[derive(Clone, Debug)]
pub struct ImportOutput {
    /// The chip root.
    pub root: String,
    /// Each type file by its path relative to the root.
    pub types: BTreeMap<String, String>,
    /// The evidence catalog.
    pub evidence: String,
    pub report: Report,
}

/// One disagreement between the sources, or an overlay entry the import
/// could not carry, and what the import made of it.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Finding {
    /// `TYPE`, `INSTANCE`, `INSTANCE.REGISTER` or `INSTANCE.REGISTER.FIELD`.
    pub entity: String,
    pub kind: FindingKind,
    /// What each source states and the decision.
    pub detail: String,
}

/// The kinds of findings.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FindingKind {
    /// The header defines a register the SVD lacks: imported from it.
    RegisterAdded,
    /// The header places the SVD's register elsewhere: the header's offset.
    RegisterMoved,
    /// The header defines no register at the SVD's: its SVD fields stay,
    /// read-only.
    RegisterSvdOnly,
    /// The SVD's register lies in a word the header's layout reserves:
    /// not imported.
    RegisterReserved,
    /// The header names the SVD's register otherwise: the SVD's name stays.
    RegisterName,
    /// The header declares a field the SVD lacks: imported from it.
    FieldAdded,
    /// An SVD field the header does not declare at the same bits: the
    /// header's fields stand.
    FieldDropped,
    /// Two SVD fields of one register share bits.
    FieldOverlap,
    /// The header names the SVD's field otherwise: the header's name.
    FieldRenamed,
    /// The SVD's reset value disagrees with the header's defaults.
    Reset,
    /// A header default this import cannot read.
    Default,
    /// The SVD's base disagrees with `reg_base.h`.
    Base,
    /// The SVD binds a source `interrupts.h` names or numbers otherwise.
    Interrupt,
    /// An SVD array whose elements differ: imported element by element.
    ArrayUnrolled,
    /// The overlay maps a peripheral to another header than the spec.
    OverlayHeader,
    /// An overlay entry the header contradicts: the header stands.
    OverlayConflict,
    /// An overlay entry for an entity the import does not carry.
    OverlayUnapplied,
    /// An overlay fact carried over as data with its sources.
    OverlayCarried,
}

impl FindingKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::RegisterAdded => "register-added",
            Self::RegisterMoved => "register-moved",
            Self::RegisterSvdOnly => "register-svd-only",
            Self::RegisterReserved => "register-reserved",
            Self::RegisterName => "register-name",
            Self::FieldAdded => "field-added",
            Self::FieldDropped => "field-dropped",
            Self::FieldOverlap => "field-overlap",
            Self::FieldRenamed => "field-renamed",
            Self::Reset => "reset",
            Self::Default => "default",
            Self::Base => "base",
            Self::Interrupt => "interrupt",
            Self::ArrayUnrolled => "array-unrolled",
            Self::OverlayHeader => "overlay-header",
            Self::OverlayConflict => "overlay-conflict",
            Self::OverlayUnapplied => "overlay-unapplied",
            Self::OverlayCarried => "overlay-carried",
        }
    }
}

/// Every finding of an import, sorted.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub(crate) fn add(&mut self, entity: impl Into<String>, kind: FindingKind, detail: String) {
        self.findings.push(Finding {
            entity: entity.into(),
            kind,
            detail,
        });
    }

    /// The count of each kind.
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for finding in &self.findings {
            *counts.entry(finding.kind.label()).or_default() += 1;
        }
        counts
    }

    /// The report as Markdown: the count of each kind, then every finding.
    pub fn markdown(&self) -> String {
        let mut out =
            String::from("# Register import report\n\n| Kind | Count |\n| --- | ---: |\n");
        for (kind, count) in self.counts() {
            out.push_str(&format!("| {kind} | {count} |\n"));
        }
        out.push_str("\n| Entity | Kind | Detail |\n| --- | --- | --- |\n");
        for finding in &self.findings {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                finding.entity,
                finding.kind.label(),
                finding.detail.replace('|', "\\|")
            ));
        }
        out
    }
}

/// Where a fact comes from, before evidence IDs are assigned: the SVD, the
/// instance's register header, or one cited ESP-IDF line.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Cite {
    Svd,
    Header,
    Line { path: String, line: usize },
}

pub(crate) type Cites = BTreeSet<Cite>;

/// The review of one register or field: its sources and provenance; the
/// other keys keep the type's defaults.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Review {
    pub(crate) sources: Cites,
    pub(crate) derived: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutValue {
    pub(crate) name: String,
    pub(crate) value: u32,
    pub(crate) description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutField {
    pub(crate) name: String,
    pub(crate) low: u32,
    pub(crate) width: u32,
    pub(crate) class: FieldClass,
    /// The header's access class before `debug-only` or `read-clears`
    /// refined it.
    pub(crate) header_class: FieldClass,
    pub(crate) description: Option<String>,
    pub(crate) values: Vec<OutValue>,
    pub(crate) review: Review,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutRegister {
    pub(crate) name: String,
    pub(crate) offset: u32,
    pub(crate) width: u32,
    /// `count` and `stride` of a repeated word.
    pub(crate) dim: Option<(u32, u32)>,
    pub(crate) description: Option<String>,
    /// Value and mask.
    pub(crate) reset: Option<(u32, u32)>,
    pub(crate) reserved: u32,
    pub(crate) fifo: Option<Cites>,
    pub(crate) data_port: Option<Cites>,
    pub(crate) read_side_effect: Option<Cites>,
    pub(crate) fields: Vec<OutField>,
    pub(crate) review: Review,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutCommand {
    pub(crate) register: String,
    pub(crate) commands: Vec<String>,
    pub(crate) qualifiers: Vec<String>,
    pub(crate) held: Vec<String>,
    pub(crate) sources: Cites,
}

/// One instance's block as imported.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Block {
    pub(crate) registers: Vec<OutRegister>,
    pub(crate) commands: Vec<OutCommand>,
}

impl Block {
    /// The block without its prose: what two instances of one type share.
    fn geometry(&self) -> Self {
        let mut block = self.clone();
        for register in &mut block.registers {
            register.description = None;
            for field in &mut register.fields {
                field.description = None;
                for value in &mut field.values {
                    value.description = None;
                }
            }
        }
        block
    }

    /// The bytes from the base to the end of the last word.
    fn span(&self) -> u64 {
        self.registers
            .iter()
            .map(|register| {
                let (count, stride) = register.dim.unwrap_or((1, 0));
                u64::from(register.offset)
                    + u64::from(count.saturating_sub(1)) * u64::from(stride)
                    + u64::from(register.width / 8)
            })
            .max()
            .unwrap_or(0)
    }
}

/// One imported instance in the root.
#[derive(Clone, Debug)]
pub(crate) struct OutInstance {
    pub(crate) name: String,
    pub(crate) block_type: String,
    pub(crate) base: u64,
    pub(crate) size: u64,
    pub(crate) sources: Cites,
}

/// One interrupt source bound to an imported instance.
#[derive(Clone, Debug)]
pub(crate) struct OutInterrupt {
    pub(crate) name: String,
    pub(crate) number: u16,
    pub(crate) instance: String,
    pub(crate) sources: Cites,
}

/// One imported type: its blocks' shared content and every instance's
/// register header.
pub(crate) struct OutType {
    pub(crate) spec: TypeSpec,
    pub(crate) block: Block,
    pub(crate) headers: Vec<String>,
    pub(crate) svd: bool,
}

/// Import `spec`'s blocks from the pinned files, with the facts of
/// `overlay` (the strictness overlay's text) where given.
pub fn import(
    spec: &ImportSpec,
    overlay: Option<&crate::StrictnessOverlay>,
    pinned: &dyn PinnedFiles,
) -> Result<ImportOutput, ImportError> {
    let mut report = Report::default();
    let (svd_text, svd_pin) = pinned.svd()?;
    let device = svd::parse(&svd_text)?;
    let mut bases = header::Bases::default();
    for path in &spec.bases {
        let (text, _) = pinned.idf(path)?;
        bases.add(path, &text);
    }
    let (interrupts_text, interrupts_pin) = pinned.idf(&spec.interrupts)?;
    let sources = header::parse_interrupts(&interrupts_text);
    if sources.is_empty() {
        return Err(ImportError::new(
            &spec.interrupts,
            "holds no interrupt source enumeration",
        ));
    }

    let mut types = Vec::new();
    let mut instances = Vec::new();
    let mut interrupts = Vec::new();
    for type_spec in &spec.types {
        let mut first: Option<(String, Block)> = None;
        let mut headers = Vec::new();
        let mut any_svd = false;
        for instance in &type_spec.instances {
            let registers_path = instance
                .registers
                .as_deref()
                .unwrap_or(&type_spec.registers);
            let layout_path = instance.layout.as_deref().or(type_spec.layout.as_deref());
            let (registers_text, _) = pinned.idf(registers_path)?;
            let layout = match layout_path {
                None => None,
                Some(path) => Some(pinned.idf(path)?.0),
            };
            let svd_block = match &instance.svd {
                None => None,
                Some(name) => Some(svd::block(&device, name)?),
            };
            any_svd |= svd_block.is_some();
            if let (Some(overlay), Some(name)) = (overlay, &instance.svd)
                && let Some(rule) = overlay.headers.iter().find(|rule| &rule.peripheral == name)
                && (rule.registers != registers_path || rule.layout.as_deref() != layout_path)
            {
                report.add(
                    &instance.name,
                    FindingKind::OverlayHeader,
                    format!(
                        "the overlay maps {name} to {} and {:?}; the spec to {registers_path} and {layout_path:?}",
                        rule.registers, rule.layout
                    ),
                );
            }
            let block = merge::instance(
                &merge::InstanceInputs {
                    name: &instance.name,
                    svd: svd_block.as_ref(),
                    svd_name: instance.svd.as_deref(),
                    header_path: registers_path,
                    header: &header::parse_registers(&registers_text),
                    layout: layout
                        .as_deref()
                        .map(crate::soc_overlay::parse_layout_reserved),
                    overlay,
                },
                &mut report,
            )?;
            // The base: `reg_base.h` states it; the SVD agrees or is reported.
            let (base, base_path, base_line) = bases.resolve(&instance.base).ok_or_else(|| {
                ImportError::new(
                    format!("instance {}", instance.name),
                    format!("no reg_base.h defines {}", instance.base),
                )
            })?;
            let mut instance_sources = Cites::from([Cite::Line {
                path: base_path,
                line: base_line,
            }]);
            let mut size = block.span();
            if let Some(svd_block) = &svd_block {
                instance_sources.insert(Cite::Svd);
                if svd_block.base != base {
                    report.add(
                        &instance.name,
                        FindingKind::Base,
                        format!(
                            "the SVD places it at {:#x}, {} at {base:#x}; the header's base",
                            svd_block.base, instance.base
                        ),
                    );
                }
                size = size.max(svd_block.size.unwrap_or(0));
                for (svd_name, number) in &svd_block.interrupts {
                    let number_source = sources.iter().find(|s| u32::from(s.number) == *number);
                    let Some(source) = number_source else {
                        report.add(
                            &instance.name,
                            FindingKind::Interrupt,
                            format!(
                                "the SVD binds {svd_name} = {number}, which interrupts.h does not number; not imported"
                            ),
                        );
                        continue;
                    };
                    if source.name != *svd_name {
                        report.add(
                            &instance.name,
                            FindingKind::Interrupt,
                            format!(
                                "the SVD names source {number} {svd_name}, interrupts.h {}; the header's name",
                                source.name
                            ),
                        );
                    }
                    interrupts.push(OutInterrupt {
                        name: source.name.clone(),
                        number: source.number,
                        instance: instance.name.clone(),
                        sources: Cites::from([
                            Cite::Svd,
                            Cite::Line {
                                path: spec.interrupts.clone(),
                                line: source.line,
                            },
                        ]),
                    });
                }
            }
            instances.push(OutInstance {
                name: instance.name.clone(),
                block_type: type_spec.name.clone(),
                base,
                size: size.div_ceil(4) * 4,
                sources: instance_sources,
            });
            if !headers.iter().any(|path| path == registers_path) {
                headers.push(registers_path.to_owned());
            }
            match &first {
                None => first = Some((instance.name.clone(), block)),
                Some((name, shared)) => {
                    if shared.geometry() != block.geometry() {
                        return Err(ImportError::new(
                            format!("type {}", type_spec.name),
                            format!(
                                "instances {name} and {} differ{}: list them as two types",
                                instance.name,
                                first_difference(shared, &block)
                            ),
                        ));
                    }
                }
            }
        }
        let Some((_, block)) = first else {
            return Err(ImportError::new(
                format!("type {}", type_spec.name),
                "has no instance",
            ));
        };
        types.push(OutType {
            spec: type_spec.clone(),
            block,
            headers,
            svd: any_svd,
        });
    }
    interrupts.sort_by_key(|interrupt| interrupt.number);
    report.findings.sort();

    let mut catalog = emit::Catalog::new(svd_pin, spec.esp_idf.clone(), interrupts_pin.revision);
    let mut files = BTreeMap::new();
    for out in &types {
        let path = format!(
            "{}/{}",
            spec.types_directory,
            type_file_name(&out.spec.name)
        );
        files.insert(path, emit::type_file(out, &mut catalog)?);
    }
    let root = emit::root_file(spec, &files, &instances, &interrupts, &mut catalog)?;
    let evidence = catalog.render(pinned)?;
    Ok(ImportOutput {
        root,
        types: files,
        evidence,
        report,
    })
}

/// Where two instances' blocks first differ.
fn first_difference(a: &Block, b: &Block) -> String {
    let a = a.geometry();
    let b = b.geometry();
    for (left, right) in a.registers.iter().zip(&b.registers) {
        if left != right {
            return format!(" at register {} / {}", left.name, right.name);
        }
    }
    if a.registers.len() != b.registers.len() {
        return format!(
            " in their register count ({} / {})",
            a.registers.len(),
            b.registers.len()
        );
    }
    " in their command registers".to_owned()
}

#[cfg(test)]
mod tests;
