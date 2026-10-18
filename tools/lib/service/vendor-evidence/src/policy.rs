//! The verdict source policy: which repository sources a shard may record.
//!
//! A shard records the sources its verdicts depend on. Packages classified
//! `open-radio.evidence = "report"` only render findings and decide no
//! verdict, so no shard may record one of their files: an edit to report
//! code must never stale evidence. Producers compute what a shard may record
//! through [`closure`] and check it with [`check_verdict_sources`];
//! [`reject_report_sources`] catches a shard of an index that bypassed them.
use crate::Result;
use oer_repo::closure::{Edges, Features, Target};
use oer_tool_data_vendor_evidence_shard::store;
use std::path::{Path, PathBuf};

/// The path packages building one package compiles, split by whether their
/// sources may decide a verdict.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct PathClosure {
    /// Directories of the packages a shard may record, the package itself
    /// included, relative to the repository root.
    pub directories: Vec<PathBuf>,
    /// Directories of the report packages the closure reaches.
    pub report: Vec<PathBuf>,
}

/// The path packages that building `package` compiles (normal and build
/// dependencies, every optional one), for the target `platform` when given,
/// as the repository model resolves them, with the report packages apart.
pub fn closure(
    model: &oer_repo::Model,
    package: &oer_repo::Package,
    platform: Option<&str>,
) -> Result<PathClosure> {
    let target = platform.map(Target::new).transpose()?;
    let mut closure = PathClosure::default();
    for package in model.closure(&[package], Edges::Build, target.as_ref(), &Features::All)? {
        let directory = PathBuf::from(&package.directory);
        if is_report(model, package)? {
            closure.report.push(directory);
        } else {
            closure.directories.push(directory);
        }
    }
    Ok(closure)
}

fn is_report(model: &oer_repo::Model, package: &oer_repo::Package) -> Result<bool> {
    Ok(model.classification(package)?.evidence == Some(oer_repo::Evidence::Report))
}

/// Directories of every report package of the repository.
pub fn report_packages(model: &oer_repo::Model) -> Result<Vec<PathBuf>> {
    let mut directories = vec![];
    for package in model.packages() {
        if is_report(model, package)? {
            directories.push(PathBuf::from(&package.directory));
        }
    }
    Ok(directories)
}

/// Fail when one of `paths` lies in one of the `report` package
/// directories: report code decides no verdict, so it must not stale the
/// evidence.
pub fn check_verdict_sources(paths: &[PathBuf], report: &[PathBuf]) -> Result<()> {
    for path in paths {
        if let Some(package) = report.iter().find(|package| path.starts_with(package)) {
            return Err(format!(
                "shard source {} belongs to the report package {}",
                path.display(),
                package.display()
            )
            .into());
        }
    }
    Ok(())
}

/// Fail when a readable shard of `directory` records a file of one of the
/// `report` packages; producers refuse to write one, and this catches a
/// shard that bypassed them.
pub fn reject_report_sources(directory: &Path, report: &[PathBuf]) -> Result<()> {
    for shard in store::shards(directory)? {
        let paths: Vec<PathBuf> = shard.sources.iter().map(|s| s.path.clone()).collect();
        check_verdict_sources(&paths, report).map_err(|error| {
            format!(
                "shard {}: {error}",
                store::path(directory, &shard.scenario).display()
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_package_file_cannot_be_a_shard_source() {
        let report = [PathBuf::from("verification/harness/report")];
        let verdict = [PathBuf::from("verification/harness/scenarios/src/shard.rs")];
        assert!(check_verdict_sources(&verdict, &report).is_ok());
        let leaked = [PathBuf::from("verification/harness/report/src/triage.rs")];
        let error = check_verdict_sources(&leaked, &report).unwrap_err();
        assert!(error.to_string().contains("report package"));
    }

    #[test]
    fn a_committed_shard_recording_report_code_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let shard = oer_tool_data_vendor_evidence_shard::Index {
            schema: oer_tool_data_vendor_evidence_shard::SCHEMA,
            command: oer_tool_data_vendor_evidence_shard::BLOBRAY.into(),
            target: "chip-a".into(),
            scenario: "leaf".into(),
            inputs: Default::default(),
            sources: vec![oer_tool_data_vendor_evidence_shard::SourceDigest {
                path: PathBuf::from("verification/harness/report/src/triage.rs"),
                sha256: "0".repeat(64),
            }],
            dependence: oer_tool_data_vendor_evidence_shard::Dependence::whole_closure("test"),
            entries: vec![],
            untriaged: vec![],
            functions: vec![],
            unobserved: vec![],
            observed: vec![],
            unprojected: vec![],
        };
        oer_tool_data_vendor_evidence_shard::store::write(directory.path(), &shard).unwrap();
        let report = [PathBuf::from("verification/harness/report")];
        let error = reject_report_sources(directory.path(), &report).unwrap_err();
        assert!(error.to_string().contains("leaf.json"), "{error}");
        assert!(reject_report_sources(directory.path(), &[]).is_ok());
    }

    #[test]
    fn this_checkout_classifies_the_report_package() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let model = oer_repo::Model::load(&oer_repo::Repo::load(&root).unwrap()).unwrap();
        let report = report_packages(&model).unwrap();
        assert!(report.contains(&PathBuf::from("verification/harness/report")));
        let package = model.owner("verification/evidence/Cargo.toml").unwrap();
        let closure = closure(&model, package, None).unwrap();
        assert!(
            closure
                .directories
                .contains(&PathBuf::from("verification/evidence"))
        );
        assert!(closure.directories.contains(&PathBuf::from("tools/repo")));
        assert!(closure.report.is_empty());
    }
}
