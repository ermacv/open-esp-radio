//! Verify every object before exposing build inputs.

use super::*;

/// Materialize only verified regular files into a newly owned build
/// directory, each read from its object in `objects`.
pub(super) fn materialize(
    directory: &Path,
    objects: &Path,
    destination: &Path,
) -> Result<(Snapshot, Manifest)> {
    let snapshot: Snapshot = serde_json::from_slice(&fs::read(directory.join("snapshot.json"))?)?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    if snapshot.schema != SNAPSHOT_SCHEMA
        || manifest.schema != MANIFEST_SCHEMA
        || digest(&serde_json::to_vec(&manifest)?) != snapshot.snapshot_id
    {
        return Err("source snapshot identity mismatch".into());
    }
    let mut roles = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for source in &manifest.sources {
        if !matches!(source.name.as_str(), "repository" | "esp-hal") || !roles.insert(&source.name)
        {
            return Err("invalid or duplicate snapshot source role".into());
        }
        for file in &source.files {
            contained(&file.path)?;
            if !matches!(file.mode, 0o644 | 0o755)
                || !paths.insert(Path::new(&source.name).join(&file.path))
            {
                return Err("invalid or duplicate snapshot input".into());
            }
        }
    }
    if !roles.contains(&"repository".to_owned()) || paths.len() != snapshot.files {
        return Err("incomplete source snapshot manifest".into());
    }
    for source in &manifest.sources {
        for file in &source.files {
            let output = destination.join(&source.name).join(&file.path);
            fs::create_dir_all(output.parent().ok_or("source file has no parent")?)?;
            let stored = object(objects, &file.sha256);
            let mut input = fs::File::open(&stored).map_err(|error| {
                format!("source object {} is missing: {error}", stored.display())
            })?;
            let mut target = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)?;
            // A checkout is transient: a failed build materializes it again,
            // and the digest below checks what was written, so no fsync is
            // needed.
            let copied = std::io::copy(&mut input, &mut target)?;
            drop(target);
            if copied != file.size_bytes
                || oer_tool_base_durable::sha256_file(&output)? != file.sha256
            {
                return Err(format!(
                    "source object {} disagrees with the manifest",
                    stored.display()
                )
                .into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&output, fs::Permissions::from_mode(file.mode))?;
            }
        }
    }
    Ok((snapshot, manifest))
}
