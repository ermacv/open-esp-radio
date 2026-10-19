//! What writes shards.
//!
//! A [`Producer`] compares pinned vendor code with compiled production code
//! scenario by scenario and writes one shard per scenario that compared
//! MATCH, through [`oer_vendor_evidence_shard::store::write`]; a scenario that is not MATCH
//! fails without a shard. The Blobray scenario engine and the host stands
//! are the producers; `cargo verification evidence` decides which of them run for
//! which stale or named shards. Each shard names its producer in `command`
//! ([`oer_vendor_evidence_shard::BLOBRAY`] or [`oer_vendor_evidence_shard::HOST_STAND`]).
use crate::{Result, policy};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One producer of a chip's shards.
pub trait Producer {
    /// The `command` its shards record, one of [`oer_vendor_evidence_shard::COMMANDS`].
    fn command(&self) -> &'static str;
    /// Whether `scenario` names one of its shards.
    fn owns(&self, scenario: &str) -> bool;
    /// Compare `scenarios` and write each one's shard into `index`; fails
    /// on the first scenario that is not MATCH.
    fn produce(&self, scenarios: &[String], index: &Path) -> Result<()>;
}

/// The host stands: a stand is a package `host/vendor/<engine>-<chip>`
/// that compiles pinned
/// vendor source for the host and compares the production engine with it.
/// Its one shard is named after its engine ([`host_stand::scenario`]); the
/// stand writes it with [`host_stand::shard`] from its own `shard` command.
pub mod host_stand {
    use super::*;

    /// The shard name of the stand in `directory` (`<engine>-<chip>`): its
    /// engine followed by `-host`.
    pub fn scenario(chip: &str, directory: &Path) -> Result<String> {
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(chip)?.strip_suffix('-'))
            .filter(|engine| crate::workspace::HOST_STAND_ENGINES.contains(engine))
            .map(|engine| format!("{engine}-host"))
            .ok_or_else(|| format!("{} names no {chip} stand", directory.display()).into())
    }

    /// The stand packages of `chip`, by shard name, with their manifests
    /// relative to the root.
    pub fn stands(workspace: &crate::workspace::Workspace, chip: &str) -> BTreeMap<String, String> {
        workspace.host_stands(chip)
    }

    /// What one stand compares.
    pub struct Stand<'a> {
        /// The chip whose shards it writes.
        pub chip: &'a str,
        /// The stand's package directory (its `CARGO_MANIFEST_DIR`).
        pub directory: &'a Path,
        /// Artifact source of the vendor files it compiles.
        pub source: &'a str,
        /// The production entry every scenario drives.
        pub production: &'a str,
    }

    /// Why a stand shard tracks its whole source closure.
    pub const WHOLE_CLOSURE: &str =
        "a host stand compiles vendor source, outside the executions files attribute";

    /// The shard of a stand whose every scenario in `matched` compared
    /// MATCH: its sources are the stand's whole path closure (report
    /// packages refused), its inputs the digests of the vendor artifacts it
    /// compiled.
    pub fn shard(
        stand: &Stand<'_>,
        matched: &[String],
        inputs: BTreeMap<String, String>,
    ) -> Result<oer_vendor_evidence_shard::Index> {
        let root = oer_os_process::checkout_of(stand.directory)
            .ok_or_else(|| format!("{} is outside a checkout", stand.directory.display()))?;
        let directory = stand
            .directory
            .canonicalize()?
            .strip_prefix(&root)?
            .to_path_buf();
        let workspace = crate::workspace::Workspace::load(&root, None)?;
        let manifest = directory.join("Cargo.toml");
        let package = workspace
            .owner(&manifest.to_string_lossy())
            .ok_or_else(|| format!("{} is no package manifest", manifest.display()))?;
        let scenario = scenario(stand.chip, &directory)?;
        let closure = policy::closure(&workspace, package);
        policy::check_verdict_sources(&closure.directories, &closure.report)?;
        let mut directories: Vec<PathBuf> = closure.directories;
        directories.sort();
        directories.dedup();
        let sources = directories
            .into_iter()
            .map(|path| {
                Ok(oer_vendor_evidence_shard::SourceDigest {
                    sha256: oer_vendor_evidence_shard::digest_directory(&root, &path)?,
                    path,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let entries = matched
            .iter()
            .map(|symbol| oer_vendor_evidence_shard::Entry {
                suite: scenario.clone(),
                source: stand.source.into(),
                symbol: symbol.clone(),
                production: stand.production.into(),
                verdict: oer_vendor_evidence_shard::MATCH.into(),
                cases: 1,
                reviews: vec![],
                coverage: None,
                observation: None,
                state: None,
            })
            .collect();
        let shard = oer_vendor_evidence_shard::Index {
            schema: oer_vendor_evidence_shard::SCHEMA,
            command: oer_vendor_evidence_shard::HOST_STAND.into(),
            target: stand.chip.into(),
            scenario,
            inputs,
            sources,
            dependence: oer_vendor_evidence_shard::Dependence::whole_closure(WHOLE_CLOSURE),
            entries,
            untriaged: vec![],
            functions: vec![],
            unobserved: vec![],
            observed: vec![],
            unprojected: vec![],
        };
        shard.validate(stand.chip)?;
        Ok(shard)
    }
}

#[cfg(test)]
mod tests {
    use super::host_stand;
    use std::path::Path;

    #[test]
    fn a_stand_is_named_after_its_engine() {
        assert_eq!(
            host_stand::scenario("chip-a", Path::new("host/vendor/ieee802154-chip-a")).unwrap(),
            "ieee802154-host"
        );
        assert!(host_stand::scenario("chip-a", Path::new("host/vendor/x-chip-b")).is_err());
    }

    #[test]
    fn this_checkout_registers_its_host_stands_by_directory() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let workspace = crate::workspace::Workspace::load(&root, None).unwrap();
        let stands = host_stand::stands(&workspace, "esp32s31");
        assert!(!stands.is_empty());
        for (scenario, manifest) in &stands {
            let directory = Path::new(manifest).parent().unwrap();
            assert_eq!(
                &host_stand::scenario("esp32s31", directory).unwrap(),
                scenario
            );
        }
    }
}
