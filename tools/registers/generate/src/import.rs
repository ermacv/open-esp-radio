//! `cargo registers import`: import a chip's vendor blocks into block
//! types and a chip root (see [`oer_registers_model::chip_import`]) from
//! the pinned vendor SVD and ESP-IDF files, and check that the root loads.

use std::{
    fs,
    path::{Path, PathBuf},
};

use oer_registers_model::{
    RegisterEvidenceSet, StrictnessOverlay,
    chip_import::{ImportError, ImportSpec, Pin, PinnedFiles, Report, import},
    chip_model::ChipModel,
};
use oer_tool_pins::Manifest;

use crate::Result;

/// The import spec of a chip, relative to the repository root.
pub fn spec_path(chip: &str) -> PathBuf {
    Path::new("chips").join(chip).join("registers/import.toml")
}

/// The pinned files of a chip, read through `cargo pins`' store.
struct Pinned<'a> {
    root: &'a Path,
    chip: &'a str,
    manifest: Manifest,
    svd: &'a str,
    esp_idf: &'a str,
}

impl Pinned<'_> {
    fn read(&self, id: &str) -> std::result::Result<(String, Pin), ImportError> {
        let fail = |error: &dyn std::fmt::Display| ImportError::new(id, error.to_string());
        let artifact = self.manifest.artifact(id).map_err(|e| fail(&e))?;
        let source = self.manifest.source_of(artifact);
        let path = oer_tool_pins::artifacts::fetched(self.root, self.chip, id)
            .map_err(|e| fail(&e))?;
        let text = fs::read_to_string(&path).map_err(|e| fail(&e))?;
        Ok((
            text,
            Pin {
                source: source.id.clone(),
                repository: source.repository.clone().unwrap_or_default(),
                revision: source.revision.clone().unwrap_or_default(),
                path: artifact.path.clone(),
                sha256: artifact.sha256.clone().unwrap_or_default(),
            },
        ))
    }
}

impl PinnedFiles for Pinned<'_> {
    fn svd(&self) -> std::result::Result<(String, Pin), ImportError> {
        self.read(self.svd)
    }

    fn idf(&self, path: &str) -> std::result::Result<(String, Pin), ImportError> {
        let id = format!("{}/{path}", self.esp_idf);
        let found = self.read(&id)?;
        if found.1.source != self.esp_idf {
            return Err(ImportError::new(
                id,
                format!("is pinned from {}, not {}", found.1.source, self.esp_idf),
            ));
        }
        Ok(found)
    }
}

/// What an import wrote.
pub struct Imported {
    pub types: usize,
    pub report: Report,
    pub report_path: PathBuf,
}

/// Import `chip`'s blocks as its `registers/import.toml` states, write the
/// root, the type files and the evidence catalog, write the report to
/// `target/registers-import/<chip>/report.md` and load the root.
pub fn run(root: &Path, chip: &str) -> Result<Imported> {
    let spec_file = root.join(spec_path(chip));
    let spec = ImportSpec::parse(&fs::read_to_string(&spec_file)?)?;
    if spec.chip != chip {
        return Err(format!("{} imports {}, not {chip}", spec_file.display(), spec.chip).into());
    }
    let base = spec_file.parent().ok_or("the import spec has no directory")?;
    let overlay = match &spec.overlay {
        None => None,
        Some(path) => Some(StrictnessOverlay::load(&base.join(path))?),
    };
    let pinned = Pinned {
        root,
        chip,
        manifest: Manifest::load(root, chip)?,
        svd: &spec.svd,
        esp_idf: &spec.esp_idf,
    };
    let output = import(&spec, overlay.as_ref(), &pinned)?;

    let root_path = base.join(&spec.root);
    let root_directory = root_path.parent().ok_or("the chip root has no directory")?;
    let types_directory = root_directory.join(&spec.types_directory);
    fs::create_dir_all(&types_directory)?;
    // The import owns its type directory: a type it no longer writes goes.
    for entry in fs::read_dir(&types_directory)? {
        let path = entry?.path();
        let relative = format!(
            "{}/{}",
            spec.types_directory,
            path.file_name().and_then(|n| n.to_str()).unwrap_or_default()
        );
        if path.extension().is_some_and(|e| e == "toml") && !output.types.contains_key(&relative)
        {
            fs::remove_file(&path)?;
        }
    }
    for (relative, text) in &output.types {
        fs::write(root_directory.join(relative), text)?;
    }
    fs::write(&root_path, &output.root)?;
    let evidence_path = base.join(&spec.evidence);
    if let Some(parent) = evidence_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&evidence_path, &output.evidence)?;

    let report_directory = root.join("target/registers-import").join(chip);
    fs::create_dir_all(&report_directory)?;
    let report_path = report_directory.join("report.md");
    fs::write(&report_path, output.report.markdown())?;

    let evidence = RegisterEvidenceSet::load_all(&[evidence_path])?;
    ChipModel::load(&root_path, &evidence)?;
    Ok(Imported {
        types: output.types.len(),
        report: output.report,
        report_path,
    })
}
