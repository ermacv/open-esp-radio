//! Versioned, multi-file register model independent of discovery evidence and XML.

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    ops::Range,
    path::{Component, Path, PathBuf},
};

use oer_register_contracts::SemanticEntityId;
use oer_register_review::{AssertionValue, EffectiveAssertion, ReviewKnowledge};
use serde::{Deserialize, Serialize};
use svd_rs::{
    Access, AddressBlockUsage, Device, FieldInfo, MaybeArray, ModifiedWriteValues, Peripheral,
    RegisterCluster, RegisterInfo, RegisterProperties, ValidateLevel,
};

mod descriptions;
mod draft;
mod geometry;
mod layouts;
pub mod memory;
pub use descriptions::{description_directories, descriptions};
pub use draft::{ModelDraft, ModelInit, PeripheralInit};
mod model_validation;
pub use geometry::{FieldGeometry, PeripheralRegion, RegisterGeometry, svd_geometry, svd_regions};
pub use layouts::{LayoutAccess, LayoutField, LayoutKind, LayoutUnit, LayoutWriter, MemoryLayout};
mod pac_api;
mod pac_api_render;
mod pac_api_svd;
mod pac_bindings;
mod partition_image;
mod register_evidence;
mod register_lints;
mod shared;

pub use pac_api::{
    BitwiseComposedDomain, BitwiseComposedDomainArgument, BitwiseComposedValueType, BoundedDomain,
    EnumDomain, EnumValue, FieldOrModify, FixedRegisterImage, FixedRegisterSequence,
    FixedRegisterSequenceStep, FixedRegisterWrite, FlagDomain, FlagValue, FullRegisterRead,
    FullRegisterWrite, IndirectRegisterField, IndirectRegisterFieldDomain, InterruptSnapshot,
    MaskedRegisterModify, OpaqueDomain, OwnershipPartition, PacApiExposure, PacApiOptions,
    PacApiPack, PartitionImageRead, RegisterImageRead, RegisterImageWrite, W1cRegisterSnapshot,
    ZeroBasedFieldWrite, ZeroRegisterWrite,
};
pub use pac_bindings::{generate_binding_index, validate_pac_crate_name};
pub use partition_image::PartitionRegister;
pub use register_evidence::{
    RegisterEvidenceCatalog, RegisterEvidenceRange, RegisterEvidenceSet, RegisterEvidenceSource,
    SourceKind,
};
pub use register_lints::RegisterLintPack;
pub use shared::{
    RegisterLibrary, RegisterLibraryManifest, SharedFragmentReference, SharedPeripheral,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid register model: {0}")]
    Invalid(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Toml(#[from] toml_edit::TomlError),

    #[error(transparent)]
    TomlDeserialize(#[from] toml_edit::de::Error),

    #[error(transparent)]
    TomlSerialize(#[from] toml_edit::ser::Error),

    #[error(transparent)]
    Svd(#[from] svd_rs::SvdError),

    #[error("invalid {kind} {path}: {reason}")]
    Manifest {
        kind: &'static str,
        path: PathBuf,
        reason: String,
        span: Option<Range<usize>>,
    },
}

impl Error {
    fn message(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    fn manifest(kind: &'static str, path: &Path, error: impl std::fmt::Display) -> Self {
        Self::manifest_span(kind, path, error, None)
    }

    fn manifest_span(
        kind: &'static str,
        path: &Path,
        error: impl std::fmt::Display,
        span: Option<Range<usize>>,
    ) -> Self {
        Self::Manifest {
            kind,
            path: path.to_owned(),
            reason: error.to_string(),
            span,
        }
    }

    /// Returns source-neutral manifest diagnostic data for a presentation layer.
    pub fn manifest_diagnostic(&self) -> Option<(&'static str, &Path, &str, Option<Range<usize>>)> {
        match self {
            Self::Manifest {
                kind,
                path,
                reason,
                span,
            } => Some((*kind, path, reason, span.clone())),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SvdExportSummary {
    /// Knowledge retained by the rich model but not representable in CMSIS-SVD.
    pub projection_omissions: Vec<String>,
    pub peripherals: usize,
    pub registers: usize,
    pub fields: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterModelManifest {
    pub schema: u32,
    pub chip: String,
    #[serde(default = "default_address_space")]
    pub address_space: String,
    pub device: ModelDevice,
    #[serde(default)]
    pub fragments: Vec<String>,
    /// Library layouts this chip places at its own base addresses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_fragments: Vec<SharedFragmentReference>,
    /// Memory layouts: structures in RAM whose format the hardware defines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layouts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ModelDevice {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<svd_rs::Cpu>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_system_filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_definitions_prefix: Option<String>,
    pub address_unit_bits: u32,
    pub width: u32,
    #[serde(default)]
    pub register_defaults: RegisterProperties,
    #[serde(default = "default_svd_schema")]
    pub svd_schema: String,
    #[serde(default = "default_svd_schema_location")]
    pub svd_schema_location: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterModelFragment {
    pub schema: u32,
    pub peripherals: Vec<Peripheral>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review: Vec<ReviewAnnotation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReviewAnnotation {
    pub entity: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<oer_register_contracts::FactProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy: Option<oer_register_contracts::FactAccuracy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completeness: Option<oer_register_contracts::FactCompleteness>,
    /// Origin of a register or field name. An opaque name, and only an opaque
    /// name, ends in [`OPAQUE_SUFFIX`]; a vendor name cites its source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub naming: Option<oer_register_contracts::NameOrigin>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisterProjection {
    pub identity: String,
    pub review: Option<ReviewAnnotation>,
}

#[derive(Clone, Debug)]
pub struct RegisterModel {
    loaded_inputs: BTreeMap<std::path::PathBuf, String>,
    chip: String,
    address_space: String,
    device: Device,
    review: Vec<ReviewAnnotation>,
    reviewed_register_facts: Vec<EffectiveAssertion>,
    shared_peripherals: BTreeMap<String, SharedPeripheral>,
    layouts: Vec<MemoryLayout>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReviewApplicationSummary {
    pub matched: usize,
    pub changed: usize,
    pub ignored: usize,
    pub materialized: usize,
}

impl RegisterModel {
    /// Returns every file whose contents define this model.
    ///
    /// Consumers that cache derived register names or layouts must depend on
    /// the fragments as well as the top-level manifest.  Depending on the
    /// manifest alone leaves derived MMIO/IR documents stale after a fragment
    /// changes without changing the fragment list.
    pub fn input_paths(path: &Path) -> Result<Vec<PathBuf>> {
        let input = fs::read_to_string(path)?;
        let document = input
            .parse::<toml_edit::Document<String>>()
            .map_err(|error| {
                let span = error.span();
                Error::manifest_span("register model manifest", path, error, span)
            })?;
        let manifest: RegisterModelManifest = toml_edit::de::from_str(&input).map_err(|error| {
            let span = error.span();
            Error::manifest_span("register model manifest", path, error, span)
        })?;
        if manifest.schema != 3 {
            return Err(Error::manifest_span(
                "register model manifest",
                path,
                "requires schema = 3",
                document.get("schema").and_then(toml_edit::Item::span),
            ));
        }
        if manifest.fragments.is_empty() && manifest.shared_fragments.is_empty() {
            return Err(Error::manifest_span(
                "register model manifest",
                path,
                "requires at least one peripheral fragment",
                document.get("fragments").and_then(toml_edit::Item::span),
            ));
        }

        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut seen = BTreeSet::new();
        let mut paths = Vec::with_capacity(manifest.fragments.len() + 1);
        paths.push(path.to_owned());
        for relative in &manifest.fragments {
            validate_relative_fragment(relative)
                .map_err(|error| Error::manifest("register model manifest", path, error))?;
            if !seen.insert(relative) {
                return Err(Error::manifest(
                    "register model manifest",
                    path,
                    format!("duplicate fragment {relative:?}"),
                ));
            }
            paths.push(base.join(relative));
        }
        for reference in &manifest.shared_fragments {
            paths.extend(shared::placed_fragment_inputs(path, reference)?);
        }
        for relative in &manifest.layouts {
            validate_relative_fragment(relative)
                .map_err(|error| Error::manifest("register model manifest", path, error))?;
            paths.push(base.join(relative));
        }
        Ok(paths)
    }

    pub fn is_model_file(path: &Path) -> Result<bool> {
        let input = fs::read_to_string(path)?;
        let document = input.parse::<toml_edit::DocumentMut>().map_err(|error| {
            let span = error.span();
            Error::manifest_span("register model manifest", path, error, span)
        })?;
        Ok(document.get("schema").and_then(toml_edit::Item::as_integer) == Some(3))
    }

    /// Loads a model, draft or reviewed. Every naming a review states must be
    /// consistent, but a register or field may still lack one.
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with(path, NamingRequirement::Consistent)
    }

    /// Loads a model for publication: every register and field must carry a
    /// reviewed naming, so no published name claims more than its origin.
    pub fn load_for_publication(path: &Path) -> Result<Self> {
        Self::load_with(path, NamingRequirement::Complete)
    }

    fn load_with(path: &Path, naming: NamingRequirement) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let mut loaded_inputs = BTreeMap::from([(path.canonicalize()?, input.clone())]);
        let document = input
            .parse::<toml_edit::Document<String>>()
            .map_err(|error| {
                let span = error.span();
                Error::manifest_span("register model manifest", path, error, span)
            })?;
        let manifest: RegisterModelManifest = toml_edit::de::from_str(&input).map_err(|error| {
            let span = error.span();
            Error::manifest_span("register model manifest", path, error, span)
        })?;
        if manifest.schema != 3 {
            return Err(Error::manifest_span(
                "register model manifest",
                path,
                "requires schema = 3",
                document.get("schema").and_then(toml_edit::Item::span),
            ));
        }
        if manifest.fragments.is_empty() && manifest.shared_fragments.is_empty() {
            return Err(Error::manifest_span(
                "register model manifest",
                path,
                "requires at least one peripheral fragment",
                document.get("fragments").and_then(toml_edit::Item::span),
            ));
        }
        if manifest.address_space.is_empty() {
            return Err(Error::manifest_span(
                "register model manifest",
                path,
                "address-space must not be empty",
                document
                    .get("address-space")
                    .and_then(toml_edit::Item::span),
            ));
        }
        SemanticEntityId::register(&manifest.chip, &manifest.address_space, 0, 1)
            .map_err(|error| Error::manifest("register model manifest", path, error))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut seen_paths = BTreeSet::new();
        let mut fragments = Vec::with_capacity(manifest.fragments.len());
        let mut peripherals = Vec::new();
        for relative in &manifest.fragments {
            validate_relative_fragment(relative)
                .map_err(|error| Error::manifest("register model manifest", path, error))?;
            if !seen_paths.insert(relative) {
                return Err(Error::manifest(
                    "register model manifest",
                    path,
                    format!("duplicate fragment {relative:?}"),
                ));
            }
            let fragment_path = base.join(relative);
            let input = fs::read_to_string(&fragment_path)?;
            loaded_inputs.insert(fragment_path.canonicalize()?, input.clone());
            let document = input
                .parse::<toml_edit::Document<String>>()
                .map_err(|error| {
                    let span = error.span();
                    Error::manifest_span("register model fragment", &fragment_path, error, span)
                })?;
            let fragment: RegisterModelFragment =
                toml_edit::de::from_str(&input).map_err(|error| {
                    let span = error.span();
                    Error::manifest_span("register model fragment", &fragment_path, error, span)
                })?;
            if fragment.schema != 2 {
                return Err(Error::manifest_span(
                    "register model fragment",
                    &fragment_path,
                    "requires schema = 2",
                    document.get("schema").and_then(toml_edit::Item::span),
                ));
            }
            if fragment.peripherals.is_empty() {
                return Err(Error::manifest_span(
                    "register model fragment",
                    &fragment_path,
                    "contains no peripherals",
                    document.get("peripherals").and_then(toml_edit::Item::span),
                ));
            }
            peripherals.extend(fragment.peripherals.iter().cloned());
            fragments.push(fragment);
        }
        let mut shared_peripherals = BTreeMap::new();
        for reference in &manifest.shared_fragments {
            let (fragment, members) =
                shared::load_placed_fragment(path, reference, &mut loaded_inputs)?;
            peripherals.extend(fragment.peripherals.iter().cloned());
            fragments.push(fragment);
            shared_peripherals.extend(members);
        }
        validate_peripheral_names(&peripherals)
            .map_err(|error| Error::manifest("register model", path, error))?;
        let layouts = layouts::load_layouts(base, &manifest.layouts, &mut loaded_inputs, naming)
            .map_err(|error| match error {
                Error::Manifest { .. } => error,
                error => Error::manifest("register model manifest", path, error),
            })?;
        if let Some(layout) = layouts
            .iter()
            .find(|layout| peripherals.iter().any(|p| p.name == layout.name))
        {
            return Err(Error::manifest(
                "register model",
                path,
                format!("layout {} has the name of a peripheral", layout.name),
            ));
        }
        validate_review_annotations(&fragments, naming)
            .map_err(|error| Error::manifest("register model", path, error))?;
        let review = fragments
            .into_iter()
            .flat_map(|fragment| fragment.review)
            .collect();
        let device = build_device(&manifest.device, peripherals)
            .map_err(|error| Error::manifest("register model", path, error))?;
        model_validation::validate_device(&device)
            .map_err(|error| Error::manifest("register model", path, error))?;
        let model = Self {
            loaded_inputs,
            chip: manifest.chip,
            address_space: manifest.address_space,
            device,
            review,
            reviewed_register_facts: Vec::new(),
            shared_peripherals,
            layouts,
        };
        model
            .register_identities()
            .map_err(|error| Error::manifest("register model", path, error))?;
        Ok(model)
    }

    /// Exact input text parsed by this model, retained for execution provenance.
    pub fn loaded_inputs(&self) -> &BTreeMap<std::path::PathBuf, String> {
        &self.loaded_inputs
    }

    pub fn render_svd(&self) -> Result<(String, SvdExportSummary)> {
        let mut device = self.device.clone();
        mark_name_origins(&mut device, &self.review);
        let mut output = svd_encoder::encode(&device).map_err(|error| {
            Error::message(format!("failed to encode register model as SVD: {error}"))
        })?;
        if !output.ends_with('\n') {
            output.push('\n');
        }
        let identities = self.register_identities()?;
        let fields = self
            .device
            .peripherals
            .iter()
            .map(count_peripheral_fields)
            .sum();
        Ok((
            output,
            SvdExportSummary {
                projection_omissions: vec![
                    "SVD does not encode explicit unknown/conflicted property states; omitted side effects do not prove ordinary hardware semantics".to_owned(),
                    "Observations, candidate fields, bit dataflow, source coverage and address gaps remain in the register inventory".to_owned(),
                    format!("{} review annotations and {} effective assertions retain provenance/applicability in the rich model, outside SVD", self.review.len(), self.reviewed_register_facts.len()),
                ],
                peripherals: self.device.peripherals.len(),
                registers: identities.len(),
                fields,
            },
        ))
    }

    pub fn register_identities(&self) -> Result<BTreeMap<(u64, u32), String>> {
        let output = self
            .register_projections()?
            .into_iter()
            .map(|(key, projection)| (key, projection.identity))
            .collect::<BTreeMap<_, _>>();
        validate_expanded_register_layout(&output)?;
        Ok(output)
    }

    /// Reviewed reusable annotations keyed by the physical register instance
    /// produced by the SVD model.
    ///
    /// Array instances are associated with their structural template entity,
    /// rather than inferred with string wildcards. This prevents an unrelated
    /// singular register such as `STATUS_META` from matching `STATUS%s`.
    pub fn register_review_annotations(&self) -> Result<BTreeMap<(u64, u32), ReviewAnnotation>> {
        Ok(self
            .register_projections()?
            .into_iter()
            .filter_map(|(key, projection)| projection.review.map(|review| (key, review)))
            .collect())
    }

    /// Expanded identities and accepted reusable review annotations produced
    /// together in one structural traversal.
    pub fn register_projections(&self) -> Result<BTreeMap<(u64, u32), RegisterProjection>> {
        let annotations = self
            .review
            .iter()
            .map(|annotation| (annotation.entity.as_str(), annotation))
            .collect::<BTreeMap<_, _>>();
        Ok(self
            .register_identity_projections()?
            .into_iter()
            .map(|(key, projection)| {
                let review = annotations
                    .get(projection.template.as_str())
                    .filter(|annotation| review_annotation_is_assertion(annotation))
                    .map(|annotation| (*annotation).clone());
                (
                    key,
                    RegisterProjection {
                        identity: projection.identity,
                        review,
                    },
                )
            })
            .collect())
    }

    fn register_identity_projections(
        &self,
    ) -> Result<BTreeMap<(u64, u32), RegisterIdentityProjection>> {
        let mut output = BTreeMap::new();
        for peripheral in &self.device.peripherals {
            let (template, peripherals) = match peripheral {
                MaybeArray::Single(info) => (info.name.as_str(), vec![info.clone()]),
                MaybeArray::Array(info, dim) => (
                    info.name.as_str(),
                    svd_rs::peripheral::expand(info, dim).collect(),
                ),
            };
            for peripheral in peripherals {
                let inherited = merge_properties(
                    self.device.default_register_properties,
                    peripheral.default_register_properties,
                );
                if let Some(children) = &peripheral.registers {
                    collect_registers(
                        &mut output,
                        peripheral.base_address,
                        0,
                        inherited,
                        &peripheral.name,
                        template,
                        children,
                    )?;
                }
            }
        }
        Ok(output)
    }

    pub fn review(&self) -> &[ReviewAnnotation] {
        &self.review
    }

    /// Every reviewed register fact applied to this effective model.
    ///
    /// The retained assertion carries its effective classification,
    /// applicability and evidence. This keeps SVD projection from erasing the
    /// provenance of descriptions, access, field and write-semantics claims.
    pub fn reviewed_register_facts(&self) -> &[EffectiveAssertion] {
        &self.reviewed_register_facts
    }

    /// Peripherals whose layout comes from a register library, by name.
    /// The model's memory layouts.
    pub fn layouts(&self) -> &[MemoryLayout] {
        &self.layouts
    }

    /// The `layouts` module of the PAC API: the layouts and the RAM regions
    /// of `memory` they name. Empty for a model without layouts.
    pub fn render_layouts_rust(&self, memory: &memory::Memory) -> Result<String> {
        layouts::render(&self.layouts, &memory.ram_regions())
    }

    pub fn shared_peripherals(&self) -> &BTreeMap<String, SharedPeripheral> {
        &self.shared_peripherals
    }

    /// Stable chip identifier used by typed reviewed register assertions.
    pub fn chip(&self) -> &str {
        &self.chip
    }

    /// Stable address-space identifier used by sparse reviewed assertions.
    pub fn address_space(&self) -> &str {
        &self.address_space
    }

    /// Apply sparse reviewed register facts in deterministic dependency order.
    ///
    /// Assertions for other domains/address spaces are ignored. A known
    /// register assertion must resolve to exactly one physical register; array
    /// instances fail closed until an array-wide subject vocabulary is
    /// introduced instead of silently editing every instance.
    pub fn apply_review_knowledge(
        &mut self,
        knowledge: &ReviewKnowledge,
    ) -> Result<ReviewApplicationSummary> {
        let mut staged = self.clone();
        let summary = staged.apply_review_knowledge_in_place(knowledge)?;
        *self = staged;
        Ok(summary)
    }

    fn apply_review_knowledge_in_place(
        &mut self,
        knowledge: &ReviewKnowledge,
    ) -> Result<ReviewApplicationSummary> {
        let mut assertions = knowledge.assertions().values().collect::<Vec<_>>();
        if let Some(removed) = assertions.iter().find(|assertion| {
            matches!(
                assertion.kind.as_str(),
                "register-declaration" | "register-name"
            )
        }) {
            return Err(Error::message(format!(
                "reviewed assertion {:?} uses removed kind {:?}; register-model accepts only one register-identity assertion with canonical REGION.NAME value",
                removed.id, removed.kind
            )));
        }
        ensure_single_applicable_register_fact(&assertions, &self.chip, &self.address_space)?;
        assertions.sort_by_key(|assertion| (assertion_priority(&assertion.kind), &assertion.id));
        let identities = prepare_register_identities(&assertions, &self.chip, &self.address_space)?;
        let mut summary = ReviewApplicationSummary::default();
        for identity in &identities {
            let outcome = self.apply_reviewed_register_identity(identity)?;
            summary.matched += 1;
            summary.changed += usize::from(outcome.changed);
            summary.materialized += usize::from(outcome.materialized);
        }
        for assertion in assertions {
            if !is_register_assertion(&assertion.kind) {
                summary.ignored += 1;
                continue;
            }
            let Some(subject) = RegisterSubject::from_entity(&assertion.subject) else {
                summary.ignored += 1;
                continue;
            };
            if !subject.belongs_to(&self.chip, &self.address_space) {
                summary.ignored += 1;
                continue;
            }
            if assertion.kind == "register-identity" {
                continue;
            }
            let old_identities = self.register_identities()?;
            let old_identity = old_identities
                .get(&(subject.address, subject.width))
                .cloned()
                .ok_or_else(|| {
                    Error::message(format!(
                        "reviewed assertion {:?} targets absent register {:#010x}/{} in address space {:?}; add one explicit register-identity assertion first",
                        assertion.id, subject.address, subject.width, subject.address_space
                    ))
                })?;
            let mut changed = false;
            let mut renamed_field = None;
            let outcome = edit_register(
                &mut self.device,
                subject.address,
                subject.width,
                &mut |register| {
                    if assertion.kind == "field-name"
                        && let Some(field) = subject.field
                    {
                        renamed_field = single_field_name(register, field)?.map(str::to_owned);
                    }
                    changed = apply_register_assertion(register, &subject, assertion)?;
                    Ok(())
                },
            )?;
            match outcome {
                RegisterEditOutcome::Edited => {}
                RegisterEditOutcome::ArrayInstance => {
                    return Err(Error::message(format!(
                        "reviewed assertion {:?} targets one expanded array instance; use a reusable array-wide base model or expand it before review",
                        assertion.id
                    )));
                }
                RegisterEditOutcome::NotFound => {
                    return Err(Error::message(format!(
                        "reviewed assertion {:?} could not locate physical register {:#010x}/{}",
                        assertion.id, subject.address, subject.width
                    )));
                }
            }
            summary.matched += 1;
            self.retain_reviewed_register_fact(assertion)?;
            if changed {
                summary.changed += 1;
                if assertion.kind == "field-name"
                    && let Some(old_field) = renamed_field
                {
                    let new_field = string_value(assertion, "field name")?;
                    rewrite_review_entity_prefix(
                        &mut self.review,
                        &format!("{old_identity}.{old_field}"),
                        &format!("{old_identity}.{new_field}"),
                    );
                }
            }
        }
        model_validation::validate_device(&self.device)?;
        self.register_identities()?;
        Ok(summary)
    }

    fn apply_reviewed_register_identity(
        &mut self,
        plan: &RegisterIdentityPlan,
    ) -> Result<RegisterIdentityOutcome> {
        let identities = self.register_identities()?;
        let physical = (plan.subject.address, plan.subject.width);
        if let Some(((address, width), _)) = identities
            .iter()
            .find(|(candidate, identity)| **identity == plan.identity && **candidate != physical)
        {
            return Err(Error::message(format!(
                "reviewed assertion {:?} identity {:?} collides with physical register {address:#010x}/{width}",
                plan.assertion.id, plan.identity
            )));
        }
        let region_index = self.concrete_region_index(plan)?;
        if let Some(old_identity) = identities.get(&physical) {
            let (old_region, old_name) = old_identity.split_once('.').ok_or_else(|| {
                Error::message(format!(
                    "existing register identity {old_identity:?} is not a direct REGION.NAME identity"
                ))
            })?;
            if old_name.contains('.') {
                return Err(Error::message(format!(
                    "reviewed assertion {:?} targets register {old_identity:?} inside a cluster; register-identity accepts only a direct singular register",
                    plan.assertion.id
                )));
            }
            if old_region != plan.region {
                return Err(Error::message(format!(
                    "reviewed assertion {:?} names region {:?}, but physical register {:#010x}/{} belongs to region {:?}",
                    plan.assertion.id,
                    plan.region,
                    plan.subject.address,
                    plan.subject.width,
                    old_region
                )));
            }
            let mut changed = false;
            let outcome = edit_register(
                &mut self.device,
                plan.subject.address,
                plan.subject.width,
                &mut |register| {
                    if register.derived_from.is_some() {
                        return Err(Error::message(format!(
                            "reviewed assertion {:?} targets a derived register; register-identity requires one concrete non-derived register",
                            plan.assertion.id
                        )));
                    }
                    changed = register.name != plan.name;
                    register.name.clone_from(&plan.name);
                    Ok(())
                },
            )?;
            match outcome {
                RegisterEditOutcome::Edited => {}
                RegisterEditOutcome::ArrayInstance => {
                    return Err(Error::message(format!(
                        "reviewed assertion {:?} targets an array register instance; register-identity requires one direct singular register",
                        plan.assertion.id
                    )));
                }
                RegisterEditOutcome::NotFound => {
                    return Err(Error::message(format!(
                        "reviewed assertion {:?} could not locate existing physical register {:#010x}/{}",
                        plan.assertion.id, plan.subject.address, plan.subject.width
                    )));
                }
            }
            if changed {
                rewrite_review_entity_prefix(&mut self.review, old_identity, &plan.identity);
            }
            self.retain_reviewed_register_identity(plan)?;
            return Ok(RegisterIdentityOutcome {
                changed,
                materialized: false,
            });
        }

        if self.device.address_unit_bits != 8 {
            return Err(Error::message(format!(
                "reviewed assertion {:?} cannot materialize byte-addressed MMIO in a model with address-unit-bits = {}",
                plan.assertion.id, self.device.address_unit_bits
            )));
        }
        if !(1..=128).contains(&plan.subject.width) {
            return Err(Error::message(format!(
                "reviewed assertion {:?} has unsupported register width {}",
                plan.assertion.id, plan.subject.width
            )));
        }
        let byte_width = u64::from(plan.subject.width).div_ceil(8);
        if !plan.subject.address.is_multiple_of(byte_width) {
            return Err(Error::message(format!(
                "reviewed assertion {:?} materializes register at {:#010x}/{} that is not aligned to {byte_width} bytes",
                plan.assertion.id, plan.subject.address, plan.subject.width
            )));
        }
        plan.subject
            .address
            .checked_add(byte_width)
            .ok_or_else(|| Error::message("reviewed register identity address overflow"))?;

        let peripheral = match &self.device.peripherals[region_index] {
            MaybeArray::Single(peripheral) => peripheral,
            MaybeArray::Array(_, _) => unreachable!("concrete_region_index rejected arrays"),
        };
        let offset = plan
            .subject
            .address
            .checked_sub(peripheral.base_address)
            .ok_or_else(|| {
                Error::message(format!(
                    "reviewed assertion {:?} address {:#010x} precedes region {:?} base {:#010x}",
                    plan.assertion.id, plan.subject.address, plan.region, peripheral.base_address
                ))
            })?;
        let address_offset = u32::try_from(offset).map_err(|_| {
            Error::message(format!(
                "reviewed assertion {:?} address offset {offset:#x} does not fit the SVD region {:?}",
                plan.assertion.id, plan.region
            ))
        })?;
        let offset_end = offset
            .checked_add(byte_width)
            .ok_or_else(|| Error::message("reviewed register identity region offset overflow"))?;
        if offset_end > u64::from(u32::MAX) + 1 {
            return Err(Error::message(format!(
                "reviewed assertion {:?} register extent does not fit the SVD region {:?}",
                plan.assertion.id, plan.region
            )));
        }
        if let Some(blocks) = &peripheral.address_block
            && !blocks.iter().any(|block| {
                block.usage == AddressBlockUsage::Registers
                    && offset >= u64::from(block.offset)
                    && offset_end <= u64::from(block.offset) + u64::from(block.size)
            })
        {
            return Err(Error::message(format!(
                "reviewed assertion {:?} extent {:#x}..{offset_end:#x} lies outside register address blocks of region {:?}",
                plan.assertion.id, offset, plan.region
            )));
        }
        let inherited = merge_properties(
            self.device.default_register_properties,
            peripheral.default_register_properties,
        );
        if inherited.access.is_some() && plan.access.is_none() {
            return Err(Error::message(format!(
                "reviewed assertion {:?} would inherit hardware access from region {:?}; add an explicit register-access assertion with identical applicability",
                plan.assertion.id, plan.region
            )));
        }

        let register = RegisterInfo::builder()
            .name(plan.name.clone())
            .address_offset(address_offset)
            .size(Some(plan.subject.width))
            .build(ValidateLevel::Strict)?;
        let peripheral = match &mut self.device.peripherals[region_index] {
            MaybeArray::Single(peripheral) => peripheral,
            MaybeArray::Array(_, _) => unreachable!("array regions returned before mutation"),
        };
        peripheral
            .registers
            .get_or_insert_with(Vec::new)
            .push(RegisterCluster::Register(MaybeArray::Single(register)));
        self.retain_reviewed_register_identity(plan)?;
        Ok(RegisterIdentityOutcome {
            changed: true,
            materialized: true,
        })
    }

    fn concrete_region_index(&self, plan: &RegisterIdentityPlan) -> Result<usize> {
        let region_index = self
            .device
            .peripherals
            .iter()
            .position(|peripheral| peripheral.name == plan.region)
            .ok_or_else(|| {
                Error::message(format!(
                    "reviewed assertion {:?} names absent peripheral/region {:?}",
                    plan.assertion.id, plan.region
                ))
            })?;
        let peripheral = match &self.device.peripherals[region_index] {
            MaybeArray::Single(peripheral) => peripheral,
            MaybeArray::Array(_, _) => {
                return Err(Error::message(format!(
                    "reviewed assertion {:?} names array peripheral/region {:?}; register-identity requires one concrete singular region",
                    plan.assertion.id, plan.region
                )));
            }
        };
        if peripheral.derived_from.is_some() {
            return Err(Error::message(format!(
                "reviewed assertion {:?} names derived peripheral/region {:?}; register-identity requires a concrete non-derived region",
                plan.assertion.id, plan.region
            )));
        }
        Ok(region_index)
    }

    fn retain_reviewed_register_identity(&mut self, plan: &RegisterIdentityPlan) -> Result<()> {
        let already_retained = self
            .reviewed_register_facts
            .iter()
            .any(|assertion| assertion == &plan.assertion);
        self.retain_reviewed_register_fact(&plan.assertion)?;
        if already_retained {
            return Ok(());
        }
        let mut sources = plan
            .assertion
            .metadata
            .evidence
            .iter()
            .map(|evidence| evidence.source.clone())
            .collect::<Vec<_>>();
        sources.sort();
        sources.dedup();
        self.review.push(ReviewAnnotation {
            entity: plan.identity.clone(),
            sources,
            provenance: Some(plan.assertion.metadata.classification.provenance),
            accuracy: Some(plan.assertion.metadata.classification.accuracy),
            completeness: Some(plan.assertion.metadata.classification.completeness),
            naming: None,
        });
        Ok(())
    }

    fn retain_reviewed_register_fact(&mut self, assertion: &EffectiveAssertion) -> Result<()> {
        if let Some(existing) = self.reviewed_register_facts.iter().find(|existing| {
            existing.id == assertion.id
                || (existing.subject == assertion.subject && existing.kind == assertion.kind)
        }) {
            if existing == assertion {
                return Ok(());
            }
            return Err(Error::message(format!(
                "reviewed assertion {:?} conflicts with retained assertion {:?} for {}/{}",
                assertion.id, existing.id, assertion.subject, assertion.kind
            )));
        }
        self.reviewed_register_facts.push(assertion.clone());
        Ok(())
    }

    pub fn validate_lints(&self, pack: &RegisterLintPack) -> Result<()> {
        pack.validate_device(&self.device)
    }
}

fn ensure_single_applicable_register_fact(
    assertions: &[&EffectiveAssertion],
    chip: &str,
    address_space: &str,
) -> Result<()> {
    let mut selected = BTreeMap::<(&SemanticEntityId, &str), &str>::new();
    for assertion in assertions
        .iter()
        .copied()
        .filter(|assertion| is_register_assertion(&assertion.kind))
    {
        let subject = register_subject(assertion)?;
        if !subject.belongs_to(chip, address_space) {
            continue;
        }
        let key = (&assertion.subject, assertion.kind.as_str());
        if let Some(previous) = selected.insert(key, &assertion.id) {
            return Err(Error::message(format!(
                "reviewed assertions {previous:?} and {:?} both target {}/{}; select reviewed knowledge for one explicit applicability context before applying it",
                assertion.id, assertion.subject, assertion.kind
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegisterSubject {
    chip: String,
    address_space: String,
    address: u64,
    width: u32,
    field: Option<(u32, u32)>,
}

impl RegisterSubject {
    fn from_entity(value: &SemanticEntityId) -> Option<Self> {
        match value {
            SemanticEntityId::Register {
                chip,
                address_space,
                address,
                width,
            } => Some(Self {
                chip: chip.clone(),
                address_space: address_space.clone(),
                address: *address,
                width: *width,
                field: None,
            }),
            SemanticEntityId::RegisterField {
                chip,
                address_space,
                address,
                register_width,
                bit_offset,
                bit_width,
            } => Some(Self {
                chip: chip.clone(),
                address_space: address_space.clone(),
                address: *address,
                width: *register_width,
                field: Some((*bit_offset, *bit_width)),
            }),
            _ => None,
        }
    }

    fn belongs_to(&self, chip: &str, address_space: &str) -> bool {
        self.chip == chip && self.address_space == address_space
    }
}

#[derive(Clone, Debug)]
struct RegisterIdentityPlan {
    subject: RegisterSubject,
    region: String,
    name: String,
    identity: String,
    assertion: EffectiveAssertion,
    access: Option<EffectiveAssertion>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RegisterIdentityOutcome {
    changed: bool,
    materialized: bool,
}

fn prepare_register_identities(
    assertions: &[&EffectiveAssertion],
    chip: &str,
    address_space: &str,
) -> Result<Vec<RegisterIdentityPlan>> {
    let mut plans = Vec::new();
    let mut physical_subjects = BTreeMap::<(u64, u32), String>::new();
    for assertion in assertions
        .iter()
        .copied()
        .filter(|assertion| assertion.kind == "register-identity")
    {
        let subject = register_subject(assertion)?;
        if !subject.belongs_to(chip, address_space) {
            continue;
        }
        if subject.field.is_some() {
            return Err(Error::message(format!(
                "reviewed assertion {:?} register-identity requires a physical register subject, not register-field",
                assertion.id
            )));
        }
        if let Some(previous) =
            physical_subjects.insert((subject.address, subject.width), assertion.id.clone())
        {
            return Err(Error::message(format!(
                "reviewed assertions {previous:?} and {:?} both identify the same physical register {:#010x}/{}; select one applicability-specific pack before composing the model",
                assertion.id, subject.address, subject.width
            )));
        }
        for companion in assertions.iter().copied().filter(|candidate| {
            is_register_assertion(&candidate.kind) && candidate.id != assertion.id
        }) {
            let candidate = register_subject(companion)?;
            if candidate.chip == subject.chip
                && candidate.address_space == subject.address_space
                && candidate.address == subject.address
                && candidate.width == subject.width
                && companion.metadata.applies_to != assertion.metadata.applies_to
            {
                return Err(Error::message(format!(
                    "reviewed assertion {:?} cannot contribute to register identity {:?}: effective applicability differs for physical register {:#010x}/{}",
                    companion.id, assertion.id, subject.address, subject.width
                )));
            }
        }
        let (region, name, identity) = parse_canonical_register_identity(assertion)?;
        let access = find_companion_assertion(assertions, assertion, &subject, "register-access")?;
        plans.push(RegisterIdentityPlan {
            subject,
            region,
            name,
            identity,
            assertion: assertion.clone(),
            access: access.cloned(),
        });
    }
    Ok(plans)
}

fn find_companion_assertion<'a>(
    assertions: &'a [&EffectiveAssertion],
    identity: &EffectiveAssertion,
    subject: &RegisterSubject,
    kind: &str,
) -> Result<Option<&'a EffectiveAssertion>> {
    let mut physical_matches = Vec::new();
    for assertion in assertions
        .iter()
        .copied()
        .filter(|assertion| assertion.kind == kind)
    {
        let candidate = register_subject(assertion)?;
        if candidate == *subject {
            physical_matches.push(assertion);
        }
    }
    let matching = physical_matches
        .iter()
        .copied()
        .filter(|assertion| assertion.metadata.applies_to == identity.metadata.applies_to)
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [assertion] => Ok(Some(*assertion)),
        [] => Ok(None),
        _ => Err(Error::message(format!(
            "reviewed assertion {:?} has multiple {kind:?} companions with identical effective applicability",
            identity.id
        ))),
    }
}

fn parse_canonical_register_identity(
    assertion: &EffectiveAssertion,
) -> Result<(String, String, String)> {
    let identity = string_value(assertion, "register identity")?;
    let mut parts = identity.split('.');
    let region = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if region.is_empty() || name.is_empty() || parts.next().is_some() {
        return Err(Error::message(format!(
            "reviewed assertion {:?} register-identity value must contain exactly one dot as canonical REGION.NAME",
            assertion.id
        )));
    }
    for (label, value) in [("region", region), ("register", name)] {
        if !is_canonical_svd_identifier(value) {
            return Err(Error::message(format!(
                "reviewed assertion {:?} register-identity {label} {value:?} is not a canonical singular SVD identifier",
                assertion.id
            )));
        }
    }
    Ok((region.to_owned(), name.to_owned(), identity.to_owned()))
}

fn is_canonical_svd_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let valid_start = chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic());
    let mut has_alphanumeric = value
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric());
    valid_start
        && chars.all(|character| {
            has_alphanumeric |= character.is_ascii_alphanumeric();
            character == '_' || character.is_ascii_alphanumeric()
        })
        && has_alphanumeric
}

fn register_subject(assertion: &EffectiveAssertion) -> Result<RegisterSubject> {
    RegisterSubject::from_entity(&assertion.subject).ok_or_else(|| {
        Error::message(format!(
            "reviewed assertion {:?} kind {:?} requires a register or register-field semantic entity, got {}",
            assertion.id, assertion.kind, assertion.subject
        ))
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegisterEditOutcome {
    NotFound,
    Edited,
    ArrayInstance,
}

fn edit_register(
    device: &mut Device,
    address: u64,
    width: u32,
    edit: &mut impl FnMut(&mut RegisterInfo) -> Result<()>,
) -> Result<RegisterEditOutcome> {
    for peripheral in &mut device.peripherals {
        let outcome = match peripheral {
            MaybeArray::Single(peripheral) => edit_children(
                peripheral.base_address,
                0,
                merge_properties(
                    device.default_register_properties,
                    peripheral.default_register_properties,
                ),
                peripheral.registers.as_deref_mut().unwrap_or_default(),
                address,
                width,
                edit,
            )?,
            MaybeArray::Array(peripheral, dim) => {
                let inherited = merge_properties(
                    device.default_register_properties,
                    peripheral.default_register_properties,
                );
                let matches = (0..dim.dim).any(|index| {
                    contains_register(
                        peripheral.base_address + u64::from(index) * u64::from(dim.dim_increment),
                        0,
                        inherited,
                        peripheral.registers.as_deref().unwrap_or_default(),
                        address,
                        width,
                    )
                });
                if matches {
                    RegisterEditOutcome::ArrayInstance
                } else {
                    RegisterEditOutcome::NotFound
                }
            }
        };
        if outcome != RegisterEditOutcome::NotFound {
            return Ok(outcome);
        }
    }
    Ok(RegisterEditOutcome::NotFound)
}

fn edit_children(
    peripheral_base: u64,
    parent_offset: u64,
    inherited: RegisterProperties,
    children: &mut [RegisterCluster],
    address: u64,
    width: u32,
    edit: &mut impl FnMut(&mut RegisterInfo) -> Result<()>,
) -> Result<RegisterEditOutcome> {
    for child in children {
        let outcome = match child {
            RegisterCluster::Register(MaybeArray::Single(register)) => {
                let properties = merge_properties(inherited, register.properties);
                if physical_match(
                    peripheral_base,
                    parent_offset + u64::from(register.address_offset),
                    properties,
                    address,
                    width,
                ) {
                    edit(register)?;
                    RegisterEditOutcome::Edited
                } else {
                    RegisterEditOutcome::NotFound
                }
            }
            RegisterCluster::Register(MaybeArray::Array(register, dim)) => {
                let properties = merge_properties(inherited, register.properties);
                let matches = (0..dim.dim).any(|index| {
                    physical_match(
                        peripheral_base,
                        parent_offset
                            + u64::from(register.address_offset)
                            + u64::from(index) * u64::from(dim.dim_increment),
                        properties,
                        address,
                        width,
                    )
                });
                if matches {
                    RegisterEditOutcome::ArrayInstance
                } else {
                    RegisterEditOutcome::NotFound
                }
            }
            RegisterCluster::Cluster(MaybeArray::Single(cluster)) => edit_children(
                peripheral_base,
                parent_offset + u64::from(cluster.address_offset),
                merge_properties(inherited, cluster.default_register_properties),
                &mut cluster.children,
                address,
                width,
                edit,
            )?,
            RegisterCluster::Cluster(MaybeArray::Array(cluster, dim)) => {
                let properties = merge_properties(inherited, cluster.default_register_properties);
                let matches = (0..dim.dim).any(|index| {
                    contains_register(
                        peripheral_base,
                        parent_offset
                            + u64::from(cluster.address_offset)
                            + u64::from(index) * u64::from(dim.dim_increment),
                        properties,
                        &cluster.children,
                        address,
                        width,
                    )
                });
                if matches {
                    RegisterEditOutcome::ArrayInstance
                } else {
                    RegisterEditOutcome::NotFound
                }
            }
        };
        if outcome != RegisterEditOutcome::NotFound {
            return Ok(outcome);
        }
    }
    Ok(RegisterEditOutcome::NotFound)
}

fn contains_register(
    peripheral_base: u64,
    parent_offset: u64,
    inherited: RegisterProperties,
    children: &[RegisterCluster],
    address: u64,
    width: u32,
) -> bool {
    children.iter().any(|child| match child {
        RegisterCluster::Register(register) => match register {
            MaybeArray::Single(register) => physical_match(
                peripheral_base,
                parent_offset + u64::from(register.address_offset),
                merge_properties(inherited, register.properties),
                address,
                width,
            ),
            MaybeArray::Array(register, dim) => (0..dim.dim).any(|index| {
                physical_match(
                    peripheral_base,
                    parent_offset
                        + u64::from(register.address_offset)
                        + u64::from(index) * u64::from(dim.dim_increment),
                    merge_properties(inherited, register.properties),
                    address,
                    width,
                )
            }),
        },
        RegisterCluster::Cluster(cluster) => match cluster {
            MaybeArray::Single(cluster) => contains_register(
                peripheral_base,
                parent_offset + u64::from(cluster.address_offset),
                merge_properties(inherited, cluster.default_register_properties),
                &cluster.children,
                address,
                width,
            ),
            MaybeArray::Array(cluster, dim) => (0..dim.dim).any(|index| {
                contains_register(
                    peripheral_base,
                    parent_offset
                        + u64::from(cluster.address_offset)
                        + u64::from(index) * u64::from(dim.dim_increment),
                    merge_properties(inherited, cluster.default_register_properties),
                    &cluster.children,
                    address,
                    width,
                )
            }),
        },
    })
}

fn physical_match(
    peripheral_base: u64,
    offset: u64,
    properties: RegisterProperties,
    address: u64,
    width: u32,
) -> bool {
    peripheral_base.checked_add(offset) == Some(address) && properties.size == Some(width)
}

fn apply_register_assertion(
    register: &mut RegisterInfo,
    subject: &RegisterSubject,
    assertion: &EffectiveAssertion,
) -> Result<bool> {
    if let Some(field) = subject.field {
        return apply_field_assertion(register, field, assertion);
    }
    match assertion.kind.as_str() {
        "register-description" => {
            replace_optional_string(&mut register.description, assertion, "register description")
        }
        "register-access" => {
            let access = parse_access(assertion)?;
            let changed = register.properties.access != Some(access);
            register.properties.access = Some(access);
            Ok(changed)
        }
        "hardware-write-semantics" => {
            let semantics = parse_write_semantics(assertion)?;
            let changed = register.modified_write_values != semantics;
            register.modified_write_values = semantics;
            Ok(changed)
        }
        kind => Err(Error::message(format!(
            "reviewed assertion {:?} kind {kind:?} requires a field subject",
            assertion.id
        ))),
    }
}

fn apply_field_assertion(
    register: &mut RegisterInfo,
    (offset, width): (u32, u32),
    assertion: &EffectiveAssertion,
) -> Result<bool> {
    let fields = register.fields.get_or_insert_with(Vec::new);
    let mut matched = None;
    for (index, field) in fields.iter().enumerate() {
        match field {
            MaybeArray::Single(field)
                if field.bit_offset() == offset && field.bit_width() == width =>
            {
                matched = Some(index);
                break;
            }
            MaybeArray::Array(field, dim)
                if (0..dim.dim).any(|index| {
                    field.bit_offset() + index * dim.dim_increment == offset
                        && field.bit_width() == width
                }) =>
            {
                return Err(Error::message(format!(
                    "reviewed assertion {:?} targets one expanded field-array instance",
                    assertion.id
                )));
            }
            _ => {}
        }
    }
    if matched.is_none() && assertion.kind == "field-name" {
        let name = string_value(assertion, "field name")?;
        let field = FieldInfo::builder()
            .name(name.to_owned())
            .bit_offset(offset)
            .bit_width(width)
            .build(ValidateLevel::Strict)?;
        fields.push(MaybeArray::Single(field));
        return Ok(true);
    }
    let Some(index) = matched else {
        return Err(Error::message(format!(
            "reviewed assertion {:?} targets absent field bits {offset}/{}; add field-name first",
            assertion.id, width
        )));
    };
    let MaybeArray::Single(field) = &mut fields[index] else {
        unreachable!("field arrays returned before mutation")
    };
    match assertion.kind.as_str() {
        "field-name" => replace_string(&mut field.name, assertion, "field name"),
        "field-description" => {
            replace_optional_string(&mut field.description, assertion, "field description")
        }
        "field-access" => {
            let access = parse_access(assertion)?;
            let changed = field.access != Some(access);
            field.access = Some(access);
            Ok(changed)
        }
        "field-write-semantics" => {
            let semantics = parse_write_semantics(assertion)?;
            let changed = field.modified_write_values != semantics;
            field.modified_write_values = semantics;
            Ok(changed)
        }
        kind => Err(Error::message(format!(
            "reviewed assertion {:?} kind {kind:?} requires a register subject",
            assertion.id
        ))),
    }
}

fn single_field_name(register: &RegisterInfo, (offset, width): (u32, u32)) -> Result<Option<&str>> {
    let Some(fields) = register.fields.as_ref() else {
        return Ok(None);
    };
    for field in fields {
        match field {
            MaybeArray::Single(field)
                if field.bit_offset() == offset && field.bit_width() == width =>
            {
                return Ok(Some(&field.name));
            }
            MaybeArray::Array(field, dim)
                if (0..dim.dim).any(|index| {
                    field.bit_offset() + index * dim.dim_increment == offset
                        && field.bit_width() == width
                }) =>
            {
                return Err(Error::message(
                    "reviewed field-name targets one expanded field-array instance",
                ));
            }
            _ => {}
        }
    }
    Ok(None)
}

fn replace_string(
    target: &mut String,
    assertion: &EffectiveAssertion,
    label: &str,
) -> Result<bool> {
    let value = string_value(assertion, label)?;
    let changed = target != value;
    target.replace_range(.., value);
    Ok(changed)
}

fn replace_optional_string(
    target: &mut Option<String>,
    assertion: &EffectiveAssertion,
    label: &str,
) -> Result<bool> {
    let value = string_value(assertion, label)?;
    let changed = target.as_deref() != Some(value);
    *target = Some(value.to_owned());
    Ok(changed)
}

fn string_value<'a>(assertion: &'a EffectiveAssertion, label: &str) -> Result<&'a str> {
    let AssertionValue::String(value) = &assertion.value else {
        return Err(Error::message(format!(
            "reviewed assertion {:?} {label} must be a string",
            assertion.id
        )));
    };
    if value.trim().is_empty() {
        return Err(Error::message(format!(
            "reviewed assertion {:?} {label} must not be empty",
            assertion.id
        )));
    }
    Ok(value)
}

fn parse_access(assertion: &EffectiveAssertion) -> Result<Access> {
    let value = string_value(assertion, "access")?;
    Access::parse_str(value).ok_or_else(|| {
        Error::message(format!(
            "reviewed assertion {:?} has unsupported access {value:?}",
            assertion.id
        ))
    })
}

fn parse_write_semantics(assertion: &EffectiveAssertion) -> Result<Option<ModifiedWriteValues>> {
    let value = string_value(assertion, "write semantics")?;
    let semantics = match value {
        "unknown" => return Ok(None),
        "w1c" | "one-to-clear" => ModifiedWriteValues::OneToClear,
        "w1s" | "one-to-set" => ModifiedWriteValues::OneToSet,
        "one-to-toggle" => ModifiedWriteValues::OneToToggle,
        "zero-to-clear" => ModifiedWriteValues::ZeroToClear,
        "zero-to-set" => ModifiedWriteValues::ZeroToSet,
        "zero-to-toggle" => ModifiedWriteValues::ZeroToToggle,
        "clear" => ModifiedWriteValues::Clear,
        "set" => ModifiedWriteValues::Set,
        "modify" => ModifiedWriteValues::Modify,
        _ => {
            return Err(Error::message(format!(
                "reviewed assertion {:?} has unsupported write semantics {value:?}",
                assertion.id
            )));
        }
    };
    Ok(Some(semantics))
}

fn is_register_assertion(kind: &str) -> bool {
    matches!(
        kind,
        "register-identity"
            | "register-description"
            | "register-access"
            | "hardware-write-semantics"
            | "field-name"
            | "field-description"
            | "field-access"
            | "field-write-semantics"
    )
}

fn assertion_priority(kind: &str) -> u8 {
    match kind {
        "register-identity" => 0,
        "field-name" => 1,
        "register-description" | "field-description" => 2,
        "register-access" | "field-access" => 3,
        "hardware-write-semantics" | "field-write-semantics" => 4,
        _ => u8::MAX,
    }
}

fn rewrite_review_entity_prefix(review: &mut [ReviewAnnotation], old: &str, new: &str) {
    for annotation in review {
        if annotation.entity == old {
            annotation.entity = new.to_owned();
        } else if let Some(suffix) = annotation.entity.strip_prefix(old)
            && suffix.starts_with('.')
        {
            annotation.entity = format!("{new}{suffix}");
        }
    }
}

/// How much reviewed naming a loaded model must carry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NamingRequirement {
    /// Stated namings are consistent; unreviewed names are allowed (drafts).
    Consistent,
    /// Every register and field is reviewed (publication).
    Complete,
}

fn validate_review_annotations(
    fragments: &[RegisterModelFragment],
    naming: NamingRequirement,
) -> Result<()> {
    let mut known_entities = BTreeMap::new();
    let mut positions = BTreeMap::new();
    for peripheral in fragments.iter().flat_map(|fragment| &fragment.peripherals) {
        collect_review_entities(peripheral, &mut known_entities, &mut positions);
    }
    let mut entities = BTreeMap::new();
    for annotation in fragments.iter().flat_map(|fragment| &fragment.review) {
        if annotation.entity.is_empty()
            || entities
                .insert(annotation.entity.as_str(), annotation)
                .is_some()
        {
            return Err(Error::message(format!(
                "empty or duplicate register review entity {:?}",
                annotation.entity
            )));
        }
        let Some(kind) = known_entities.get(annotation.entity.as_str()) else {
            return Err(Error::message(format!(
                "register review entity {:?} does not exist in the model",
                annotation.entity
            )));
        };
        let classification_fields = usize::from(annotation.provenance.is_some())
            + usize::from(annotation.accuracy.is_some())
            + usize::from(annotation.completeness.is_some());
        if classification_fields != 0 && classification_fields != 3 {
            return Err(Error::message(format!(
                "register review entity {:?} must define provenance, accuracy and completeness together",
                annotation.entity
            )));
        }
        let mut sources = BTreeSet::new();
        if annotation
            .sources
            .iter()
            .any(|source| source.is_empty() || !sources.insert(source))
        {
            return Err(Error::message(format!(
                "register review entity {:?} has empty or duplicate sources",
                annotation.entity
            )));
        }
        if annotation.naming.is_some() && !kind.is_named_bits() {
            return Err(Error::message(format!(
                "register review entity {:?} is a {}; naming applies only to registers and fields",
                annotation.entity,
                kind.label()
            )));
        }
    }
    for (entity, kind) in &known_entities {
        let annotation = entities.get(entity.as_str()).copied();
        let stated = annotation.is_some_and(|annotation| annotation.naming.is_some());
        if kind.is_named_bits() && (stated || naming == NamingRequirement::Complete) {
            validate_naming(entity, annotation, &positions[entity])?;
        }
    }
    Ok(())
}

/// Suffix that marks a register or field whose meaning is not established.
pub const OPAQUE_SUFFIX: &str = "_OPAQUE";

/// Other spellings of unknown meaning; an opaque name uses [`OPAQUE_SUFFIX`].
const UNESTABLISHED_WORDS: [&str; 3] = ["UNKNOWN", "UNNAMED", "UNCLASSIFIED"];

/// Name words that record how the vendor writes bits rather than what they
/// mean: a name made of them describes no established meaning.
const WRITER_WORDS: [&[&str]; 3] = [&["SET", "BY"], &["FORCE", "ZERO"], &["FORCE", "ONE"]];

/// A name that states position or a writer's action instead of meaning:
/// a trailing register offset, bit positions alone, a writer's action, or a
/// complete written image. Such a name is opaque and ends in
/// [`OPAQUE_SUFFIX`], with the position inside it.
fn positional_reason(leaf: &str) -> Option<&'static str> {
    let words: Vec<&str> = leaf.split('_').collect();
    let offset = words.last().is_some_and(|word| {
        (3..=4).contains(&word.len())
            && word
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
            && word.chars().any(|c| c.is_ascii_digit())
    });
    if offset && words.len() > 1 {
        return Some("a register offset");
    }
    if words
        .iter()
        .all(|word| matches!(*word, "BIT" | "BITS") || word.chars().all(|c| c.is_ascii_digit()))
    {
        return Some("bit positions alone");
    }
    if WRITER_WORDS.iter().any(|pattern| {
        words
            .windows(pattern.len())
            .any(|window| window == *pattern)
    }) {
        return Some("the vendor's write action");
    }
    if matches!(words.as_slice(), ["IMAGE"] | ["INIT", "IMAGE"]) {
        return Some("a complete written image");
    }
    None
}

/// The only name an opaque register or field may carry: its position.
/// A register is `WORD_<offset>_OPAQUE` with its four-digit hexadecimal
/// offset in its parent, a field `BIT_<n>_OPAQUE` or
/// `BITS_<low>_<high>_OPAQUE`; an array inserts `_%s` before the suffix and
/// names the position of its first element.
fn opaque_position_name(position: Position, array: bool) -> String {
    let index = if array { "_%s" } else { "" };
    match position {
        Position::Register { offset } => format!("WORD_{offset:04X}{index}{OPAQUE_SUFFIX}"),
        Position::Field { offset, width: 1 } => format!("BIT_{offset}{index}{OPAQUE_SUFFIX}"),
        Position::Field { offset, width } => format!(
            "BITS_{offset}_{}{index}{OPAQUE_SUFFIX}",
            offset + width.saturating_sub(1)
        ),
    }
}

/// Where a register or field lies: a register's offset in its parent, a
/// field's lowest bit and width.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Position {
    Register { offset: u32 },
    Field { offset: u32, width: u32 },
}

/// A register or field name claims no more meaning than its origin supports:
/// every one is reviewed as vendor, descriptive or opaque; an opaque entity,
/// and only an opaque one, ends in [`OPAQUE_SUFFIX`] and is named by its
/// position (`opaque_name`), no name spells unknown meaning another way,
/// and a vendor name cites the source it was taken from. A name of ours that
/// states position or a writer's action instead of meaning is opaque.
fn validate_naming(
    entity: &str,
    annotation: Option<&ReviewAnnotation>,
    opaque_name: &str,
) -> Result<()> {
    use oer_register_contracts::NameOrigin;

    let leaf = entity.rsplit('.').next().unwrap_or(entity);
    if let Some(word) = UNESTABLISHED_WORDS
        .iter()
        .find(|word| leaf.contains(**word))
    {
        return Err(Error::message(format!(
            "register entity {entity:?} names unknown meaning with {word:?}; end it in {OPAQUE_SUFFIX:?} and review it as opaque"
        )));
    }
    let positional = opaque_name;
    let opaque_name = leaf.ends_with(OPAQUE_SUFFIX);
    if !opaque_name && leaf.contains(&OPAQUE_SUFFIX[1..]) {
        return Err(Error::message(format!(
            "register entity {entity:?} spells opaque inside its name; end it in {OPAQUE_SUFFIX:?}"
        )));
    }
    let Some(naming) = annotation.and_then(|annotation| annotation.naming) else {
        return Err(Error::message(format!(
            "register entity {entity:?} has no reviewed naming (vendor, descriptive or opaque)"
        )));
    };
    if naming == NameOrigin::Descriptive
        && let Some(reason) = positional_reason(leaf)
    {
        return Err(Error::message(format!(
            "register entity {entity:?} names {reason} instead of a meaning; name it by position with {OPAQUE_SUFFIX:?} and review it as opaque"
        )));
    }
    match Some(naming) {
        Some(NameOrigin::Opaque) if !opaque_name => Err(Error::message(format!(
            "register entity {entity:?} is reviewed as opaque but its name does not end in {OPAQUE_SUFFIX:?}"
        ))),
        Some(NameOrigin::Opaque) if leaf != positional => Err(Error::message(format!(
            "register entity {entity:?} is opaque but not named by its position; name it {positional:?}"
        ))),
        Some(NameOrigin::Vendor | NameOrigin::Descriptive) | None if opaque_name => {
            Err(Error::message(format!(
                "register entity {entity:?} ends in {OPAQUE_SUFFIX:?} but is not reviewed with naming = \"opaque\""
            )))
        }
        Some(NameOrigin::Vendor)
            if annotation.is_none_or(|annotation| annotation.sources.is_empty()) =>
        {
            Err(Error::message(format!(
                "register entity {entity:?} claims a vendor name without citing its source"
            )))
        }
        _ => Ok(()),
    }
}

/// Carry the name origin into published descriptions, which SVD consumers
/// and the generated PAC documentation show: a reader of a raw accessor must
/// see that its name is ours or that its meaning is unknown.
fn mark_name_origins(device: &mut Device, review: &[ReviewAnnotation]) {
    let naming = review
        .iter()
        .filter_map(|annotation| Some((annotation.entity.as_str(), annotation.naming?)))
        .collect::<BTreeMap<_, _>>();
    for peripheral in &mut device.peripherals {
        let name = peripheral.name.clone();
        if let Some(children) = &mut peripheral.registers {
            mark_child_name_origins(&name, children, &naming);
        }
    }
}

fn mark_child_name_origins(
    parent: &str,
    children: &mut [RegisterCluster],
    naming: &BTreeMap<&str, oer_register_contracts::NameOrigin>,
) {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let path = format!("{parent}.{}", register.name);
                mark_description(&mut register.description, naming.get(path.as_str()));
                for field in register.fields.iter_mut().flatten() {
                    let field_path = format!("{path}.{}", field.name);
                    mark_description(&mut field.description, naming.get(field_path.as_str()));
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let path = format!("{parent}.{}", cluster.name);
                mark_child_name_origins(&path, &mut cluster.children, naming);
            }
        }
    }
}

fn mark_description(
    description: &mut Option<String>,
    naming: Option<&oer_register_contracts::NameOrigin>,
) {
    use oer_register_contracts::NameOrigin;

    let marker = match naming {
        Some(NameOrigin::Opaque) => "Opaque: meaning not established.",
        Some(NameOrigin::Descriptive) => "Project-assigned name.",
        Some(NameOrigin::Vendor) | None => return,
    };
    *description = Some(match description.take() {
        Some(text) if !text.trim().is_empty() => format!("{marker} {text}"),
        _ => marker.to_owned(),
    });
}

fn review_annotation_is_assertion(annotation: &ReviewAnnotation) -> bool {
    !annotation.sources.is_empty()
        && annotation
            .provenance
            .is_some_and(|provenance| provenance != oer_register_contracts::FactProvenance::Hint)
        && annotation.accuracy.is_some()
        && annotation.completeness.is_some()
}

/// Kind of a reviewable model entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReviewEntityKind {
    Peripheral,
    Interrupt,
    Cluster,
    Register,
    Field,
    EnumeratedValue,
}

impl ReviewEntityKind {
    fn is_named_bits(self) -> bool {
        matches!(self, Self::Register | Self::Field)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Peripheral => "peripheral",
            Self::Interrupt => "interrupt",
            Self::Cluster => "cluster",
            Self::Register => "register",
            Self::Field => "field",
            Self::EnumeratedValue => "enumerated value",
        }
    }
}

/// Every reviewable entity of `peripheral` with its kind, and the opaque
/// name of each register and field in `positions`.
fn collect_review_entities(
    peripheral: &Peripheral,
    entities: &mut BTreeMap<String, ReviewEntityKind>,
    positions: &mut BTreeMap<String, String>,
) {
    entities.insert(peripheral.name.clone(), ReviewEntityKind::Peripheral);
    for interrupt in &peripheral.interrupt {
        entities.insert(
            format!("{}.interrupt.{}", peripheral.name, interrupt.name),
            ReviewEntityKind::Interrupt,
        );
    }
    if let Some(children) = &peripheral.registers {
        collect_child_review_entities(&peripheral.name, children, entities, positions);
    }
}

fn collect_child_review_entities(
    parent: &str,
    children: &[RegisterCluster],
    entities: &mut BTreeMap<String, ReviewEntityKind>,
    positions: &mut BTreeMap<String, String>,
) {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let path = format!("{parent}.{}", register.name);
                entities.insert(path.clone(), ReviewEntityKind::Register);
                positions.insert(
                    path.clone(),
                    opaque_position_name(
                        Position::Register {
                            offset: register.address_offset,
                        },
                        matches!(register, MaybeArray::Array(..)),
                    ),
                );
                if let Some(fields) = &register.fields {
                    for field in fields {
                        let field_path = format!("{path}.{}", field.name);
                        entities.insert(field_path.clone(), ReviewEntityKind::Field);
                        positions.insert(
                            field_path.clone(),
                            opaque_position_name(
                                Position::Field {
                                    offset: field.bit_offset(),
                                    width: field.bit_width(),
                                },
                                matches!(field, MaybeArray::Array(..)),
                            ),
                        );
                        for values in &field.enumerated_values {
                            for value in &values.values {
                                entities.insert(
                                    format!("{field_path}.{}", value.name),
                                    ReviewEntityKind::EnumeratedValue,
                                );
                            }
                        }
                    }
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let path = format!("{parent}.{}", cluster.name);
                entities.insert(path.clone(), ReviewEntityKind::Cluster);
                collect_child_review_entities(&path, &cluster.children, entities, positions);
            }
        }
    }
}

fn default_address_space() -> String {
    "cpu".to_owned()
}

fn default_svd_schema() -> String {
    "1.3".to_owned()
}

fn default_svd_schema_location() -> String {
    "CMSIS-SVD.xsd".to_owned()
}

pub(crate) fn validate_relative_fragment(value: &str) -> Result<()> {
    let path = PathBuf::from(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error::message(format!(
            "register model fragment must be a safe relative path: {value:?}"
        )));
    }
    Ok(())
}

pub(crate) fn validate_peripheral_names(peripherals: &[Peripheral]) -> Result<()> {
    let mut names = BTreeSet::new();
    for peripheral in peripherals {
        if !names.insert(peripheral.name.as_str()) {
            return Err(Error::message(format!(
                "duplicate register model peripheral {:?}",
                peripheral.name
            )));
        }
    }
    Ok(())
}

pub(crate) fn build_device(metadata: &ModelDevice, peripherals: Vec<Peripheral>) -> Result<Device> {
    Ok(Device::builder()
        .name(metadata.name.clone())
        .vendor(metadata.vendor.clone())
        .vendor_id(metadata.vendor_id.clone())
        .series(metadata.series.clone())
        .version(metadata.version.clone())
        .description(metadata.description.clone())
        .license_text(metadata.license_text.clone())
        .cpu(metadata.cpu.clone())
        .header_system_filename(metadata.header_system_filename.clone())
        .header_definitions_prefix(metadata.header_definitions_prefix.clone())
        .address_unit_bits(metadata.address_unit_bits)
        .width(metadata.width)
        .default_register_properties(metadata.register_defaults)
        .peripherals(peripherals)
        .schema_version(metadata.svd_schema.clone())
        .no_namespace_schema_location(metadata.svd_schema_location.clone())
        .build(ValidateLevel::Strict)?)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegisterIdentityProjection {
    identity: String,
    template: String,
}

fn collect_registers(
    output: &mut BTreeMap<(u64, u32), RegisterIdentityProjection>,
    peripheral_base: u64,
    parent_offset: u64,
    inherited: RegisterProperties,
    path: &str,
    template_path: &str,
    children: &[RegisterCluster],
) -> Result<()> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let (template_name, registers) = match register {
                    MaybeArray::Single(info) => (info.name.as_str(), vec![info.clone()]),
                    MaybeArray::Array(info, dim) => (
                        info.name.as_str(),
                        svd_rs::register::expand(info, dim).collect(),
                    ),
                };
                for register in registers {
                    let properties = merge_properties(inherited, register.properties);
                    let width = properties.size.ok_or_else(|| {
                        Error::message(format!(
                            "register {path}.{} has no inherited size",
                            register.name
                        ))
                    })?;
                    let address = peripheral_base
                        .checked_add(parent_offset)
                        .and_then(|address| address.checked_add(u64::from(register.address_offset)))
                        .ok_or_else(|| Error::message("register model address overflow"))?;
                    let identity = format!("{path}.{}", register.name);
                    let template = format!("{template_path}.{template_name}");
                    if let Some(previous) = output.insert(
                        (address, width),
                        RegisterIdentityProjection {
                            identity: identity.clone(),
                            template,
                        },
                    ) {
                        return Err(Error::message(format!(
                            "register model aliases {} and {identity} at {address:#010x}/{width}; explicit alias support is required",
                            previous.identity
                        )));
                    }
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let (template_name, clusters) = match cluster {
                    MaybeArray::Single(info) => (info.name.as_str(), vec![info.clone()]),
                    MaybeArray::Array(info, dim) => (
                        info.name.as_str(),
                        svd_rs::cluster::expand(info, dim).collect(),
                    ),
                };
                for cluster in clusters {
                    let offset = parent_offset
                        .checked_add(u64::from(cluster.address_offset))
                        .ok_or_else(|| Error::message("register cluster address overflow"))?;
                    let properties =
                        merge_properties(inherited, cluster.default_register_properties);
                    collect_registers(
                        output,
                        peripheral_base,
                        offset,
                        properties,
                        &format!("{path}.{}", cluster.name),
                        &format!("{template_path}.{template_name}"),
                        &cluster.children,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn validate_expanded_register_layout(registers: &BTreeMap<(u64, u32), String>) -> Result<()> {
    let mut previous: Option<(u64, u64, &str)> = None;
    for ((address, width), identity) in registers {
        if !(1..=128).contains(width) {
            return Err(Error::message(format!(
                "register {identity} at {address:#010x} has unsupported width {width}"
            )));
        }
        let size_bytes = u64::from(*width).div_ceil(8);
        if address % size_bytes != 0 {
            return Err(Error::message(format!(
                "register {identity} at {address:#010x} is not aligned to {size_bytes} bytes"
            )));
        }
        let end = address
            .checked_add(size_bytes)
            .ok_or_else(|| Error::message("register model address overflow"))?;
        if let Some((previous_start, previous_end, previous_identity)) = previous
            && *address < previous_end
        {
            return Err(Error::message(format!(
                "physical register ranges overlap: {previous_identity} at \
                 {previous_start:#010x}..{previous_end:#010x} and {identity} at \
                 {address:#010x}..{end:#010x}; explicit alias support is required"
            )));
        }
        previous = Some((*address, end, identity));
    }
    Ok(())
}

fn merge_properties(parent: RegisterProperties, child: RegisterProperties) -> RegisterProperties {
    let mut merged = parent;
    if child.size.is_some() {
        merged.size = child.size;
    }
    if child.access.is_some() {
        merged.access = child.access;
    }
    if child.protection.is_some() {
        merged.protection = child.protection;
    }
    if child.reset_value.is_some() {
        merged.reset_value = child.reset_value;
    }
    if child.reset_mask.is_some() {
        merged.reset_mask = child.reset_mask;
    }
    merged
}

fn count_peripheral_fields(peripheral: &Peripheral) -> usize {
    let multiplier = match peripheral {
        MaybeArray::Single(_) => 1,
        MaybeArray::Array(_, dim) => dim.dim as usize,
    };
    peripheral
        .registers
        .as_deref()
        .map(count_fields)
        .unwrap_or(0)
        * multiplier
}

fn count_fields(children: &[RegisterCluster]) -> usize {
    children
        .iter()
        .map(|child| match child {
            RegisterCluster::Register(register) => {
                let multiplier = match register {
                    MaybeArray::Single(_) => 1,
                    MaybeArray::Array(_, dim) => dim.dim as usize,
                };
                register
                    .fields
                    .as_ref()
                    .map(|fields| {
                        fields
                            .iter()
                            .map(|field| match field {
                                MaybeArray::Single(_) => 1,
                                MaybeArray::Array(_, dim) => dim.dim as usize,
                            })
                            .sum::<usize>()
                            * multiplier
                    })
                    .unwrap_or(0)
            }
            RegisterCluster::Cluster(cluster) => {
                let multiplier = match cluster {
                    MaybeArray::Single(_) => 1,
                    MaybeArray::Array(_, dim) => dim.dim as usize,
                };
                count_fields(&cluster.children) * multiplier
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_register_review::ReviewPack;

    fn parse_rendered_model(model: &RegisterModel) -> (Device, SvdExportSummary) {
        let (svd, summary) = model.render_svd().unwrap();
        let device = svd_parser::parse(&svd).unwrap();
        (device, summary)
    }

    fn find_singular_register<'a>(
        device: &'a Device,
        peripheral_name: &str,
        register_name: &str,
    ) -> Option<&'a RegisterInfo> {
        let peripheral = device
            .peripherals
            .iter()
            .find_map(|peripheral| match peripheral {
                MaybeArray::Single(peripheral) if peripheral.name == peripheral_name => {
                    Some(peripheral)
                }
                _ => None,
            })?;
        peripheral
            .registers
            .as_ref()?
            .iter()
            .find_map(|register| match register {
                RegisterCluster::Register(MaybeArray::Single(register))
                    if register.name == register_name =>
                {
                    Some(register)
                }
                _ => None,
            })
    }

    fn singular_register<'a>(
        device: &'a Device,
        peripheral_name: &str,
        register_name: &str,
    ) -> &'a RegisterInfo {
        find_singular_register(device, peripheral_name, register_name).unwrap()
    }

    fn singular_field<'a>(register: &'a RegisterInfo, field_name: &str) -> &'a FieldInfo {
        register
            .fields
            .as_ref()
            .unwrap()
            .iter()
            .find_map(|field| match field {
                MaybeArray::Single(field) if field.name == field_name => Some(field),
                _ => None,
            })
            .unwrap()
    }

    fn fixture_model(name: &str, modified_write_values: Option<&str>) -> (PathBuf, RegisterModel) {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-register-overlay-{}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("device.toml");
        fs::write(
            &manifest,
            "schema = 3\nchip = \"fixture-chip\"\naddress-space = \"cpu\"\nfragments = [\"radio.toml\"]\n\n[device]\nname = \"device\"\nversion = \"1\"\ndescription = \"device\"\naddress-unit-bits = 8\nwidth = 32\n",
        )
        .unwrap();
        let semantics = modified_write_values.map_or_else(String::new, |value| {
            format!("modifiedWriteValues = \"{value}\"\n")
        });
        fs::write(
            directory.join("radio.toml"),
            format!(
                "schema = 2\n\n[[peripherals]]\nname = \"RADIO\"\nbaseAddress = 0x1000\n\n[[peripherals.registers]]\n[peripherals.registers.register]\nname = \"STATUS\"\naddressOffset = 0\nsize = 32\naccess = \"read-write\"\n{semantics}\n[[peripherals.registers.register.fields]]\nname = \"FLAG\"\nbitOffset = 3\nbitWidth = 1\n\n[[review]]\nentity = \"RADIO.STATUS\"\nsources = [\"fixture\"]\nprovenance = \"reviewed\"\naccuracy = \"exact\"\ncompleteness = \"complete\"\nnaming = \"descriptive\"\n\n[[review]]\nentity = \"RADIO.STATUS.FLAG\"\nsources = [\"fixture\"]\nprovenance = \"reviewed\"\naccuracy = \"exact\"\ncompleteness = \"complete\"\nnaming = \"descriptive\"\n"
            ),
        )
        .unwrap();
        let model = RegisterModel::load(&manifest).unwrap();
        (directory, model)
    }

    /// A chip directory `chip/` and a library `lib/`, with the chip placing
    /// the library's `RADIO` layout.
    fn shared_fixture(name: &str, layout_extra: &str, placement: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "open-radio-register-shared-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("chip")).unwrap();
        fs::create_dir_all(root.join("lib")).unwrap();
        fs::write(
            root.join("lib/library.toml"),
            "schema = 1\nname = \"fixture\"\ncrate-name = \"fixture_pac_raw\"\nfragments = [\"radio.toml\"]\n\n[device]\nname = \"layouts\"\nversion = \"1\"\ndescription = \"layouts\"\naddress-unit-bits = 8\nwidth = 32\n",
        )
        .unwrap();
        fs::write(
            root.join("lib/radio.toml"),
            format!(
                "schema = 2\n\n[[peripherals]]\nname = \"RADIO\"\n{layout_extra}\n[[peripherals.registers]]\n[peripherals.registers.register]\nname = \"STATUS\"\naddressOffset = 4\nsize = 32\naccess = \"read-write\"\n\n[[peripherals.registers.register.fields]]\nname = \"FLAG\"\nbitOffset = 3\nbitWidth = 1\n"
            ),
        )
        .unwrap();
        fs::write(
            root.join("chip/radio.review.toml"),
            "schema = 1\n\n[[review]]\nentity = \"RADIO.STATUS\"\nsources = [\"fixture\"]\nprovenance = \"derived\"\naccuracy = \"exact\"\ncompleteness = \"partial\"\nnaming = \"descriptive\"\n\n[[review]]\nentity = \"RADIO.STATUS.FLAG\"\nnaming = \"descriptive\"\n",
        )
        .unwrap();
        let manifest = root.join("chip/device.toml");
        fs::write(
            &manifest,
            format!(
                "schema = 3\nchip = \"fixture-chip\"\naddress-space = \"cpu\"\n\n[[shared-fragments]]\nlibrary = \"../lib/library.toml\"\nfragment = \"radio.toml\"\nreview = \"radio.review.toml\"\nplacement = {{ {placement} }}\n\n[device]\nname = \"device\"\nversion = \"1\"\ndescription = \"device\"\naddress-unit-bits = 8\nwidth = 32\n"
            ),
        )
        .unwrap();
        manifest
    }

    #[test]
    fn a_chip_places_a_library_layout_with_its_own_reviews() {
        let manifest = shared_fixture("placed", "", "RADIO = 0x2000");
        let model = RegisterModel::load(&manifest).unwrap();
        let radio = &model.device.peripherals[0];
        assert_eq!(radio.base_address, 0x2000);
        assert_eq!(
            model.shared_peripherals().get("RADIO"),
            Some(&SharedPeripheral {
                library: "fixture".into(),
                crate_name: "fixture_pac_raw".into(),
            })
        );
        assert_eq!(model.review.len(), 2);
        assert_eq!(model.review[0].entity, "RADIO.STATUS");
        let inputs = RegisterModel::input_paths(&manifest).unwrap();
        assert!(inputs.iter().any(|path| path.ends_with("lib/radio.toml")));
        assert!(
            inputs
                .iter()
                .any(|path| path.ends_with("chip/radio.review.toml"))
        );
    }

    #[test]
    fn layouts_carry_neither_placement_nor_reviews() {
        let based = shared_fixture("based", "baseAddress = 0x10\n", "RADIO = 0x2000");
        let error = RegisterModel::load(&based).unwrap_err().to_string();
        assert!(error.contains("has a baseAddress"), "{error}");

        let reviewed = shared_fixture("reviewed", "", "RADIO = 0x2000");
        let layout = reviewed
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("lib/radio.toml");
        let mut text = fs::read_to_string(&layout).unwrap();
        text.push_str("\n[[review]]\nentity = \"RADIO\"\nsources = [\"fixture\"]\n");
        fs::write(&layout, text).unwrap();
        let error = RegisterModel::load(&reviewed).unwrap_err().to_string();
        assert!(error.contains("carries reviews"), "{error}");
    }

    #[test]
    fn a_placement_names_exactly_the_layout_peripherals() {
        let missing = shared_fixture("missing", "", "OTHER = 0x2000");
        let error = RegisterModel::load(&missing).unwrap_err().to_string();
        assert!(error.contains("places no peripheral \"RADIO\""), "{error}");

        let extra = shared_fixture("extra", "", "RADIO = 0x2000, OTHER = 0x3000");
        let error = RegisterModel::load(&extra).unwrap_err().to_string();
        assert!(error.contains("does not declare"), "{error}");
    }

    fn knowledge(assertions: &str) -> ReviewKnowledge {
        let pack = ReviewPack::from_toml(&format!(
            "schema = 2\nid = \"fixture.overlay\"\n[classification]\nprovenance = \"reviewed\"\naccuracy = \"exact\"\ncompleteness = \"partial\"\n{assertions}"
        ))
        .unwrap();
        ReviewKnowledge::merge([pack]).unwrap()
    }

    #[test]
    fn review_subjects_require_one_canonical_typed_spelling() {
        for subject in [
            "mmio:cpu:0x1000/32",
            "register:fixture-chip/cpu/0x01000/32",
            "register-field:fixture-chip/cpu/0x1000/32/03/1",
        ] {
            let input = format!(
                r#"schema = 2
id = "fixture.canonical-subject"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"
[[assertions]]
id = "fixture.canonical-subject.fact"
subject = "{subject}"
kind = "register-access"
value = "read-write"
[[assertions.evidence]]
source = "fixture"
locator = "manual"
"#,
            );
            let error = ReviewPack::from_toml(&input).unwrap_err();
            assert!(
                error.to_string().contains("semantic entity")
                    || error.to_string().contains("canonical"),
                "subject {subject:?}: {error}"
            );
        }
    }

    #[test]
    fn sparse_review_facts_override_names_fields_and_write_semantics() {
        let (directory, mut model) = fixture_model("apply", None);
        let knowledge = knowledge(
            r#"
[[assertions]]
id = "overlay.register.name"
subject = "register:fixture-chip/cpu/0x1000/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "fixture"
locator = "register"

[[assertions]]
id = "overlay.field.name"
subject = "register-field:fixture-chip/cpu/0x1000/32/3/1"
kind = "field-name"
value = "PENDING"
[[assertions.evidence]]
source = "fixture"
locator = "field"

[[assertions]]
id = "overlay.field.write"
subject = "register-field:fixture-chip/cpu/0x1000/32/3/1"
kind = "field-write-semantics"
value = "w1c"
[[assertions.evidence]]
source = "fixture"
locator = "write"

[[assertions]]
id = "overlay.function"
subject = "function:radio_start"
kind = "function-name"
value = "RADIO_START"
[[assertions.evidence]]
source = "fixture"
locator = "function"
"#,
        );

        let summary = model.apply_review_knowledge(&knowledge).unwrap();
        let (rendered, _) = parse_rendered_model(&model);
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(
            summary,
            ReviewApplicationSummary {
                matched: 3,
                changed: 3,
                ignored: 1,
                materialized: 0,
            }
        );
        let register = singular_register(&rendered, "RADIO", "EVENT_STATUS");
        let field = singular_field(register, "PENDING");
        assert_eq!(
            field.modified_write_values,
            Some(ModifiedWriteValues::OneToClear)
        );
        assert_eq!(model.reviewed_register_facts().len(), 3);
        assert_eq!(
            model
                .reviewed_register_facts()
                .iter()
                .map(|assertion| assertion.kind.as_str())
                .collect::<Vec<_>>(),
            ["register-identity", "field-name", "field-write-semantics"]
        );
        assert!(model.reviewed_register_facts().iter().all(|assertion| {
            assertion.metadata.classification.provenance
                == oer_register_contracts::FactProvenance::Reviewed
                && assertion.metadata.evidence.len() == 1
        }));
        assert_eq!(
            model
                .review()
                .iter()
                .map(|annotation| annotation.entity.as_str())
                .collect::<Vec<_>>(),
            [
                "RADIO.EVENT_STATUS",
                "RADIO.EVENT_STATUS.PENDING",
                "RADIO.EVENT_STATUS"
            ]
        );
    }

    #[test]
    fn reviewed_array_instances_are_bound_to_the_structural_template() {
        let (directory, mut model) = fixture_model("reviewed-array-template", None);
        let MaybeArray::Single(peripheral) = &mut model.device.peripherals[0] else {
            panic!("fixture peripheral must be singular")
        };
        let RegisterCluster::Register(MaybeArray::Single(mut register)) =
            peripheral.registers.as_mut().unwrap().remove(0)
        else {
            panic!("fixture register must be singular")
        };
        let mut collision = register.clone();
        collision.name = "STATUS_META".to_owned();
        collision.address_offset = 8;
        register.name = "STATUS%s".to_owned();
        let dim = svd_rs::DimElement::builder()
            .dim(2)
            .dim_increment(4)
            .build(ValidateLevel::Disabled)
            .unwrap();
        peripheral.registers.as_mut().unwrap().extend([
            RegisterCluster::Register(MaybeArray::Array(register, dim)),
            RegisterCluster::Register(MaybeArray::Single(collision)),
        ]);
        model.review = vec![ReviewAnnotation {
            entity: "RADIO.STATUS%s".to_owned(),
            sources: vec!["fixture-array".to_owned()],
            provenance: Some(oer_register_contracts::FactProvenance::Reviewed),
            accuracy: Some(oer_register_contracts::FactAccuracy::Exact),
            completeness: Some(oer_register_contracts::FactCompleteness::Complete),
            naming: None,
        }];

        let identities = model.register_identities().unwrap();
        let annotations = model.register_review_annotations().unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(identities[&(0x1000, 32)], "RADIO.STATUS0");
        assert_eq!(identities[&(0x1004, 32)], "RADIO.STATUS1");
        assert_eq!(identities[&(0x1008, 32)], "RADIO.STATUS_META");
        assert_eq!(annotations[&(0x1000, 32)].entity, "RADIO.STATUS%s");
        assert_eq!(annotations[&(0x1004, 32)].entity, "RADIO.STATUS%s");
        assert!(!annotations.contains_key(&(0x1008, 32)));
    }

    #[test]
    fn incomplete_and_hint_annotations_never_become_reviewed_assertions() {
        let (directory, _) = fixture_model("review-annotation-metadata", None);
        let fragment = directory.join("radio.toml");
        let input = fs::read_to_string(&fragment).unwrap();
        let input = input
            .replace("sources = [\"fixture\"]\n", "")
            .replace("provenance = \"reviewed\"\n", "")
            .replace("accuracy = \"exact\"\n", "")
            .replace("completeness = \"complete\"\n", "");
        fs::write(&fragment, input).unwrap();

        let model = RegisterModel::load(&directory.join("device.toml")).unwrap();
        assert!(model.register_review_annotations().unwrap().is_empty());

        let input = fs::read_to_string(&fragment).unwrap().replace(
            "entity = \"RADIO.STATUS\"\n",
            "entity = \"RADIO.STATUS\"\nsources = [\"fixture\"]\nprovenance = \"hint\"\naccuracy = \"exact\"\ncompleteness = \"complete\"\n",
        );
        fs::write(&fragment, input).unwrap();
        let model = RegisterModel::load(&directory.join("device.toml")).unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert!(model.register_review_annotations().unwrap().is_empty());
    }

    #[test]
    fn unselected_revision_facts_cannot_override_each_other() {
        let (directory, mut model) = fixture_model("revision-selection", None);
        let make = |id: &str, revision: &str, value: &str| {
            ReviewPack::from_toml(&format!(
                r#"schema = 2
id = "{id}"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"
[applies-to]
chip-revisions = ["{revision}"]
[[assertions]]
id = "{id}.name"
subject = "register:fixture-chip/cpu/0x1000/32"
kind = "register-identity"
value = "RADIO.{value}"
[[assertions.evidence]]
source = "fixture"
locator = "manual"
"#
            ))
            .unwrap()
        };
        let knowledge = ReviewKnowledge::merge([
            make("fixture.rev0", "rev0", "STATUS_REV0"),
            make("fixture.rev1", "rev1", "STATUS_REV1"),
        ])
        .unwrap();

        let error = model.apply_review_knowledge(&knowledge).unwrap_err();
        assert!(error.to_string().contains("explicit applicability context"));

        let selected = knowledge
            .select_for(&oer_register_contracts::ApplicabilityContext {
                chip_revisions: vec!["rev0".to_owned()],
                ..oer_register_contracts::ApplicabilityContext::default()
            })
            .unwrap();
        model.apply_review_knowledge(&selected).unwrap();
        let (rendered, _) = parse_rendered_model(&model);
        fs::remove_dir_all(directory).unwrap();
        singular_register(&rendered, "RADIO", "STATUS_REV0");
        assert!(find_singular_register(&rendered, "RADIO", "STATUS_REV1").is_none());
    }

    #[test]
    fn explicit_unknown_clears_an_unreviewed_write_semantic() {
        let (directory, mut model) = fixture_model("clear", Some("oneToClear"));
        let knowledge = knowledge(
            r#"
[[assertions]]
id = "overlay.register.write"
subject = "register:fixture-chip/cpu/0x1000/32"
kind = "hardware-write-semantics"
value = "unknown"
[[assertions.evidence]]
source = "fixture"
locator = "write"
"#,
        );

        let summary = model.apply_review_knowledge(&knowledge).unwrap();
        let (rendered, _) = parse_rendered_model(&model);
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(summary.matched, 1);
        assert_eq!(summary.changed, 1);
        assert_eq!(
            singular_register(&rendered, "RADIO", "STATUS").modified_write_values,
            None
        );
    }

    #[test]
    fn register_identity_materializes_an_absent_register_and_retains_one_assertion() {
        let (directory, mut model) = fixture_model("declare", None);
        let knowledge = knowledge(
            r#"
[[assertions]]
id = "overlay.register.identity"
subject = "register:fixture-chip/cpu/0x1004/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[assertions.applies-to]
chips = ["fixture-chip"]
chip-revisions = ["rev-a"]
[[assertions.evidence]]
source = "fixture"
locator = "identity"
"#,
        );

        let summary = model.apply_review_knowledge(&knowledge).unwrap();
        let identities = model.register_identities().unwrap();
        let (rendered, export) = parse_rendered_model(&model);
        let retained = &model.reviewed_register_facts()[0];
        let MaybeArray::Single(peripheral) = &model.device.peripherals[0] else {
            panic!("fixture peripheral must be singular")
        };
        let RegisterCluster::Register(MaybeArray::Single(register)) =
            peripheral.registers.as_ref().unwrap().last().unwrap()
        else {
            panic!("materialized register must be singular")
        };
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(
            summary,
            ReviewApplicationSummary {
                matched: 1,
                changed: 1,
                ignored: 0,
                materialized: 1,
            }
        );
        assert_eq!(
            identities.get(&(0x1004, 32)),
            Some(&"RADIO.EVENT_STATUS".to_owned())
        );
        assert_eq!(export.registers, 2);
        singular_register(&rendered, "RADIO", "EVENT_STATUS");
        assert_eq!(register.properties.size, Some(32));
        assert_eq!(register.properties.access, None);
        assert_eq!(register.modified_write_values, None);
        assert_eq!(
            retained.subject.to_string(),
            "register:fixture-chip/cpu/0x1004/32"
        );
        assert_eq!(retained.pack, "fixture.overlay");
        assert_eq!(retained.metadata.evidence[0].locator, "identity");
        assert_eq!(
            retained.metadata.applies_to.chips,
            ["fixture-chip".to_owned()]
        );
        assert_eq!(retained.id, "overlay.register.identity");
        let annotation = model
            .review()
            .iter()
            .find(|annotation| annotation.entity == "RADIO.EVENT_STATUS")
            .unwrap();
        assert_eq!(annotation.sources, ["fixture"]);
        assert_eq!(
            annotation.provenance,
            Some(oer_register_contracts::FactProvenance::Reviewed)
        );
    }

    #[test]
    fn register_identity_renames_existing_register_and_reapply_is_idempotent() {
        let (directory, mut model) = fixture_model("rename", None);
        let reviewed = knowledge(
            r#"
[[assertions]]
id = "overlay.register.identity"
subject = "register:fixture-chip/cpu/0x1000/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "fixture"
locator = "identity"
"#,
        );
        let renamed = model.apply_review_knowledge(&reviewed).unwrap();
        let noop = model.apply_review_knowledge(&reviewed).unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(renamed.matched, 1);
        assert_eq!(renamed.changed, 1);
        assert_eq!(renamed.materialized, 0);
        assert_eq!(noop.matched, 1);
        assert_eq!(noop.changed, 0);
        assert_eq!(model.reviewed_register_facts().len(), 1);
        assert_eq!(
            model
                .review()
                .iter()
                .filter(|annotation| annotation.entity == "RADIO.EVENT_STATUS")
                .count(),
            2
        );
    }

    #[test]
    fn removed_declaration_and_name_kinds_are_explicit_errors_and_atomic() {
        for kind in ["register-declaration", "register-name"] {
            let (directory, mut model) = fixture_model(kind, None);
            let before = model.register_identities().unwrap();
            let reviewed = knowledge(&format!(
                r#"
[[assertions]]
id = "overlay.removed"
subject = "register:fixture-chip/cpu/0x1004/32"
kind = "{kind}"
value = "RADIO"
[[assertions.evidence]]
source = "fixture"
locator = "removed"
"#
            ));
            let error = model.apply_review_knowledge(&reviewed).unwrap_err();
            fs::remove_dir_all(directory).unwrap();
            assert!(error.to_string().contains("uses removed kind"));
            assert!(error.to_string().contains("register-identity"));
            assert_eq!(model.register_identities().unwrap(), before);
            assert!(model.reviewed_register_facts().is_empty());
        }
    }

    #[test]
    fn register_identity_fails_closed_on_shape_subject_region_and_overlap() {
        let cases = [
            (
                "field",
                "register-field:fixture-chip/cpu/0x1004/32/0/1",
                "RADIO.NEW_REGISTER",
                "requires a physical register subject",
            ),
            (
                "alignment",
                "register:fixture-chip/cpu/0x1002/32",
                "RADIO.NEW_REGISTER",
                "not aligned",
            ),
            (
                "before",
                "register:fixture-chip/cpu/0xffc/32",
                "RADIO.NEW_REGISTER",
                "precedes region",
            ),
            (
                "region",
                "register:fixture-chip/cpu/0x1004/32",
                "MISSING.NEW_REGISTER",
                "absent peripheral/region",
            ),
            (
                "overlap",
                "register:fixture-chip/cpu/0x1002/16",
                "RADIO.NEW_REGISTER",
                "physical register ranges overlap",
            ),
            (
                "identity-collision",
                "register:fixture-chip/cpu/0x1004/32",
                "RADIO.STATUS",
                "identity \"RADIO.STATUS\" collides",
            ),
            (
                "missing-dot",
                "register:fixture-chip/cpu/0x1004/32",
                "RADIO",
                "exactly one dot",
            ),
            (
                "extra-dot",
                "register:fixture-chip/cpu/0x1004/32",
                "RADIO.BLOCK.STATUS",
                "exactly one dot",
            ),
            (
                "placeholder",
                "register:fixture-chip/cpu/0x1004/32",
                "RADIO.REG%s",
                "canonical singular SVD identifier",
            ),
            (
                "whitespace",
                "register:fixture-chip/cpu/0x1004/32",
                "RADIO.EVENT STATUS",
                "canonical singular SVD identifier",
            ),
            (
                "underscore-only-region",
                "register:fixture-chip/cpu/0x1004/32",
                "_.STATUS",
                "canonical singular SVD identifier",
            ),
            (
                "underscore-only-register",
                "register:fixture-chip/cpu/0x1004/32",
                "RADIO._",
                "canonical singular SVD identifier",
            ),
        ];
        for (name, subject, identity, expected) in cases {
            let (directory, mut model) = fixture_model(name, None);
            let reviewed = knowledge(&format!(
                r#"
[[assertions]]
id = "overlay.register.identity"
subject = "{subject}"
kind = "register-identity"
value = "{identity}"
[[assertions.evidence]]
source = "fixture"
locator = "identity"
"#
            ));

            let error = model.apply_review_knowledge(&reviewed).unwrap_err();
            fs::remove_dir_all(directory).unwrap();
            assert!(
                error.to_string().contains(expected),
                "case {name}: expected {expected:?}, got {error}"
            );
            assert_eq!(model.register_identities().unwrap().len(), 1);
        }
    }

    #[test]
    fn register_identity_respects_address_blocks_and_model_identity() {
        let (directory, mut model) = fixture_model("declare-region", None);
        assert_eq!(model.chip(), "fixture-chip");
        assert_eq!(model.address_space(), "cpu");
        let MaybeArray::Single(peripheral) = &mut model.device.peripherals[0] else {
            panic!("fixture peripheral must be singular")
        };
        peripheral.address_block = Some(vec![
            svd_rs::AddressBlock::builder()
                .offset(0)
                .size(4)
                .usage(AddressBlockUsage::Registers)
                .build(ValidateLevel::Strict)
                .unwrap(),
        ]);
        let outside = knowledge(
            r#"
[[assertions]]
id = "overlay.register.identity"
subject = "register:fixture-chip/cpu/0x1004/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "fixture"
locator = "identity"
"#,
        );
        assert!(
            model
                .apply_review_knowledge(&outside)
                .unwrap_err()
                .to_string()
                .contains("outside register address blocks")
        );

        let foreign = knowledge(
            r#"
[[assertions]]
id = "overlay.register.identity"
subject = "register:fixture-chip/other/0x1004/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "fixture"
locator = "identity"
"#,
        );
        let summary = model.apply_review_knowledge(&foreign).unwrap();
        let foreign_chip = knowledge(
            r#"
[[assertions]]
id = "overlay.foreign-chip.identity"
subject = "register:other-chip/cpu/0x1004/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "fixture"
locator = "identity"
"#,
        );
        let chip_summary = model.apply_review_knowledge(&foreign_chip).unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(summary.ignored, 1);
        assert_eq!(summary.materialized, 0);
        assert_eq!(chip_summary.ignored, 1);
        assert_eq!(chip_summary.materialized, 0);
        assert!(model.reviewed_register_facts().is_empty());
        assert_eq!(model.register_identities().unwrap().len(), 1);
    }

    #[test]
    fn register_identity_rejects_array_derived_regions_and_implicit_access() {
        let reviewed = || {
            knowledge(
                r#"
[[assertions]]
id = "overlay.register.identity"
subject = "register:fixture-chip/cpu/0x1004/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "fixture"
locator = "identity"
"#,
            )
        };

        let (array_directory, mut array_model) = fixture_model("array-region", None);
        let MaybeArray::Single(peripheral) = array_model.device.peripherals.remove(0) else {
            panic!("fixture peripheral must be singular")
        };
        let dim = svd_rs::DimElement::builder()
            .dim(2)
            .dim_increment(0x100)
            .build(ValidateLevel::Disabled)
            .unwrap();
        array_model
            .device
            .peripherals
            .push(MaybeArray::Array(peripheral, dim));
        assert!(
            array_model
                .apply_review_knowledge(&reviewed())
                .unwrap_err()
                .to_string()
                .contains("array peripheral/region")
        );
        fs::remove_dir_all(array_directory).unwrap();

        let (derived_directory, mut derived_model) = fixture_model("derived-region", None);
        let MaybeArray::Single(peripheral) = &mut derived_model.device.peripherals[0] else {
            panic!("fixture peripheral must be singular")
        };
        peripheral.derived_from = Some("RADIO_BASE".to_owned());
        assert!(
            derived_model
                .apply_review_knowledge(&reviewed())
                .unwrap_err()
                .to_string()
                .contains("derived peripheral/region")
        );
        fs::remove_dir_all(derived_directory).unwrap();

        let (access_directory, mut access_model) = fixture_model("implicit-access", None);
        let MaybeArray::Single(peripheral) = &mut access_model.device.peripherals[0] else {
            panic!("fixture peripheral must be singular")
        };
        peripheral.default_register_properties.access = Some(Access::ReadOnly);
        assert!(
            access_model
                .apply_review_knowledge(&reviewed())
                .unwrap_err()
                .to_string()
                .contains("would inherit hardware access")
        );
        fs::remove_dir_all(access_directory).unwrap();
    }

    #[test]
    fn register_identity_keeps_access_separate_and_rejects_register_arrays_and_clusters() {
        let (access_directory, mut access_model) = fixture_model("explicit-access", None);
        let MaybeArray::Single(peripheral) = &mut access_model.device.peripherals[0] else {
            panic!("fixture peripheral must be singular")
        };
        peripheral.default_register_properties.access = Some(Access::ReadOnly);
        let with_access = knowledge(
            r#"
[[assertions]]
id = "overlay.register.identity"
subject = "register:fixture-chip/cpu/0x1004/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "fixture"
locator = "identity"

[[assertions]]
id = "overlay.register.access"
subject = "register:fixture-chip/cpu/0x1004/32"
kind = "register-access"
value = "read-write"
[[assertions.evidence]]
source = "fixture"
locator = "access"
"#,
        );
        let summary = access_model.apply_review_knowledge(&with_access).unwrap();
        let (rendered, _) = parse_rendered_model(&access_model);
        fs::remove_dir_all(access_directory).unwrap();
        assert_eq!(summary.matched, 2);
        assert_eq!(summary.materialized, 1);
        assert_eq!(
            singular_register(&rendered, "RADIO", "EVENT_STATUS")
                .properties
                .access,
            Some(Access::ReadWrite)
        );

        let rename = || {
            knowledge(
                r#"
[[assertions]]
id = "overlay.register.identity"
subject = "register:fixture-chip/cpu/0x1000/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "fixture"
locator = "identity"
"#,
            )
        };
        let (array_directory, mut array_model) = fixture_model("register-array", None);
        let MaybeArray::Single(peripheral) = &mut array_model.device.peripherals[0] else {
            panic!("fixture peripheral must be singular")
        };
        let RegisterCluster::Register(MaybeArray::Single(mut register)) =
            peripheral.registers.as_mut().unwrap().remove(0)
        else {
            panic!("fixture register must be singular")
        };
        register.name = "STATUS%s".to_owned();
        let dim = svd_rs::DimElement::builder()
            .dim(2)
            .dim_increment(4)
            .build(ValidateLevel::Disabled)
            .unwrap();
        peripheral
            .registers
            .as_mut()
            .unwrap()
            .push(RegisterCluster::Register(MaybeArray::Array(register, dim)));
        assert!(
            array_model
                .apply_review_knowledge(&rename())
                .unwrap_err()
                .to_string()
                .contains("array register instance")
        );
        fs::remove_dir_all(array_directory).unwrap();

        let (cluster_directory, mut cluster_model) = fixture_model("register-cluster", None);
        let MaybeArray::Single(peripheral) = &mut cluster_model.device.peripherals[0] else {
            panic!("fixture peripheral must be singular")
        };
        let register = peripheral.registers.as_mut().unwrap().remove(0);
        let cluster = svd_rs::ClusterInfo::builder()
            .name("GROUP".to_owned())
            .address_offset(0)
            .children(vec![register])
            .build(ValidateLevel::Strict)
            .unwrap();
        peripheral
            .registers
            .as_mut()
            .unwrap()
            .push(RegisterCluster::Cluster(MaybeArray::Single(cluster)));
        assert!(
            cluster_model
                .apply_review_knowledge(&rename())
                .unwrap_err()
                .to_string()
                .contains("inside a cluster")
        );
        fs::remove_dir_all(cluster_directory).unwrap();

        let (derived_directory, mut derived_model) = fixture_model("derived-register", None);
        let MaybeArray::Single(peripheral) = &mut derived_model.device.peripherals[0] else {
            panic!("fixture peripheral must be singular")
        };
        let RegisterCluster::Register(MaybeArray::Single(register)) =
            &mut peripheral.registers.as_mut().unwrap()[0]
        else {
            panic!("fixture register must be singular")
        };
        register.derived_from = Some("RADIO_BASE.STATUS".to_owned());
        assert!(
            derived_model
                .apply_review_knowledge(&rename())
                .unwrap_err()
                .to_string()
                .contains("derived register")
        );
        fs::remove_dir_all(derived_directory).unwrap();
    }

    #[test]
    fn expanded_layout_rejects_unaligned_and_overlapping_registers() {
        let unaligned = BTreeMap::from([((0x1001, 32), "RADIO.UNALIGNED".to_owned())]);
        assert!(
            validate_expanded_register_layout(&unaligned)
                .unwrap_err()
                .to_string()
                .contains("not aligned")
        );

        let overlapping = BTreeMap::from([
            ((0x1000, 64), "RADIO.WIDE".to_owned()),
            ((0x1004, 32), "RADIO.NARROW".to_owned()),
        ]);
        assert!(
            validate_expanded_register_layout(&overlapping)
                .unwrap_err()
                .to_string()
                .contains("physical register ranges overlap")
        );
    }

    #[test]
    fn malformed_manifest_error_retains_the_input_path() {
        let path = std::env::temp_dir().join(format!(
            "open-radio-register-model-malformed-{}.toml",
            std::process::id()
        ));
        fs::write(&path, "schema = [\n").unwrap();
        let error = RegisterModel::load(&path).unwrap_err();
        fs::remove_file(&path).unwrap();

        let (kind, reported, _, span) = error.manifest_diagnostic().unwrap();
        assert_eq!(kind, "register model manifest");
        assert_eq!(reported, path);
        assert!(span.is_some());
    }

    #[test]
    fn schema_error_retains_the_exact_value_span() {
        let path = std::env::temp_dir().join(format!(
            "open-radio-register-model-schema-{}.toml",
            std::process::id()
        ));
        let input = "schema = 2\nchip = \"fixture-chip\"\nfragments = [\"peripherals.toml\"]\n\n[device]\nname = \"device\"\nversion = \"1\"\ndescription = \"device\"\naddress-unit-bits = 8\nwidth = 32\n";
        fs::write(&path, input).unwrap();
        let error = RegisterModel::load(&path).unwrap_err();
        fs::remove_file(&path).unwrap();

        let (_, _, _, span) = error.manifest_diagnostic().unwrap();
        assert_eq!(
            span.unwrap(),
            input.find('2').unwrap()..input.find('2').unwrap() + 1
        );
    }

    #[test]
    fn parsed_model_retains_the_input_that_was_loaded_before_replacement() {
        let (directory, model) = fixture_model("execution-input", None);
        let path = directory.join("radio.toml").canonicalize().unwrap();
        let original = fs::read_to_string(&path).unwrap();
        fs::write(&path, original.replace("STATUS", "CHANGED")).unwrap();
        assert_eq!(model.loaded_inputs()[&path], original);
        assert_ne!(
            RegisterModel::load(&directory.join("device.toml"))
                .unwrap()
                .loaded_inputs()[&path],
            original
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn model_inputs_include_every_fragment() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-register-model-inputs-{}",
            std::process::id()
        ));
        let path = directory.join("device.toml");
        fs::create_dir_all(directory.join("peripherals")).unwrap();
        fs::write(
            &path,
            "schema = 3\nchip = \"fixture-chip\"\nfragments = [\"peripherals/baseband.toml\", \"peripherals/agc.toml\"]\n\n[device]\nname = \"device\"\nversion = \"1\"\ndescription = \"device\"\naddress-unit-bits = 8\nwidth = 32\n",
        )
        .unwrap();

        let inputs = RegisterModel::input_paths(&path).unwrap();
        fs::remove_dir_all(&directory).unwrap();

        assert_eq!(
            inputs,
            vec![
                path,
                directory.join("peripherals/baseband.toml"),
                directory.join("peripherals/agc.toml"),
            ]
        );
    }

    /// A one-register fragment whose entities `review` does not name are
    /// reviewed as descriptive, so each test states only what it exercises.
    fn naming_fragment(register: &str, review: &str) -> RegisterModelFragment {
        let mut review = review.to_owned();
        for entity in [
            format!("RADIO.{register}"),
            format!("RADIO.{register}.LEVEL"),
        ] {
            if !review.contains(&format!("entity = \"{entity}\"\n")) {
                review.push_str(&format!(
                    "\n[[review]]\nentity = \"{entity}\"\nnaming = \"descriptive\"\n"
                ));
            }
        }
        toml_edit::de::from_str(&format!(
            "schema = 2\n\n[[peripherals]]\nname = \"RADIO\"\nbaseAddress = 0x1000\n\n[[peripherals.registers]]\n[peripherals.registers.register]\nname = \"{register}\"\naddressOffset = 0x0\nsize = 32\n\n[[peripherals.registers.register.fields]]\nname = \"LEVEL\"\nbitOffset = 0\nbitWidth = 4\n\n{review}"
        ))
        .unwrap()
    }

    fn naming_error(register: &str, review: &str) -> Option<String> {
        validate_review_annotations(
            &[naming_fragment(register, review)],
            NamingRequirement::Complete,
        )
        .err()
        .map(|error| error.to_string())
    }

    #[test]
    fn opaque_naming_and_the_opaque_suffix_require_each_other() {
        let opaque = "[[review]]\nentity = \"RADIO.WORD_0000_OPAQUE\"\nnaming = \"opaque\"\n";
        assert_eq!(naming_error("WORD_0000_OPAQUE", opaque), None);
        assert!(
            naming_error("WORD_0000_OPAQUE", "")
                .unwrap()
                .contains("not reviewed with naming = \"opaque\"")
        );
        let descriptive =
            "[[review]]\nentity = \"RADIO.CONTROL_OPAQUE\"\nnaming = \"descriptive\"\n";
        assert!(naming_error("CONTROL_OPAQUE", descriptive).is_some());
        let unsuffixed = "[[review]]\nentity = \"RADIO.CONTROL\"\nnaming = \"opaque\"\n";
        assert!(
            naming_error("CONTROL", unsuffixed)
                .unwrap()
                .contains("does not end in")
        );
    }

    #[test]
    fn every_register_and_field_is_reviewed_and_unknown_is_spelled_opaque() {
        let unreviewed = naming_fragment("CONTROL", "");
        let mut unreviewed = unreviewed;
        unreviewed
            .review
            .retain(|annotation| annotation.entity != "RADIO.CONTROL.LEVEL");
        assert!(
            validate_review_annotations(&[unreviewed.clone()], NamingRequirement::Complete)
                .unwrap_err()
                .to_string()
                .contains("has no reviewed naming")
        );
        for name in ["BIT_3_UNKNOWN", "UNNAMED_WORD", "UNCLASSIFIED_31_8"] {
            assert!(
                naming_error(name, "")
                    .unwrap()
                    .contains("names unknown meaning"),
                "{name}"
            );
        }
        assert_eq!(naming_error("CONTROL", ""), None);
        // A draft may lack naming; publication may not.
        assert!(validate_review_annotations(&[unreviewed], NamingRequirement::Consistent).is_ok());
    }

    #[test]
    fn positional_and_writer_names_are_opaque() {
        for name in [
            "INIT_BYTES_0254",
            "CONFIG_00D0",
            "BITS_5_9",
            "BIT_18",
            "SET_BY_PHY_INIT_BRANCH_BIT_18",
            "CONFIG_FORCE_ZERO_29",
            "IMAGE",
            "INIT_IMAGE",
        ] {
            assert!(
                naming_error(name, "")
                    .unwrap()
                    .contains("instead of a meaning"),
                "{name}"
            );
        }
        // Words and indices that carry meaning stay descriptive.
        for name in [
            "TSF_BITS_35_10",
            "SOURCE_17",
            "RST_MODEM_ECB",
            "RX_SETUP_IMAGE_0",
        ] {
            assert_eq!(naming_error(name, ""), None, "{name}");
        }
        // A vendor name keeps the source's spelling.
        let vendor = "[[review]]\nentity = \"RADIO.CONFIG_00D0\"\nsources = [\"VENDOR_HEADER\"]\nnaming = \"vendor\"\n";
        assert_eq!(naming_error("CONFIG_00D0", vendor), None);
        assert!(
            naming_error("OPAQUE_25", "")
                .unwrap()
                .contains("spells opaque inside")
        );
    }

    #[test]
    fn an_opaque_name_is_its_position() {
        let opaque = |entity: &str| {
            format!("[[review]]\nentity = \"RADIO.{entity}\"\nnaming = \"opaque\"\n")
        };
        // A hint of meaning, or a position other than its own, is refused
        // with the name the geometry gives.
        for name in ["NOISE_FLOOR_STATUS_OPAQUE", "WORD_0004_OPAQUE"] {
            assert!(
                naming_error(name, &opaque(name))
                    .unwrap()
                    .contains("not named by its position; name it \"WORD_0000_OPAQUE\""),
                "{name}"
            );
        }
        // LEVEL is bits 3:0 of the fixture register.
        let field = |name: &str| {
            let fragment: RegisterModelFragment = toml_edit::de::from_str(&format!(
                "schema = 2\n\n[[peripherals]]\nname = \"RADIO\"\nbaseAddress = 0x1000\n\n[[peripherals.registers]]\n[peripherals.registers.register]\nname = \"CONTROL\"\naddressOffset = 0x0\nsize = 32\n\n[[peripherals.registers.register.fields]]\nname = \"{name}\"\nbitOffset = 0\nbitWidth = 4\n\n[[peripherals.registers.register.fields]]\nname = \"BIT_4_OPAQUE\"\nbitOffset = 4\nbitWidth = 1\n\n[[peripherals.registers]]\n[peripherals.registers.register]\nname = \"WORD_0010_%s_OPAQUE\"\naddressOffset = 0x10\nsize = 32\ndim = 2\ndimIncrement = 4\ndimIndex = \"0,1\"\n\n[[review]]\nentity = \"RADIO.CONTROL\"\nnaming = \"descriptive\"\n\n{}{}{}",
                opaque(&format!("CONTROL.{name}")),
                opaque("CONTROL.BIT_4_OPAQUE"),
                opaque("WORD_0010_%s_OPAQUE"),
            ))
            .unwrap();
            validate_review_annotations(&[fragment], NamingRequirement::Complete)
                .err()
                .map(|error| error.to_string())
        };
        assert_eq!(field("BITS_0_3_OPAQUE"), None);
        assert!(
            field("LEVEL_OPAQUE")
                .unwrap()
                .contains("name it \"BITS_0_3_OPAQUE\"")
        );
    }

    #[test]
    fn drafts_still_reject_an_inconsistent_stated_naming() {
        let unsuffixed = naming_fragment(
            "CONTROL",
            "[[review]]\nentity = \"RADIO.CONTROL\"\nnaming = \"opaque\"\n",
        );
        assert!(
            validate_review_annotations(&[unsuffixed], NamingRequirement::Consistent)
                .unwrap_err()
                .to_string()
                .contains("does not end in")
        );
    }

    #[test]
    fn vendor_naming_cites_its_source() {
        let uncited = "[[review]]\nentity = \"RADIO.CONTROL.LEVEL\"\nnaming = \"vendor\"\n";
        assert!(
            naming_error("CONTROL", uncited)
                .unwrap()
                .contains("without citing its source")
        );
        let cited = "[[review]]\nentity = \"RADIO.CONTROL.LEVEL\"\nsources = [\"VENDOR_HEADER\"]\nnaming = \"vendor\"\n";
        assert_eq!(naming_error("CONTROL", cited), None);
    }

    #[test]
    fn naming_applies_only_to_registers_and_fields() {
        let peripheral = "[[review]]\nentity = \"RADIO\"\nnaming = \"descriptive\"\n";
        assert!(
            naming_error("CONTROL", peripheral)
                .unwrap()
                .contains("applies only to registers and fields")
        );
    }

    #[test]
    fn published_descriptions_carry_a_non_vendor_name_origin() {
        use oer_register_contracts::NameOrigin;

        let mut description = Some("Written by the calibration path.".to_owned());
        mark_description(&mut description, Some(&NameOrigin::Opaque));
        assert_eq!(
            description.as_deref(),
            Some("Opaque: meaning not established. Written by the calibration path.")
        );
        let mut description = None;
        mark_description(&mut description, Some(&NameOrigin::Descriptive));
        assert_eq!(description.as_deref(), Some("Project-assigned name."));
        let mut description = Some("Vendor text.".to_owned());
        mark_description(&mut description, Some(&NameOrigin::Vendor));
        assert_eq!(description.as_deref(), Some("Vendor text."));
    }
}
