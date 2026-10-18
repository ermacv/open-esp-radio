//! The catalog of tracked ESP-IDF firmware the stand's boards run, and its
//! builds.
//!
//! Every ESP-IDF project with a `firmware.toml` beside its `CMakeLists.txt`
//! is an entry: peers in `hil/peers/<project>/` and vendor references in
//! `verification/<chip>/hil-vendor/<project>/`. Each is an application that
//! its own ESP-IDF bootloader loads (`Boot::EspIdfBootloader`), the only
//! images that do not boot staged. The manifest names the image (its name in
//! the board journal), the target chip and the chip whose `artifacts.toml`
//! pins the ESP-IDF; every entry builds against that one pin through
//! `oer_esp_idf`.
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;
use oer_esp_idf as idf;

/// Manifest file of a catalog entry.
const MANIFEST: &str = "firmware.toml";

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// Name of the image in the board journal.
    image: String,
    /// ESP-IDF target chip.
    chip: String,
    /// Chip whose `artifacts.toml` pins the ESP-IDF and vendor archives.
    pins: String,
    /// Why the image must not be flashed now; flashing it is refused.
    #[serde(default)]
    hold: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct Entry {
    pub image: String,
    pub chip: String,
    pub pins: String,
    /// Why the image must not be flashed now.
    pub hold: Option<String>,
    /// Project directory, relative to the repository root.
    pub directory: PathBuf,
}

impl Entry {
    /// The entry's ESP-IDF project in the tree at `root`.
    pub fn project(&self, root: &Path) -> idf::Project {
        idf::Project {
            name: self
                .directory
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            source: root.join(&self.directory),
            chip: self.chip.clone(),
        }
    }
}

/// Every catalog entry below `root`, by image name.
pub fn entries(root: &Path) -> Result<Vec<Entry>> {
    let mut directories = subdirectories(&root.join("hil/peers"))?;
    for chip in subdirectories(&root.join("verification"))? {
        directories.extend(subdirectories(&chip.join("hil-vendor"))?);
    }
    let mut entries = Vec::new();
    for directory in directories {
        let path = directory.join(MANIFEST);
        if !path.is_file() {
            continue;
        }
        let manifest: Manifest = toml::from_str(&std::fs::read_to_string(&path)?)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if entries
            .iter()
            .any(|entry: &Entry| entry.image == manifest.image)
        {
            return Err(format!("firmware image `{}` is defined twice", manifest.image).into());
        }
        entries.push(Entry {
            image: manifest.image,
            chip: manifest.chip,
            pins: manifest.pins,
            hold: manifest.hold,
            directory: directory.strip_prefix(root)?.to_owned(),
        });
    }
    entries.sort_by(|a, b| a.image.cmp(&b.image));
    Ok(entries)
}

fn subdirectories(directory: &Path) -> Result<Vec<PathBuf>> {
    let Ok(listing) = std::fs::read_dir(directory) else {
        return Ok(Vec::new());
    };
    let mut directories = Vec::new();
    for entry in listing {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            directories.push(entry.path());
        }
    }
    directories.sort();
    Ok(directories)
}

pub fn entry<'a>(entries: &'a [Entry], image: &str) -> Result<&'a Entry> {
    entries
        .iter()
        .find(|entry| entry.image == image)
        .ok_or_else(|| {
            let known = entries
                .iter()
                .map(|entry| entry.image.as_str())
                .collect::<Vec<_>>();
            format!("no firmware image `{image}`; catalog: {}", known.join(", ")).into()
        })
}

/// Build the catalog entry `image` of the tree at `root` against its pinned
/// ESP-IDF.
pub fn build(root: &Path, image: &str) -> Result<idf::Build> {
    let entries = entries(root)?;
    let entry = entry(&entries, image)?;
    idf::build(root, &entry.pins, &[entry.project(root)])?
        .pop()
        .ok_or_else(|| "the build produced no image".into())
}

/// The record of the catalog entry `image`'s last build in the tree at
/// `root` when it followed the entry's current recipe; an error naming the
/// build command when the entry is not built or its sources or pins changed
/// since.
pub fn current(root: &Path, image: &str) -> Result<idf::Build> {
    let entries = entries(root)?;
    let entry = entry(&entries, image)?;
    idf::current(root, &entry.pins, &entry.project(root))
        .map_err(|error| format!("{error}; run `cargo hil firmware build {image}`").into())
}

/// The application a built entry provides and its SHA-256.
pub fn product(build: &idf::Build) -> (PathBuf, String) {
    (
        PathBuf::from(&build.application),
        build.application_sha256.clone(),
    )
}

/// The image bundle of the built catalog image `entry` in `output`: its
/// application with the bootloader and partition table of its own ESP-IDF
/// build, at its chip's flash map. The build's `flasher_args.json` must place
/// them there and flash nothing else, so a bundle holds every byte
/// `idf.py flash` would write.
pub fn bundle(
    root: &Path,
    entry: &Entry,
    build: &idf::Build,
    output: &Path,
) -> Result<oer_image_bundle::ImageBundle> {
    let directory = idf::output(root, &entry.project(root)).join("build");
    only_boot_files(&directory)?;
    let flash = crate::profile(root, &entry.chip)?
        .flash
        .ok_or_else(|| format!("{} names no flash map", entry.chip))?;
    let (bootloader, partition_table) = super::flash_files(&directory, &flash)?;
    crate::bundle::around(
        root,
        &entry.chip,
        Path::new(&build.application),
        crate::bundle::BootFiles::Given {
            bootloader: &bootloader,
            partition_table: &partition_table,
        },
        output,
    )
}

/// Refuse a build whose `flasher_args.json` flashes more than the
/// bootloader, the partition table and the application.
fn only_boot_files(build: &Path) -> Result<()> {
    let arguments: serde_json::Value =
        serde_json::from_slice(&std::fs::read(build.join("flasher_args.json"))?)?;
    let files = arguments["flash_files"]
        .as_object()
        .ok_or("flasher_args.json lists no flash_files")?;
    if files.len() != 3 {
        return Err(format!(
            "the catalog build flashes {} files ({}); an image bundle holds the bootloader, the \
             partition table and the application",
            files.len(),
            files
                .iter()
                .map(|(address, file)| format!("{address} {}", file.as_str().unwrap_or("?")))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root: &Path, directory: &str, manifest: &str) {
        let directory = root.join(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("CMakeLists.txt"), "").unwrap();
        std::fs::write(directory.join(MANIFEST), manifest).unwrap();
    }

    #[test]
    fn peers_and_vendor_references_form_one_catalog() {
        let root = tempfile::tempdir().unwrap();
        project(
            root.path(),
            "hil/peers/chip-b-ieee802154",
            "image = \"ieee802154-peer\"\nchip = \"chip-b\"\npins = \"chip-a\"\n",
        );
        project(
            root.path(),
            "verification/chip-a/hil-vendor/calibration",
            "image = \"vendor-calibration\"\nchip = \"chip-a\"\npins = \"chip-a\"\n",
        );
        std::fs::create_dir_all(root.path().join("hil/peers/untracked")).unwrap();
        let entries = entries(root.path()).unwrap();
        assert_eq!(
            entries,
            [
                Entry {
                    image: "ieee802154-peer".into(),
                    chip: "chip-b".into(),
                    pins: "chip-a".into(),
                    hold: None,
                    directory: "hil/peers/chip-b-ieee802154".into(),
                },
                Entry {
                    image: "vendor-calibration".into(),
                    chip: "chip-a".into(),
                    pins: "chip-a".into(),
                    hold: None,
                    directory: "verification/chip-a/hil-vendor/calibration".into(),
                },
            ]
        );
        assert_eq!(entries[0].project(root.path()).name, "chip-b-ieee802154");
        assert!(entry(&entries, "missing").is_err());
        project(
            root.path(),
            "hil/peers/copy",
            "image = \"ieee802154-peer\"\nchip = \"chip-b\"\npins = \"chip-a\"\n",
        );
        assert!(
            super::entries(root.path()).is_err(),
            "image names are unique"
        );
    }

    #[test]
    fn the_tracked_catalog_names_its_projects() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let entries = entries(&root).unwrap();
        for image in ["ieee802154-peer", "vendor-calibration"] {
            assert!(entries.iter().any(|entry| entry.image == image), "{image}");
        }
    }

    #[test]
    fn a_catalog_build_flashing_more_than_its_boot_files_is_refused() {
        let build = tempfile::tempdir().unwrap();
        let write = |files: &str| {
            std::fs::write(
                build.path().join("flasher_args.json"),
                format!(r#"{{"flash_files": {{{files}}}}}"#),
            )
            .unwrap();
        };
        write(
            r#""0x2000": "bootloader/bootloader.bin", "0x8000": "partition_table/partition-table.bin", "0x10000": "app.bin""#,
        );
        only_boot_files(build.path()).unwrap();
        write(
            r#""0x2000": "bootloader/bootloader.bin", "0x8000": "partition_table/partition-table.bin", "0xd000": "ota_data_initial.bin", "0x10000": "app.bin""#,
        );
        let error = only_boot_files(build.path()).unwrap_err().to_string();
        assert!(error.contains("ota_data_initial.bin"), "{error}");
    }
}
