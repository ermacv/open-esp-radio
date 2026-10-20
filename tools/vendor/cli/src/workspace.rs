//! Discover vendor host stands from the workspace's package directories.
use crate::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};

const ENGINES: &[&str] = &["ieee802154"];

#[derive(Debug, Eq, PartialEq)]
pub struct HostStand {
    pub library: PathBuf,
    pub program: PathBuf,
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}
#[derive(Deserialize)]
struct Package {
    source: Option<String>,
    manifest_path: PathBuf,
}

pub fn host_stands(root: &Path, chip: &str) -> Result<Vec<HostStand>> {
    let mut command =
        oer_os_process::command(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--locked", "--no-deps"]);
    let output = oer_os_process::capture(&mut command)?;
    let metadata: Metadata = serde_json::from_slice(&output.stdout)?;
    let root = root.canonicalize()?;
    let manifests = metadata
        .packages
        .into_iter()
        .filter(|p| p.source.is_none())
        .map(|p| Ok(p.manifest_path.strip_prefix(&root)?.to_path_buf()))
        .collect::<Result<Vec<_>>>()?;
    Ok(stands(&manifests, chip))
}

fn stands(manifests: &[PathBuf], chip: &str) -> Vec<HostStand> {
    ENGINES
        .iter()
        .filter_map(|engine| {
            let directory = format!("tools/vendor/{engine}-{chip}");
            let library = PathBuf::from(format!("{directory}/Cargo.toml"));
            let program = PathBuf::from(format!("{directory}-cli/Cargo.toml"));
            (manifests.contains(&library) && manifests.contains(&program))
                .then_some(HostStand { library, program })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_host_stand_needs_both_packages_and_belongs_to_its_chip() {
        let library = PathBuf::from("tools/vendor/ieee802154-chip-a/Cargo.toml");
        let program = PathBuf::from("tools/vendor/ieee802154-chip-a-cli/Cargo.toml");
        assert!(stands(std::slice::from_ref(&library), "chip-a").is_empty());
        assert!(stands(std::slice::from_ref(&program), "chip-a").is_empty());
        let manifests = vec![library.clone(), program.clone()];
        assert_eq!(
            stands(&manifests, "chip-a"),
            [HostStand { library, program }]
        );
        assert!(stands(&manifests, "chip-b").is_empty());
    }
}
