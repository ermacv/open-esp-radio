//! Compiler value reports, selected by Cargo artifacts rather than cache age.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Result, build_log::BuildLog};

const BINARY: &str = "oer-tool-image-large-values";
const SUFFIX: &str = ".large-values.json";

#[derive(Clone, Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
}

fn workspace(root: &Path) -> Result<Vec<Package>> {
    #[derive(Deserialize)]
    struct Metadata {
        packages: Vec<Package>,
        workspace_members: BTreeSet<String>,
    }
    let output = crate::cargo::command()
        .current_dir(root)
        .args(["metadata", "--format-version=1", "--no-deps", "--locked"])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "large-values workspace metadata: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let metadata: Metadata = serde_json::from_slice(&output.stdout)?;
    metadata
        .packages
        .into_iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .map(|mut package| {
            package.manifest_path = package.manifest_path.canonicalize()?;
            Ok(package)
        })
        .collect()
}

/// Install the reporter without changing Cargo's artifact namespace or rustc arguments.
pub(crate) fn configure(
    command: &mut Command,
    root: &Path,
    target: &Path,
    rust_target: &str,
    limit: u64,
) -> Result<()> {
    let binary = crate::compiler::build_host_tool(root, target, BINARY, BINARY)?;
    let manifests: Vec<_> = workspace(root)?
        .into_iter()
        .map(|package| package.manifest_path)
        .collect();
    command
        .env("RUSTC_WRAPPER", &binary)
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env(
            "OER_LARGE_VALUES_WORKSPACE",
            serde_json::to_string(&manifests)?,
        )
        .env("OER_LARGE_VALUES_TARGET", rust_target)
        .env("OER_LARGE_VALUES_LIMIT", limit.to_string())
        .env(
            "OER_LARGE_VALUES_DRIVER_ID",
            oer_os_durable::sha256_file(&binary)?,
        );
    Ok(())
}

#[derive(Deserialize)]
struct Artifact {
    reason: String,
    #[serde(default)]
    package_id: String,
    #[serde(default)]
    manifest_path: PathBuf,
    #[serde(default)]
    target: Target,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    filenames: Vec<PathBuf>,
    executable: Option<PathBuf>,
}

#[derive(Default, Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
}

#[derive(Serialize)]
struct Coverage {
    stage: String,
    package_id: String,
    target: String,
    features: Vec<String>,
    status: String,
    /// Versioned compiler facts. Additional fact collections can be added
    /// by the compiler tool without changing the image envelope.
    facts: Option<Value>,
    error: Option<String>,
}

#[derive(Default, Serialize)]
pub(crate) struct Report {
    schema: u32,
    max_move_bytes: u64,
    crates: Vec<Coverage>,
}

impl Report {
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            schema: 1,
            max_move_bytes: limit,
            crates: Vec::new(),
        }
    }

    pub(crate) fn append(&mut self, other: Self) {
        self.crates.extend(other.crates);
    }

    pub(crate) fn publish(&mut self, output: &Path) -> Result<()> {
        use std::fmt::Write as _;
        self.sort();
        let mut text = format!(
            "MIR values > {} bytes (before LLVM; not final stack frames)\n",
            self.max_move_bytes
        );
        for coverage in &self.crates {
            writeln!(
                text,
                "\n{} {} {} {:?}: {}",
                coverage.stage,
                coverage.package_id,
                coverage.target,
                coverage.features,
                coverage.status
            )?;
            if let Some(error) = &coverage.error {
                writeln!(text, "  {error}")?;
            }
            if let Some(facts) = &coverage.facts {
                writeln!(
                    text,
                    "  {} instances; {} without a MIR body",
                    facts["instances"],
                    facts["bodies_unavailable"].as_array().map_or(0, Vec::len)
                )?;
                if let Some(errors) = facts["errors"].as_array() {
                    for error in errors {
                        writeln!(text, "  inspection error: {error}")?;
                    }
                }
                if let Some(records) = facts["records"].as_array() {
                    for record in records {
                        writeln!(
                            text,
                            "  {} B {} {} in {}\n    {}\n    {}",
                            record["bytes"],
                            record["role"].as_str().unwrap_or_default(),
                            record["place"].as_str().unwrap_or_default(),
                            record["instance"].as_str().unwrap_or_default(),
                            record["ty"].as_str().unwrap_or_default(),
                            record["span"].as_str().unwrap_or_default()
                        )?;
                    }
                }
            }
        }
        fs::write(
            output.join("large-values.json"),
            serde_json::to_vec_pretty(self)?,
        )?;
        fs::write(output.join("large-values.txt"), text)?;
        Ok(())
    }

    fn sort(&mut self) {
        self.crates.sort_by(|a, b| {
            (&a.stage, &a.package_id, &a.target, &a.features).cmp(&(
                &b.stage,
                &b.package_id,
                &b.target,
                &b.features,
            ))
        });
    }
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

struct Collector {
    packages: BTreeMap<String, Package>,
    release: PathBuf,
    driver_id: String,
    limit: u64,
}

impl Collector {
    fn new(
        root: &Path,
        cache: &Path,
        target: &str,
        profile: &str,
        command: &Command,
    ) -> Result<Self> {
        let environment = |name: &str| {
            command.get_envs().find_map(|(key, value)| {
                (key == name)
                    .then_some(value)
                    .flatten()
                    .map(|value| value.to_string_lossy().into_owned())
            })
        };
        Ok(Self {
            packages: workspace(root)?
                .into_iter()
                .map(|package| (package.id.clone(), package))
                .collect(),
            release: cache.join(target).join(profile),
            driver_id: environment("OER_LARGE_VALUES_DRIVER_ID")
                .ok_or("no compiler reporter identity")?,
            limit: environment("OER_LARGE_VALUES_LIMIT")
                .ok_or("no compiler value limit")?
                .parse()?,
        })
    }

    fn validate(&self, path: &Path, manifest: &Path) -> Result<Value> {
        let report: Value = serde_json::from_slice(&fs::read(path)?)?;
        if report["schema"] != 2
            || report["driver_id"] != self.driver_id
            || report["max_move_bytes"] != self.limit
            || report["manifest"] != manifest.to_string_lossy().as_ref()
            || !report["records"].is_array()
            || !report["metadata_hash"].is_string()
        {
            return Err(format!("stale or invalid compiler facts: {}", path.display()).into());
        }
        Ok(report)
    }

    fn artifact(&self, artifact: &Artifact) -> Result<PathBuf> {
        let deps = self.release.join("deps");
        if let Some(binary) = &artifact.executable {
            // Cargo's unhashed uplift is a hard link of the selected unit.
            // Identity distinguishes even feature variants with identical ELF bytes.
            let selected = fs::metadata(binary)?;
            let prefix = format!("{}-", artifact.target.name.replace('-', "_"));
            for entry in fs::read_dir(&deps)? {
                let path = entry?.path();
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if name.strip_prefix(&prefix).is_some_and(valid_hash) {
                    let candidate = fs::metadata(&path)?;
                    if (selected.dev(), selected.ino()) == (candidate.dev(), candidate.ino()) {
                        return Ok(path);
                    }
                }
            }
            return Err(format!("no hashed Cargo artifact for {}", binary.display()).into());
        }
        artifact
            .filenames
            .iter()
            .find(|path| {
                path.parent() == Some(deps.as_path())
                    && path
                        .extension()
                        .is_some_and(|extension| extension == "rlib" || extension == "rmeta")
            })
            .cloned()
            .ok_or_else(|| "no hashed Rust library artifact".into())
    }

    fn facts(&self, artifact: &Artifact, manifest: &Path) -> Result<Value> {
        let path = self.artifact(artifact)?;
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("invalid artifact name")?;
        let name = if artifact.executable.is_some() {
            stem
        } else {
            stem.strip_prefix("lib")
                .ok_or("invalid Rust library name")?
        };
        let report = self.validate(&path.with_file_name(format!("{name}{SUFFIX}")), manifest)?;
        if let Some(binary) = &artifact.executable
            && report["binary_sha256"] != oer_os_durable::sha256_file(binary)?
        {
            return Err("binary compiler facts do not match the ELF".into());
        }
        Ok(report)
    }

    fn collect(
        &self,
        messages: &[u8],
        stage: &str,
    ) -> Result<(Report, BTreeSet<(String, String)>)> {
        let mut report = Report::new(self.limit);
        let mut missing = BTreeSet::new();
        for line in messages
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let artifact: Artifact = serde_json::from_slice(line)?;
            if artifact.reason != "compiler-artifact" {
                continue;
            }
            let mut coverage = Coverage {
                stage: stage.into(),
                package_id: artifact.package_id.clone(),
                target: artifact.target.name.clone(),
                features: artifact.features.clone(),
                status: String::new(),
                facts: None,
                error: None,
            };
            if artifact
                .target
                .kind
                .iter()
                .any(|kind| kind == "custom-build" || kind == "proc-macro")
                || !artifact
                    .filenames
                    .iter()
                    .any(|path| path.starts_with(&self.release))
            {
                coverage.status = "host (ordinary rustc)".into();
            } else if let Some(package) = self.packages.get(&artifact.package_id) {
                if artifact.manifest_path.canonicalize()? != package.manifest_path {
                    return Err("Cargo artifact manifest differs from workspace metadata".into());
                }
                match self.facts(&artifact, &package.manifest_path) {
                    Ok(facts) => {
                        coverage.status = "checked".into();
                        coverage.facts = Some(facts);
                    }
                    Err(error) => {
                        let path = self.artifact(&artifact)?;
                        let hash = path
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .and_then(|stem| stem.rsplit_once('-'))
                            .map(|(_, hash)| hash)
                            .filter(|hash| valid_hash(hash))
                            .ok_or("invalid Cargo unit hash")?;
                        missing.insert((package.id.clone(), hash.to_owned()));
                        coverage.status = "missing compiler facts".into();
                        coverage.error = Some(error.to_string());
                    }
                }
            } else {
                coverage.status = "outside workspace (ordinary rustc)".into();
            }
            report.crates.push(coverage);
        }
        report.sort();
        Ok((report, missing))
    }

    fn invalidate(&self, units: &BTreeSet<(String, String)>) -> Result<()> {
        for (id, hash) in units {
            let package = self.packages.get(id).ok_or("missing workspace package")?;
            if !valid_hash(hash) {
                return Err("invalid Cargo unit hash".into());
            }
            eprintln!(
                "large-values: rebuilding {} for missing compiler facts",
                package.name
            );
            fs::remove_dir_all(
                self.release
                    .join(".fingerprint")
                    .join(format!("{}-{hash}", package.name)),
            )?;
        }
        Ok(())
    }
}

/// Run Cargo, restoring only workspace package fingerprints whose exact
/// artifacts lack current facts. Cargo messages include newly built and Fresh units.
pub(crate) fn build(
    command: &mut Command,
    log: &BuildLog,
    root: &Path,
    cache: &Path,
    target: &str,
    profile: &str,
    stage: &str,
    output: &Path,
) -> Result<Report> {
    let collector = Collector::new(root, cache, target, profile, command)?;
    let messages = output.join(format!("{stage}-compiler-artifacts.jsonl"));
    if !command
        .get_args()
        .any(|argument| argument.to_string_lossy().starts_with("--message-format"))
    {
        command.arg("--message-format=json-render-diagnostics");
    }
    for attempt in 0..2 {
        command.stdout(Stdio::from(fs::File::create(&messages)?));
        log.run(command, &format!("build {stage}"))?;
        let (report, missing) = collector.collect(&fs::read(&messages)?, stage)?;
        if missing.is_empty() {
            return Ok(report);
        }
        if attempt == 0 {
            collector.invalidate(&missing)?;
        } else {
            eprintln!("warning: large-values: missing compiler facts after retry: {missing:?}");
            return Ok(report);
        }
    }
    unreachable!("both compiler report attempts return")
}

#[cfg(test)]
mod tests;
