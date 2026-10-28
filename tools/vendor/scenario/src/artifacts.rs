//! The pinned vendor artifacts of the installed chip's `artifacts.toml`,
//! read through `oer-pins-artifacts`, the one reader of the pins.
//!
//! Every scenario input of a pinned artifact is authenticated against one
//! digest: the pinned SHA-256 of a fetched artifact, which has one cache
//! location that `cargo pins fetch` fills, or that of a firmware
//! output at its pinned location once its build follows the current recipe
//! of its catalog image. The manifest is the only place a pin changes.
use crate::harness::{Result, invalid};
use oer_pins_artifacts::{Artifact, Manifest};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The manifest compiled into this binary.
pub fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        Manifest::parse(crate::chip().manifest_text).expect("tracked artifact manifest")
    })
}

fn artifact(id: &str) -> &'static Artifact {
    manifest()
        .artifact(id)
        .unwrap_or_else(|_| panic!("artifact {id} is not pinned"))
}

/// The SHA-256 every input of artifact `id` must have: its pin for a fetched
/// artifact; for a firmware output, the digest of the file at its pinned
/// location, an error unless that build follows its image's current recipe.
pub fn sha256(id: &str) -> Result<&'static str> {
    let artifact = artifact(id);
    if let Some(pin) = &artifact.sha256 {
        return Ok(pin);
    }
    static BUILT: OnceLock<BTreeMap<&'static str, OnceLock<std::result::Result<String, String>>>> =
        OnceLock::new();
    let built = BUILT.get_or_init(|| {
        manifest()
            .artifact
            .iter()
            .filter(|artifact| artifact.sha256.is_none())
            .map(|artifact| (artifact.id.as_str(), OnceLock::new()))
            .collect()
    });
    built[artifact.id.as_str()]
        .get_or_init(|| built_digest(artifact).map_err(|error| error.to_string()))
        .as_deref()
        .map_err(|error| invalid(format!("{id}: {error}")))
}

/// The digest of the firmware output `artifact` of the current build.
fn built_digest(artifact: &Artifact) -> Result<String> {
    let root = oer_os_process::built_root();
    let image = manifest()
        .source_of(artifact)
        .image
        .as_deref()
        .ok_or_else(|| invalid("a firmware source names its image"))?;
    oer_image_build::esp_idf::catalog::current(&root, image)?;
    oer_os_durable::sha256_file(&path(&root, &artifact.id))
}

/// Location of artifact `id` below the repository `root`: the fetch cache
/// for downloaded artifacts, the build output for firmware ones.
pub fn path(root: &Path, id: &str) -> PathBuf {
    manifest()
        .location(root, artifact(id))
        .unwrap_or_else(|error| panic!("artifact {id}: {error}"))
}

/// Pinned location of `id` in this checkout: the default of every scenario
/// input argument.
pub fn default_path(id: &str) -> PathBuf {
    path(&oer_os_process::built_root(), id)
}
