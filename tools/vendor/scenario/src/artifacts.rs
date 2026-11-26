//! The pinned vendor artifacts of the installed chip's `artifacts.toml`,
//! read through `oer-tools-pins-artifacts`, the one reader of the pins.
//!
//! Every scenario input of a pinned artifact is authenticated against one
//! digest: the pinned SHA-256 of a fetched artifact, which has one cache
//! location that `cargo pins fetch` fills, or that of a firmware
//! output at its pinned location once its build follows the current recipe
//! of its catalog image. The manifest is the only place a pin changes.
use crate::harness::{Result, invalid};
use oer_tools_pins_artifacts::{Artifact, Manifest, Origin};
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
    let image = match &artifact.origin {
        Origin::Fetched { sha256, .. } => return Ok(sha256),
        Origin::Firmware(source) => &source.image,
    };
    static BUILT: OnceLock<BTreeMap<&'static str, OnceLock<std::result::Result<String, String>>>> =
        OnceLock::new();
    let built = BUILT.get_or_init(|| {
        manifest()
            .artifact
            .iter()
            .filter(|artifact| matches!(artifact.origin, Origin::Firmware(_)))
            .map(|artifact| (artifact.id.as_str(), OnceLock::new()))
            .collect()
    });
    built[artifact.id.as_str()]
        .get_or_init(|| built_digest(artifact, image).map_err(|error| error.to_string()))
        .as_deref()
        .map_err(|error| invalid(format!("{id}: {error}")))
}

/// The digest of the firmware output `artifact` of the current build of
/// its source's `image`.
fn built_digest(artifact: &Artifact, image: &str) -> Result<String> {
    let root = oer_tools_os_process::built_root();
    oer_tools_image_build::esp_idf::catalog::current(&root, image)?;
    oer_tools_os_durable::sha256_file(&path(&root, &artifact.id))
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
    path(&oer_tools_os_process::built_root(), id)
}
