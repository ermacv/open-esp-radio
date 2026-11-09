//! The identity of the bundle the standard `cargo fw build esp32s31-rx-rebind`
//! produced, from that build's own records in the bundle directory:
//! `sizes.json`'s provenance (package, commit, dirty) and `bundle.json`'s
//! flash files. Nothing is taken from the launching directory, and a bundle
//! of another image, or one whose application differs from its records, is
//! refused.

use serde_json::Value;
use std::path::Path;

/// The probe package the bundle must have been built from.
pub const PACKAGE: &str = "oer-hil-probes-esp32s31-rx-rebind";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identity {
    pub commit: String,
    pub dirty: bool,
    pub application_sha256: String,
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// The identity the records in `bundle` state, checked against each other
/// and against `application.bin` itself.
pub fn identity(bundle: &Path) -> Result<Identity, String> {
    let sizes = read_json(&bundle.join("sizes.json"))?;
    let manifest = read_json(&bundle.join("bundle.json"))?;
    let provenance = sizes
        .get("provenance")
        .ok_or("sizes.json has no provenance")?;
    let text = |name: &str| {
        provenance
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("sizes.json provenance has no {name}"))
    };
    let package = text("package")?;
    if package != PACKAGE {
        return Err(format!(
            "the bundle was built from `{package}`, not `{PACKAGE}`"
        ));
    }
    let commit = text("commit")?;
    let dirty = provenance
        .get("dirty")
        .and_then(Value::as_bool)
        .ok_or("sizes.json provenance has no dirty flag")?;
    let application = manifest
        .get("flash_files")
        .and_then(Value::as_array)
        .and_then(|files| {
            files
                .iter()
                .find(|file| file.get("file").and_then(Value::as_str) == Some("application.bin"))
        })
        .ok_or("bundle.json lists no application.bin")?;
    let application_sha256 = application
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or("bundle.json gives application.bin no SHA-256")?
        .to_owned();
    let listed_bytes = application.get("bytes").and_then(Value::as_u64);
    let sized_bytes = sizes
        .pointer("/image_files/application.bin")
        .and_then(Value::as_u64);
    if listed_bytes.is_none() || listed_bytes != sized_bytes {
        return Err("bundle.json and sizes.json describe different applications".to_owned());
    }
    let actual = oer_tools_os_durable::sha256_file(&bundle.join("application.bin"))
        .map_err(|error| format!("application.bin: {error}"))?;
    if actual != application_sha256 {
        return Err(format!(
            "application.bin is {actual}, the bundle's manifest lists {application_sha256}"
        ));
    }
    Ok(Identity {
        commit,
        dirty,
        application_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bundle(directory: &Path, package: &str, application: &[u8], listed_sha256: Option<&str>) {
        std::fs::write(directory.join("application.bin"), application).unwrap();
        let sha256 = listed_sha256
            .map(str::to_owned)
            .unwrap_or_else(|| oer_tools_os_durable::sha256_bytes(application));
        let manifest = json!({
            "schema": 8,
            "flash_files": [
                {"file": "bootloader.bin", "sha256": "aa", "bytes": 1},
                {"file": "application.bin", "sha256": sha256, "bytes": application.len()},
            ],
        });
        let sizes = json!({
            "schema": 2,
            "provenance": {"commit": "834fd061621c", "dirty": false, "package": package},
            "image_files": {"application.bin": application.len()},
        });
        std::fs::write(directory.join("bundle.json"), manifest.to_string()).unwrap();
        std::fs::write(directory.join("sizes.json"), sizes.to_string()).unwrap();
    }

    #[test]
    fn the_identity_is_the_builds_own_record() {
        let directory = tempfile::tempdir().unwrap();
        bundle(directory.path(), PACKAGE, b"probe", None);
        let identity = identity(directory.path()).unwrap();
        assert_eq!(identity.commit, "834fd061621c");
        assert!(!identity.dirty);
        assert_eq!(
            identity.application_sha256,
            oer_tools_os_durable::sha256_bytes(b"probe")
        );
    }

    #[test]
    fn a_bundle_of_another_image_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        bundle(
            directory.path(),
            "oer-hil-probes-esp32s31-rx-chain",
            b"probe",
            None,
        );
        assert!(identity(directory.path()).is_err());
    }

    #[test]
    fn an_application_that_differs_from_its_manifest_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        bundle(directory.path(), PACKAGE, b"probe", Some("00"));
        assert!(identity(directory.path()).is_err());
    }

    #[test]
    fn records_of_different_builds_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        bundle(directory.path(), PACKAGE, b"probe", None);
        let sizes = json!({
            "schema": 2,
            "provenance": {"commit": "834fd061621c", "dirty": false, "package": PACKAGE},
            "image_files": {"application.bin": 99},
        });
        std::fs::write(directory.path().join("sizes.json"), sizes.to_string()).unwrap();
        assert!(identity(directory.path()).is_err());
    }

    #[test]
    fn a_directory_without_records_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        assert!(identity(directory.path()).is_err());
    }
}
