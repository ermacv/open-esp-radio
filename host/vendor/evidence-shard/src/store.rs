//! The one reader and writer of shard files.
use crate::{Index, Result, SHARD_EXTENSION};
use std::path::{Path, PathBuf};

/// The file of `scenario`'s shard in `directory`.
pub fn path(directory: &Path, scenario: &str) -> PathBuf {
    directory.join(format!("{scenario}.{SHARD_EXTENSION}"))
}

/// Write `index` as its scenario's shard of the index at `directory`,
/// creating the directory; returns the file.
pub fn write(directory: &Path, index: &Index) -> Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let path = path(directory, &index.scenario);
    let mut bytes = serde_json::to_vec_pretty(index)?;
    bytes.push(b'\n');
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// The shard `name` in `directory`, when it exists and parses. A shard git
/// left conflicted, or one of another schema, reads as none.
pub fn read(directory: &Path, name: &str) -> Option<Index> {
    std::fs::read_to_string(path(directory, name))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

/// The scenario names of the shard files in `directory`, sorted; a missing
/// directory holds none.
pub fn names(directory: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };
    let mut names = vec![];
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some(SHARD_EXTENSION) {
            continue;
        }
        names.push(
            path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or("shard without a name")?
                .to_owned(),
        );
    }
    names.sort();
    Ok(names)
}

/// Every shard of `directory` that parses, by name.
pub fn shards(directory: &Path) -> Result<Vec<Index>> {
    Ok(names(directory)?
        .iter()
        .filter_map(|name| read(directory, name))
        .collect())
}

#[cfg(test)]
pub(crate) mod tests;
