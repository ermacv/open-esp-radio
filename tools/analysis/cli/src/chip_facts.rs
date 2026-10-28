//! Refuse pins before an older executable tries to parse newer chip facts:
//! every layer file a chip's facts read must be the one this build compiled
//! in (`oer_espressif_facts::tracked`).

use std::path::Path;

use oer_analysis_riscv_model::Result;
use oer_espressif_facts::{Source, tracked};

/// The layer files below a repository root, as they are now.
struct Tree<'a>(&'a Path);

impl Source for Tree<'_> {
    fn read(&mut self, path: &str) -> oer_espressif_facts::Result<String> {
        std::fs::read_to_string(self.0.join(path))
            .map_err(|error| format!("{path}: {error}").into())
    }

    fn files(&mut self, directory: &str) -> oer_espressif_facts::Result<Vec<String>> {
        let Ok(entries) = std::fs::read_dir(self.0.join(directory)) else {
            return Ok(Vec::new());
        };
        Ok(entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect())
    }
}

pub fn check(root: &Path, chip: &str) -> Result<()> {
    let chip_file = oer_espressif_facts::chip_file(chip);
    // Let the pin loader report a chip without pins with the chips that have them.
    if !root.join(&chip_file).is_file() {
        return Ok(());
    }
    let stale = |file: &str| {
        super::invalid(&format!(
            "Blobray is stale for {}; run `cargo blobray ...` from the repository root to rebuild it",
            root.join(file).display()
        ))
    };
    let mut tree = Tree(root);
    let files = oer_espressif_facts::top_layer(&mut tree, chip)
        .and_then(|top| oer_espressif_facts::chain(&mut tree, &top))
        .map_err(|_| stale(&chip_file))?;
    for file in files {
        let current = std::fs::read_to_string(root.join(&file)).map_err(super::io_error)?;
        if tracked::text(&file) != Some(current.as_str()) {
            return Err(stale(&file));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn tracked_tree() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for (path, text) in tracked::FILES {
            write(directory.path(), path, text.as_bytes());
        }
        directory
    }

    #[test]
    fn current_chip_facts_pass_the_pin_loader() {
        let directory = tracked_tree();
        for chip in tracked::ids() {
            write(
                directory.path(),
                &format!("chips/{chip}/vendor/artifacts.toml"),
                b"schema = 1\nsource = []\nartifact = []\n",
            );
            check(directory.path(), chip).unwrap();
            // This is the same profile parser used by --input ROLE=pin:CHIP/ID,
            // including core-start, table-class and every other current key.
            oer_pins_artifacts::Manifest::load(directory.path(), chip).unwrap();
        }
    }

    #[test]
    fn a_fact_added_after_build_is_refused_with_a_rebuild_command() {
        let directory = tracked_tree();
        let chip = tracked::ids()[0];
        for path in [
            oer_espressif_facts::chip_file(chip),
            "chips/espressif/platform/family.toml".to_owned(),
        ] {
            let built = tracked::text(&path).unwrap();
            write(
                directory.path(),
                &path,
                format!("{built}\n[new-fact]\nvalue = 1\n").as_bytes(),
            );
            let error = check(directory.path(), chip).unwrap_err().to_string();
            assert!(error.contains("Blobray is stale"), "{error}");
            assert!(error.contains(&path), "{error}");
            assert!(error.contains("cargo blobray"), "{error}");
            write(directory.path(), &path, built.as_bytes());
            check(directory.path(), chip).unwrap();
        }
    }

    #[test]
    fn a_layer_or_chip_added_after_build_is_also_refused() {
        let directory = tracked_tree();
        let chip = tracked::ids()[0];
        write(
            directory.path(),
            &format!("chips/{chip}/platform/boards/another.toml"),
            b"schema = 2\n",
        );
        let error = check(directory.path(), chip).unwrap_err();
        assert!(error.to_string().contains("Blobray is stale"), "{error}");

        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "chips/new-chip/platform/chip.toml",
            b"schema = 2\n",
        );
        let error = check(directory.path(), "new-chip").unwrap_err();
        assert!(error.to_string().contains("Blobray is stale"), "{error}");
    }

    #[test]
    fn an_unsupported_chip_is_left_to_the_pin_loader() {
        let directory = tempfile::tempdir().unwrap();
        check(directory.path(), "no-such-chip").unwrap();
    }
}
