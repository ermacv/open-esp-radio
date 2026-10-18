//! The pins of a chip's vendor artifacts: the one reader of its tracked
//! `verification/<chip>/artifacts.toml` (sources, artifacts, the SHA-256 of
//! each fetched one) and where a pinned artifact lies in a checkout.
//! Fetching and the host-wide store are `oer-vendor-artifacts`.
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Directory of every chip's verification project, relative to the root.
const VERIFICATION: &str = "verification";

/// Cache of fetched artifacts, relative to the repository root; a link to
/// the host-wide store.
pub const CACHE: &str = "target/vendor";

/// The pin of `chip`'s vendor artifacts, relative to the repository root,
/// whether or not the chip has one.
pub fn manifest(chip: &str) -> String {
    format!("{VERIFICATION}/{chip}/artifacts.toml")
}

/// Tracked manifest of `chip`, relative to the repository root: an error
/// that lists the supported chips for an unsupported one, or names the
/// missing manifest of a chip without a vendor verification project.
pub fn manifest_path(root: &Path, chip: &str) -> Result<String> {
    let profile = oer_chip_profile::Profile::load(root, chip)?;
    let manifest = manifest(&profile.id);
    if !root.join(&manifest).is_file() {
        return Err(format!("chip {chip} has no vendor verification project ({manifest})").into());
    }
    Ok(manifest)
}

/// The kind of a pinned source.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// A file at `path` of a git revision.
    Git,
    /// A member `path` of a release asset tarball.
    Release,
    /// An output at `path` below the repository root of the tracked firmware
    /// catalog image `image`, built by `cargo hil firmware build <image>`.
    /// The recipe is the pin: the output carries no SHA-256.
    Firmware,
}

/// One pinned source of `artifacts.toml`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub kind: SourceKind,
    pub repository: Option<String>,
    pub revision: Option<String>,
    pub asset: Option<String>,
    pub sha256: Option<String>,
    /// The firmware catalog image of a firmware source.
    pub image: Option<String>,
}

/// One pinned artifact of `artifacts.toml`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    /// The id of its source.
    pub source: String,
    /// Its path in the source: in the git tree, the release tarball or,
    /// for a firmware build output, below the repository root.
    pub path: String,
    /// The pinned SHA-256 of a fetched artifact; a firmware output has none.
    pub sha256: Option<String>,
}

/// A chip's `artifacts.toml`: the one reader of the pins.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub source: Vec<Source>,
    pub artifact: Vec<Artifact>,
}

impl Manifest {
    /// Parses a manifest and checks that every artifact names a declared
    /// source, every fetched artifact pins its SHA-256 and every firmware
    /// source names its image while its outputs pin none.
    pub fn parse(text: &str) -> Result<Self> {
        let manifest: Self = toml::from_str(text)?;
        if manifest.schema != 1 {
            return Err(format!("unsupported artifact schema {}", manifest.schema).into());
        }
        for source in &manifest.source {
            let firmware = source.kind == SourceKind::Firmware;
            if firmware != source.image.is_some() {
                return Err(
                    format!("{}: `image` names exactly a firmware source", source.id).into(),
                );
            }
            if firmware
                && (source.repository.is_some()
                    || source.revision.is_some()
                    || source.asset.is_some()
                    || source.sha256.is_some())
            {
                return Err(format!("{}: a firmware source pins only its image", source.id).into());
            }
        }
        for artifact in &manifest.artifact {
            let Some(source) = manifest.source.iter().find(|s| s.id == artifact.source) else {
                return Err(format!("{}: unknown source {}", artifact.id, artifact.source).into());
            };
            if (source.kind == SourceKind::Firmware) == artifact.sha256.is_some() {
                return Err(format!(
                    "{}: a fetched artifact pins its SHA-256 and a firmware output none",
                    artifact.id
                )
                .into());
            }
        }
        Ok(manifest)
    }

    /// The tracked manifest of `chip` in the repository at `root`.
    pub fn load(root: &Path, chip: &str) -> Result<Self> {
        let manifest = manifest_path(root, chip)?;
        Self::parse(&std::fs::read_to_string(root.join(&manifest))?)
            .map_err(|error| format!("{manifest}: {error}").into())
    }

    /// The artifact `id`.
    pub fn artifact(&self, id: &str) -> Result<&Artifact> {
        self.artifact
            .iter()
            .find(|artifact| artifact.id == id)
            .ok_or_else(|| format!("artifacts.toml pins no `{id}` artifact").into())
    }

    /// The source `id`.
    pub fn source(&self, id: &str) -> Result<&Source> {
        self.source
            .iter()
            .find(|source| source.id == id)
            .ok_or_else(|| format!("artifacts.toml pins no `{id}` source").into())
    }

    /// Where the fetched `source` lies for the checkout at `root`:
    /// `<root>/target/vendor/<source>/<revision>`.
    pub fn source_directory(&self, root: &Path, source: &Source) -> Result<PathBuf> {
        cache_directory(&root.join(CACHE), source)
    }

    /// The source `artifact` names; [`Manifest::parse`] checked it exists.
    pub fn source_of(&self, artifact: &Artifact) -> &Source {
        self.source
            .iter()
            .find(|source| source.id == artifact.source)
            .expect("parsed manifests name declared sources")
    }

    /// Where `artifact` lies for the checkout at `root`: its build output
    /// for a firmware source, else `<root>/target/vendor/<source>/<revision>/<path>`.
    pub fn location(&self, root: &Path, artifact: &Artifact) -> Result<PathBuf> {
        let source = self.source_of(artifact);
        Ok(match source.kind {
            SourceKind::Firmware => root.join(&artifact.path),
            SourceKind::Git | SourceKind::Release => {
                self.source_directory(root, source)?.join(&artifact.path)
            }
        })
    }
}

/// `<base>/<source>/<revision>`, where `base` is a store or a checkout's
/// `target/vendor`.
pub fn cache_directory(base: &Path, source: &Source) -> Result<PathBuf> {
    let revision = source
        .revision
        .as_deref()
        .ok_or_else(|| format!("{} lacks `revision`", source.id))?;
    Ok(base.join(&source.id).join(revision))
}

/// A pinned git source of `chip` with its artifacts' paths and SHA-256.
#[derive(Debug)]
pub struct GitPin {
    pub id: String,
    pub repository: String,
    pub revision: String,
    /// `(path, sha256)` of each artifact, relative to the checkout.
    pub artifacts: Vec<(String, String)>,
}

/// Every pinned git source of `chip`.
pub fn git_pins(root: &Path, chip: &str) -> Result<Vec<GitPin>> {
    let Manifest {
        source, artifact, ..
    } = Manifest::load(root, chip)?;
    let artifacts = artifact;
    source
        .into_iter()
        .filter(|s| s.kind == SourceKind::Git)
        .map(|source| {
            Ok(GitPin {
                artifacts: artifacts
                    .iter()
                    .filter(|a| a.source == source.id)
                    .map(|a| {
                        let sha256 = a
                            .sha256
                            .clone()
                            .expect("parsed fetched artifacts pin a SHA-256");
                        (a.path.clone(), sha256)
                    })
                    .collect(),
                repository: source
                    .repository
                    .ok_or_else(|| format!("{} lacks `repository`", source.id))?,
                revision: source
                    .revision
                    .ok_or_else(|| format!("{} lacks `revision`", source.id))?,
                id: source.id,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(source: &str, artifact: &str) -> Result<Manifest> {
        Manifest::parse(&format!(
            "schema = 1\n[[source]]\nid = \"s\"\n{source}\n[[artifact]]\nid = \"a\"\nsource = \"s\"\npath = \"p\"\n{artifact}\n"
        ))
    }

    #[test]
    fn a_firmware_output_pins_its_recipe_and_no_digest() {
        let firmware = "kind = \"firmware\"\nimage = \"i\"";
        let parsed = manifest(firmware, "").unwrap();
        let artifact = parsed.artifact("a").unwrap();
        assert_eq!(artifact.sha256, None);
        assert_eq!(
            parsed.location(Path::new("/repo"), artifact).unwrap(),
            Path::new("/repo/p")
        );
        assert!(manifest(firmware, "sha256 = \"00\"").is_err());
        assert!(manifest("kind = \"firmware\"", "").is_err());
        assert!(manifest(&format!("{firmware}\nrevision = \"r\""), "").is_err());
    }

    #[test]
    fn a_fetched_artifact_pins_its_digest() {
        let release = "kind = \"release\"\nrepository = \"r\"\nrevision = \"v\"\nasset = \"t\"";
        assert!(manifest(release, "sha256 = \"00\"").is_ok());
        assert!(manifest(release, "").is_err());
        assert!(manifest(&format!("{release}\nimage = \"i\""), "sha256 = \"00\"").is_err());
    }
}
