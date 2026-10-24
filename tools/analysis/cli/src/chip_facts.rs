//! Refuse pins before an older executable tries to parse newer chip facts.

use std::path::Path;

use oer_analysis_riscv_model::Result;

include!(concat!(env!("OUT_DIR"), "/chip_facts.rs"));

pub fn check(root: &Path, chip: &str) -> Result<()> {
    let path = root.join("chips").join(chip).join("platform/chip.toml");
    let current = match std::fs::read(&path) {
        Ok(current) => current,
        // Let the pin loader report unsupported chips with its supported list.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(super::io_error(error)),
    };
    if PROFILES
        .iter()
        .any(|(id, built)| *id == chip && *built == current)
    {
        return Ok(());
    }
    Err(super::invalid(&format!(
        "Blobray is stale for {}; run `cargo blobray ...` from the repository root to rebuild it",
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn current_chip_facts_pass_the_pin_loader() {
        let directory = tempfile::tempdir().unwrap();
        for (chip, bytes) in PROFILES {
            write(
                directory.path(),
                &format!("chips/{chip}/platform/chip.toml"),
                bytes,
            );
            write(
                directory.path(),
                &format!("chips/{chip}/vendor/artifacts.toml"),
                b"schema = 1\nsource = []\nartifact = []\n",
            );
            check(directory.path(), chip).unwrap();
            // This is the same profile parser used by --input ROLE=pin:CHIP/ID,
            // including core-start, table-class and every other current key.
            oer_tool_pins::Manifest::load(directory.path(), chip).unwrap();
        }
    }

    #[test]
    fn a_fact_added_after_build_is_refused_with_a_rebuild_command() {
        let directory = tempfile::tempdir().unwrap();
        let (chip, built) = PROFILES.first().unwrap();
        let path = format!("chips/{chip}/platform/chip.toml");
        let mut changed = built.to_vec();
        changed.extend_from_slice(b"\n[new-fact]\nvalue = 1\n");
        write(directory.path(), &path, &changed);
        let error = check(directory.path(), chip).unwrap_err().to_string();
        assert!(error.contains("Blobray is stale"), "{error}");
        assert!(error.contains(&path), "{error}");
        assert!(error.contains("cargo blobray"), "{error}");
        write(directory.path(), &path, built);
        check(directory.path(), chip).unwrap();
    }

    #[test]
    fn a_chip_added_after_build_is_also_refused() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "chips/new-chip/platform/chip.toml",
            b"schema = 1\n",
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
