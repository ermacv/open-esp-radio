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
mod shared_blocks;
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
    raw: PathBuf,
    api: PathBuf,
    crate_name: String,
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
            oer_registers_model::validate_pac_crate_name(&pac.crate_name)?;
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
        let mut destinations = BTreeSet::new();
        let pac = outputs
            .pac
            .as_mut()
            .map(|pac| [&mut pac.raw, &mut pac.api])
            .into_iter()
            .flatten();
        let linker = outputs.linker.as_mut().map(|linker| &mut linker.path);
        let analog = outputs.analog.as_mut();
        for p in [&mut outputs.svd, &mut outputs.bindings]
            .into_iter()
            .chain(pac)
            .chain(linker)
            .chain(analog)
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
            pac.map(|pac| pac.crate_name.as_str()),
        )?;
        let mut outputs = vec![(&self.outputs.bindings, bindings)];
        if let Some(path) = &self.outputs.analog {
            outputs.push((
                path,
                host::format(&self.model.render_analog_rust()?, "2024")?,
            ));
        }
        if let (Some(pac), Some(policy)) = (pac, &self.api) {
            let (raw, api) = self.render_pac(&svd, pac, policy)?;
            outputs.extend([(&pac.raw, raw), (&pac.api, api)]);
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

    /// The raw PAC and its API facade.
    fn render_pac(
        &self,
        svd: &str,
        pac: &PacOutputs,
        policy: &PacApiPack,
    ) -> Result<(String, String)> {
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
        let mut raw = svd2rust::generate(svd, &config)
            .map_err(|e| e.to_string())?
            .lib_rs;
        if policy.options.allow_clippy_empty_docs {
            raw.insert_str(0, "#![allow(clippy::empty_docs)]\n");
        }
        let mut raw =
            shared_blocks::reexport_library_blocks(&raw, self.model.shared_peripherals())?;
        self.model.validate_pointer_writes(policy)?;
        raw.push_str(&policy.render_rust(svd)?);
        let raw = host::format(&raw, &pac.edition)?;
        let mut api = policy.render_facade_rust()?;
        api.push_str(&self.model.render_layouts_rust(&self.memory)?);
        let api = host::format(&api, &pac.edition)?;
        Ok((raw, api))
    }
}
