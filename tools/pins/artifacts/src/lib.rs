//! The pins of a chip's vendor artifacts: the one reader of its tracked
//! `chips/<chip>/vendor/artifacts.toml` (sources, artifacts, the SHA-256 of
//! each fetched one) and where a pinned artifact lies in a checkout.
//! Fetching and the host-wide store are `oer-tools-pins-artifacts`.
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
pub mod artifacts;
pub mod provenance;

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Directory of every chip, relative to the root.
const CHIPS: &str = "chips";

/// Cache of fetched artifacts, relative to the repository root; a link to
/// the host-wide store.
pub const CACHE: &str = "target/vendor";

/// The pin of `chip`'s vendor artifacts, relative to the repository root,
/// whether or not the chip has one.
pub fn manifest(chip: &str) -> String {
    format!("{CHIPS}/{chip}/vendor/artifacts.toml")
}

/// Tracked manifest of `chip`, relative to the repository root: an error
/// that lists the chips with pins when `chip` has none. The chip needs no
/// chip profile, so a peer chip outside the supported set (an ESP-IDF
/// reference board) pins its artifacts the same way.
pub fn manifest_path(root: &Path, chip: &str) -> Result<String> {
    let plain = !chip.is_empty()
        && chip
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if plain && root.join(manifest(chip)).is_file() {
        return Ok(manifest(chip));
    }
    let mut chips = Vec::new();
    for entry in std::fs::read_dir(root.join(CHIPS))? {
        let id = entry?.file_name().to_string_lossy().into_owned();
        if root.join(manifest(&id)).is_file() {
            chips.push(id);
        }
    }
    chips.sort();
    Err(format!(
        "chip `{chip}` has no vendor artifact pins; chips with pins: {}",
        chips.join(", ")
    )
    .into())
}

/// The kind of a fetched source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceKind {
    /// A file at `path` of a git revision.
    Git,
    /// A member `path` of a release asset tarball.
    Release,
}

/// A source whose artifacts are fetched and verified against their pinned
/// SHA-256.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchedSource {
    pub id: String,
    pub kind: SourceKind,
    pub repository: Option<String>,
    pub revision: Option<String>,
    pub asset: Option<String>,
    pub sha256: Option<String>,
}

/// A source whose artifacts are outputs, at their `path` below the
/// repository root, of the tracked firmware catalog image `image`, built by
/// `cargo fw idf build <image>`. The recipe is the pin: an output carries no
/// SHA-256.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirmwareSource {
    pub id: String,
    pub image: String,
}

/// One pinned source of `artifacts.toml`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Source {
    Fetched(FetchedSource),
    Firmware(FirmwareSource),
}

impl Source {
    pub fn id(&self) -> &str {
        match self {
            Self::Fetched(source) => &source.id,
            Self::Firmware(source) => &source.id,
        }
    }
}

/// The source an artifact names and what pins the artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Origin {
    /// Fetched from `source` and verified against the pinned `sha256`.
    Fetched {
        source: FetchedSource,
        sha256: String,
    },
    /// Built from the recipe of its source's image.
    Firmware(FirmwareSource),
}

/// One pinned artifact of `artifacts.toml`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Artifact {
    pub id: String,
    /// Its path in the source: in the git tree, the release tarball or,
    /// for a firmware build output, below the repository root.
    pub path: String,
    pub origin: Origin,
}

impl Artifact {
    /// The id of its source.
    pub fn source(&self) -> &str {
        match &self.origin {
            Origin::Fetched { source, .. } => &source.id,
            Origin::Firmware(source) => &source.id,
        }
    }
}

/// A chip's `artifacts.toml`: the one reader of the pins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Manifest {
    pub schema: u32,
    pub source: Vec<Source>,
    pub artifact: Vec<Artifact>,
}

/// `artifacts.toml` as it is written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    schema: u32,
    source: Vec<SourceFile>,
    artifact: Vec<ArtifactFile>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum KindFile {
    Git,
    Release,
    Firmware,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFile {
    id: String,
    kind: KindFile,
    repository: Option<String>,
    revision: Option<String>,
    asset: Option<String>,
    sha256: Option<String>,
    /// The firmware catalog image of a firmware source.
    image: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactFile {
    id: String,
    /// The id of its source.
    source: String,
    path: String,
    /// The pinned SHA-256 of a fetched artifact; a firmware output has none.
    sha256: Option<String>,
}

impl SourceFile {
    /// The source this row pins: a firmware source names its image and
    /// nothing else, a fetched one no image.
    fn source(self) -> Result<Source> {
        let kind = match (self.kind, self.image) {
            (KindFile::Git, None) => SourceKind::Git,
            (KindFile::Release, None) => SourceKind::Release,
            (KindFile::Firmware, Some(image)) => {
                if self.repository.is_some()
                    || self.revision.is_some()
                    || self.asset.is_some()
                    || self.sha256.is_some()
                {
                    return Err(
                        format!("{}: a firmware source pins only its image", self.id).into(),
                    );
                }
                return Ok(Source::Firmware(FirmwareSource { id: self.id, image }));
            }
            (KindFile::Firmware, None) | (KindFile::Git | KindFile::Release, Some(_)) => {
                return Err(format!("{}: `image` names exactly a firmware source", self.id).into());
            }
        };
        Ok(Source::Fetched(FetchedSource {
            id: self.id,
            kind,
            repository: self.repository,
            revision: self.revision,
            asset: self.asset,
            sha256: self.sha256,
        }))
    }
}

impl Manifest {
    /// Parses a manifest and checks that every artifact names a declared
    /// source, every fetched artifact pins its SHA-256 and every firmware
    /// source names its image while its outputs pin none.
    pub fn parse(text: &str) -> Result<Self> {
        let file: ManifestFile = toml::from_str(text)?;
        if file.schema != 1 {
            return Err(format!("unsupported artifact schema {}", file.schema).into());
        }
        let source = file
            .source
            .into_iter()
            .map(SourceFile::source)
            .collect::<Result<Vec<_>>>()?;
        let mut artifact = Vec::with_capacity(file.artifact.len());
        for row in file.artifact {
            let Some(named) = source.iter().find(|source| source.id() == row.source) else {
                return Err(format!("{}: unknown source {}", row.id, row.source).into());
            };
            let origin = match (named, row.sha256) {
                (Source::Fetched(source), Some(sha256)) => Origin::Fetched {
                    source: source.clone(),
                    sha256,
                },
                (Source::Firmware(source), None) => Origin::Firmware(source.clone()),
                (Source::Fetched(_), None) | (Source::Firmware(_), Some(_)) => {
                    return Err(format!(
                        "{}: a fetched artifact pins its SHA-256 and a firmware output none",
                        row.id
                    )
                    .into());
                }
            };
            artifact.push(Artifact {
                id: row.id,
                path: row.path,
                origin,
            });
        }
        Ok(Self {
            schema: file.schema,
            source,
            artifact,
        })
    }

    /// The tracked manifest of `chip` in the repository at `root`.
    pub fn load(root: &Path, chip: &str) -> Result<Self> {
        Self::load_with_inputs(
            root,
            chip,
            &mut oer_tools_os_durable::inputs::Inputs::default(),
        )
    }

    /// Include the artifact pins in the computation that consumes an artifact.
    pub fn load_with_inputs(
        root: &Path,
        chip: &str,
        inputs: &mut oer_tools_os_durable::inputs::Inputs,
    ) -> Result<Self> {
        let manifest = manifest_path(root, chip)?;
        Self::parse(&inputs.read_to_string(&root.join(&manifest))?)
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
            .find(|source| source.id() == id)
            .ok_or_else(|| format!("artifacts.toml pins no `{id}` source").into())
    }

    /// Where the fetched `source` lies for the checkout at `root`:
    /// `<root>/target/vendor/<source>/<revision>`.
    pub fn source_directory(&self, root: &Path, source: &FetchedSource) -> Result<PathBuf> {
        cache_directory(&root.join(CACHE), source)
    }

    /// Where `artifact` lies for the checkout at `root`: its build output
    /// for a firmware source, else `<root>/target/vendor/<source>/<revision>/<path>`.
    pub fn location(&self, root: &Path, artifact: &Artifact) -> Result<PathBuf> {
        Ok(match &artifact.origin {
            Origin::Firmware(_) => root.join(&artifact.path),
            Origin::Fetched { source, .. } => {
                self.source_directory(root, source)?.join(&artifact.path)
            }
        })
    }
}

/// `<base>/<source>/<revision>`, where `base` is a store or a checkout's
/// `target/vendor`.
pub fn cache_directory(base: &Path, source: &FetchedSource) -> Result<PathBuf> {
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
        .filter_map(|source| match source {
            Source::Fetched(source) if source.kind == SourceKind::Git => Some(source),
            Source::Fetched(_) | Source::Firmware(_) => None,
        })
        .map(|source| {
            Ok(GitPin {
                artifacts: artifacts
                    .iter()
                    .filter_map(|artifact| match &artifact.origin {
                        Origin::Fetched {
                            source: named,
                            sha256,
                        } if named.id == source.id => Some((artifact.path.clone(), sha256.clone())),
                        Origin::Fetched { .. } | Origin::Firmware(_) => None,
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
    fn a_peer_chip_without_a_profile_resolves_its_pins() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let pins = root.join("chips/peer/vendor");
        std::fs::create_dir_all(&pins).unwrap();
        std::fs::write(
            pins.join("artifacts.toml"),
            "schema = 1\n[[source]]\nid = \"idf\"\nkind = \"git\"\n\
             repository = \"https://example.invalid/idf\"\nrevision = \"r1\"\n\
             [[artifact]]\nid = \"a\"\nsource = \"idf\"\npath = \"p\"\nsha256 = \"00\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("chips/bare")).unwrap();
        assert_eq!(
            manifest_path(root, "peer").unwrap(),
            super::manifest("peer")
        );
        let pins = git_pins(root, "peer").unwrap();
        assert_eq!(
            (pins[0].id.as_str(), pins[0].revision.as_str()),
            ("idf", "r1")
        );
        assert_eq!(pins[0].artifacts, [("p".to_owned(), "00".to_owned())]);
        for chip in ["bare", "../peer", "", "peer/vendor"] {
            let error = manifest_path(root, chip).unwrap_err().to_string();
            assert!(error.contains("has no vendor artifact pins"), "{error}");
            assert!(error.ends_with("chips with pins: peer"), "{error}");
        }
    }

    #[test]
    fn a_firmware_output_pins_its_recipe_and_no_digest() {
        let firmware = "kind = \"firmware\"\nimage = \"i\"";
        let parsed = manifest(firmware, "").unwrap();
        let artifact = parsed.artifact("a").unwrap();
        assert!(matches!(artifact.origin, Origin::Firmware(_)));
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
