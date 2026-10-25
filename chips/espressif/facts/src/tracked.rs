//! The tracked layer files, and each chip's stack policy, as this build
//! compiled them in: a [`Source`] for host tests, for the build scripts that
//! generate from the facts and for tools that detect a tree newer than
//! themselves.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use crate::{Result, Source};

/// Every tracked layer file and each chip's stack policy
/// (`chips/<id>/platform/stack.toml`, whose default task stack the layout
/// takes): its path relative to the root and its text.
pub const FILES: &[(&str, &str)] = &[
    (
        "chips/espressif/platform/family.toml",
        include_str!("../../platform/family.toml"),
    ),
    (
        "chips/espressif/platform/isa/rv32imac.toml",
        include_str!("../../platform/isa/rv32imac.toml"),
    ),
    (
        "chips/espressif/platform/isa/rv32imafc.toml",
        include_str!("../../platform/isa/rv32imafc.toml"),
    ),
    (
        "chips/esp32c5/platform/chip.toml",
        include_str!("../../../esp32c5/platform/chip.toml"),
    ),
    (
        "chips/esp32c5/platform/stack.toml",
        include_str!("../../../esp32c5/platform/stack.toml"),
    ),
    (
        "chips/esp32c5/platform/modules/esp32-c5-wroom-1-n16r8.toml",
        include_str!("../../../esp32c5/platform/modules/esp32-c5-wroom-1-n16r8.toml"),
    ),
    (
        "chips/esp32c5/platform/boards/esp32-c5-devkit.toml",
        include_str!("../../../esp32c5/platform/boards/esp32-c5-devkit.toml"),
    ),
    (
        "chips/esp32s31/platform/chip.toml",
        include_str!("../../../esp32s31/platform/chip.toml"),
    ),
    (
        "chips/esp32s31/platform/stack.toml",
        include_str!("../../../esp32s31/platform/stack.toml"),
    ),
    (
        "chips/esp32s31/platform/modules/esp32-s31-wroom.toml",
        include_str!("../../../esp32s31/platform/modules/esp32-s31-wroom.toml"),
    ),
    (
        "chips/esp32s31/platform/boards/esp32-s31-function-coreboard-1.toml",
        include_str!("../../../esp32s31/platform/boards/esp32-s31-function-coreboard-1.toml"),
    ),
];

/// The text of the tracked file at `path`.
pub fn text(path: &str) -> Option<&'static str> {
    FILES
        .iter()
        .find(|(file, _)| *file == path)
        .map(|(_, text)| *text)
}

/// The ids of the tracked chips, sorted.
pub fn ids() -> Vec<&'static str> {
    let mut ids: Vec<_> = FILES
        .iter()
        .filter_map(|(path, _)| {
            path.strip_prefix("chips/")?
                .strip_suffix("/platform/chip.toml")
        })
        .collect();
    ids.sort_unstable();
    ids
}

/// The tracked files as a [`Source`].
pub struct Tracked;

impl Source for Tracked {
    fn read(&mut self, path: &str) -> Result<String> {
        text(path)
            .map(ToString::to_string)
            .ok_or_else(|| format!("{path} is not a tracked layer file").into())
    }

    fn files(&mut self, directory: &str) -> Result<Vec<String>> {
        let prefix = format!("{directory}/");
        Ok(FILES
            .iter()
            .filter_map(|(path, _)| path.strip_prefix(&prefix))
            .filter(|name| !name.contains('/'))
            .map(ToString::to_string)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn toml_files(directory: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("a readable entry").path();
            if path.is_dir() {
                toml_files(&path, found);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "toml")
            {
                found.push(path);
            }
        }
    }

    /// The layer files of the tree this crate is built from: the family's
    /// and, for every chip, its chip layer, stack policy, modules and boards.
    #[test]
    fn the_tracked_files_are_every_layer_of_the_tree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let chips = root.join("chips");
        let mut found = Vec::new();
        toml_files(&chips.join("espressif/platform"), &mut found);
        for entry in std::fs::read_dir(&chips).expect("chips/ is readable") {
            let platform = entry.expect("a readable entry").path().join("platform");
            if platform.join("chip.toml").is_file() {
                found.push(platform.join("chip.toml"));
                found.push(platform.join("stack.toml"));
                toml_files(&platform.join("modules"), &mut found);
                toml_files(&platform.join("boards"), &mut found);
            }
        }
        let mut found: Vec<String> = found
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .expect("below the root")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        found.sort();
        let mut tracked: Vec<String> = FILES.iter().map(|(path, _)| path.to_string()).collect();
        tracked.sort();
        assert_eq!(tracked, found);
        for (path, text) in FILES {
            assert_eq!(
                std::fs::read_to_string(root.join(path)).expect("readable"),
                *text,
                "{path}"
            );
        }
        assert_eq!(ids(), ["esp32c5", "esp32s31"]);
    }
}
