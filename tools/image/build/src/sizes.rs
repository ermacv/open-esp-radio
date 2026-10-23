//! Versioned image size summaries and comparisons under the owner's rule #375.
use crate::{ImageSpec, Result};
use oer_image_checks::sizes::Measurements;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, process::Command};

pub mod compare;
mod refresh;
pub use compare::{Comparison, Verdict, compare_directories};
pub use refresh::refresh;

pub const FILE: &str = "sizes.json";
pub const SCHEMA: u32 = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub schema: u32,
    pub type_min_bytes: u64,
    pub provenance: Provenance,
    pub elfs: BTreeMap<String, Measurements>,
    pub image_files: BTreeMap<String, u64>,
    pub embedded_payloads: Vec<EmbeddedPayload>,
}

/// A byte-verified copy of a payload already measured through its own ELF.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddedPayload {
    pub source_file: String,
    pub elf: String,
    pub section: String,
    pub address: u64,
    pub bytes: u64,
    pub sha256: String,
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
        let mut configuration_sha256: BTreeMap<String, Option<String>> = spec
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
        // Cargo's [env] defaults and ambient compiler configuration are part
        // of the image too. Keep values private, including those set by a
        // caller instead of by the runtime recipe.
        let config: toml::Table = toml::from_str(&std::fs::read_to_string(
            spec.root.join(".cargo/config.toml"),
        )?)?;
        let mut forced = BTreeMap::new();
        if let Some(defaults) = config.get("env").and_then(toml::Value::as_table) {
            for (name, setting) in defaults
                .iter()
                .filter(|(name, _)| image_configuration(name))
            {
                let value = setting
                    .as_str()
                    .or_else(|| setting.get("value").and_then(toml::Value::as_str))
                    .ok_or("invalid image environment setting")?;
                let digest = Some(oer_os_durable::sha256_bytes(value.as_bytes()));
                configuration_sha256.insert(name.clone(), digest.clone());
                if setting.get("force").and_then(toml::Value::as_bool) == Some(true) {
                    forced.insert(name.clone(), digest);
                }
            }
        }
        for (name, value) in std::env::vars_os() {
            if let Some(name) = name.to_str().filter(|name| image_configuration(name)) {
                configuration_sha256.insert(
                    name.into(),
                    Some(oer_os_durable::sha256_bytes(value.as_encoded_bytes())),
                );
            }
        }
        configuration_sha256.extend(forced);
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
        let profiles = manifest.get("profile").ok_or("missing Cargo profiles")?;
        let firmware = profiles
            .get(crate::FIRMWARE_PROFILE)
            .ok_or("missing firmware profile")?;
        let parent = firmware
            .get("inherits")
            .and_then(toml::Value::as_str)
            .ok_or("firmware profile has no parent")?;
        let profile_settings = format!(
            "{firmware}\n{parent}: {}",
            profiles.get(parent).ok_or("missing parent profile")?
        );
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
            image_files: BTreeMap::new(),
            embedded_payloads: Vec::new(),
        })
    }

    pub(crate) fn flags(&mut self, elf: &str, command: &Command, root: &Path) -> Result<()> {
        let mut flags = BTreeMap::new();
        for name in ["CARGO_ENCODED_RUSTFLAGS", "CFLAGS", "CXXFLAGS", "CC", "CXX"] {
            if let Some(value) = std::env::var_os(name) {
                flags.insert(
                    name.into(),
                    value
                        .to_str()
                        .ok_or("non-UTF-8 compiler flags")?
                        .replace(root.to_str().ok_or("non-UTF-8 checkout path")?, "$ROOT"),
                );
            }
        }
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
        let mut measured = self.clone();
        measured.contents(output)?;
        oer_os_durable::atomic_json(&output.join(FILE), &measured)?;
        Ok(())
    }

    fn contents(&mut self, directory: &Path) -> Result<()> {
        use crate::bundle::files;
        self.image_files.clear();
        self.embedded_payloads.clear();
        for name in [files::RUNTIME_BIN, files::APPLICATION] {
            let path = directory.join(name);
            if path.is_file() {
                self.image_files
                    .insert(name.into(), std::fs::metadata(path)?.len());
            }
        }
        let bootstrap = directory.join(files::BOOTSTRAP_ELF);
        let runtime = directory.join(files::RUNTIME_BIN);
        if !bootstrap.is_file() || !runtime.is_file() {
            return Ok(());
        }
        let payload = std::fs::read(runtime)?;
        if payload.is_empty() {
            return Err("sizes: embedded runtime payload is empty".into());
        }
        let data = std::fs::read(bootstrap)?;
        let elf = oer_analysis_elf::Elf::parse(&data)?;
        for section in elf
            .sections()
            .filter(|s| s.allocated && !s.writable && !s.nobits)
        {
            for (offset, _) in section
                .data
                .windows(payload.len())
                .enumerate()
                .filter(|(_, bytes)| *bytes == payload.as_slice())
            {
                self.embedded_payloads.push(EmbeddedPayload {
                    source_file: files::RUNTIME_BIN.into(),
                    elf: "bootstrap".into(),
                    section: section.name.into(),
                    address: section.address + offset as u64,
                    bytes: payload.len() as u64,
                    sha256: oer_os_durable::sha256_bytes(&payload),
                });
            }
        }
        if self.embedded_payloads.is_empty() {
            return Err("sizes: bootstrap contains no byte-identical copy of runtime.bin".into());
        }
        if self.embedded_payloads.len() != 1 {
            return Err("sizes: bootstrap runtime copy is ambiguous".into());
        }
        Ok(())
    }

    pub fn read(directory: &Path) -> Result<Self> {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join(FILE))?)?;
        if value["schema"].as_u64() != Some(u64::from(SCHEMA)) {
            return Err("incompatible sizes schema; regenerate with cargo fw sizes-refresh".into());
        }
        let summary: Self = serde_json::from_value(value)?;
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

fn image_configuration(name: &str) -> bool {
    name.starts_with("ESP_HAL_CONFIG_") || name.starts_with("CARGO_PROFILE_")
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
