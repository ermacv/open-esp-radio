use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use sha2::{Digest as _, Sha256};

use super::{
    Artifact, ArtifactRole, ArtifactState, BUNDLE_SCHEMA, Bundle, InstallPlan, Provider,
    SourceIdentity, model::PlannedArtifact,
};
use crate::Result;

pub fn build_plan(root: &Path, provider: Provider) -> Result<InstallPlan> {
    let artifacts = provider
        .artifact_specs()
        .iter()
        .map(|artifact| {
            let source = root.join(artifact.source);
            PlannedArtifact {
                role: artifact.role,
                state: if source.is_file() {
                    ArtifactState::PresentUnverified
                } else {
                    ArtifactState::Missing
                },
                source,
                target: artifact.target.into(),
            }
        })
        .collect();
    let build_steps = match provider {
        Provider::LinuxNet => vec![
            "cargo stand fixture build-hostapd".to_owned(),
            "cargo build --locked -p oer-stand-fixture-install -p oer-hil-fixture --bin open-radio-net-launcher --bin open-radio-probe-launcher --bin open-radio-probe --bin open-radio-fixture-install".to_owned(),
        ],
    };
    Ok(InstallPlan {
        schema: 1,
        provider,
        artifacts,
        build_steps,
        authorization_boundary: "foreground sudo exec after unprivileged preparation".to_owned(),
        activation_boundary: "root-owned generation switch after candidate policy validation"
            .to_owned(),
        readonly_verification: vec![
            "imported artifact bytes and modes".to_owned(),
            "effective sudoers policy".to_owned(),
            "finite helper capabilities".to_owned(),
        ],
        automatic_hardware_checks: false,
    })
}

pub fn prepare(root: &Path, provider: Provider, operator: &str) -> Result<PathBuf> {
    validate_operator(operator)?;
    let output = root.join("target/hil/fixture-install");
    fs::create_dir_all(&output)?;
    let _lock = oer_process::lock::FileLock::acquire(
        &output.join("prepare.lock"),
        oer_process::lock::Mode::Exclusive,
    )?;

    let source = source_identity(root)?;
    let temporary = tempfile::Builder::new()
        .prefix(&format!("{}-", provider.as_str()))
        .tempdir_in(&output)?;
    let artifact_directory = temporary.path().join("artifacts");
    fs::create_dir(&artifact_directory)?;
    let mut artifacts = Vec::new();
    for spec in provider.artifact_specs() {
        let source_path = root.join(spec.source);
        let metadata = fs::symlink_metadata(&source_path).map_err(|error| {
            format!(
                "prepared artifact {} is unavailable: {error}",
                source_path.display()
            )
        })?;
        if !metadata.file_type().is_file() {
            return Err(format!(
                "prepared artifact is not a regular file: {}",
                source_path.display()
            )
            .into());
        }
        let destination = artifact_directory.join(spec.name);
        copy_and_hash(&source_path, &destination)?;
        fs::set_permissions(&destination, fs::Permissions::from_mode(spec.mode))?;
        let bytes = fs::read(&destination)?;
        artifacts.push(Artifact {
            role: spec.role,
            file_name: spec.name.to_owned(),
            target: spec.target.into(),
            size_bytes: bytes.len() as u64,
            sha256: oer_durable::sha256_bytes(&bytes),
            mode: spec.mode,
        });
    }
    validate_prepared_contract(provider, &artifact_directory, &artifacts)?;

    let mut bundle = Bundle {
        schema: BUNDLE_SCHEMA,
        provider,
        generation: String::new(),
        operator: operator.to_owned(),
        source,
        runtime_contract: provider.runtime_contract().to_owned(),
        artifacts,
    };
    bundle.generation = bundle_identity(&bundle)?;
    let final_directory = output.join(provider.as_str()).join(&bundle.generation);
    if final_directory.exists() {
        validate_existing_bundle(&final_directory, &bundle)?;
        return Ok(final_directory);
    }
    fs::create_dir_all(final_directory.parent().expect("generation parent"))?;
    write_json(&temporary.path().join("bundle.json"), &bundle)?;
    fs::rename(temporary.keep(), &final_directory)?;
    Ok(final_directory)
}

fn validate_prepared_contract(
    provider: Provider,
    directory: &Path,
    artifacts: &[Artifact],
) -> Result<()> {
    match provider {
        Provider::LinuxNet => {
            let hostapd = artifacts
                .iter()
                .find(|artifact| artifact.role == ArtifactRole::Hostapd)
                .ok_or("hostapd artifact missing")?;
            let provenance: serde_json::Value =
                serde_json::from_slice(&fs::read(directory.join("open-radio-hostapd.json"))?)?;
            if provenance
                .get("binary_sha256")
                .and_then(serde_json::Value::as_str)
                != Some(hostapd.sha256.as_str())
            {
                return Err("hostapd provenance does not identify the prepared binary".into());
            }
            require_capabilities(
                &directory.join("open-radio-net"),
                provider.runtime_contract(),
            )
        }
    }
}

fn require_capabilities(helper: &Path, expected: &str) -> Result<()> {
    require_capabilities_with_timeout(helper, expected, std::time::Duration::from_secs(5))
}

fn require_capabilities_with_timeout(
    helper: &Path,
    expected: &str,
    timeout: std::time::Duration,
) -> Result<()> {
    let output = oer_process::output(
        oer_process::command(helper).arg("capabilities"),
        Some(timeout),
    )?;
    if !output.status.success() || String::from_utf8(output.stdout)?.trim() != expected {
        return Err(format!(
            "prepared helper has an incompatible runtime contract: {}",
            helper.display()
        )
        .into());
    }
    Ok(())
}

fn validate_existing_bundle(directory: &Path, expected: &Bundle) -> Result<()> {
    let actual: Bundle = serde_json::from_slice(&fs::read(directory.join("bundle.json"))?)?;
    if &actual != expected {
        return Err(format!("existing bundle identity mismatch: {}", directory.display()).into());
    }
    for artifact in &actual.artifacts {
        let path = directory.join("artifacts").join(&artifact.file_name);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file()
            || metadata.len() != artifact.size_bytes
            || metadata.permissions().mode() & 0o777 != artifact.mode
            || oer_durable::sha256_file(&path)? != artifact.sha256
        {
            return Err(format!("existing bundle artifact mismatch: {}", path.display()).into());
        }
    }
    validate_prepared_contract(
        actual.provider,
        &directory.join("artifacts"),
        &actual.artifacts,
    )?;
    Ok(())
}

fn source_identity(root: &Path) -> Result<SourceIdentity> {
    let commit = oer_process::git::text(root, ["rev-parse", "HEAD"])?;
    let state = oer_process::git::output(
        root,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .map_err(|error| format!("cannot capture repository state for fixture bundle: {error}"))?;
    let diff = oer_process::git::output(root, ["diff", "--binary", "--no-ext-diff", "HEAD", "--"])
        .map_err(|error| {
            format!("cannot capture repository content identity for fixture bundle: {error}")
        })?;
    let mut workspace = Sha256::new();
    workspace.update(&state);
    workspace.update(&diff);
    Ok(SourceIdentity {
        commit,
        dirty: !state.is_empty(),
        workspace_state_sha256: format!("{:x}", workspace.finalize()),
    })
}

fn bundle_identity(bundle: &Bundle) -> Result<String> {
    let mut identity = bundle.clone();
    identity.generation.clear();
    Ok(oer_durable::sha256_bytes(&serde_json::to_vec(&identity)?))
}

fn copy_and_hash(source: &Path, destination: &Path) -> Result<String> {
    let mut source = File::open(source)?;
    let mut destination = File::create(destination)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        destination.write_all(&buffer[..count])?;
        digest.update(&buffer[..count]);
    }
    destination.sync_all()?;
    Ok(format!("{:x}", digest.finalize()))
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let mut file = File::create(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn validate_operator(operator: &str) -> Result<()> {
    if operator.is_empty()
        || operator == "root"
        || !operator
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("operator must be a non-root Linux account name".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_capabilities_cannot_hold_preparation_open_indefinitely() {
        let _serial = crate::tests::serial();
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("helper");
        fs::write(&helper, "#!/bin/sh\nexec sleep 60\n").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let error = require_capabilities_with_timeout(
            &helper,
            "expected",
            std::time::Duration::from_millis(30),
        )
        .unwrap_err();
        assert!(
            error.is::<oer_process::owned::DeadlineExceeded>(),
            "{error}"
        );
    }
}
