//! The catalog of tracked ESP-IDF firmware the stand's boards run, and its
//! builds.
//!
//! Every ESP-IDF project with a `firmware.toml` beside its `CMakeLists.txt`
//! is an entry: peers in `firmware/peers/<project>/` and vendor references in
//! `firmware/vendor/<chip>/<project>/`. Each is an application that
//! its own ESP-IDF bootloader loads (`Boot::EspIdfBootloader`), the only
//! images that do not boot staged. The manifest names the image (its name in
//! the board journal), the target chip and the chip whose `artifacts.toml`
//! pins the ESP-IDF; every entry builds against that one pin through
//! `oer_image_esp_idf`.
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;
use oer_image_esp_idf as idf;

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
    let mut directories = subdirectories(&root.join("firmware/peers"))?;
    for chip in subdirectories(&root.join("firmware/vendor"))? {
        directories.extend(subdirectories(&chip)?);
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
) -> Result<crate::bundle::ImageBundle> {
    let directory = idf::output(root, &entry.project(root)).join("build");
    only_boot_files(&directory)?;
    let chip = Chip::of(root, &entry.chip)?;
    let (bootloader, partition_table) = super::flash_files(&directory, &chip.flash)?;
    crate::around::esp_idf(
        &crate::around::EspIdfChip {
            id: &entry.chip,
            rust_target: &chip.rust_target,
            espflash_chip: &chip.espflash_chip,
            flash: &chip.flash,
        },
        Path::new(&build.application),
        &bootloader,
        &partition_table,
        output,
    )
}

/// The file of a peer chip's flash map, below the repository root.
pub fn peer_flash_file(chip: &str) -> String {
    format!("chips/{chip}/platform/flash.toml")
}

/// `chips/<chip>/platform/flash.toml`: what the image bundle of a chip
/// without a profile, an ESP-IDF peer, needs from the chip.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct PeerFlash {
    schema: u32,
    /// The ISA layer (`chips/espressif/platform/isa/<isa>.toml`) whose
    /// `rust-target` the bundle records.
    isa: String,
    espflash_chip: String,
    flash: oer_espressif_facts::FlashMap,
}

/// The identity and flash map the bundle of a catalog image's chip records.
struct Chip {
    rust_target: String,
    espflash_chip: String,
    flash: oer_espressif_facts::FlashMap,
}

impl Chip {
    /// `chip` of the tree at `root`: its profile when it has one, otherwise
    /// its peer flash file. A chip with a profile and a flash file is
    /// refused, so one chip has one source.
    fn of(root: &Path, chip: &str) -> Result<Self> {
        let file = root.join(peer_flash_file(chip));
        let profiled = oer_chip_profile::supported(root)?
            .iter()
            .any(|supported| supported == chip);
        if profiled {
            if file.exists() {
                return Err(format!(
                    "{chip} has a chip profile and {}: one chip names its flash map once",
                    peer_flash_file(chip)
                )
                .into());
            }
            let profile = crate::profile(root, chip)?;
            return Ok(Self {
                rust_target: profile.facts.rust_target,
                espflash_chip: profile.facts.espflash_chip,
                flash: profile
                    .facts
                    .flash
                    .ok_or_else(|| format!("{chip} names no flash map"))?,
            });
        }
        let text = std::fs::read_to_string(&file).map_err(|error| {
            format!(
                "{chip} has no chip profile and no {}: {error}",
                peer_flash_file(chip)
            )
        })?;
        let peer: PeerFlash =
            toml::from_str(&text).map_err(|error| format!("{}: {error}", file.display()))?;
        if peer.schema != 1 {
            return Err(format!("{}: unsupported schema {}", file.display(), peer.schema).into());
        }
        let isa_file = root.join(format!("chips/espressif/platform/isa/{}.toml", peer.isa));
        let isa_text = std::fs::read_to_string(&isa_file)
            .map_err(|error| format!("{}: {error}", isa_file.display()))?;
        let isa: toml::Table = toml::from_str(&isa_text)
            .map_err(|error| format!("{}: {error}", isa_file.display()))?;
        let rust_target = isa
            .get("rust-target")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("the ISA layer {} names no rust-target", peer.isa))?
            .to_owned();
        Ok(Self {
            rust_target,
            espflash_chip: peer.espflash_chip,
            flash: peer.flash,
        })
    }
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
            "firmware/peers/chip-b-ieee802154",
            "image = \"ieee802154-peer\"\nchip = \"chip-b\"\npins = \"chip-a\"\n",
        );
        project(
            root.path(),
            "firmware/vendor/chip-a/calibration",
            "image = \"vendor-calibration\"\nchip = \"chip-a\"\npins = \"chip-a\"\n",
        );
        std::fs::create_dir_all(root.path().join("firmware/peers/untracked")).unwrap();
        let entries = entries(root.path()).unwrap();
        assert_eq!(
            entries,
            [
                Entry {
                    image: "ieee802154-peer".into(),
                    chip: "chip-b".into(),
                    pins: "chip-a".into(),
                    hold: None,
                    directory: "firmware/peers/chip-b-ieee802154".into(),
                },
                Entry {
                    image: "vendor-calibration".into(),
                    chip: "chip-a".into(),
                    pins: "chip-a".into(),
                    hold: None,
                    directory: "firmware/vendor/chip-a/calibration".into(),
                },
            ]
        );
        assert_eq!(entries[0].project(root.path()).name, "chip-b-ieee802154");
        assert!(entry(&entries, "missing").is_err());
        project(
            root.path(),
            "firmware/peers/copy",
            "image = \"ieee802154-peer\"\nchip = \"chip-b\"\npins = \"chip-a\"\n",
        );
        assert!(
            super::entries(root.path()).is_err(),
            "image names are unique"
        );
    }

    #[test]
    fn the_tracked_catalog_names_its_projects() {
        let repository = crate::tests::repository();
        let root = repository.path();
        let entries = entries(&root).unwrap();
        for image in ["ieee802154-peer", "vendor-calibration"] {
            assert!(entries.iter().any(|entry| entry.image == image), "{image}");
        }
    }

    /// The catalog entry `image` of the fixture tree and a finished ESP-IDF
    /// build of it holding the files `flasher_args.json` names.
    fn built_peer(root: &Path, image: &str) -> (Entry, idf::Build) {
        let entries = entries(root).unwrap();
        let entry = entry(&entries, image).unwrap();
        let entry = Entry {
            image: entry.image.clone(),
            chip: entry.chip.clone(),
            pins: entry.pins.clone(),
            hold: None,
            directory: entry.directory.clone(),
        };
        let build = idf::output(root, &entry.project(root)).join("build");
        std::fs::create_dir_all(&build).unwrap();
        let chip = crate::encode::chip(&entry.chip).unwrap();
        let mut bootloader = vec![0; 24];
        bootloader[0] = 0xe9;
        bootloader[1] = 1;
        bootloader[12..14].copy_from_slice(&chip.id().to_le_bytes());
        std::fs::write(build.join("bootloader.bin"), &bootloader).unwrap();
        let table = Chip::of(root, &entry.chip).unwrap().flash.partitions;
        std::fs::write(
            build.join("partition-table.bin"),
            crate::encode::partition_table(&root.join(table)).unwrap(),
        )
        .unwrap();
        std::fs::write(build.join("app.bin"), b"application").unwrap();
        std::fs::write(
            build.join("flasher_args.json"),
            r#"{
                "flash_files": {"0x2000": "bootloader.bin", "0x8000": "partition-table.bin", "0x10000": "app.bin"},
                "bootloader": {"offset": "0x2000", "file": "bootloader.bin"},
                "partition-table": {"offset": "0x8000", "file": "partition-table.bin"},
                "app": {"offset": "0x10000", "file": "app.bin"}
            }"#,
        )
        .unwrap();
        let idf_build = serde_json::from_value(serde_json::json!({
            "chip": entry.chip,
            "project": entry.project(root).name,
            "idf_revision": "r",
            "overrides": [],
            "application": build.join("app.bin"),
            "application_sha256": "00",
            "elf": build.join("app.elf"),
            "recipe": "r",
        }))
        .unwrap();
        (entry, idf_build)
    }

    #[test]
    fn a_peer_chip_without_a_profile_bundles_with_its_flash_file() {
        let repository = crate::tests::repository();
        let root = repository.path();
        assert!(
            !oer_chip_profile::supported(root)
                .unwrap()
                .contains(&"esp32c5".to_owned())
        );
        let (entry, build) = built_peer(root, "ble-dtm-peer");
        let output = tempfile::tempdir().unwrap();
        let bundle = bundle(root, &entry, &build, &output.path().join("bundle")).unwrap();
        assert_eq!(bundle.chip, "esp32c5");
        assert_eq!(bundle.boot, crate::Boot::EspIdfBootloader);
        assert_eq!(bundle.rust_target, "riscv32imac-unknown-none-elf");
        assert_eq!(bundle.flash.start, oer_espressif_facts::Start::PowerOn);
        assert_eq!(
            bundle
                .segments()
                .iter()
                .map(|segment| segment.offset)
                .collect::<Vec<_>>(),
            [0x2000, 0x8000, 0x1_0000]
        );
        assert_eq!(
            crate::bundle::ImageBundle::load(&output.path().join("bundle")).unwrap(),
            bundle
        );
    }

    #[test]
    fn a_chip_names_its_flash_map_once() {
        let repository = crate::tests::repository();
        let root = repository.path();
        // Neither a profile nor a flash file.
        let error = Chip::of(root, "esp32x9").err().unwrap().to_string();
        assert!(error.contains("no chip profile"), "{error}");
        // Both a profile and a flash file.
        let file = root.join(peer_flash_file("esp32s31"));
        std::fs::write(&file, "schema = 1\n").unwrap();
        let error = Chip::of(root, "esp32s31").err().unwrap().to_string();
        assert!(error.contains("names its flash map once"), "{error}");
        // A flash file with an unknown key or schema is refused.
        let file = root.join(peer_flash_file("esp32c5"));
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, text.replace("schema = 1", "schema = 2")).unwrap();
        let error = Chip::of(root, "esp32c5").err().unwrap().to_string();
        assert!(error.contains("unsupported schema 2"), "{error}");
        std::fs::write(&file, format!("{text}extra = 1\n")).unwrap();
        assert!(Chip::of(root, "esp32c5").is_err());
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
