//! Evidence shards: one scenario's claims with the digests of every source
//! its verdicts depend on, written to the scenario package's evidence index.
use crate::harness::Result;
use crate::session;
use crate::{coverage, observation, state};
use oer_vendor_evidence::policy;
use oer_vendor_evidence_shard::Index;
use std::path::{Path, PathBuf};

/// The compiled production images a chip's scenarios run, and where their
/// sources are resolved.
pub struct ProbeImages {
    /// Probe workspace manifest, relative to the repository root.
    pub manifest: &'static str,
    /// Probe ELF packages whose images the shards may name.
    pub packages: &'static [&'static str],
    /// Target triple the probes are built for.
    pub target: &'static str,
}

/// Blobray workspace, which also resolves the scenario packages and the
/// engine behind every verdict.
const TOOL_MANIFEST: &str = "verification/Cargo.toml";
/// Production crates, which shards track by executed file.
const PRODUCTION_CRATES: &str = "crates";
/// The pinned toolchain, and the lock file and package manifest names.
const TOOLCHAIN: &str = "rust-toolchain.toml";
/// The directory name Cargo writes build outputs below; like
/// [`oer_vendor_evidence_shard::digest_directory`], a shard never records it.
const BUILD_OUTPUT: &str = "target";
const LOCK_FILE: &str = "Cargo.lock";
const PACKAGE_MANIFEST: &str = "Cargo.toml";
/// The probe compiler, a host build tool whose sources Cargo's dep-info of
/// the probe image does not list.
const PROBE_COMPILER: [&str; 2] = [
    "verification/harness/codegen",
    "verification/harness/macros",
];
/// Directory of a probe workspace's image packages: their placement and
/// entry code apply to every probe, so they stay global.
const PROBE_IMAGE_DIRECTORY: &str = "elf";

/// Repository-relative sources Cargo's dep-info records for the artifact at
/// `artifact`, from the `.d` file beside it: every compiled source file,
/// `include!`/`include_str!`/`include_bytes!` input and build-script
/// `rerun-if-changed` path. Files outside the repository are pinned by the
/// lock files and the toolchain. Files a build writes below a `target`
/// directory, such as a build script's generated catalog, change with every
/// rebuild; the build script and the sources it reads are recorded instead.
pub fn dep_info(root: &Path, artifact: &Path) -> Result<Vec<PathBuf>> {
    let mut path = artifact.as_os_str().to_owned();
    path.push(".d");
    dep_info_file(root, Path::new(&path))
}

/// Repository-relative sources the dep-info file at `path` records, as
/// [`dep_info`] reads them.
pub fn dep_info_file(root: &Path, path: &Path) -> Result<Vec<PathBuf>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {e}; build the artifact first", path.display()))?;
    let first = text.lines().next().unwrap_or_default();
    let list = first.split_once(": ").map_or("", |(_, list)| list);
    let mut files = std::collections::BTreeSet::new();
    let mut current = String::new();
    let mut escaped = false;
    for c in list.chars().chain([' ']) {
        match (escaped, c) {
            (false, '\\') => escaped = true,
            (false, ' ') => {
                if !current.is_empty() {
                    let file = PathBuf::from(std::mem::take(&mut current));
                    if let Ok(canonical) = file.canonicalize()
                        && let Ok(relative) = canonical.strip_prefix(root)
                        && !relative.components().any(|c| c.as_os_str() == BUILD_OUTPUT)
                    {
                        files.insert(relative.to_path_buf());
                    }
                }
            }
            (_, c) => {
                escaped = false;
                current.push(c);
            }
        }
    }
    Ok(files.into_iter().collect())
}

/// The path closure of `package` of the workspace `manifest` for the
/// target `platform` ([`policy::closure`]).
fn path_closure(
    model: &oer_repo::Model,
    manifest: &str,
    package: &str,
    platform: Option<&str>,
) -> Result<policy::PathClosure> {
    let package = model
        .members(manifest)
        .find(|member| member.name == package)
        .ok_or_else(|| format!("package {package} missing from {manifest}"))?;
    policy::closure(model, package, platform)
}

/// Why a shard cannot be written without the verdict libraries' dep-info.
const VERDICT_DEP_INFO_REQUIRED: &str = "shards are written only through cargo verification scenario, which passes the verdict libraries' dep-info";

/// What Cargo compiled for one shard: the probe image's dep-info, the
/// dep-info of the libraries that decide the verdicts, and the manifests of
/// their path packages.
pub struct Closures {
    pub probe: Vec<PathBuf>,
    pub tool: Vec<PathBuf>,
    pub manifests: Vec<PathBuf>,
}

/// The sources a shard records, sorted: production crates and probe
/// libraries by the files the executions ran (all of them on a fallback),
/// and globally what Cargo compiled into the verdict libraries, the probe
/// images' placement and entry code, every package manifest, the lock
/// files, the probe compiler and the toolchain. The shard format itself is a
/// verdict library, so its files come with the verdict libraries' dep-info.
pub fn source_paths(
    closures: &Closures,
    probe_manifest: &Path,
    executed: &std::collections::BTreeSet<PathBuf>,
    fallback: bool,
) -> Vec<PathBuf> {
    let probe_workspace = probe_manifest.parent().unwrap_or(Path::new(""));
    let attributable = |path: &Path| {
        path.starts_with(PRODUCTION_CRATES)
            || (path.starts_with(probe_workspace)
                && !path
                    .components()
                    .any(|c| c.as_os_str() == PROBE_IMAGE_DIRECTORY))
    };
    let (per_file, mut global): (Vec<PathBuf>, Vec<PathBuf>) = closures
        .probe
        .iter()
        .cloned()
        .partition(|path| attributable(path));
    global.extend(closures.tool.iter().cloned());
    global.extend(closures.manifests.iter().cloned());
    global.extend(PROBE_COMPILER.map(PathBuf::from));
    global.push(PathBuf::from(TOOLCHAIN));
    global.push(PathBuf::from(TOOL_MANIFEST).with_file_name(LOCK_FILE));
    global.push(probe_manifest.to_path_buf());
    global.push(probe_manifest.with_file_name(LOCK_FILE));
    let production: Vec<PathBuf> = if fallback {
        per_file
    } else {
        executed
            .iter()
            .filter(|file| !global.iter().any(|g| file.starts_with(g)))
            .cloned()
            .collect()
    };
    let mut paths: Vec<PathBuf> = global.into_iter().chain(production).collect();
    paths.sort();
    paths.dedup();
    paths
}

/// The shard of `scenario`'s claims against the probe image at
/// `production`: the sources are the image package's path closure, the
/// verdict libraries named by their dep-info files `verdict`. Report
/// packages the scenario package reaches are left out, and a source inside
/// one is an error ([`policy::check_verdict_sources`]).
pub fn shard(
    scenario: &str,
    production: &Path,
    claims: &session::Claims,
    probes: &ProbeImages,
    tool_package: &str,
    verdict: &[PathBuf],
) -> Result<Index> {
    if verdict.is_empty() {
        return Err(VERDICT_DEP_INFO_REQUIRED.into());
    }
    let root = oer_process::built_root();
    let target = crate::chip().name;
    let package = production
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| probes.packages.contains(n))
        .ok_or_else(|| {
            format!(
                "{} is not a probe image the index knows the sources of",
                production.display()
            )
        })?;
    let model = oer_repo::Model::load(&oer_repo::Repo::load(&root)?)?;
    let probe_closure = path_closure(&model, probes.manifest, package, Some(probes.target))?;
    let tool_closure = path_closure(&model, TOOL_MANIFEST, tool_package, None)?;
    let manifests = probe_closure
        .directories
        .iter()
        .chain(&tool_closure.directories)
        .map(|directory| directory.join(PACKAGE_MANIFEST))
        .collect();
    let mut tool = std::collections::BTreeSet::new();
    for file in verdict {
        tool.extend(dep_info_file(&root, file)?);
    }
    // A firmware input pins no digest in the manifest: the recipe of its
    // catalog project is its identity, so the project is a source.
    tool.extend(crate::artifacts::firmware_projects(&root)?);
    let closures = Closures {
        probe: dep_info(&root, production)?,
        tool: tool.into_iter().collect(),
        manifests,
    };
    let dependencies = &claims.dependencies;
    let paths = source_paths(
        &closures,
        Path::new(probes.manifest),
        &dependencies.files,
        dependencies.fallback.is_some(),
    );
    let report: Vec<PathBuf> = probe_closure
        .report
        .into_iter()
        .chain(tool_closure.report)
        .collect();
    policy::check_verdict_sources(&paths, &report)?;
    let sources = paths
        .into_iter()
        .map(|path| {
            Ok(oer_vendor_evidence_shard::SourceDigest {
                sha256: oer_vendor_evidence_shard::digest_source(&root, &path)?,
                path,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let line = |(path, line): (PathBuf, u32)| oer_vendor_evidence_shard::SourceLine { path, line };
    let mut observation = observation::Sources::default();
    let (_, unobserved) =
        observation.classify(&root, crate::chip().observation, &claims.lines.unobserved())?;
    let mut unprojected = std::collections::BTreeSet::new();
    for (vendor, production, byte) in &claims.unprojected {
        let (_, untriaged) = state::classify(
            crate::chip().state,
            (vendor, production),
            &std::collections::BTreeSet::from([byte.clone()]),
        );
        unprojected.extend(untriaged);
    }
    let functions: Vec<String> = claims
        .closures
        .iter()
        .flat_map(|c| c.functions.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let decisions = PathBuf::from(crate::chip().coverage);
    let index = Index {
        schema: oer_vendor_evidence_shard::SCHEMA,
        command: oer_vendor_evidence_shard::BLOBRAY.into(),
        target: target.into(),
        scenario: scenario.into(),
        inputs: claims.inputs.clone(),
        sources,
        dependence: oer_vendor_evidence_shard::Dependence {
            read_data: dependencies.read_data.clone(),
            fallback: dependencies.fallback.clone(),
            coverage_decisions: Some(oer_vendor_evidence_shard::DecisionDigest {
                sha256: oer_vendor_evidence_shard::CoverageDecisions::read(&root, &decisions)?
                    .applicable_digest(&functions),
                path: decisions,
            }),
        },
        entries: claims.entries.clone(),
        untriaged: coverage::uncovered_everywhere(&claims.closures, claims.untriaged.clone())
            .into_iter()
            .collect(),
        functions,
        unobserved: unobserved.into_iter().map(line).collect(),
        observed: claims.lines.observed.iter().cloned().map(line).collect(),
        unprojected: state::ranges(&unprojected)
            .into_iter()
            .map(
                |(symbol, offset, length)| oer_vendor_evidence_shard::StateRange {
                    symbol,
                    offset,
                    length,
                },
            )
            .collect(),
    };
    index.validate(target)?;
    Ok(index)
}

/// Write `index` into the index at `directory` through the one shard
/// writer, and name the file.
pub fn record(directory: &Path, index: &Index) -> Result<()> {
    let path = oer_vendor_evidence_shard::store::write(directory, index)?;
    println!("evidence shard {}", path.display());
    Ok(())
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use std::collections::BTreeSet;

    const PROBES: &str = "verification/chip/probes/Cargo.toml";

    fn closures() -> Closures {
        Closures {
            probe: [
                "crates/hal/src/mac.rs",
                "crates/hal/src/phy.rs",
                "verification/chip/probes/radio/library/src/rx.rs",
                "verification/chip/probes/radio/library/src/i2c.rs",
                "verification/chip/probes/radio/elf/src/main.rs",
                "verification/chip/probes/radio/elf/link.x",
            ]
            .map(PathBuf::from)
            .to_vec(),
            tool: [
                "tools/blobray/crates/domain/src/lib.rs",
                "verification/chip/scenarios/src/lib.rs",
                "verification/chip/artifacts.toml",
            ]
            .map(PathBuf::from)
            .to_vec(),
            manifests: vec![PathBuf::from("crates/hal/Cargo.toml")],
        }
    }

    fn sources(executed: &[&str]) -> BTreeSet<PathBuf> {
        let executed = executed.iter().map(PathBuf::from).collect();
        source_paths(&closures(), Path::new(PROBES), &executed, false)
            .into_iter()
            .collect()
    }

    #[test]
    fn a_probe_file_is_a_source_only_of_the_shard_that_executes_it() {
        let rx = sources(&[
            "crates/hal/src/mac.rs",
            "verification/chip/probes/radio/library/src/rx.rs",
        ]);
        let i2c = sources(&[
            "crates/hal/src/phy.rs",
            "verification/chip/probes/radio/library/src/i2c.rs",
        ]);
        let edited = Path::new("verification/chip/probes/radio/library/src/rx.rs");
        assert!(rx.contains(edited));
        assert!(!i2c.contains(edited));
        assert!(!rx.contains(Path::new("crates/hal/src/phy.rs")));
    }

    #[test]
    fn a_test_or_readme_edit_is_no_shard_source() {
        // Cargo compiles neither into the scenario binary or the probe.
        for shard in [sources(&["crates/hal/src/mac.rs"]), sources(&[])] {
            assert!(!shard.contains(Path::new("tools/blobray/cli/tests/execution.rs")));
            assert!(!shard.contains(Path::new("tools/blobray/README.md")));
            assert!(!shard.iter().any(|p| p == Path::new("tools/blobray")));
        }
    }

    #[test]
    fn image_placement_and_included_data_are_sources_of_every_shard() {
        for shard in [
            sources(&["crates/hal/src/mac.rs"]),
            sources(&["crates/hal/src/phy.rs"]),
        ] {
            assert!(shard.contains(Path::new("verification/chip/probes/radio/elf/link.x")));
            assert!(shard.contains(Path::new("verification/chip/probes/radio/elf/src/main.rs")));
            // An include_str!-ed data file of the scenario binary.
            assert!(shard.contains(Path::new("verification/chip/artifacts.toml")));
            assert!(shard.contains(Path::new("crates/hal/Cargo.toml")));
        }
    }

    #[test]
    fn a_fallback_tracks_every_attributable_file() {
        let shard: BTreeSet<PathBuf> =
            source_paths(&closures(), Path::new(PROBES), &BTreeSet::new(), true)
                .into_iter()
                .collect();
        assert!(shard.contains(Path::new("crates/hal/src/phy.rs")));
        assert!(shard.contains(Path::new(
            "verification/chip/probes/radio/library/src/i2c.rs"
        )));
    }

    #[test]
    fn dep_info_reads_escaped_spaces_and_keeps_repository_files() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::write(root.join("src/a b.txt"), "").unwrap();
        std::fs::create_dir_all(root.join("target/out")).unwrap();
        std::fs::write(root.join("target/out/catalog.rs"), "").unwrap();
        let artifact = root.join("app");
        std::fs::write(
            root.join("app.d"),
            format!(
                "{}: {} {} {} /usr/lib/outside.rs\n\n{}:\n",
                artifact.display(),
                root.join("target/out/catalog.rs").display(),
                root.join("src/lib.rs").display(),
                root.join("src/a b.txt")
                    .display()
                    .to_string()
                    .replace(' ', "\\ "),
                root.join("src/lib.rs").display(),
            ),
        )
        .unwrap();
        assert_eq!(
            dep_info(&root, &artifact).unwrap(),
            [PathBuf::from("src/a b.txt"), PathBuf::from("src/lib.rs")]
        );
    }
}
