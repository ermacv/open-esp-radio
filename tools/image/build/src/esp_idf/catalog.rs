//! The catalog of tracked ESP-IDF firmware the stand's boards run, and its
//! builds.
//!
//! Every ESP-IDF project with a `firmware.toml` beside its `CMakeLists.txt`
//! is an entry: peers in `tools/lab/peers/<project>/` and vendor references in
//! `tools/vendor/references/<chip>/<project>/`. Each is an application that
//! its own ESP-IDF bootloader loads (`Boot::EspIdfBootloader`), the only
//! images that do not boot staged. The manifest names the image (its name in
//! the board journal), the target chip and the chip whose `artifacts.toml`
//! pins the ESP-IDF; every entry builds against that one pin through
//! `oer_tools_image_esp_idf`. A project that builds from files outside its
//! directory (another entry's source or configuration) names them in its
//! manifest's `shared`, so they take part in its build recipe: a change to
//! one makes every entry that reads it not current.
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;
use oer_tools_image_esp_idf as idf;

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
    /// Files outside the project directory its build reads, relative to
    /// it: they take part in its build recipe.
    #[serde(default)]
    shared: Vec<PathBuf>,
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
    /// Files outside the project directory its build reads, relative to it.
    pub shared: Vec<PathBuf>,
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
            shared: self.shared.clone(),
        }
    }
}

/// Every catalog entry below `root`, by image name.
pub fn entries(root: &Path) -> Result<Vec<Entry>> {
    let mut directories = subdirectories(&root.join("tools/lab/peers"))?;
    for chip in subdirectories(&root.join("tools/vendor/references"))? {
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
            shared: manifest.shared,
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
        .map_err(|error| format!("{error}; run `cargo fw idf build {image}`").into())
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
) -> Result<oer_tools_image_bundle::ImageBundle> {
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
    format!("chips/{chip}/facts/flash.toml")
}

/// `chips/<chip>/facts/flash.toml`: what the image bundle of a chip
/// without a profile, an ESP-IDF peer, needs from the chip.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct PeerFlash {
    schema: u32,
    /// The ISA layer (`chips/family/facts/layers/isa/<isa>.toml`) whose
    /// `rust-target` the bundle records.
    isa: String,
    espflash_chip: String,
    flash: oer_chips_family_facts::FlashMap,
}

/// The identity and flash map the bundle of a catalog image's chip records.
struct Chip {
    rust_target: String,
    espflash_chip: String,
    flash: oer_chips_family_facts::FlashMap,
}

impl Chip {
    /// `chip` of the tree at `root`: its profile when it has one, otherwise
    /// its peer flash file. A chip with a profile and a flash file is
    /// refused, so one chip has one source.
    fn of(root: &Path, chip: &str) -> Result<Self> {
        let file = root.join(peer_flash_file(chip));
        let profiled = oer_tools_chip_profile::supported(root)?
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
        let isa_file = root.join(format!("chips/family/facts/layers/isa/{}.toml", peer.isa));
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

    /// Record a build of `entry` that followed its current recipe.
    fn record_current_build(root: &Path, entry: &Entry) {
        let project = entry.project(root);
        let output = idf::output(root, &project);
        std::fs::create_dir_all(&output).unwrap();
        let recipe = idf::recipe(root, &entry.pins, &project).unwrap();
        let build: idf::Build = serde_json::from_value(serde_json::json!({
            "chip": entry.chip,
            "project": project.name,
            "idf_revision": "aa",
            "overrides": [],
            "application": "",
            "application_sha256": "",
            "elf": "",
            "recipe": recipe,
        }))
        .unwrap();
        std::fs::write(
            output.join("build.json"),
            serde_json::to_vec(&build).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn a_shared_file_makes_every_entry_that_reads_it_not_current_when_it_changes() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let pins = root.join("chips/chip-a/vendor/artifacts.toml");
        std::fs::create_dir_all(pins.parent().unwrap()).unwrap();
        std::fs::write(
            &pins,
            "schema = 1\nartifact = []\n\n[[source]]\nid = \"esp-idf\"\nkind = \"git\"\n\
             repository = \"https://example.invalid/esp-idf\"\nrevision = \"aa\"\n",
        )
        .unwrap();
        const TABLE: &str = "../../../../chips/chip-a/facts/partitions/applications.csv";
        project(
            root,
            "tools/lab/peers/socket",
            &format!(
                "image = \"socket-peer\"\nchip = \"chip-a\"\npins = \"chip-a\"\n\
                 shared = [\"{TABLE}\"]\n"
            ),
        );
        project(
            root,
            "tools/lab/peers/fast",
            &format!(
                "image = \"fast-peer\"\nchip = \"chip-a\"\npins = \"chip-a\"\n\
                 shared = [\"../socket/main/peer.c\", \"../socket/sdkconfig.defaults\", \"{TABLE}\"]\n"
            ),
        );
        let socket = root.join("tools/lab/peers/socket");
        std::fs::create_dir_all(socket.join("main")).unwrap();
        std::fs::write(socket.join("main/peer.c"), "#define PROTOCOL_VERSION 2\n").unwrap();
        std::fs::write(
            socket.join("sdkconfig.defaults"),
            format!("CONFIG_PARTITION_TABLE_CUSTOM_FILENAME=\"{TABLE}\"\n"),
        )
        .unwrap();
        let table = root.join("chips/chip-a/facts/partitions/applications.csv");
        std::fs::create_dir_all(table.parent().unwrap()).unwrap();
        std::fs::write(&table, "ota_0, app, ota_0, 0x20000, 0x400000,\n").unwrap();
        let entries = entries(root).unwrap();
        for entry in &entries {
            record_current_build(root, entry);
        }
        let current = |image: &str| super::current(root, image).is_ok();
        assert!(current("socket-peer") && current("fast-peer"));
        for (path, changed) in [
            (socket.join("main/peer.c"), "#define PROTOCOL_VERSION 3\n"),
            (socket.join("sdkconfig.defaults"), "CONFIG_A=n\n"),
            (table.clone(), "ota_0, app, ota_0, 0x20000, 0x300000,\n"),
        ] {
            let file = path.display();
            let before = std::fs::read(&path).unwrap();
            std::fs::write(&path, changed).unwrap();
            assert!(!current("socket-peer"), "{file} changed: socket current");
            assert!(!current("fast-peer"), "{file} changed: fast current");
            std::fs::write(&path, &before).unwrap();
            assert!(
                current("socket-peer") && current("fast-peer"),
                "{file} restored"
            );
            std::fs::remove_file(&path).unwrap();
            assert!(!current("socket-peer"), "{file} removed: socket current");
            let error = super::current(root, "fast-peer").err().unwrap().to_string();
            assert!(error.contains("shared file"), "{file} removed: {error}");
            std::fs::write(&path, &before).unwrap();
            assert!(
                current("socket-peer") && current("fast-peer"),
                "{file} restored"
            );
        }
    }

    #[test]
    fn peers_and_vendor_references_form_one_catalog() {
        let root = tempfile::tempdir().unwrap();
        project(
            root.path(),
            "tools/lab/peers/chip-b-ieee802154",
            "image = \"ieee802154-peer\"\nchip = \"chip-b\"\npins = \"chip-a\"\n",
        );
        project(
            root.path(),
            "tools/vendor/references/chip-a/calibration",
            "image = \"vendor-calibration\"\nchip = \"chip-a\"\npins = \"chip-a\"\n",
        );
        std::fs::create_dir_all(root.path().join("tools/lab/peers/untracked")).unwrap();
        let entries = entries(root.path()).unwrap();
        assert_eq!(
            entries,
            [
                Entry {
                    image: "ieee802154-peer".into(),
                    chip: "chip-b".into(),
                    pins: "chip-a".into(),
                    hold: None,
                    directory: "tools/lab/peers/chip-b-ieee802154".into(),
                    shared: vec![],
                },
                Entry {
                    image: "vendor-calibration".into(),
                    chip: "chip-a".into(),
                    pins: "chip-a".into(),
                    hold: None,
                    directory: "tools/vendor/references/chip-a/calibration".into(),
                    shared: vec![],
                },
            ]
        );
        assert_eq!(entries[0].project(root.path()).name, "chip-b-ieee802154");
        assert!(entry(&entries, "missing").is_err());
        project(
            root.path(),
            "tools/lab/peers/copy",
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
        for image in ["ieee802154-peer", "phy-tracking-reference"] {
            assert!(entries.iter().any(|entry| entry.image == image), "{image}");
        }
    }

    /// The paths a CMake file names in quotes and starting with `..`, and
    /// the `SDKCONFIG_DEFAULTS` files it sets.
    fn cmake_paths(cmake: &str) -> (Vec<String>, Vec<String>) {
        let quoted = |line: &str| -> Vec<String> {
            line.split('"')
                .skip(1)
                .step_by(2)
                .filter(|quoted| quoted.starts_with("../") || quoted.contains("}/../"))
                .map(|quoted| {
                    quoted
                        .split_once("}/")
                        .map_or(quoted, |(_, path)| path)
                        .to_owned()
                })
                .collect()
        };
        let defaults = cmake
            .lines()
            .filter(|line| line.contains("SDKCONFIG_DEFAULTS"))
            .flat_map(quoted)
            .collect();
        (quoted(cmake), defaults)
    }

    /// The paths an sdkconfig defaults file names as quoted `CONFIG_`
    /// values starting with `..`, relative to the project directory (as
    /// ESP-IDF resolves `CONFIG_PARTITION_TABLE_CUSTOM_FILENAME`).
    fn sdkconfig_paths(defaults: &str) -> Vec<String> {
        defaults
            .lines()
            .filter_map(|line| line.trim().strip_prefix("CONFIG_")?.split_once('='))
            .filter_map(|(_, value)| value.strip_prefix('"')?.strip_suffix('"'))
            .filter(|value| value.starts_with("../"))
            .map(str::to_owned)
            .collect()
    }

    /// Each file outside `entry`'s directory its CMake files or sdkconfig
    /// defaults read that its `shared` does not name, or that cannot be
    /// found.
    fn unnamed_outside_files(root: &Path, entry: &Entry) -> Vec<String> {
        let project = root.join(&entry.directory).canonicalize().unwrap();
        let mut problems = Vec::new();
        let shared = entry
            .shared
            .iter()
            .filter_map(|file| match project.join(file).canonicalize() {
                Ok(path) => Some(path),
                Err(error) => {
                    problems.push(format!("shared {}: {error}", file.display()));
                    None
                }
            })
            .collect::<Vec<_>>();
        let mut reads = Vec::new();
        let mut defaults = vec![project.join("sdkconfig.defaults")];
        for (directory, file) in [
            (project.clone(), "CMakeLists.txt"),
            (project.join("main"), "main/CMakeLists.txt"),
        ] {
            let Ok(cmake) = std::fs::read_to_string(project.join(file)) else {
                continue;
            };
            let (paths, defaults_files) = cmake_paths(&cmake);
            for path in paths {
                reads.push((format!("{file} names {path}"), directory.join(&path)));
            }
            defaults.extend(defaults_files.iter().map(|path| directory.join(path)));
        }
        for file in defaults {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            for path in sdkconfig_paths(&text) {
                reads.push((
                    format!("{} names {path}", file.display()),
                    project.join(&path),
                ));
            }
        }
        for (origin, path) in reads {
            let resolved = match path.canonicalize() {
                Ok(resolved) => resolved,
                Err(error) => {
                    problems.push(format!("{origin}: {error}"));
                    continue;
                }
            };
            if resolved.is_file() && !resolved.starts_with(&project) && !shared.contains(&resolved)
            {
                problems.push(format!("{origin}, which `shared` does not name"));
            }
        }
        problems
    }

    #[test]
    fn every_tracked_entry_names_the_outside_files_its_build_reads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let root = root.canonicalize().unwrap();
        for entry in entries(&root).unwrap() {
            let problems = unnamed_outside_files(&root, &entry);
            assert!(problems.is_empty(), "{}: {problems:#?}", entry.image);
        }
    }

    #[test]
    fn the_outside_check_follows_sources_and_sdkconfig_references() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let table = root.join("chips/chip-a/facts/partitions/applications.csv");
        std::fs::create_dir_all(table.parent().unwrap()).unwrap();
        std::fs::write(&table, "ota_0, app, ota_0, 0x20000, 0x400000,\n").unwrap();
        let reference = "\"../../../../chips/chip-a/facts/partitions/applications.csv\"";
        let check = |manifest: &str, cmake: &str, main: &str| {
            let directory = root.join("tools/lab/peers/peer");
            let _ = std::fs::remove_dir_all(&directory);
            project(root, "tools/lab/peers/peer", manifest);
            std::fs::create_dir_all(directory.join("main")).unwrap();
            std::fs::write(directory.join("CMakeLists.txt"), cmake).unwrap();
            std::fs::write(directory.join("main/CMakeLists.txt"), main).unwrap();
            std::fs::write(
                directory.join("sdkconfig.defaults"),
                format!("CONFIG_PARTITION_TABLE_CUSTOM_FILENAME={reference}\n"),
            )
            .unwrap();
            let entries = entries(root).unwrap();
            unnamed_outside_files(root, &entries[0])
        };
        let plain = "image = \"peer\"\nchip = \"chip-a\"\npins = \"chip-a\"\n";
        let problems = check(plain, "", "");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("applications.csv"), "{problems:?}");
        let named = format!("{plain}shared = [{reference}]\n");
        assert!(check(&named, "", "").is_empty());
        // A source outside the project, named or not.
        std::fs::create_dir_all(root.join("tools/lab/peers/other/main")).unwrap();
        std::fs::write(root.join("tools/lab/peers/other/main/x.c"), "x").unwrap();
        let main = "idf_component_register(SRCS \"../../other/main/x.c\")\n";
        assert_eq!(check(&named, "", main).len(), 1);
        let both = format!("{plain}shared = [{reference}, \"../other/main/x.c\"]\n");
        assert!(check(&both, "", main).is_empty());
        // A defaults file another project's CMake file names is read too.
        std::fs::write(
            root.join("tools/lab/peers/other/sdkconfig.defaults"),
            format!("CONFIG_PARTITION_TABLE_CUSTOM_FILENAME={reference}\n"),
        )
        .unwrap();
        let cmake =
            "set(SDKCONFIG_DEFAULTS \"${CMAKE_CURRENT_LIST_DIR}/../other/sdkconfig.defaults\")\n";
        let problems = check(&both, cmake, main);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("sdkconfig.defaults"), "{problems:?}");
        let all = format!(
            "{plain}shared = [{reference}, \"../other/main/x.c\", \"../other/sdkconfig.defaults\"]\n"
        );
        assert!(check(&all, cmake, main).is_empty());
        // A shared file that does not exist is a problem too.
        let missing = format!("{plain}shared = [{reference}, \"../other/gone.c\"]\n");
        assert!(
            check(&missing, "", "")
                .iter()
                .any(|problem| problem.contains("gone.c"))
        );
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
            shared: entry.shared.clone(),
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
            !oer_tools_chip_profile::supported(root)
                .unwrap()
                .contains(&"esp32c5".to_owned())
        );
        let (entry, build) = built_peer(root, "ieee802154-peer");
        let output = tempfile::tempdir().unwrap();
        let bundle = bundle(root, &entry, &build, &output.path().join("bundle")).unwrap();
        assert_eq!(bundle.chip, "esp32c5");
        assert_eq!(bundle.boot, crate::Boot::EspIdfBootloader);
        assert_eq!(bundle.rust_target, "riscv32imac-unknown-none-elf");
        assert_eq!(bundle.flash.start, oer_chips_family_facts::Start::PowerOn);
        assert_eq!(
            bundle
                .segments()
                .iter()
                .map(|segment| segment.offset)
                .collect::<Vec<_>>(),
            [0x2000, 0x8000, 0x1_0000]
        );
        assert_eq!(
            oer_tools_image_bundle::ImageBundle::load(&output.path().join("bundle")).unwrap(),
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
