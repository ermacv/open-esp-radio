//! Exclusive draft creation; the manifest is the last published file.
use crate::*;
use oer_register_model::{ModelDraft, ModelInit};
use std::io::Write;

/// Create a native unreviewed model in a new directory from an explicit TOML request.
/// The exact request is retained as `initialization.toml`.
pub fn initialize_model(request: &Path, directory: &Path) -> Result<usize> {
    let source = fs::read_to_string(request)?;
    let request: ModelInit = toml_edit::de::from_str(&source)?;
    let draft = ModelDraft::initialize(request)?;
    write(draft, directory, "initialization.toml", &source)
}

/// Import CMSIS-SVD into a new unreviewed model directory, retaining the exact XML.
/// Existing paths are never overwritten. No review pack or publication policy is invented.
pub fn import_svd(source: &Path, directory: &Path, chip: &str, space: &str) -> Result<usize> {
    let xml = fs::read_to_string(source)?;
    let draft = ModelDraft::import_svd(&xml, chip.into(), space.into())?;
    write(draft, directory, "source.svd", &xml)
}

fn write(draft: ModelDraft, directory: &Path, source_name: &str, source: &str) -> Result<usize> {
    oer_process::check_cancelled()?;
    fs::create_dir(directory)?;
    let result = (|| {
        for (name, text) in [
            (source_name, source),
            ("peripherals.toml", &draft.fragment),
            ("device.toml", &draft.manifest),
        ] {
            oer_process::check_cancelled()?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(directory.join(name))?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        fs::File::open(directory)?.sync_all()?;
        Ok(draft.peripherals)
    })();
    if result.is_err() {
        // Only this invocation's exclusively created directory is eligible.
        fs::remove_dir_all(directory)?;
    }
    result
}
