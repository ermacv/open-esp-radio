//! The repository files a firmware image was built from.
//!
//! Cargo writes the source files of every crate linked into a binary to its
//! dep-info file (`<binary>.d`), and each build script's `rerun-if-changed`
//! inputs to its `output`. Together they name the repository files that
//! produced the image, so evidence recorded from it stays valid while those
//! files are unchanged. Files outside the repository belong to locked
//! registry or Git packages and are identified by the lockfiles instead.
//!
//! The build also reads files no dep-info names: the firmware workspaces'
//! manifests and locks, the Cargo configuration and toolchain files, the
//! workspace manifest each compiled package inherits from, the stack policy
//! and partition table, and the code that builds, packs and audits the image:
//! every source of the builder packages' dependency closure, which the
//! caller resolves (`ImageSpec::builder_inputs`), so a new analyzer or
//! linker crate is an input the day it is linked. The record lists those too, so whoever asks what an
//! image depends on (the evidence closure, `check changed`) reads it here
//! instead of keeping its own list.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use crate::Result;

/// Format of `source-inputs.json`. Schema 2 lists every repository file the
/// build read; schema 1 listed only the compiled sources.
pub const SCHEMA: u32 = 2;

/// Repository-relative files the binaries below each `(release directory,
/// binary name)` were built from, plus the manifest and build script of each
/// repository package they compiled.
pub fn collect(repository: &Path, builds: &[(&Path, &str)]) -> Result<BTreeSet<PathBuf>> {
    let repository = repository.canonicalize()?;
    let mut files = BTreeSet::new();
    for (release, binary) in builds {
        let dep_info = fs::read_to_string(release.join(format!("{binary}.d")))?;
        for path in dependencies(&dep_info) {
            if let Some(relative) = inside(&repository, &path) {
                files.insert(relative);
            }
        }
    }
    let packages = packages(&repository, &files)?;
    for directory in packages.values() {
        for file in ["Cargo.toml", "build.rs"] {
            if repository.join(directory).join(file).is_file() {
                files.insert(directory.join(file));
            }
        }
    }
    for (release, _) in builds {
        let Ok(scripts) = fs::read_dir(release.join("build")) else {
            continue;
        };
        for script in scripts.flatten() {
            let Ok(output) = fs::read_to_string(script.path().join("output")) else {
                continue;
            };
            let name = script.file_name().to_string_lossy().into_owned();
            let package = name
                .rsplit_once('-')
                .and_then(|(package, _)| packages.get(package));
            for line in output.lines() {
                let Some(path) = line
                    .strip_prefix("cargo::rerun-if-changed=")
                    .or_else(|| line.strip_prefix("cargo:rerun-if-changed="))
                else {
                    continue;
                };
                let path = Path::new(path);
                let absolute = match (path.is_absolute(), package) {
                    (true, _) => path.to_owned(),
                    (false, Some(directory)) => repository.join(directory).join(path),
                    // A relative input of a registry or Git package.
                    (false, None) => continue,
                };
                if let Some(relative) = inside(&repository, &absolute)
                    && repository.join(&relative).is_file()
                {
                    files.insert(relative);
                }
            }
        }
    }
    Ok(files)
}

/// The files a build read besides the `compiled` sources: each of the
/// `workspaces`' manifest and lock file, every Cargo configuration and
/// toolchain file above those workspaces and the compiled packages, the
/// workspace manifest each compiled package inherits from and the `read`
/// policy files.
pub fn configuration(
    repository: &Path,
    compiled: &BTreeSet<PathBuf>,
    workspaces: &[&Path],
    read: &[&Path],
) -> Result<BTreeSet<PathBuf>> {
    let repository = repository.canonicalize()?;
    let is_file = |path: &Path| repository.join(path).is_file();
    let mut files = BTreeSet::new();
    let packages = packages(&repository, compiled)?;
    for workspace in workspaces {
        for name in ["Cargo.toml", "Cargo.lock"] {
            let path = workspace.join(name);
            if is_file(&path) {
                files.insert(path);
            }
        }
    }
    for directory in workspaces
        .iter()
        .map(|workspace| workspace.to_path_buf())
        .chain(packages.values().cloned())
    {
        for ancestor in directory.ancestors() {
            for name in [
                ".cargo/config.toml",
                ".cargo/config",
                "rust-toolchain.toml",
                "rust-toolchain",
            ] {
                let path = ancestor.join(name);
                if is_file(&path) {
                    files.insert(path);
                }
            }
        }
    }
    for package in packages.values() {
        if let Some(workspace) = package.ancestors().skip(1).find(|ancestor| {
            fs::read_to_string(repository.join(ancestor).join("Cargo.toml"))
                .is_ok_and(|manifest| manifest.contains("[workspace"))
        }) {
            files.insert(workspace.join("Cargo.toml"));
        }
    }
    files.extend(
        read.iter()
            .map(|path| path.to_path_buf())
            .filter(|path| is_file(path)),
    );
    Ok(files)
}

/// Write `files` as `source-inputs.json` below `output`.
pub fn write(output: &Path, files: &BTreeSet<PathBuf>) -> Result<PathBuf> {
    let path = output.join("source-inputs.json");
    oer_durable::atomic_json(
        &path,
        &serde_json::json!({"schema": SCHEMA, "files": files}),
    )?;
    Ok(path)
}

/// The paths of a dep-info file, whose spaces inside a path are escaped.
fn dependencies(dep_info: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in dep_info.lines() {
        let Some((_, list)) = line.split_once(": ") else {
            continue;
        };
        let mut current = String::new();
        let mut characters = list.chars().peekable();
        while let Some(character) = characters.next() {
            match character {
                '\\' if characters.peek() == Some(&' ') => {
                    current.push(' ');
                    characters.next();
                }
                ' ' => {
                    if !current.is_empty() {
                        paths.push(PathBuf::from(std::mem::take(&mut current)));
                    }
                }
                character => current.push(character),
            }
        }
        if !current.is_empty() {
            paths.push(PathBuf::from(current));
        }
    }
    paths
}

/// `path` relative to `repository` after resolving `.` and `..` lexically,
/// when it lies inside it and outside `target/`.
fn inside(repository: &Path, path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            component => normalized.push(component),
        }
    }
    let relative = normalized.strip_prefix(repository).ok()?.to_owned();
    (!relative.starts_with("target") && relative.components().next().is_some()).then_some(relative)
}

/// The repository packages owning `files`, by package name.
fn packages(repository: &Path, files: &BTreeSet<PathBuf>) -> Result<BTreeMap<String, PathBuf>> {
    let mut packages = BTreeMap::new();
    for file in files {
        let Some(directory) = file
            .ancestors()
            .skip(1)
            .find(|directory| repository.join(directory).join("Cargo.toml").is_file())
        else {
            continue;
        };
        let manifest: toml::Value = toml::from_str(&fs::read_to_string(
            repository.join(directory).join("Cargo.toml"),
        )?)?;
        // A workspace manifest has no `[package]`.
        if let Some(name) = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(|name| name.as_str())
        {
            packages.insert(name.to_owned(), directory.to_owned());
        }
    }
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_names_what_the_build_reads_beside_the_sources() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        for (path, content) in [
            ("Cargo.toml", "[workspace]\nmembers = []\n"),
            ("rust-toolchain.toml", ""),
            (".cargo/config.toml", ""),
            ("docs/guide.md", ""),
            (
                "crates/radio/Cargo.toml",
                "[package]\nname = \"oer-radio\"\n",
            ),
            ("crates/radio/src/lib.rs", ""),
            ("hil/targets/chip/Cargo.toml", "[workspace]\nmembers = []\n"),
            ("hil/targets/chip/Cargo.lock", ""),
            ("hil/targets/chip/stack.toml", ""),
        ] {
            write_file(&root.join(path), content);
        }
        let compiled = BTreeSet::from([PathBuf::from("crates/radio/src/lib.rs")]);
        let files = configuration(
            &root,
            &compiled,
            &[Path::new("hil/targets/chip")],
            &[
                Path::new("hil/targets/chip/stack.toml"),
                Path::new("platform/missing.csv"),
            ],
        )
        .unwrap();
        let expected = [
            ".cargo/config.toml",
            "Cargo.toml",
            "hil/targets/chip/Cargo.lock",
            "hil/targets/chip/Cargo.toml",
            "hil/targets/chip/stack.toml",
            "rust-toolchain.toml",
        ];
        assert_eq!(
            files,
            expected.iter().map(PathBuf::from).collect::<BTreeSet<_>>()
        );
    }

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn inputs_are_the_compiled_sources_and_repository_build_script_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repo");
        let root = repository.canonicalize().unwrap_or_else(|_| {
            fs::create_dir_all(&repository).unwrap();
            repository.canonicalize().unwrap()
        });
        write_file(
            &root.join("crates/radio/Cargo.toml"),
            "[package]\nname = \"oer-radio\"\n",
        );
        write_file(&root.join("crates/radio/build.rs"), "");
        write_file(&root.join("crates/radio/src/lib.rs"), "");
        write_file(&root.join("crates/radio/src/my file.rs"), "");
        write_file(&root.join("crates/radio/memory.x"), "");
        write_file(&root.join("crates/other/src/lib.rs"), "");
        // A workspace manifest owns a compiled file without being a package.
        write_file(&root.join("Cargo.toml"), "[workspace]\n");
        write_file(&root.join("tool.rs"), "");
        write_file(&root.join("platform/linker/link.x"), "");
        let release = directory.path().join("target/release");
        write_file(
            &release.join("app.d"),
            &format!(
                "{0}/target/release/app: {0}/crates/radio/src/lib.rs {0}/crates/radio/src/my\\ file.rs {0}/tool.rs /registry/esp-hal/src/lib.rs\n",
                root.display()
            ),
        );
        write_file(
            &release.join("build/oer-radio-0123abcd/output"),
            &format!(
                "cargo:rerun-if-changed=memory.x\ncargo::rerun-if-changed={}/crates/radio/../../platform/linker/link.x\ncargo:rustc-link-arg=-Tlink.x\n",
                root.display()
            ),
        );
        write_file(
            &release.join("build/esp-hal-89ab/output"),
            "cargo:rerun-if-changed=ld/memory.x\n",
        );
        let files = collect(&root, &[(&release, "app")]).unwrap();
        let expected = [
            "crates/radio/Cargo.toml",
            "crates/radio/build.rs",
            "crates/radio/memory.x",
            "crates/radio/src/lib.rs",
            "crates/radio/src/my file.rs",
            "platform/linker/link.x",
            "tool.rs",
        ]
        .map(PathBuf::from);
        assert_eq!(files.into_iter().collect::<Vec<_>>(), expected);
    }
}
