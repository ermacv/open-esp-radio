//! Publication of a register library: typed register blocks of the shared
//! layouts and the reviewed transactions over them, without addresses. Each
//! chip that places a layout renders its address, claim and owner over the
//! library's block.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use oer_registers_model::{PacApiPack, RegisterEvidenceSet, RegisterLibrary};
use serde::Deserialize;

use crate::{Result, host};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Manifest {
    schema: u32,
    library: PathBuf,
    api: PathBuf,
    /// Evidence catalogs of every chip whose sources the library API cites.
    evidence: Vec<PathBuf>,
    outputs: Outputs,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Outputs {
    /// The library crate's generated module.
    pac_api: PathBuf,
    edition: String,
}

/// Validated library layouts, their API and output destinations.
pub struct LibraryPublication {
    library: RegisterLibrary,
    api: PacApiPack,
    outputs: Outputs,
}

impl LibraryPublication {
    pub fn load(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let base = path.parent().ok_or("manifest has no parent")?;
        let m: Manifest = toml_edit::de::from_str(&fs::read_to_string(&path)?)?;
        if m.schema != 1 || m.evidence.is_empty() {
            return Err("a schema 1 library publication requires evidence catalogs".into());
        }
        let library = RegisterLibrary::load(&base.join(&m.library))?;
        let api = PacApiPack::load(&base.join(&m.api))?;
        if api.ownership_partition_count() != 0 || api.options.device_access {
            return Err(
                "a register library owns no peripherals: its API declares neither ownership partitions nor device access"
                    .into(),
            );
        }
        let evidence = RegisterEvidenceSet::load_all(
            &m.evidence.iter().map(|p| base.join(p)).collect::<Vec<_>>(),
        )?;
        evidence.validate_references("library PAC API", api.source_ids())?;
        evidence
            .validate_references("library reserved register bits", library.reserved_sources())?;
        api.validate_against_svd(&library.render_layout_svd()?)?;
        let mut outputs = m.outputs;
        if !matches!(outputs.edition.as_str(), "2021" | "2024") {
            return Err("unsupported PAC edition".into());
        }
        let mut inputs: BTreeSet<PathBuf> = library.loaded_inputs().keys().cloned().collect();
        for p in m.evidence.iter().chain([&m.api]) {
            inputs.insert(base.join(p).canonicalize()?);
        }
        inputs.insert(path.clone());
        outputs.pac_api = host::destination(&base.join(&outputs.pac_api))?;
        if inputs.contains(&outputs.pac_api) {
            return Err("publication output aliases an input".into());
        }
        Ok(Self {
            library,
            api,
            outputs,
        })
    }

    pub fn generate(&self, check: bool) -> Result<()> {
        let svd = self.library.render_layout_svd()?;
        oer_registers_model::validate_pointer_writes(&[], &self.api)?;
        let mut api = String::from("#![forbid(unsafe_code)]\n");
        let mut facade = self.api.clone();
        facade.options.typed_peripherals = self.library.peripherals().map(str::to_owned).collect();
        api.push_str(&facade.without_typed_operations().render_facade_rust()?);
        api.push_str(&self.api.render_library_typed_rust(&svd)?);
        let api = host::format(&api, &self.outputs.edition)?;
        host::publish(&[(&self.outputs.pac_api, api)], check)
    }

    /// The library identifier.
    pub fn name(&self) -> &str {
        self.library.name()
    }
}
