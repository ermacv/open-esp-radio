//! Temporary fixture trees for the model's tests.

use std::fs;

use tempfile::TempDir;

/// A temporary directory holding `files` as `(path, contents)`.
pub fn tree(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    dir
}
