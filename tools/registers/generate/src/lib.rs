//! Source-only publication from one validated reviewed model. No binary-analysis authority.

#![forbid(unsafe_code)]
use oer_registers_contracts::ApplicabilityContext;
use oer_registers_model::{PacApiPack, RegisterEvidenceSet, RegisterLintPack, RegisterModel};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub mod checks;
mod drafts;
mod host;
mod library;
pub mod signatures;
pub use drafts::{import_svd, initialize_model};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Manifest {
    schema: u32,
    model: PathBuf,
    reviewed: Vec<PathBuf>,
    memory: PathBuf,
    ownership: PathBuf,
    /// The PAC API policy; exactly a publication with a PAC has one.
    api: Option<PathBuf>,
    lints: PathBuf,
    evidence: Vec<PathBuf>,
    applicability: Context,
    outputs: Outputs,
    /// The whole vendor SVD, rendered into the PAC API's `soc` module.
    #[serde(default)]
    soc: Option<SocManifest>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct SocManifest {
    /// The vendor SVD, kept unmodified.
    svd: PathBuf,
    /// The chip's strictness overlay.
    overlay: PathBuf,
    /// The generated report of every register the conservative pass narrowed.
    report: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Context {
    ecosystems: Vec<String>,
    chip: String,
    chip_revisions: Vec<String>,
    artifact_lineages: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Outputs {
    svd: PathBuf,
    bindings: PathBuf,
    /// Native indirect vocabulary, compiled for the PAC's host tests.
    analog: Option<PathBuf>,
    /// The PAC a publication generates; a publication read only through its
    /// SVD and binding index, such as a chip's platform registers, has none.
    pac: Option<PacOutputs>,
    /// The linker fragment the chip's images include: the DMA window and the
    /// width of the address its DMA hardware reports.
    linker: Option<LinkerOutput>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct LinkerOutput {
    path: PathBuf,
    /// `PERIPHERAL.REGISTER.FIELD`: the address the hardware reports, from
    /// bit 0, whose width the link checks DMA storage against; without one
    /// no DMA window is known and the link refuses DMA storage.
    #[serde(default)]
    address_low_field: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct PacOutputs {
    /// The svd2rust crate of every peripheral the policy neither types nor
    /// leaves unexposed; a PAC whose peripherals are all typed has none.
    #[serde(default)]
    raw: Option<PathBuf>,
    api: PathBuf,
    /// The raw crate's Rust name, exactly when there is a raw crate.
    #[serde(default)]
    crate_name: Option<String>,
    target: String,
    edition: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Ownership {
    schema: u32,
    owned_ranges: Vec<String>,
}

/// The source files a chip's publication manifest names, resolved against
/// its directory, without loading or validating them: what the register
/// checks read.
pub struct ChipSources {
    /// The register model manifest (`model/device.toml`).
    pub model: PathBuf,
    /// The PAC API policy (`policy/api.toml`).
    pub api: PathBuf,
    /// The published SVD.
    pub svd: PathBuf,
    memory: PathBuf,
    ownership: PathBuf,
}

impl ChipSources {
    /// Read the publication manifest at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let base = path.parent().ok_or("manifest has no parent")?;
        let m: Manifest = toml_edit::de::from_str(&fs::read_to_string(path)?)?;
        Ok(Self {
            model: base.join(m.model),
            api: base.join(m.api.ok_or("the publication has no PAC API policy")?),
            svd: base.join(m.outputs.svd),
            memory: base.join(m.memory),
            ownership: base.join(m.ownership),
        })
    }

    /// The owned MMIO ranges of the memory map, by name, as `start..end`.
    pub fn owned_mmio(&self) -> Result<Vec<(String, u64, u64)>> {
        let memory = oer_registers_model::memory::Memory::load(&self.memory)?;
        let ownership: Ownership = toml_edit::de::from_str(&fs::read_to_string(&self.ownership)?)?;
        ownership
            .owned_ranges
            .iter()
            .map(|name| {
                memory
                    .mmio(name)
                    .map(|(start, end)| (name.clone(), start, end))
                    .ok_or_else(|| {
                        format!("owned range {name} is not an MMIO region of the memory map").into()
                    })
            })
            .collect()
    }
}

/// `svd` without the peripherals `names`: each `<peripheral>` element is
/// cut out of the text, so every other byte stays as published.
fn svd_without(svd: &str, names: &BTreeSet<&str>) -> Result<String> {
    if names.is_empty() {
        return Ok(svd.to_owned());
    }
    let document = roxmltree::Document::parse(svd)?;
    let mut cuts = Vec::new();
    for peripheral in document
        .descendants()
        .filter(|node| node.has_tag_name("peripheral"))
    {
        let name = peripheral
            .children()
            .find(|child| child.has_tag_name("name"))
            .and_then(|child| child.text())
            .unwrap_or_default();
        if names.contains(name) {
            cuts.push(peripheral.range());
        }
    }
    if cuts.len() != names.len() {
        return Err("a typed peripheral is missing from the published SVD".into());
    }
    let mut output = String::with_capacity(svd.len());
    let mut start = 0;
    for cut in cuts {
        output.push_str(svd.get(start..cut.start).ok_or("SVD cut out of range")?);
        start = cut.end;
    }
    output.push_str(svd.get(start..).ok_or("SVD cut out of range")?);
    Ok(output)
}

/// `path` from the repository root (the directory holding `chips/`), as
/// the generated `SOURCE:` lines name it; the whole path when it has none.
fn repository_path(path: &Path) -> String {
    let parts = path.components().collect::<Vec<_>>();
    parts
        .iter()
        .position(|part| part.as_os_str() == "chips")
        .map_or_else(
            || path.display().to_string(),
            |start| {
                parts[start..]
                    .iter()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            },
        )
}

/// One validated publication: a chip's register project, or a register
/// library of layouts shared between chips.
pub enum Publication {
    Chip(Box<ChipPublication>),
    Library(Box<library::LibraryPublication>),
}
impl Publication {
    /// Load a publication manifest; a manifest naming a `library` publishes a
    /// register library, any other one a chip's register project.
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        let document = text.parse::<toml_edit::DocumentMut>()?;
        if document.contains_key("library") {
            Ok(Self::Library(Box::new(library::LibraryPublication::load(
                path,
            )?)))
        } else {
            Ok(Self::Chip(Box::new(ChipPublication::load(path)?)))
        }
    }

    /// Prepare every output before checking or replacing any destination.
    pub fn generate(&self, check: bool) -> Result<()> {
        match self {
            Self::Chip(publication) => publication.generate(check),
            Self::Library(publication) => publication.generate(check),
        }
    }

    /// Full native indirect geometry, independent of PAC exposure. CPU SVD
    /// intentionally contains no aliases for these transport addresses.
    pub fn analog_inventory(&self) -> Result<serde_json::Value> {
        let Self::Chip(publication) = self else {
            return Err("a register library has no chip analog space".into());
        };
        let spaces = publication.model.analog_spaces().iter().map(|space| {
            Ok(serde_json::json!({"kind":space.kind, "blocks":space.blocks, "review":space.review, "registers":space.geometry(publication.model.chip())?}))
        }).collect::<Result<Vec<_>>>()?;
        Ok(serde_json::json!({"chip":publication.model.chip(), "spaces":spaces}))
    }
}

/// Owns validated model, policy and output destinations. Rendering never reloads inputs.
pub struct ChipPublication {
    model: RegisterModel,
    api: Option<PacApiPack>,
    /// The memory map, which supplies the RAM regions layouts name.
    memory: oer_registers_model::memory::Memory,
    outputs: Outputs,
    soc: Option<Soc>,
}
/// The loaded inputs of the `soc` module and its report's destination.
struct Soc {
    vendor_svd: String,
    /// The vendor SVD's path from the repository root, which `SOURCE:` names.
    vendor_path: String,
    overlay: oer_registers_model::StrictnessOverlay,
    report: PathBuf,
}
impl ChipPublication {
    /// Load explicitly selected source documents. Artifact-specific assertions cannot
    /// be authenticated by this source-only tool: no binary identities are invented.
    pub fn load(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let base = path.parent().ok_or("manifest has no parent")?;
        let m: Manifest = toml_edit::de::from_str(&fs::read_to_string(&path)?)?;
        if m.schema != 1 || m.reviewed.is_empty() || m.evidence.is_empty() {
            return Err("schema 1 requires explicit reviewed packs and evidence catalogs".into());
        }
        let context = ApplicabilityContext::new(
            m.applicability.ecosystems,
            vec![m.applicability.chip],
            m.applicability.chip_revisions,
            m.applicability.artifact_lineages,
            vec![],
        )?;
        let mut model = RegisterModel::load_for_publication(&base.join(&m.model))?;
        if context.chips != [model.chip()] {
            return Err("publication chip differs from register model".into());
        }
        let paths = |items: &[PathBuf]| items.iter().map(|p| base.join(p)).collect::<Vec<_>>();
        let review = oer_registers_review::ReviewKnowledge::load_all(&paths(&m.reviewed))?
            .select_for(&context)?;
        model.apply_review_knowledge(&review)?;
        if m.api.is_some() != m.outputs.pac.is_some() {
            return Err(
                "a publication has a PAC API policy exactly when it generates a PAC".into(),
            );
        }
        let api = m
            .api
            .as_ref()
            .map(|api| PacApiPack::load(&base.join(api)))
            .transpose()?;
        let lints = RegisterLintPack::load(&base.join(&m.lints))?;
        model.validate_lints(&lints)?;
        let evidence = RegisterEvidenceSet::load_all(&paths(&m.evidence))?;
        evidence.validate_references(
            "model review",
            model
                .review()
                .iter()
                .flat_map(|r| r.sources.iter().map(String::as_str))
                .chain(
                    model
                        .reviewed_register_facts()
                        .iter()
                        .flat_map(|a| a.metadata.evidence.iter().map(|e| e.source.as_str())),
                ),
        )?;
        evidence.validate_references("reserved register bits", model.reserved_sources())?;
        evidence.validate_references(
            "analog-I2C model",
            model.analog_spaces().iter().flat_map(|s| s.sources()),
        )?;
        evidence.validate_references(
            "memory layout review",
            model
                .layouts()
                .iter()
                .flat_map(|layout| &layout.review)
                .flat_map(|r| r.sources.iter().map(String::as_str)),
        )?;
        if (!model.layouts().is_empty() || !model.analog_spaces().is_empty())
            && m.outputs.pac.is_none()
        {
            return Err(
                "a model with memory layouts or analog spaces publishes them through a PAC".into(),
            );
        }
        let api_sources = api.iter().flat_map(PacApiPack::source_ids);
        evidence.validate_references("PAC API", api_sources)?;
        if let Some(peripheral) = api
            .iter()
            .flat_map(PacApiPack::operation_peripherals)
            .into_iter()
            .find(|peripheral| model.shared_peripherals().contains_key(*peripheral))
        {
            return Err(format!(
                "PAC API defines a transaction on {peripheral}, whose layout a register library owns; define it in the library's API"
            )
            .into());
        }
        let memory = oer_registers_model::memory::Memory::load(&base.join(&m.memory))?;
        let ownership: Ownership =
            toml_edit::de::from_str(&fs::read_to_string(base.join(&m.ownership))?)?;
        if ownership.schema != 1 || ownership.owned_ranges.is_empty() {
            return Err("invalid ownership policy".into());
        }
        memory.validate(&model, &ownership.owned_ranges, &evidence)?;
        let (svd, _) = model.render_svd()?;
        if let Some(api) = &api {
            api.validate_against_svd(&svd)?;
        }
        let mut outputs = m.outputs;
        if model.analog_spaces().is_empty() != outputs.analog.is_none() {
            return Err("an analog model requires exactly one native analog output".into());
        }
        if let Some(pac) = &outputs.pac {
            if !matches!(pac.target.as_str(), "none" | "riscv")
                || !matches!(pac.edition.as_str(), "2021" | "2024")
            {
                return Err("unsupported PAC target or edition".into());
            }
            match (&pac.raw, &pac.crate_name) {
                (Some(_), Some(crate_name)) => {
                    oer_registers_model::validate_pac_crate_name(crate_name)?;
                }
                (None, None) => {}
                _ => return Err("a PAC names its raw crate exactly when it has one".into()),
            }
        }
        let mut input_paths: BTreeSet<_> = model.loaded_inputs().keys().cloned().collect();
        for p in m
            .reviewed
            .iter()
            .chain(m.evidence.iter())
            .chain(m.api.iter())
            .chain([&m.lints, &m.memory, &m.ownership])
        {
            input_paths.insert(base.join(p).canonicalize()?);
        }
        input_paths.insert(path.clone());
        let mut soc = match &m.soc {
            None => None,
            Some(soc) => {
                if outputs.pac.is_none() {
                    return Err("the vendor SVD renders into a PAC".into());
                }
                let svd = base.join(&soc.svd).canonicalize()?;
                let overlay = base.join(&soc.overlay).canonicalize()?;
                input_paths.insert(svd.clone());
                input_paths.insert(overlay.clone());
                Some(Soc {
                    vendor_svd: fs::read_to_string(&svd)?,
                    vendor_path: repository_path(&svd),
                    overlay: oer_registers_model::StrictnessOverlay::load(&overlay)?,
                    report: base.join(&soc.report),
                })
            }
        };
        let mut destinations = BTreeSet::new();
        let pac = outputs
            .pac
            .as_mut()
            .map(|pac| [Some(&mut pac.api), pac.raw.as_mut()].into_iter().flatten())
            .into_iter()
            .flatten();
        let linker = outputs.linker.as_mut().map(|linker| &mut linker.path);
        let analog = outputs.analog.as_mut();
        let report = soc.as_mut().map(|soc| &mut soc.report);
        for p in [&mut outputs.svd, &mut outputs.bindings]
            .into_iter()
            .chain(pac)
            .chain(linker)
            .chain(analog)
            .chain(report)
        {
            *p = host::destination(&base.join(&*p))?;
            if input_paths.contains(p) || !destinations.insert(p.clone()) {
                return Err("publication output aliases an input or another output".into());
            }
        }
        Ok(Self {
            model,
            api,
            memory,
            outputs,
            soc,
        })
    }

    /// Prepare every output before checking or replacing any destination.
    /// Check is read-only. Each replacement is atomic; a failed batch is reported
    /// as incomplete rather than claiming cross-directory atomicity.
    pub fn generate(&self, check: bool) -> Result<()> {
        let (svd, _) = self.model.render_svd()?;
        let pac = self.outputs.pac.as_ref();
        let bindings = oer_registers_model::generate_binding_index(
            &svd,
            pac.and_then(|pac| pac.crate_name.as_deref()),
        )?;
        let mut outputs = vec![(&self.outputs.bindings, bindings)];
        if let Some(path) = &self.outputs.analog {
            outputs.push((
                path,
                host::format(&self.model.render_analog_rust()?, "2024")?,
            ));
        }
        if let (Some(pac), Some(policy)) = (pac, &self.api) {
            let soc = match &self.soc {
                None => None,
                Some(soc) => {
                    let curated_typed = policy.typed_peripherals();
                    let rendered =
                        oer_registers_model::render_soc(&oer_registers_model::SocInputs {
                            vendor_svd: &soc.vendor_svd,
                            vendor_path: &soc.vendor_path,
                            overlay: &soc.overlay,
                            curated_svd: &svd,
                            curated_typed: &curated_typed,
                        })?;
                    outputs.push((&soc.report, rendered.report));
                    Some(rendered.rust)
                }
            };
            let (raw, api) = self.render_pac(&svd, pac, policy, soc.as_deref())?;
            if let (Some(destination), Some(raw)) = (&pac.raw, raw) {
                outputs.push((destination, raw));
            }
            outputs.push((&pac.api, api));
        }
        if let Some(linker) = &self.outputs.linker {
            let fragment = oer_registers_model::render_dma_linker_fragment(
                &svd,
                &self.memory,
                linker.address_low_field.as_deref(),
            )?;
            outputs.push((&linker.path, fragment));
        }
        outputs.push((&self.outputs.svd, svd));
        host::publish(&outputs, check)
    }

    /// The raw PAC, if the publication has one, and the PAC API module.
    fn render_pac(
        &self,
        svd: &str,
        pac: &PacOutputs,
        policy: &PacApiPack,
        soc: Option<&str>,
    ) -> Result<(Option<String>, String)> {
        let raw = match &pac.raw {
            Some(_) => Some(self.render_raw_pac(svd, pac, policy)?),
            None => {
                let rendered = policy.typed_peripherals();
                let unexposed = policy
                    .options
                    .unexposed_peripherals
                    .iter()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>();
                let document = roxmltree::Document::parse(svd)?;
                if let Some(name) = document
                    .descendants()
                    .filter(|node| node.has_tag_name("peripheral"))
                    .filter_map(|peripheral| {
                        peripheral
                            .children()
                            .find(|child| child.has_tag_name("name"))
                            .and_then(|child| child.text())
                    })
                    .find(|name| !rendered.contains(name) && !unexposed.contains(name))
                {
                    return Err(format!(
                        "PAC without a raw crate leaves {name} neither typed nor unexposed"
                    )
                    .into());
                }
                if policy.options.device_access {
                    return Err(
                        "a PAC without a raw crate orders device accesses through oer-mmio".into(),
                    );
                }
                None
            }
        };
        self.model.validate_pointer_writes(policy)?;
        let mut api = String::from("#![forbid(unsafe_code)]\n");
        api.push_str(&policy.without_typed_operations().render_facade_rust()?);
        api.push_str(&self.model.render_layouts_rust(&self.memory)?);
        api.push_str(&policy.render_typed_rust(svd, self.model.shared_peripherals())?);
        if let Some(soc) = soc {
            api.push_str(soc);
        }
        let api = host::format(&api, &pac.edition)?;
        Ok((raw, api))
    }

    /// The svd2rust crate of the peripherals the policy does not type.
    fn render_raw_pac(&self, svd: &str, pac: &PacOutputs, policy: &PacApiPack) -> Result<String> {
        let mut config = svd2rust::config::Config::default();
        config.target = if pac.target == "none" {
            svd2rust::Target::None
        } else {
            svd2rust::Target::RISCV
        };
        config.edition = if pac.edition == "2024" {
            svd2rust::config::RustEdition::E2024
        } else {
            svd2rust::config::RustEdition::E2021
        };
        config.strict = true;
        // svd2rust renders every peripheral the policy does not render as a
        // typed register block (`typed-peripherals`).
        let mut cut = policy.typed_peripherals();
        cut.extend(
            policy
                .options
                .unexposed_peripherals
                .iter()
                .map(String::as_str),
        );
        let raw_svd = svd_without(svd, &cut)?;
        let mut raw = svd2rust::generate(&raw_svd, &config)
            .map_err(|e| e.to_string())?
            .lib_rs;
        if policy.options.allow_clippy_empty_docs {
            raw.insert_str(0, "#![allow(clippy::empty_docs)]\n");
        }
        raw.push_str(&policy.without_typed().render_rust(&raw_svd)?);
        host::format(&raw, &pac.edition)
    }
}
