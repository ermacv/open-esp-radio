//! Content-addressed observer builds.
//!
//! The runner's observer record names the build it was compiled from. Nearly
//! every run shares one build, so a record refers to it by digest instead of
//! embedding it: the build lives once in
//! `observers/<build_sha256>.json` beside the runs of a run store, as the
//! exact bytes of `serde_json::to_vec(&build)`. The digest
//! the record names is the SHA-256 of those bytes, so the file's name is its
//! own integrity check; a missing file or one whose bytes do not hash to its
//! name is an error, never an absent identity.
//!
//! The runner produces its record in the [`EMBEDDED`] form, with the build
//! inline, and [`detach`]es it before writing; a stored record is always a
//! reference, and [`attach`] reads only references.
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The directory, beside the runs, holding the builds.
pub const DIRECTORY: &str = "observers";
/// A record that embeds its build: the producer's form before [`detach`]
/// and the reader's after [`attach`], never a stored one.
pub const EMBEDDED: u64 = 1;
/// A record that refers to its build by digest.
pub const REFERENCED: u64 = 2;

/// Whether `text` is a lowercase hexadecimal SHA-256.
pub fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Where the build with `sha256` lives under `directory`.
pub fn path(directory: &Path, sha256: &str) -> Result<PathBuf> {
    if !is_digest(sha256) {
        return Err(format!("observer build digest `{sha256}` is not a SHA-256").into());
    }
    Ok(directory.join(DIRECTORY).join(format!("{sha256}.json")))
}

/// Store `build` under `directory` and return its digest. An existing file
/// is kept when its bytes hash to its name and replaced otherwise; either
/// way its modification time becomes now, so [`collect_garbage`] spares a
/// build a starting run is about to name.
pub fn store(directory: &Path, build: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(build)?;
    let sha256 = oer_tool_base_durable::sha256_bytes(&bytes);
    let file = path(directory, &sha256)?;
    if fs::read(&file)
        .is_ok_and(|existing| oer_tool_base_durable::sha256_bytes(&existing) == sha256)
    {
        fs::File::options()
            .append(true)
            .open(&file)?
            .set_modified(std::time::SystemTime::now())?;
        return Ok(sha256);
    }
    // Concurrent writers of one digest write the same bytes; each renames its
    // own temporary file into place.
    oer_tool_base_durable::atomic_write(&file, &bytes)?;
    Ok(sha256)
}

/// The build with `sha256` under `directory`, after checking that its bytes
/// hash to that digest. A build that is absent is a message; any other
/// failure to read it stays an I/O error, so a reader can tell a broken
/// store from a broken bundle.
pub fn load(directory: &Path, sha256: &str) -> Result<Value> {
    let file = path(directory, sha256)?;
    let bytes = fs::read(&file).map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
        let context = format!("observer build {}", file.display());
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("{context} is missing: {error}").into()
        } else {
            Box::new(std::io::Error::new(
                error.kind(),
                format!("{context}: {error}"),
            ))
        }
    })?;
    if oer_tool_base_durable::sha256_bytes(&bytes) != sha256 {
        return Err(format!(
            "observer build {} does not hash to its name",
            file.display()
        )
        .into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// The digest a record refers to, or embeds a build under.
pub fn build_digest(record: &Value) -> Option<&str> {
    record["build_sha256"].as_str()
}

/// `record` with its build moved into `directory`: an embedded record becomes
/// a reference; a reference is returned unchanged once its build is present.
pub fn detach(record: &Value, directory: &Path) -> Result<Value> {
    match record["schema"].as_u64() {
        Some(EMBEDDED) => {
            let sha256 = store(directory, &record["build"])?;
            if build_digest(record) != Some(sha256.as_str()) {
                return Err("observer record's build does not hash to its build_sha256".into());
            }
            Ok(json!({
                "schema": REFERENCED,
                "executable_sha256": record["executable_sha256"],
                "build_sha256": sha256,
            }))
        }
        Some(REFERENCED) => {
            load(
                directory,
                build_digest(record).ok_or("observer reference names no build")?,
            )?;
            Ok(record.clone())
        }
        _ => Err("observer record has an unknown schema".into()),
    }
}

/// The stored reference `record` with its build inline, loaded from
/// `directory`. The result has the [`EMBEDDED`] form; a stored record that
/// embeds its build is rejected.
pub fn attach(record: &Value, directory: &Path) -> Result<Value> {
    match record["schema"].as_u64() {
        Some(EMBEDDED) => {
            Err("a stored observer record embeds its build; only references are read".into())
        }
        Some(REFERENCED) => {
            let sha256 = build_digest(record).ok_or("observer reference names no build")?;
            Ok(json!({
                "schema": EMBEDDED,
                "executable_sha256": record["executable_sha256"],
                "build_sha256": sha256,
                "build": load(directory, sha256)?,
            }))
        }
        _ => Err("observer record has an unknown schema".into()),
    }
}

/// Remove the builds under `directory` that no digest in `keep` names and
/// that were stored at least `min_age` ago; returns the digests removed.
/// The age protects a build stored by a run whose manifest does not name it
/// yet. Files whose name is not a digest are left for integrity checks to
/// report.
pub fn collect_garbage(
    directory: &Path,
    keep: &BTreeSet<String>,
    min_age: std::time::Duration,
) -> Result<Vec<String>> {
    let entries = match fs::read_dir(directory.join(DIRECTORY)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };
    let mut removed = vec![];
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(sha256) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
            continue;
        };
        let age = entry.metadata()?.modified()?.elapsed().unwrap_or_default();
        if is_digest(sha256) && !keep.contains(sha256) && age >= min_age {
            fs::remove_file(entry.path())?;
            removed.push(sha256.to_owned());
        }
    }
    removed.sort();
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absent build is a message; a build that cannot be read stays an
    /// I/O error, so the evaluator reports it as the store's failure.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_build_stays_an_io_error() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let digest = oer_tool_base_durable::sha256_bytes(b"{}");
        let missing = load(directory.path(), &digest).unwrap_err();
        assert!(!missing.is::<std::io::Error>(), "{missing}");
        let file = path(directory.path(), &digest).unwrap();
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"{}").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read(&file).is_ok();
        let result = load(directory.path(), &digest);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        if !readable {
            assert!(result.unwrap_err().is::<std::io::Error>());
        }
    }

    fn embedded() -> Value {
        let build = json!({"schema": 2, "resolved": {"nodes": [1, 2, 3]}});
        json!({
            "schema": EMBEDDED,
            "executable_sha256": "e".repeat(64),
            "build_sha256": oer_tool_base_durable::sha256_bytes(&serde_json::to_vec(&build).unwrap()),
            "build": build,
        })
    }

    #[test]
    fn a_detached_record_attaches_to_the_same_build() {
        let directory = tempfile::tempdir().unwrap();
        let record = embedded();
        let reference = detach(&record, directory.path()).unwrap();
        assert_eq!(reference["schema"], REFERENCED);
        assert!(reference.get("build").is_none());
        assert_eq!(attach(&reference, directory.path()).unwrap(), record);
        assert_eq!(detach(&reference, directory.path()).unwrap(), reference);
        assert!(
            attach(&record, directory.path()).is_err(),
            "a stored record never embeds its build"
        );
    }

    #[test]
    fn a_missing_or_altered_build_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let reference = detach(&embedded(), directory.path()).unwrap();
        let sha256 = build_digest(&reference).unwrap().to_owned();
        let file = path(directory.path(), &sha256).unwrap();
        fs::write(&file, b"{}").unwrap();
        assert!(attach(&reference, directory.path()).is_err());
        fs::remove_file(&file).unwrap();
        assert!(attach(&reference, directory.path()).is_err());
        assert!(load(directory.path(), "not-a-digest").is_err());
    }

    #[test]
    fn an_embedded_record_with_a_wrong_digest_is_not_stored_under_it() {
        let directory = tempfile::tempdir().unwrap();
        let mut record = embedded();
        record["build_sha256"] = json!("0".repeat(64));
        assert!(detach(&record, directory.path()).is_err());
    }

    #[test]
    fn unreferenced_builds_are_collected() {
        let directory = tempfile::tempdir().unwrap();
        let kept = store(directory.path(), &json!({"a": 1})).unwrap();
        let dropped = store(directory.path(), &json!({"b": 2})).unwrap();
        let keep = BTreeSet::from([kept.clone()]);
        let recent = collect_garbage(
            directory.path(),
            &keep,
            std::time::Duration::from_secs(3600),
        );
        assert!(
            recent.unwrap().is_empty(),
            "a build stored just now is spared"
        );
        let removed = collect_garbage(directory.path(), &keep, std::time::Duration::ZERO).unwrap();
        assert_eq!(removed, [dropped]);
        assert!(load(directory.path(), &kept).is_ok());
    }
}
