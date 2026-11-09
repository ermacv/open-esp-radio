//! The prepared build's record: written by `prepare` after the image build,
//! read under the lease. The image identity comes from it and from the
//! bundle's own manifest, never from the launching directory's HEAD.

use serde_json::{Value, json};
use std::path::Path;

/// The record's file in the bundle directory.
pub const RECORD: &str = "rx-rebind-prepared.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prepared {
    /// The source revision the build was made from, and whether its tree
    /// had changes.
    pub commit: String,
    pub dirty: bool,
    /// `application.bin`'s SHA-256 as the bundle manifest lists it.
    pub application_sha256: String,
}

/// `application.bin`'s SHA-256 in a bundle manifest (`bundle.json`).
pub fn application_sha256(manifest: &Value) -> Result<String, String> {
    manifest
        .get("flash_files")
        .and_then(Value::as_array)
        .and_then(|files| {
            files
                .iter()
                .find(|file| file.get("file").and_then(Value::as_str) == Some("application.bin"))
        })
        .and_then(|file| file.get("sha256"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "the bundle manifest lists no application.bin SHA-256".to_owned())
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Write the record of the bundle in `bundle`, built from `commit`.
pub fn write(bundle: &Path, commit: &str, dirty: bool) -> Result<Prepared, String> {
    let application_sha256 = application_sha256(&read_json(&bundle.join("bundle.json"))?)?;
    let prepared = Prepared {
        commit: commit.to_owned(),
        dirty,
        application_sha256,
    };
    let record = json!({
        "image": "esp32s31-rx-rebind",
        "commit": prepared.commit,
        "dirty": prepared.dirty,
        "application_sha256": prepared.application_sha256,
    });
    std::fs::write(bundle.join(RECORD), format!("{record:#}\n"))
        .map_err(|error| format!("{RECORD}: {error}"))?;
    Ok(prepared)
}

/// The record of the bundle in `bundle`, refused when the bundle's manifest
/// no longer names the recorded application.
pub fn read(bundle: &Path) -> Result<Prepared, String> {
    let record = read_json(&bundle.join(RECORD))?;
    let field = |name: &str| {
        record
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{RECORD} has no {name}"))
    };
    let prepared = Prepared {
        commit: field("commit")?,
        dirty: record
            .get("dirty")
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("{RECORD} has no dirty flag"))?,
        application_sha256: field("application_sha256")?,
    };
    let current = application_sha256(&read_json(&bundle.join("bundle.json"))?)?;
    if current != prepared.application_sha256 {
        return Err(format!(
            "the bundle changed since it was prepared: application {current}, recorded {}",
            prepared.application_sha256
        ));
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(directory: &Path, sha256: &str) {
        let manifest = json!({
            "schema": 8,
            "flash_files": [
                {"file": "bootloader.bin", "sha256": "aa", "bytes": 1},
                {"file": "application.bin", "sha256": sha256, "bytes": 2},
            ],
        });
        std::fs::write(directory.join("bundle.json"), manifest.to_string()).unwrap();
    }

    #[test]
    fn the_record_names_the_built_revision_and_application() {
        let directory = tempfile::tempdir().unwrap();
        manifest(directory.path(), "bb");
        let written = write(directory.path(), "6177dc9f2", false).unwrap();
        assert_eq!(read(directory.path()), Ok(written.clone()));
        assert_eq!(written.application_sha256, "bb");
        assert_eq!(written.commit, "6177dc9f2");
    }

    #[test]
    fn a_rebuilt_bundle_refuses_its_old_record() {
        let directory = tempfile::tempdir().unwrap();
        manifest(directory.path(), "bb");
        write(directory.path(), "6177dc9f2", true).unwrap();
        manifest(directory.path(), "cc");
        assert!(read(directory.path()).is_err());
    }

    #[test]
    fn a_bundle_without_a_record_is_not_prepared() {
        let directory = tempfile::tempdir().unwrap();
        manifest(directory.path(), "bb");
        assert!(read(directory.path()).is_err());
    }
}
