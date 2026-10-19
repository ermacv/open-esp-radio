//! The HIL scenario catalog format: the family-independent header of a
//! scenario document, its schema, the role and profile request, the
//! laboratory requirements, the Wi-Fi link vocabulary ([`link`]), and the one discovery of a catalog's documents.
//! Readers that do not run scenarios read it alone.
#![forbid(unsafe_code)]

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub mod link;
pub mod requirements;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const SCENARIO_SCHEMA: u16 = 5;

/// What a scenario is for. A scenario is a qualification scenario because a
/// qualification program references it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Referenced by a qualification program; it runs on a product image.
    Qualification,
    /// Referenced by no program: diagnostics, measurements and experiments.
    Investigation,
}

/// Identity and role shared by every scenario family.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub schema: u16,
    pub id: String,
    pub description: String,
    pub role: Role,
    #[serde(default = "one_repetition")]
    pub repetitions: u8,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Why the scenario cannot run on the current firmware. The runner
    /// refuses to select it, before any image build, with this reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported: Option<String>,
    /// Sample the program counter of the image's harts during the
    /// workload's measured window. It is part of the procedure: a profiled
    /// scenario is a diagnostic, never a throughput or timing measurement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileRequest>,
}

/// A scenario's request for a program-counter profile.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ProfileRequest {
    pub harts: ProfileHarts,
    pub period_us: u32,
}

/// The harts a profile samples.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileHarts {
    Both,
    Core0,
    Core1,
}

impl ProfileRequest {
    /// Periods the sampler supports: short enough to resolve a window,
    /// long enough that sampling stays a small part of each hart's time.
    pub const PERIOD_US: std::ops::RangeInclusive<u32> = 100..=100_000;
}

const fn one_repetition() -> u8 {
    1
}

pub const HEADER_FIELDS: [&str; 8] = [
    "schema",
    "id",
    "description",
    "role",
    "repetitions",
    "tags",
    "unsupported",
    "profile",
];

impl Header {
    /// Read the family-independent identity of a recorded scenario snapshot.
    ///
    /// Recorded runs keep the schema they executed under; only the identity
    /// fields shared by every schema are interpreted here.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Self> {
        let serde_json::Value::Object(mut document) = serde_json::from_slice(bytes)? else {
            return Err("scenario snapshot is not a table".into());
        };
        document.retain(|key, _| HEADER_FIELDS.contains(&key.as_str()));
        let header: Self = serde_json::from_value(serde_json::Value::Object(document))?;
        if !valid_id(&header.id) {
            return Err(format!("invalid recorded scenario id `{}`", header.id).into());
        }
        Ok(header)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != SCENARIO_SCHEMA {
            return Err(format!(
                "scenario schema {} is unsupported (expected {SCENARIO_SCHEMA})",
                self.schema
            )
            .into());
        }
        if !valid_id(&self.id) {
            return Err(format!("invalid scenario id `{}`", self.id).into());
        }
        if self.description.trim().is_empty() {
            return Err("scenario description is empty".into());
        }
        if self
            .unsupported
            .as_ref()
            .is_some_and(|reason| reason.trim().is_empty())
        {
            return Err(format!("scenario `{}` gives an empty unsupported reason", self.id).into());
        }
        if let Some(profile) = self.profile {
            let range = ProfileRequest::PERIOD_US;
            bounded(
                profile.period_us,
                *range.start(),
                *range.end(),
                "profile.period-us",
            )?;
            // Sampling perturbs timing, so a profile never shapes a
            // performance or qualification figure.
            if self.role == Role::Qualification || self.tags.iter().any(|tag| tag == "performance")
            {
                return Err(format!(
                    "scenario `{}` profiles a performance or qualification scenario; profile a \
                     diagnostic copy of it instead",
                    self.id
                )
                .into());
            }
        }
        bounded(self.repetitions, 1, 20, "repetitions")
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Reject a value outside `minimum..=maximum`, naming its field.
pub fn bounded<T>(value: T, minimum: T, maximum: T, field: &str) -> Result<()>
where
    T: PartialOrd + std::fmt::Display,
{
    if value < minimum || value > maximum {
        return Err(format!("{field}={value} is outside {minimum}..={maximum}").into());
    }
    Ok(())
}

/// One scenario document of a catalog, read without its family's types:
/// its validated header and the whole document as a value.
#[derive(Clone, Debug)]
pub struct Document {
    pub path: PathBuf,
    pub header: Header,
    pub value: serde_json::Value,
}

/// The documents of the catalog below `directory`, ordered by id: the one
/// discovery of a catalog, which every reader shares. Each is a regular
/// TOML file named by its id, or a README.md; a symlink anywhere in the
/// path, another file kind, a duplicate id, a header outside this build's
/// schema or a document without exactly one family table fails closed.
pub fn documents(directory: &Path) -> Result<Vec<Document>> {
    // Do not let a symlink in the supplied catalog path bypass the same
    // no-follow policy applied to entries below that directory.
    for component in directory
        .ancestors()
        .filter(|path| !path.as_os_str().is_empty())
    {
        if fs::symlink_metadata(component)?.file_type().is_symlink() {
            return Err(format!(
                "scenario catalog path contains a symlink: {}",
                component.display()
            )
            .into());
        }
    }
    if !fs::symlink_metadata(directory)?.file_type().is_dir() {
        return Err(format!(
            "scenario catalog is not a regular directory: {}",
            directory.display()
        )
        .into());
    }
    let mut documents = Vec::new();
    let mut pending = vec![directory.to_owned()];
    let mut ids = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            if !kind.is_file() {
                return Err(format!(
                    "scenario catalog contains a non-regular entry: {}",
                    path.display()
                )
                .into());
            }
            if entry.file_name() == "README.md" {
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "toml") {
                return Err(format!(
                    "scenario catalog contains an unsupported file: {}",
                    path.display()
                )
                .into());
            }
            let text = fs::read_to_string(&path)?;
            let document =
                document(&text).map_err(|error| format!("{}: {error}", path.display()))?;
            if path.file_stem().and_then(|name| name.to_str()) != Some(document.0.id.as_str()) {
                return Err(format!(
                    "scenario filename does not match ID `{}`: {}",
                    document.0.id,
                    path.display()
                )
                .into());
            }
            if !ids.insert(document.0.id.clone()) {
                return Err(format!("duplicate HIL scenario id `{}`", document.0.id).into());
            }
            documents.push(Document {
                path,
                header: document.0,
                value: document.1,
            });
        }
    }
    if documents.is_empty() {
        return Err(format!("scenario catalog is empty: {}", directory.display()).into());
    }
    // Order by identity, independent of domain-folder placement.
    documents.sort_by(|left, right| left.header.id.cmp(&right.header.id));
    Ok(documents)
}

/// The validated header and value of one TOML scenario document, which must
/// hold exactly one table besides its header: its family's.
fn document(text: &str) -> Result<(Header, serde_json::Value)> {
    let table: toml::Table = toml::from_str(text)?;
    let value = serde_json::to_value(table)?;
    let serde_json::Value::Object(fields) = &value else {
        return Err("scenario document is not a table".into());
    };
    let header = fields
        .iter()
        .filter(|(key, _)| HEADER_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    let header: Header = serde_json::from_value(serde_json::Value::Object(header))?;
    header.validate()?;
    let families = fields
        .iter()
        .filter(|(key, _)| !HEADER_FIELDS.contains(&key.as_str()))
        .collect::<Vec<_>>();
    if !matches!(families.as_slice(), [(_, serde_json::Value::Object(_))]) {
        return Err(format!(
            "scenario `{}` must name exactly one family table",
            header.id
        )
        .into());
    }
    Ok((header, value))
}
