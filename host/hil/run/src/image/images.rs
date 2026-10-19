//! A directory of built HIL images, the one form a run takes its images in
//! (`cargo hil run --images DIR`): each class's bundle in `<class>/`, and
//! `images.json` naming the chip, the tree they were built from (its commit
//! and whether it was dirty), the layout seed, the feature delta and the
//! SHA-256 of every file of every bundle.
//!
//! The bundles are the directory's own: a run that builds its images builds
//! them into a directory of its own, and an enqueued run builds them into
//! its job's directory when it is enqueued. A later build of the checkout
//! writes elsewhere, and [`load`] refuses a bundle whose files changed.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use oer_hil_model::schema::image::{FeatureDelta, ImageClass};
use oer_hil_runs::format::run::RepositoryProvenance;
use oer_image_build::bundle::ImageBundle;

use crate::image::{Artifacts, LayoutSeed, Result};

/// The directory's description.
pub const MANIFEST: &str = "images.json";
/// Format of [`MANIFEST`].
pub const SCHEMA: u16 = 1;

/// What [`MANIFEST`] records.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u16,
    pub chip: String,
    /// The tree the images were built from.
    pub repository: RepositoryProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_seed: LayoutSeed,
    #[serde(default, skip_serializing_if = "FeatureDelta::is_empty")]
    pub features: FeatureDelta,
    /// Every built class's files, by class id and by name below its bundle,
    /// with their SHA-256.
    pub images: BTreeMap<String, BTreeMap<String, String>>,
}

/// A directory of images being built or read.
#[derive(Clone, Debug)]
pub struct Images {
    directory: PathBuf,
    manifest: Manifest,
}

impl Images {
    /// A new, empty directory `directory` for `chip`'s images built from
    /// `root` with `layout_seed` and `features`; it must not exist yet.
    pub fn create(
        directory: &Path,
        root: &Path,
        chip: &str,
        layout_seed: LayoutSeed,
        features: &FeatureDelta,
    ) -> Result<Self> {
        if directory.exists() {
            return Err(format!("{} exists already", directory.display()).into());
        }
        fs::create_dir_all(directory)?;
        let images = Self {
            directory: fs::canonicalize(directory)?,
            manifest: Manifest {
                schema: SCHEMA,
                chip: chip.to_owned(),
                repository: oer_hil_runs::build::repository(root)?,
                layout_seed,
                features: features.clone(),
                images: BTreeMap::new(),
            },
        };
        images.write()?;
        Ok(images)
    }

    /// The directory at `directory`, as its manifest describes it.
    pub fn open(directory: &Path) -> Result<Self> {
        let path = directory.join(MANIFEST);
        let manifest: Manifest = serde_json::from_slice(
            &fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?,
        )
        .map_err(|error| format!("{}: {error}", path.display()))?;
        if manifest.schema != SCHEMA {
            return Err(format!(
                "{} has schema {}, expected {SCHEMA}",
                path.display(),
                manifest.schema
            )
            .into());
        }
        Ok(Self {
            directory: fs::canonicalize(directory)?,
            manifest,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Where `class`'s bundle lies.
    pub fn bundle_directory(&self, class: ImageClass) -> PathBuf {
        self.directory.join(class.id())
    }

    /// Build `class` from the tree at `root` into its bundle directory and
    /// record its files.
    pub fn build(&mut self, root: &Path, class: ImageClass) -> Result<Artifacts> {
        if self.manifest.images.contains_key(class.id()) {
            return Err(
                format!("{} holds {} already", self.directory.display(), class.id()).into(),
            );
        }
        let features = self.manifest.features.clone();
        let spec = crate::image::spec(
            root,
            &self.manifest.chip,
            class,
            (self.manifest.layout_seed, &features),
            self.bundle_directory(class),
        )?;
        let artifacts = crate::image::built(&spec, &features, self.manifest.repository.clone())?;
        self.manifest.images.insert(
            class.id().to_owned(),
            digests(&self.bundle_directory(class))?,
        );
        self.write()?;
        Ok(artifacts)
    }

    /// `class`'s image, every file of its bundle checked against the digest
    /// the manifest records.
    pub fn load(&self, class: ImageClass) -> Result<Artifacts> {
        let recorded =
            self.manifest.images.get(class.id()).ok_or_else(|| {
                format!("{} holds no {} image", self.directory.display(), class.id())
            })?;
        let directory = self.bundle_directory(class);
        if &digests(&directory)? != recorded {
            return Err(format!(
                "the {} image in {} changed after it was built",
                class.id(),
                self.directory.display()
            )
            .into());
        }
        Ok(Artifacts {
            bundle: ImageBundle::load(&directory)?,
            features: self.manifest.features.clone(),
            repository: self.manifest.repository.clone(),
        })
    }

    fn write(&self) -> Result<()> {
        oer_os_durable::atomic_json(&self.directory.join(MANIFEST), &self.manifest)
    }
}

/// Every regular file below `directory`, by its path relative to it, with
/// its SHA-256.
fn digests(directory: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![directory.to_owned()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                let relative = path.strip_prefix(directory)?.to_string_lossy().into_owned();
                files.insert(relative, oer_os_durable::sha256_file(&path)?);
            } else {
                return Err(format!("{} is not a regular file", path.display()).into());
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_bundle_file_is_refused_and_an_unbuilt_class_is_named() {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("system-watchdog");
        fs::create_dir_all(bundle.join("nested")).unwrap();
        fs::write(bundle.join("application.bin"), b"application").unwrap();
        fs::write(bundle.join("nested/report.txt"), b"report").unwrap();
        let recorded = digests(&bundle).unwrap();
        assert_eq!(
            recorded.keys().collect::<Vec<_>>(),
            ["application.bin", "nested/report.txt"]
        );
        let images = Images {
            directory: directory.path().to_owned(),
            manifest: Manifest {
                schema: SCHEMA,
                chip: "chip-a".into(),
                repository: RepositoryProvenance {
                    commit: "abc".into(),
                    dirty: false,
                },
                layout_seed: None,
                features: FeatureDelta::default(),
                images: BTreeMap::from([(ImageClass::SystemWatchdog.id().to_owned(), recorded)]),
            },
        };
        images.write().unwrap();
        assert_eq!(
            Images::open(directory.path()).unwrap().manifest(),
            images.manifest()
        );
        fs::write(bundle.join("application.bin"), b"applicatioN").unwrap();
        let error = images.load(ImageClass::SystemWatchdog).unwrap_err();
        assert!(
            error.to_string().contains("changed after it was built"),
            "{error}"
        );
        let error = images.load(ImageClass::SystemPanicReset).unwrap_err();
        assert!(
            error.to_string().contains("holds no system-panic-reset"),
            "{error}"
        );
    }
}
