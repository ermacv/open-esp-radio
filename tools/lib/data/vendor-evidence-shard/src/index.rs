//! The shard format: typed-scenario verdicts per vendor root, and the source
//! digests that keep them current. Shards carry identities and verdicts
//! only, never vendor bytes.
//!
//! Some facts span scenarios: a vendor location is untriaged only when no
//! claim of any scenario covers it, and a production line is unobserved only
//! when no scenario observes it. Each shard therefore keeps the scenario-local
//! sets from which [`Evidence`] derives those cross-scenario views exactly.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::Result;

/// Shard format.
pub const SCHEMA: u32 = 11;
/// The `command` of a shard the Blobray scenario engine wrote.
pub const BLOBRAY: &str = "vendor-scenario";
/// The `command` of a shard a host stand wrote: it compiles pinned vendor
/// source and measures no coverage.
pub const HOST_STAND: &str = "vendor-host";
/// Every producer a shard may name.
pub const COMMANDS: [&str; 2] = [BLOBRAY, HOST_STAND];
/// Extension of a shard file; its stem is the scenario name.
pub const SHARD_EXTENSION: &str = "json";
/// The only verdict an entry carries: a claim exists only when its root
/// compared with MATCH in every comparison of the run that selects it.
pub const MATCH: &str = "match";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Index {
    pub schema: u32,
    /// The producer that wrote the shard, one of [`COMMANDS`].
    pub command: String,
    pub target: String,
    /// The scenario whose claims this shard holds; the file stem.
    pub scenario: String,
    /// SHA-256 of each input the scenario captured that a fetched pin
    /// identifies, by session input index. The production probe ELF and
    /// firmware outputs are built on the host, so no digest of theirs is
    /// recorded: their recipes are among the `sources`.
    pub inputs: BTreeMap<String, String>,
    /// Digests of the sources every verdict depends on, sorted by path and
    /// unique: each production file the scenario's executions ran or whose
    /// named data they read, and whole directories for the probes, the
    /// scenario code, the Blobray engine and the shared schema, plus the
    /// toolchain file. A shard whose executions could not be attributed to
    /// files lists its probe's whole production closure instead
    /// ([`Dependence::fallback`]).
    pub sources: Vec<SourceDigest>,
    /// The staleness layers beyond `sources`.
    pub dependence: Dependence,
    pub entries: Vec<Entry>,
    /// Uncovered vendor locations of the scenario's claimed root closures
    /// that no reviewed decision excludes and no claim of this scenario whose
    /// closure contains their function covers, ascending and unique.
    pub untriaged: Vec<Location>,
    /// Every vendor function the scenario's claimed closures contain,
    /// ascending and unique: another scenario's untriaged location in one of
    /// them is covered unless it is untriaged here too.
    pub functions: Vec<String>,
    /// Production hardware source lines the scenario executed without
    /// observing them and no reviewed decision covers, ascending and unique.
    pub unobserved: Vec<SourceLine>,
    /// Production hardware source lines the scenario observed, ascending and
    /// unique.
    pub observed: Vec<SourceLine>,
    /// Persistent vendor bytes a claim's cases write without comparing them
    /// and no reviewed decision covers, coalesced by data symbol, ascending.
    pub unprojected: Vec<StateRange>,
}

/// What a shard's verdicts depend on beyond its source digests.
/// [`Index::is_current`] checks the source digests only, without a build;
/// `read_data` changes only with the probe data the executions read, which
/// only a rerun recomputes: the derived index is computed whole for each
/// evaluated checkout (`cargo verification evidence`), so a constant edited
/// in a file the executions did not run is caught there.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Dependence {
    /// SHA-256 of the probe data the executions read, by content.
    pub read_data: String,
    /// Why `sources` holds the probe's whole production closure rather than
    /// the executed files, when it does.
    pub fallback: Option<String>,
    /// The chip's reviewed coverage decisions the shard's `untriaged` set
    /// depends on: those with a place in one of its `functions`
    /// ([`CoverageDecisions::applicable_digest`]), so an edit to another
    /// decision leaves the shard current. None for a shard without claimed
    /// closures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_decisions: Option<DecisionDigest>,
}

/// SHA-256 of the applicable decisions of a decision file.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DecisionDigest {
    /// The decision file, relative to the repository root.
    pub path: PathBuf,
    pub sha256: String,
}

/// A chip's reviewed coverage decisions, as its decision file holds them.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoverageDecisions {
    #[serde(rename = "decision", default)]
    pub decisions: Vec<CoverageDecision>,
}

/// One reviewed exclusion of uncovered vendor locations, with its reason.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoverageDecision {
    pub reason: String,
    #[serde(rename = "place")]
    pub places: Vec<CoveragePlace>,
}

/// What one decision excludes of vendor `function`: every uncovered
/// location, those of a function that only produces diagnostic output, or
/// those at offsets from `start` up to `end`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoveragePlace {
    pub function: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub diagnostic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u32>,
}

impl CoverageDecisions {
    /// Read and validate the decision file at `relative` below `root`.
    pub fn read(root: &Path, relative: &Path) -> Result<Self> {
        let text = fs::read_to_string(root.join(relative))
            .map_err(|error| format!("coverage decisions {}: {error}", relative.display()))?;
        let decisions: Self = toml::from_str(&text)
            .map_err(|error| format!("coverage decisions {}: {error}", relative.display()))?;
        for decision in &decisions.decisions {
            if decision.places.is_empty() {
                return Err(
                    format!("coverage decision without a place: {}", decision.reason).into(),
                );
            }
            for place in &decision.places {
                let range = match (place.start, place.end) {
                    (None, None) => false,
                    (Some(start), Some(end)) if start < end => true,
                    _ => {
                        return Err(format!(
                            "coverage decision place in {} needs an ascending start and end",
                            place.function
                        )
                        .into());
                    }
                };
                if range && place.diagnostic {
                    return Err(format!(
                        "diagnostic coverage decision place in {} names a range",
                        place.function
                    )
                    .into());
                }
            }
        }
        Ok(decisions)
    }

    /// SHA-256 of the decisions with a place in one of `functions`, in file
    /// order: what a shard whose closures contain `functions` depends on.
    pub fn applicable_digest(&self, functions: &[String]) -> String {
        let applicable: Vec<&CoverageDecision> = self
            .decisions
            .iter()
            .filter(|decision| {
                decision
                    .places
                    .iter()
                    .any(|place| functions.contains(&place.function))
            })
            .collect();
        let bytes = serde_json::to_vec(&applicable).expect("decisions serialize");
        format!("{:x}", Sha256::digest(bytes))
    }
}

impl Dependence {
    /// A shard whose sources are its whole source closure for `reason`,
    /// with no probe data read.
    pub fn whole_closure(reason: &str) -> Self {
        Self {
            read_data: format!("{:x}", Sha256::new().finalize()),
            fallback: Some(reason.into()),
            coverage_decisions: None,
        }
    }
}

/// Persistent vendor bytes a claim's compared cases wrote: those every
/// writing case compares, and the others by whether a reviewed decision
/// covers them.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct State {
    pub written: u64,
    pub compared: u64,
    pub reviewed: u64,
    pub untriaged: u64,
}

/// Bytes `offset..offset + length` of a vendor data symbol; bytes outside
/// every sized data symbol are named by their hexadecimal address.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct StateRange {
    pub symbol: String,
    pub offset: u32,
    pub length: u32,
}

/// Production PHY source lines a claim's executions executed, by whether a
/// compared observation of those executions depends on them. A line another
/// claim observes can be unobserved here; the index lists only lines no
/// scenario observes.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Observation {
    pub executed: u64,
    pub observed: u64,
    /// Unobserved lines a reviewed decision covers.
    pub reviewed: u64,
    /// Unobserved lines without a decision.
    pub untriaged: u64,
}

/// One line of a repository source file.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SourceLine {
    pub path: PathBuf,
    pub line: u32,
}

/// Reached and total basic blocks or branch directions.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Count {
    pub reached: u64,
    pub total: u64,
}

/// Vendor coverage of one claimed root's closure over its executions.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Coverage {
    pub blocks: Count,
    /// Both directions of every conditional branch.
    pub directions: Count,
    /// Transfer sites the closure leaves open: indirect transfers followed
    /// only to their executed targets, and unresolved transfers.
    pub open: u64,
    /// Uncovered blocks and directions and open sites a reviewed decision
    /// excludes.
    pub excluded: u64,
    /// Uncovered blocks and directions and open sites without a decision.
    pub untriaged: u64,
}

/// One vendor block, branch direction or open transfer site, by function
/// symbol and offset.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Location {
    pub function: String,
    pub offset: u32,
    pub kind: LocationKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum LocationKind {
    Block,
    Taken,
    Fallthrough,
    /// An indirect transfer the closure follows only to its executed
    /// targets; targets no execution reached are outside the closure.
    Followed,
    /// A transfer whose target the closure cannot determine, or that leaves
    /// the executable captured code.
    Unresolved,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SourceDigest {
    pub path: PathBuf,
    pub sha256: String,
}

/// One vendor root compared with one compiled production entry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Entry {
    /// Scenario that owns the comparison.
    pub suite: String,
    /// Vendor source kind (`archive` or `rom`) and root symbol.
    pub source: String,
    pub symbol: String,
    /// Compiled production entry compared with the root.
    pub production: String,
    pub verdict: String,
    /// Compared cases of the root/entry pair.
    pub cases: u64,
    /// SHA-256 digests of the reviewed content of the effect contracts and
    /// output projections those comparisons selected, ascending and unique:
    /// rules or fields, applicability and reason, without the endpoints that
    /// name one run's imported revision, so unchanged sources reproduce them.
    /// The contracts are typed values reviewed through git.
    pub reviews: Vec<String>,
    /// Vendor coverage, production observation and vendor state of the
    /// comparisons, which Blobray measures; a stand that compiles vendor
    /// source does not measure them and leaves them out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<Observation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<State>,
}

/// SHA-256 over every file below `relative`, excluding `target` and hidden
/// directories and Markdown documentation, which establishes no verdict:
/// sorted relative paths, each with its length and bytes.
pub fn digest_directory(root: &Path, relative: &Path) -> Result<String> {
    fn walk(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                if name == "target" || name.starts_with('.') {
                    continue;
                }
                walk(&entry.path(), files)?;
            } else if kind.is_file() {
                if !name.ends_with(".md") {
                    files.push(entry.path());
                }
            } else {
                return Err(format!("unsupported source entry {}", entry.path().display()).into());
            }
        }
        Ok(())
    }
    let base = root.join(relative);
    let mut files = vec![];
    walk(&base, &mut files)?;
    files.sort();
    let mut hash = Sha256::new();
    for file in files {
        let name = file.strip_prefix(root)?.to_string_lossy().into_owned();
        let bytes = fs::read(&file)?;
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// SHA-256 of the source at `relative`: a file's bytes, or every file of
/// a directory ([`digest_directory`]).
pub fn digest_source(root: &Path, relative: &Path) -> Result<String> {
    let path = root.join(relative);
    if path.is_file() {
        let bytes = fs::read(&path)?;
        let mut hash = Sha256::new();
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
        Ok(format!("{:x}", hash.finalize()))
    } else {
        digest_directory(root, relative)
    }
}

impl Index {
    /// A well-formed index for `target`: supported schema and producer,
    /// valid digests, unique entries and only MATCH verdicts.
    pub fn validate(&self, target: &str) -> Result<()> {
        let valid = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if self.schema != SCHEMA
            || !COMMANDS.contains(&self.command.as_str())
            || self.target != target
            || !is_relative(Path::new(&self.scenario))
            || self.scenario.contains('.')
        {
            return Err("unsupported scenario evidence shard".into());
        }
        if self.entries.iter().any(|e| e.suite != self.scenario) {
            return Err(format!("shard {} holds another scenario's entry", self.scenario).into());
        }
        if self.functions.windows(2).any(|w| w[0] >= w[1]) {
            return Err("closure functions are not ascending and unique".into());
        }
        if self.observed.windows(2).any(|w| w[0] >= w[1])
            || self.observed.iter().any(|l| !is_relative(&l.path))
        {
            return Err("observed lines are not relative, ascending and unique".into());
        }
        if self.sources.is_empty()
            || self
                .sources
                .iter()
                .any(|s| !valid(&s.sha256) || !is_relative(&s.path))
            || self.sources.windows(2).any(|w| w[0].path >= w[1].path)
            || !valid(&self.dependence.read_data)
            || self
                .dependence
                .coverage_decisions
                .as_ref()
                .is_some_and(|d| !valid(&d.sha256) || !is_relative(&d.path))
            || self.inputs.values().any(|v| !valid(v))
        {
            return Err("scenario evidence index has invalid digests".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for entry in &self.entries {
            if entry.verdict != MATCH
                || entry.cases == 0
                || entry.reviews.windows(2).any(|w| w[0] >= w[1])
                || entry.reviews.iter().any(|r| !valid(r))
            {
                return Err(format!(
                    "entry {} {} is not a MATCH claim",
                    entry.source, entry.symbol
                )
                .into());
            }
            if let Some(c) = &entry.coverage
                && (c.blocks.reached > c.blocks.total
                    || c.directions.reached > c.directions.total
                    || c.excluded + c.untriaged
                        != (c.blocks.total - c.blocks.reached)
                            + (c.directions.total - c.directions.reached)
                            + c.open)
            {
                return Err(format!(
                    "entry {} {} has inconsistent coverage",
                    entry.source, entry.symbol
                )
                .into());
            }
            if let Some(o) = &entry.observation
                && o.observed + o.reviewed + o.untriaged != o.executed
            {
                return Err(format!(
                    "entry {} {} has inconsistent observation",
                    entry.source, entry.symbol
                )
                .into());
            }
            if let Some(s) = &entry.state
                && s.compared + s.reviewed + s.untriaged != s.written
            {
                return Err(format!(
                    "entry {} {} has inconsistent state",
                    entry.source, entry.symbol
                )
                .into());
            }
            if !seen.insert((
                &entry.suite,
                &entry.source,
                &entry.symbol,
                &entry.production,
            )) {
                return Err(format!("entry {} {} repeats", entry.source, entry.symbol).into());
            }
        }
        if self.untriaged.windows(2).any(|w| w[0] >= w[1]) {
            return Err("untriaged locations are not ascending and unique".into());
        }
        if self.unobserved.windows(2).any(|w| w[0] >= w[1])
            || self.unobserved.iter().any(|l| !is_relative(&l.path))
        {
            return Err("unobserved lines are not relative, ascending and unique".into());
        }
        if self.unprojected.iter().any(|r| r.length == 0)
            || self.unprojected.windows(2).any(|w| {
                w[0].symbol > w[1].symbol
                    || (w[0].symbol == w[1].symbol
                        && u64::from(w[0].offset) + u64::from(w[0].length)
                            >= u64::from(w[1].offset))
            })
        {
            return Err("unprojected state is not coalesced, ascending and unique".into());
        }
        Ok(())
    }

    /// The generator's question: whether the shard can be kept as it is.
    /// Anything that differs or no longer exists means regenerating it.
    pub fn is_current(&self, root: &Path) -> bool {
        self.sources.iter().all(|source| {
            digest_source(root, &source.path).is_ok_and(|digest| digest == source.sha256)
        }) && self
            .dependence
            .coverage_decisions
            .as_ref()
            .is_none_or(|decisions| {
                CoverageDecisions::read(root, &decisions.path)
                    .is_ok_and(|file| file.applicable_digest(&self.functions) == decisions.sha256)
            })
    }
}

/// Every shard of an index directory with whether its sources are current.
/// A missing directory holds no shard. The index is derived data, so what
/// only another checkout explains is staleness, not an error: a recorded
/// source or decision file that no longer exists makes a shard not current,
/// and a shard of another schema is listed in `other_schema`.
pub struct Evidence {
    pub shards: Vec<(Index, bool)>,
    /// Scenarios whose shard another schema of the format wrote.
    pub other_schema: Vec<String>,
}

impl Evidence {
    /// Load and validate every shard of `directory` below `root`.
    pub fn load(root: &Path, directory: &Path, target: &str) -> Result<Self> {
        let mut shards = vec![];
        let mut other_schema = vec![];
        let entries = match fs::read_dir(root.join(directory)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    shards,
                    other_schema,
                });
            }
            Err(error) => return Err(error.into()),
        };
        let mut paths = entries
            .map(|entry| Ok(entry?.path()))
            .collect::<Result<Vec<_>>>()?;
        paths.sort();
        for path in paths {
            if path.extension().and_then(|e| e.to_str()) != Some(SHARD_EXTENSION) {
                return Err(format!("unexpected evidence file {}", path.display()).into());
            }
            let text = fs::read_to_string(&path)?;
            let schema = serde_json::from_str::<serde_json::Value>(&text)
                .map_err(|error| format!("evidence shard {}: {error}", path.display()))?
                .get("schema")
                .and_then(serde_json::Value::as_u64);
            if schema != Some(u64::from(SCHEMA)) {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default();
                other_schema.push(name.to_owned());
                continue;
            }
            let shard: Index = serde_json::from_str(&text)?;
            shard
                .validate(target)
                .map_err(|error| format!("evidence shard {}: {error}", path.display()))?;
            if path.file_stem().and_then(|s| s.to_str()) != Some(shard.scenario.as_str()) {
                return Err(
                    format!("evidence shard {} names another scenario", path.display()).into(),
                );
            }
            let current = shard.is_current(root);
            shards.push((shard, current));
        }
        Ok(Self {
            shards,
            other_schema,
        })
    }

    /// Untriaged locations no scenario covers ([`untriaged`]).
    pub fn untriaged(&self) -> Vec<Location> {
        let shards: Vec<&Index> = self.shards.iter().map(|(shard, _)| shard).collect();
        untriaged(&shards).into_iter().collect()
    }

    /// Lines some scenario executed without observing and no scenario
    /// observes.
    pub fn unobserved(&self) -> Vec<SourceLine> {
        let observed: std::collections::BTreeSet<&SourceLine> =
            self.shards.iter().flat_map(|(s, _)| &s.observed).collect();
        let lines: std::collections::BTreeSet<SourceLine> = self
            .shards
            .iter()
            .flat_map(|(s, _)| &s.unobserved)
            .filter(|l| !observed.contains(l))
            .cloned()
            .collect();
        lines.into_iter().collect()
    }
}

/// Untriaged locations no scenario of `shards` covers: untriaged in some
/// shard and in every shard whose closures contain their function.
pub fn untriaged(shards: &[&Index]) -> std::collections::BTreeSet<Location> {
    shards
        .iter()
        .flat_map(|shard| shard.untriaged.iter())
        .filter(|location| {
            shards.iter().all(|shard| {
                shard.functions.binary_search(&location.function).is_err()
                    || shard.untriaged.binary_search(location).is_ok()
            })
        })
        .cloned()
        .collect()
}

fn is_relative(path: &Path) -> bool {
    path.is_relative()
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}
