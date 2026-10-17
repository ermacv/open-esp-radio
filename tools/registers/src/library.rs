//! Publication of a register library: shared register blocks and the reviewed
//! transactions over them, without addresses.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use oer_register_model::{PacApiPack, RegisterEvidenceSet, RegisterLibrary};
use serde::Deserialize;

use crate::{Result, host, shared_blocks};

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
    pac_raw: PathBuf,
    pac_api: PathBuf,
    target: String,
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
        api.validate_against_svd(&library.render_layout_svd()?)?;
        let mut outputs = m.outputs;
        if !matches!(outputs.target.as_str(), "none" | "riscv")
            || !matches!(outputs.edition.as_str(), "2021" | "2024")
        {
            return Err("unsupported PAC target or edition".into());
        }
        let mut inputs: BTreeSet<PathBuf> = library.loaded_inputs().keys().cloned().collect();
        for p in m.evidence.iter().chain([&m.api]) {
            inputs.insert(base.join(p).canonicalize()?);
        }
        inputs.insert(path.clone());
        let mut destinations = BTreeSet::new();
        for p in [&mut outputs.pac_raw, &mut outputs.pac_api] {
            *p = host::destination(&base.join(&*p))?;
            if inputs.contains(p) || !destinations.insert(p.clone()) {
                return Err("publication output aliases an input or another output".into());
            }
        }
        Ok(Self {
            library,
            api,
            outputs,
        })
    }

    pub fn generate(&self, check: bool) -> Result<()> {
        let svd = self.library.render_layout_svd()?;
        let mut config = svd2rust::config::Config::default();
        config.target = if self.outputs.target == "none" {
            svd2rust::Target::None
        } else {
            svd2rust::Target::RISCV
        };
        config.edition = if self.outputs.edition == "2024" {
            svd2rust::config::RustEdition::E2024
        } else {
            svd2rust::config::RustEdition::E2021
        };
        config.strict = true;
        let rendered = svd2rust::generate(&svd, &config)
            .map_err(|e| e.to_string())?
            .lib_rs;
        let peripherals: Vec<&str> = self.library.peripherals().collect();
        let mut raw = shared_blocks::library_blocks(&rendered, &peripherals)?;
        if self.api.options.allow_clippy_empty_docs {
            raw.insert_str(0, "#![allow(clippy::empty_docs)]\n");
        }
        raw.push_str(&self.api.render_rust(&svd)?);
        let raw = host::format(&raw, &self.outputs.edition)?;
        let api = host::format(&self.api.render_facade_rust()?, &self.outputs.edition)?;
        host::publish(
            &[(&self.outputs.pac_raw, raw), (&self.outputs.pac_api, api)],
            check,
        )
    }

    /// The library identifier.
    pub fn name(&self) -> &str {
        self.library.name()
    }
}
