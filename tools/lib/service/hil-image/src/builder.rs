//! The sources of the code that builds a HIL image, which join its source
//! inputs: the dependency closure of the image pipeline, its checks and this
//! builder, as Cargo resolves the workspace.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use oer_tool_service_hil_source_snapshot::checkout::{Edges, IndexSnapshot, package_closure};

use crate::Result;

/// The sources of the code that builds, packs and audits an image: every
/// repository package the `roots` (the builder packages) reach through
/// their normal and build dependencies, with every optional dependency; of
/// each its manifest, build script and `src/`, without tests and prose.
pub fn closure(repository: &Path, roots: &[&str]) -> Result<BTreeSet<PathBuf>> {
    let directories = package_closure(repository, roots, Edges::Build)?;
    let index = IndexSnapshot::read(repository)?;
    let mut files = BTreeSet::new();
    for file in index.tracked.iter().chain(&index.untracked) {
        let path = Path::new(file);
        let Some(directory) = directories
            .iter()
            .find(|directory| path.starts_with(directory))
        else {
            continue;
        };
        if !repository.join(path).is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(directory)
            .unwrap_or(path)
            .to_string_lossy();
        let builds = relative == "Cargo.toml" || relative == "build.rs";
        let compiled = relative.starts_with("src/")
            && !relative.split('/').any(|part| part == "tests")
            && !relative.ends_with("/tests.rs")
            && !relative.ends_with(".md");
        if builds || compiled {
            files.insert(path.to_owned());
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builder_is_the_dependency_closure_of_the_image_pipeline_and_its_linker() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let files = closure(
            &root,
            &[
                "oer-tool-service-image",
                "oer-tool-service-image-linker",
                "oer-tool-service-image-checks",
            ],
        )
        .unwrap();
        for file in [
            "tools/image/pipeline/Cargo.toml",
            "tools/image/pipeline/src/lib.rs",
            "tools/image/linker/src/main.rs",
            "tools/elf/src/lib.rs",
            "tools/riscv/stack/src/lib.rs",
            "tools/riscv/decode/src/lib.rs",
            "tools/chip-profile/src/lib.rs",
            "tools/process/src/lib.rs",
            "tools/toolchain/src/lib.rs",
            "tools/vendor-artifacts/src/lib.rs",
        ] {
            assert!(files.contains(Path::new(file)), "{file}");
        }
        for file in &files {
            let path = file.to_string_lossy();
            assert!(
                !path.ends_with("/tests.rs") && !path.ends_with(".md") && !path.contains("/tests/"),
                "{path}"
            );
        }
        // Packages the pipeline does not link are no inputs.
        assert!(!files.iter().any(|file| file.starts_with("hil/")));
        assert!(!files.iter().any(|file| file.starts_with("tools/xtask")));
        // The image pipeline reads chips as data, not through the
        // repository model.
        assert!(!files.iter().any(|file| file.starts_with("tools/repo/")));
    }
}
