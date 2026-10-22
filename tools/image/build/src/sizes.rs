//! Versioned image size summaries and comparisons under the owner's rule #375.
use crate::{ImageSpec, Result};
use oer_image_checks::sizes::Measurements;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, process::Command};

pub mod compare;
pub use compare::{Comparison, Verdict, compare_directories};

pub const FILE: &str = "sizes.json";
pub const SCHEMA: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub schema: u32,
    pub type_min_bytes: u64,
    pub provenance: Provenance,
    pub elfs: BTreeMap<String, Measurements>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub commit: String,
    pub dirty: bool,
    pub rustc: String,
    pub chip: String,
    pub package: String,
    pub binary: String,
    pub target: String,
    pub profile: String,
    pub profile_settings: String,
    pub features: Vec<String>,
    pub default_features: bool,
    pub purpose: crate::bundle::Purpose,
    pub layout_seed: Option<u32>,
    /// Values may contain stand credentials: record digests, never plaintext.
    pub configuration_sha256: BTreeMap<String, Option<String>>,
    pub stack_policy: String,
    pub stack_policy_sources: BTreeMap<String, String>,
    /// Actual compiler flags set on each Cargo command, with the checkout
    /// prefix normalized so two checkouts can be compared.
    pub flags: BTreeMap<String, BTreeMap<String, String>>,
}

impl Summary {
    pub(crate) fn new(spec: &ImageSpec, profile: &oer_chip_profile::Profile) -> Result<Self> {
        let mut git = Command::new("git");
        git.current_dir(&spec.root).args(["rev-parse", "HEAD"]);
        let commit = capture(&mut git)?;
        let dirty = !capture(Command::new("git").current_dir(&spec.root).args([
            "status",
            "--porcelain",
            "--untracked-files=normal",
        ]))?
        .is_empty();
        let rustc = capture(
            oer_os_toolchain::command(oer_os_toolchain::Tool::Rustc)?
                .current_dir(&spec.root)
                .arg("-vV"),
        )?;
        let configuration_sha256 = spec
            .runtime_environment
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value
                        .as_ref()
                        .map(|v| oer_os_durable::sha256_bytes(v.as_bytes())),
                )
            })
            .collect();
        let policy_path = spec.root.join(&spec.stack_policy);
        let source = std::fs::read_to_string(&policy_path)?;
        let policy: toml::Table = toml::from_str(&source)?;
        let mut stack_policy_sources = BTreeMap::from([(
            spec.stack_policy.to_string_lossy().into_owned(),
            oer_os_durable::sha256_bytes(source.as_bytes()),
        )]);
        if let Some(base) = policy.get("extends").and_then(toml::Value::as_str) {
            let path = policy_path
                .parent()
                .ok_or("policy has no parent")?
                .join(base);
            let relative = path.strip_prefix(&spec.root)?;
            stack_policy_sources.insert(
                relative.to_string_lossy().into_owned(),
                oer_os_durable::sha256_file(&path)?,
            );
        }
        let manifest: toml::Table =
            toml::from_str(&std::fs::read_to_string(spec.root.join("Cargo.toml"))?)?;
        let profile_settings = manifest
            .get("profile")
            .and_then(|p| p.get(crate::FIRMWARE_PROFILE))
            .ok_or("missing firmware profile")?
            .to_string();
        let mut features = spec.application.features.clone();
        features.sort();
        features.dedup();
        Ok(Self {
            schema: SCHEMA,
            type_min_bytes: oer_image_checks::sizes::TYPE_MIN_BYTES,
            provenance: Provenance {
                commit,
                dirty,
                rustc,
                chip: spec.chip.clone(),
                package: spec.application.package.clone(),
                binary: spec.application.binary.clone(),
                target: profile.rust_target.clone(),
                profile: crate::FIRMWARE_PROFILE.into(),
                profile_settings,
                features,
                default_features: spec.application.default_features,
                purpose: spec.purpose,
                layout_seed: spec.layout_seed.map(u32::from),
                configuration_sha256,
                stack_policy: spec.stack_policy.to_string_lossy().into_owned(),
                stack_policy_sources,
                flags: BTreeMap::new(),
            },
            elfs: BTreeMap::new(),
        })
    }

    pub(crate) fn flags(&mut self, elf: &str, command: &Command, root: &Path) -> Result<()> {
        let mut flags = BTreeMap::new();
        for (key, value) in command.get_envs() {
            let key = key.to_str().ok_or("non-UTF-8 compiler environment name")?;
            if (key.contains("FLAGS") || key.starts_with("OPEN_RADIO_"))
                && let Some(value) = value
            {
                flags.insert(
                    key.into(),
                    value
                        .to_str()
                        .ok_or("non-UTF-8 compiler flags")?
                        .replace(root.to_str().ok_or("non-UTF-8 checkout path")?, "$ROOT"),
                );
            }
        }
        self.provenance.flags.insert(elf.into(), flags);
        Ok(())
    }

    pub(crate) fn publish(&self, output: &Path) -> Result<()> {
        oer_os_durable::atomic_json(&output.join(FILE), self)?;
        Ok(())
    }

    pub fn read(directory: &Path) -> Result<Self> {
        let summary: Self = serde_json::from_slice(&std::fs::read(directory.join(FILE))?)?;
        if summary.schema != SCHEMA
            || summary.type_min_bytes != oer_image_checks::sizes::TYPE_MIN_BYTES
        {
            return Err("incompatible sizes schema or type inventory threshold".into());
        }
        if summary.elfs.is_empty() {
            return Err("sizes summary has no ELF measurements".into());
        }
        Ok(summary)
    }
}

fn capture(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "size provenance command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().into())
}
