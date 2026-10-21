use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use sha2::{Digest as _, Sha256};

use super::*;
use crate::{
    model::validate_adapters,
    transaction::test_support::{Effects, Layout, Stage, apply, policy_bytes},
};

const OPERATOR: &str = "fixture_test";

/// Serializes tests that hold installation locks or write executables while
/// other tests spawn processes. A concurrent fork inherits every open
/// descriptor until its exec, so it can briefly keep a session flock
/// (`EAGAIN`) or a written helper (`ETXTBSY`) owned by another test.
pub(super) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct TestEffects {
    fail: Option<Stage>,
    panic: Option<Stage>,
    reject_candidate: bool,
    make_receipt_unwritable: Option<PathBuf>,
    calls: Vec<&'static str>,
}

impl TestEffects {
    fn passing() -> Self {
        Self {
            fail: None,
            panic: None,
            reject_candidate: false,
            make_receipt_unwritable: None,
            calls: Vec::new(),
        }
    }
}

impl Effects for TestEffects {
    fn checkpoint(&mut self, stage: Stage) -> crate::Result<()> {
        if self.panic == Some(stage) {
            panic!("simulated abrupt termination at {stage:?}");
        }
        if self.fail == Some(stage) {
            return Err(format!("injected failure at {stage:?}").into());
        }
        if stage == Stage::Receipt
            && let Some(directory) = &self.make_receipt_unwritable
        {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o555))?;
        }
        Ok(())
    }

    fn validate_candidate(&mut self, _layout: &Layout, candidate: &Path) -> crate::Result<()> {
        self.calls.push("candidate-policy");
        let policy = fs::read_to_string(candidate)?;
        if self.reject_candidate || !policy.contains("NOPASSWD") {
            return Err("candidate policy rejected".into());
        }
        Ok(())
    }

    fn validate_effective(&mut self, _layout: &Layout) -> crate::Result<()> {
        self.calls.push("effective-policy");
        Ok(())
    }

    fn verify_capabilities(&mut self, helper: &Path, expected: &str) -> crate::Result<()> {
        self.calls.push("capabilities");
        let imported = fs::read_to_string(helper)?;
        if !imported.contains(expected) {
            return Err("capabilities mismatch".into());
        }
        Ok(())
    }

    fn require_provider_idle(&mut self, _provider: Provider) -> crate::Result<()> {
        self.calls.push("provider-idle");
        Ok(())
    }
}

fn artifact_declarations(provider: Provider, suffix: &str) -> Vec<(Artifact, Vec<u8>)> {
    let network = format!(
        "#!/bin/sh\ntest \"$1\" = capabilities && printf '%s\\n' '{}'\n# {suffix}\n",
        Provider::LinuxNet.runtime_contract()
    );
    let bluetooth = format!(
        "#!/bin/sh\ntest \"$1\" = capabilities && printf '%s\\n' '{}'\n# {suffix}\n",
        Provider::LinuxBluetooth.runtime_contract()
    );
    let entries: Vec<(ArtifactRole, &str, &str, u32, Vec<u8>)> = match provider {
        Provider::LinuxNet => vec![
            (
                ArtifactRole::NetworkClient,
                "open-radio-net-client",
                "/usr/local/libexec/open-radio-net-client",
                0o555,
                format!("client-{suffix}").into_bytes(),
            ),
            (
                ArtifactRole::Udhcpc,
                "udhcpc",
                "/usr/local/libexec/open-radio-udhcpc",
                0o555,
                format!("udhcpc-{suffix}").into_bytes(),
            ),
            (
                ArtifactRole::UdhcpcProvenance,
                "udhcpc-provenance.json",
                "/usr/local/libexec/open-radio-udhcpc.json",
                0o444,
                format!("{{\"generation\":\"{suffix}\"}}\n").into_bytes(),
            ),
            (
                ArtifactRole::NetworkLauncher,
                "open-radio-net-launcher",
                "/usr/local/libexec/open-radio-net-launcher",
                0o555,
                format!("network-launcher-{suffix}").into_bytes(),
            ),
            (
                ArtifactRole::NetworkHelper,
                "open-radio-net",
                "/usr/local/sbin/open-radio-net",
                0o555,
                network.into_bytes(),
            ),
            (
                ArtifactRole::ProbeLauncher,
                "open-radio-probe-launcher",
                "/usr/local/libexec/open-radio-probe-launcher",
                0o555,
                format!("probe-launcher-{suffix}").into_bytes(),
            ),
            (
                ArtifactRole::ProbeHelper,
                "open-radio-probe",
                "/usr/local/libexec/open-radio-probe",
                0o555,
                format!("probe-{suffix}").into_bytes(),
            ),
            (
                ArtifactRole::Hostapd,
                "open-radio-hostapd",
                "/usr/local/libexec/open-radio-hostapd",
                0o555,
                format!("hostapd-{suffix}").into_bytes(),
            ),
            (
                ArtifactRole::HostapdProvenance,
                "open-radio-hostapd.json",
                "/usr/local/libexec/open-radio-hostapd.json",
                0o444,
                format!("{{\"generation\":\"{suffix}\"}}\n").into_bytes(),
            ),
        ],
        Provider::LinuxBluetooth => vec![
            (
                ArtifactRole::BluetoothLauncher,
                "open-radio-bluetooth-launcher",
                "/usr/local/libexec/open-radio-bluetooth-launcher",
                0o555,
                format!("bluetooth-launcher-{suffix}").into_bytes(),
            ),
            (
                ArtifactRole::BluetoothHelper,
                "open-radio-bluetooth",
                "/usr/local/libexec/open-radio-bluetooth",
                0o555,
                bluetooth.into_bytes(),
            ),
        ],
    };
    entries
        .into_iter()
        .map(|(role, name, target, mode, bytes)| {
            (
                Artifact {
                    role,
                    file_name: name.to_owned(),
                    target: target.into(),
                    size_bytes: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                    mode,
                },
                bytes,
            )
        })
        .collect()
}

fn make_bundle(directory: &Path, provider: Provider, suffix: &str) -> (PathBuf, Bundle) {
    let bundle_directory = directory.join(format!("bundle-{}-{suffix}", provider.as_str()));
    let artifact_directory = bundle_directory.join("artifacts");
    fs::create_dir_all(&artifact_directory).unwrap();
    fs::set_permissions(&bundle_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let declarations = artifact_declarations(provider, suffix);
    for (artifact, bytes) in &declarations {
        let path = artifact_directory.join(&artifact.file_name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(artifact.mode)).unwrap();
    }
    let mut bundle = Bundle {
        schema: BUNDLE_SCHEMA,
        provider,
        generation: String::new(),
        operator: OPERATOR.to_owned(),
        source: SourceIdentity {
            commit: "1".repeat(40),
            dirty: true,
            workspace_state_sha256: format!("{:x}", Sha256::digest(suffix.as_bytes())),
        },
        runtime_contract: provider.runtime_contract().to_owned(),
        allowed_bluetooth_adapters: provider.default_adapters(),
        artifacts: declarations
            .into_iter()
            .map(|(artifact, _)| artifact)
            .collect(),
    };
    bundle.generation = bundle_hash(&bundle);
    fs::write(
        bundle_directory.join("bundle.json"),
        serde_json::to_vec_pretty(&bundle).unwrap(),
    )
    .unwrap();
    (bundle_directory, bundle)
}

fn bundle_hash(bundle: &Bundle) -> String {
    let mut identity = bundle.clone();
    identity.generation.clear();
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&identity).unwrap())
    )
}

fn test_apply(
    root: &Path,
    provider: Provider,
    bundle: &Path,
    effects: &mut TestEffects,
) -> InstallResult {
    let layout = Layout::test(root, provider).unwrap();
    apply(
        &layout,
        provider,
        bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        effects,
    )
    .unwrap()
}

#[test]
fn plans_are_provider_specific_deterministic_and_offline() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let net = build_plan(root.path(), Provider::LinuxNet, &[]).unwrap();
    assert_eq!(
        net,
        build_plan(root.path(), Provider::LinuxNet, &[]).unwrap()
    );
    assert_eq!(net.provider, Provider::LinuxNet);
    assert!(net.build_steps.iter().any(|step| step.contains("hostapd")));
    assert!(
        net.artifacts
            .iter()
            .all(|item| item.state == ArtifactState::Missing)
    );
    assert!(!net.automatic_hardware_checks);

    let bluetooth = build_plan(root.path(), Provider::LinuxBluetooth, &[]).unwrap();
    assert_eq!(bluetooth.allowed_bluetooth_adapters, ["hci0"]);
    assert!(
        bluetooth
            .build_steps
            .iter()
            .all(|step| !step.contains("hostapd"))
    );
}

#[test]
fn bluetooth_adapter_policy_is_explicit_and_canonical() {
    let _serial = serial();
    assert_eq!(
        validate_adapters(
            Provider::LinuxBluetooth,
            &["hci2".into(), "hci0".into(), "hci2".into()]
        )
        .unwrap(),
        ["hci0", "hci2"]
    );
    for invalid in ["hci01", "hci65535", "../hci0", "hci*", "wlan0"] {
        assert!(
            validate_adapters(Provider::LinuxBluetooth, &[invalid.into()]).is_err(),
            "accepted {invalid}"
        );
    }
    assert!(validate_adapters(Provider::LinuxNet, &["hci0".into()]).is_err());
}

#[test]
fn fresh_repeat_and_upgrade_use_complete_generations() {
    let _serial = serial();
    use std::os::unix::fs::MetadataExt as _;

    let root = tempfile::tempdir().unwrap();
    let (first_path, first) = make_bundle(root.path(), Provider::LinuxBluetooth, "first");
    let result = test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &first_path,
        &mut TestEffects::passing(),
    );
    assert_eq!(
        result.state,
        InstallState::SoftwareVerified,
        "{:?}",
        result.primary_error
    );
    assert!(result.changed);
    let lease_path = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth/session.lock");
    let lease_inode = fs::metadata(&lease_path).unwrap().ino();
    let stable = root.path().join("usr/local/libexec/open-radio-bluetooth");
    assert_eq!(
        fs::read(&stable).unwrap(),
        artifact_declarations(Provider::LinuxBluetooth, "first")[1].1
    );

    let repeat = test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &first_path,
        &mut TestEffects::passing(),
    );
    assert_eq!(repeat.state, InstallState::SoftwareVerified);
    assert!(!repeat.changed);
    assert_eq!(fs::metadata(&lease_path).unwrap().ino(), lease_inode);

    let (second_path, second) = make_bundle(root.path(), Provider::LinuxBluetooth, "second");
    let upgrade = test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &second_path,
        &mut TestEffects::passing(),
    );
    assert_eq!(
        upgrade.previous_generation.as_deref(),
        Some(first.generation.as_str())
    );
    assert_eq!(upgrade.generation, second.generation);
    assert_eq!(fs::metadata(&lease_path).unwrap().ino(), lease_inode);
    assert_eq!(
        fs::read(&stable).unwrap(),
        artifact_declarations(Provider::LinuxBluetooth, "second")[1].1
    );
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-bluetooth/generations")
            .join(first.generation)
            .exists()
    );
}

#[test]
fn post_install_dispatch_contains_only_policy_bytes_and_capabilities_checks() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "software-only");
    let mut effects = TestEffects::passing();
    let result = test_apply(root.path(), Provider::LinuxBluetooth, &bundle, &mut effects);
    assert_eq!(result.state, InstallState::SoftwareVerified);
    assert_eq!(
        effects.calls,
        [
            "provider-idle",
            "candidate-policy",
            "effective-policy",
            "effective-policy",
            "capabilities"
        ]
    );
}

#[test]
fn invalid_candidate_rolls_back_without_replacing_old_bytes_or_policy() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (first, _) = make_bundle(root.path(), Provider::LinuxNet, "old");
    let installed = test_apply(
        root.path(),
        Provider::LinuxNet,
        &first,
        &mut TestEffects::passing(),
    );
    let stable = root.path().join("usr/local/sbin/open-radio-net");
    let policy = root.path().join("etc/sudoers.d/open-radio-net");
    let old_bytes = fs::read(&stable).unwrap();
    let old_policy = fs::read(&policy).unwrap();
    let (second, _) = make_bundle(root.path(), Provider::LinuxNet, "new");
    let mut effects = TestEffects::passing();
    effects.reject_candidate = true;
    let rejected = test_apply(root.path(), Provider::LinuxNet, &second, &mut effects);
    assert_eq!(rejected.state, InstallState::RolledBack);
    assert!(rejected.primary_error.unwrap().contains("candidate policy"));
    assert_eq!(fs::read(stable).unwrap(), old_bytes);
    assert_eq!(fs::read(policy).unwrap(), old_policy);
    assert_eq!(
        fs::read_link(
            root.path()
                .join("var/lib/open-radio/fixture/linux-net/current")
        )
        .unwrap(),
        PathBuf::from("generations").join(installed.generation)
    );
}

#[test]
fn effective_policy_failure_rolls_back_and_never_runs_capabilities() {
    let _serial = serial();
    struct EffectiveReject {
        effective_calls: usize,
    }
    impl Effects for EffectiveReject {
        fn validate_candidate(&mut self, _: &Layout, _: &Path) -> crate::Result<()> {
            Ok(())
        }
        fn validate_effective(&mut self, _: &Layout) -> crate::Result<()> {
            self.effective_calls += 1;
            if self.effective_calls == 1 {
                Err("effective policy conflict".into())
            } else {
                Ok(())
            }
        }
        fn verify_capabilities(&mut self, _: &Path, _: &str) -> crate::Result<()> {
            panic!("capabilities must not run after policy rejection")
        }
    }

    let root = tempfile::tempdir().unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "policy-conflict");
    let mut effects = EffectiveReject { effective_calls: 0 };
    let result = apply(
        &layout,
        Provider::LinuxBluetooth,
        &bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut effects,
    )
    .unwrap();
    assert_eq!(result.state, InstallState::RolledBack);
    assert!(result.primary_error.unwrap().contains("effective policy"));
    assert_eq!(effects.effective_calls, 2);
    assert!(
        !root
            .path()
            .join("etc/sudoers.d/open-radio-bluetooth")
            .exists()
    );
}

#[test]
fn every_control_stage_fails_closed_and_rollback_failure_is_reported() {
    let _serial = serial();
    for stage in [
        Stage::Imported,
        Stage::LinksPrepared,
        Stage::CandidateValidated,
        Stage::PolicyPublished,
        Stage::Activated,
        Stage::SoftwareVerified,
    ] {
        let root = tempfile::tempdir().unwrap();
        let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "failure");
        let mut effects = TestEffects::passing();
        effects.fail = Some(stage);
        let result = test_apply(root.path(), Provider::LinuxBluetooth, &bundle, &mut effects);
        assert_eq!(result.state, InstallState::RolledBack, "stage {stage:?}");
        assert!(
            !root
                .path()
                .join("usr/local/libexec/open-radio-bluetooth")
                .exists(),
            "stage {stage:?} left a stable helper"
        );
    }

    struct RollbackFailure;
    impl Effects for RollbackFailure {
        fn checkpoint(&mut self, stage: Stage) -> crate::Result<()> {
            if stage == Stage::CandidateValidated || stage == Stage::Rollback {
                return Err(format!("failure at {stage:?}").into());
            }
            Ok(())
        }
        fn validate_candidate(&mut self, _: &Layout, _: &Path) -> crate::Result<()> {
            Ok(())
        }
        fn validate_effective(&mut self, _: &Layout) -> crate::Result<()> {
            Ok(())
        }
        fn verify_capabilities(&mut self, _: &Path, _: &str) -> crate::Result<()> {
            Ok(())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "rollback");
    let result = apply(
        &layout,
        Provider::LinuxBluetooth,
        &bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut RollbackFailure,
    )
    .unwrap();
    assert_eq!(result.state, InstallState::RecoveryRequired);
    assert!(result.primary_error.is_some());
    assert!(result.recovery_error.is_some());
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-bluetooth/transaction.json")
            .exists()
    );
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_err());
}

#[test]
fn abrupt_process_loss_is_recovered_from_persisted_journal() {
    let _serial = serial();
    for stage in [Stage::PolicyPublished, Stage::Activated] {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
        let (bundle, expected) = make_bundle(root.path(), Provider::LinuxBluetooth, "crash");
        let mut crashing = TestEffects::passing();
        crashing.panic = Some(stage);
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = apply(
                &layout,
                Provider::LinuxBluetooth,
                &bundle,
                OPERATOR,
                rustix::process::geteuid().as_raw(),
                &mut crashing,
            );
        }));
        assert!(crashed.is_err());
        assert!(
            root.path()
                .join("var/lib/open-radio/fixture/linux-bluetooth/transaction.json")
                .exists()
        );
        assert!(
            crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_err()
        );

        let recovered = apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .unwrap();
        assert_eq!(recovered.state, InstallState::SoftwareVerified);
        assert_eq!(
            fs::read_link(
                root.path()
                    .join("var/lib/open-radio/fixture/linux-bluetooth/current")
            )
            .unwrap(),
            PathBuf::from("generations").join(expected.generation)
        );
        assert!(
            !root
                .path()
                .join("var/lib/open-radio/fixture/linux-bluetooth/transaction.json")
                .exists()
        );
    }
}

#[test]
fn receipt_failure_after_commit_preserves_truth_and_next_apply_finishes_recovery() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let (bundle, expected) = make_bundle(root.path(), Provider::LinuxBluetooth, "receipt");
    let mut effects = TestEffects::passing();
    let receipts = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth/receipts");
    effects.make_receipt_unwritable = Some(receipts.clone());
    let result = apply(
        &layout,
        Provider::LinuxBluetooth,
        &bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut effects,
    )
    .unwrap();
    assert_eq!(result.state, InstallState::RecoveryRequired);
    assert_eq!(
        fs::read_link(
            root.path()
                .join("var/lib/open-radio/fixture/linux-bluetooth/current")
        )
        .unwrap(),
        PathBuf::from("generations").join(&expected.generation)
    );
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-bluetooth/transaction.json")
            .exists()
    );
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_err());

    fs::set_permissions(&receipts, fs::Permissions::from_mode(0o755)).unwrap();
    let recovered = apply(
        &layout,
        Provider::LinuxBluetooth,
        &bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut TestEffects::passing(),
    )
    .unwrap();
    assert_eq!(recovered.state, InstallState::SoftwareVerified);
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-bluetooth/receipt.json")
            .exists()
    );
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_ok());
}

#[test]
fn source_replacement_symlink_and_partial_installation_are_rejected() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "source");
    let replaced = bundle.join("artifacts/open-radio-bluetooth");
    fs::set_permissions(&replaced, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(replaced, b"replaced").unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );

    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "symlink");
    let original = bundle.join("artifacts/open-radio-bluetooth");
    fs::remove_file(&original).unwrap();
    std::os::unix::fs::symlink("/bin/true", &original).unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );

    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "partial");
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    fs::write(
        root.path().join("usr/local/sbin/open-radio-net"),
        b"unmanaged",
    )
    .unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxNet,
            &bundle,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );
}

#[test]
fn manifest_traversal_operator_mismatch_and_writable_bundle_are_rejected() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (bundle_path, mut bundle) = make_bundle(root.path(), Provider::LinuxBluetooth, "manifest");
    bundle.artifacts[0].target = PathBuf::from("/usr/local/libexec/../escape");
    bundle.generation = bundle_hash(&bundle);
    fs::write(
        bundle_path.join("bundle.json"),
        serde_json::to_vec(&bundle).unwrap(),
    )
    .unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle_path,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );

    let root = tempfile::tempdir().unwrap();
    let (bundle_path, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "operator");
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle_path,
            "another_operator",
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );
    fs::set_permissions(&bundle_path, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle_path,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );
}

#[test]
fn unmanaged_installation_requires_explicit_recovery_without_replacing_files() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let stable = root.path().join("usr/local/libexec/open-radio-bluetooth");
    fs::write(&stable, b"unmanaged helper").unwrap();
    fs::set_permissions(&stable, fs::Permissions::from_mode(0o555)).unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "new");
    let error = apply(
        &layout,
        Provider::LinuxBluetooth,
        &bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut TestEffects::passing(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unmanaged fixture file"));
    assert_eq!(fs::read(stable).unwrap(), b"unmanaged helper");
    assert!(
        !root
            .path()
            .join("var/lib/open-radio/fixture/linux-bluetooth/current")
            .exists()
    );
}

#[test]
fn active_session_lock_refuses_upgrade_and_providers_are_independent() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (net_bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "net");
    let (bt_bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "bt");
    test_apply(
        root.path(),
        Provider::LinuxNet,
        &net_bundle,
        &mut TestEffects::passing(),
    );
    test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &bt_bundle,
        &mut TestEffects::passing(),
    );
    assert!(root.path().join("usr/local/sbin/open-radio-net").exists());
    assert!(
        root.path()
            .join("usr/local/libexec/open-radio-bluetooth")
            .exists()
    );

    let lock_path = root
        .path()
        .join("var/lib/open-radio/fixture/linux-net/session.lock");
    let lock = oer_os_process::lock::FileLock::try_lock(
        File::open(&lock_path).unwrap(),
        &lock_path,
        oer_os_process::lock::Mode::Shared,
    )
    .unwrap()
    .unwrap();
    let (upgrade, _) = make_bundle(root.path(), Provider::LinuxNet, "upgrade");
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    let error = apply(
        &layout,
        Provider::LinuxNet,
        &upgrade,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut TestEffects::passing(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("active HIL session"));
    drop(lock);

    let install_lock_path = root.path().join("var/lib/open-radio/fixture/install.lock");
    let install_lock = oer_os_process::lock::FileLock::try_lock(
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&install_lock_path)
            .unwrap(),
        &install_lock_path,
        oer_os_process::lock::Mode::Exclusive,
    )
    .unwrap()
    .unwrap();
    let error = apply(
        &layout,
        Provider::LinuxNet,
        &upgrade,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut TestEffects::passing(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("another fixture installation"));
    let bluetooth_layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let error = apply(
        &bluetooth_layout,
        Provider::LinuxBluetooth,
        &bt_bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut TestEffects::passing(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("another fixture installation"));
    drop(install_lock);
}

#[test]
fn missing_launcher_is_incomplete_not_an_implicit_upgrade_path() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (first, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "installed");
    test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &first,
        &mut TestEffects::passing(),
    );
    let launcher = root
        .path()
        .join("usr/local/libexec/open-radio-bluetooth-launcher");
    fs::remove_file(&launcher).unwrap();
    let (upgrade, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "new");
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let error = apply(
        &layout,
        Provider::LinuxBluetooth,
        &upgrade,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut TestEffects::passing(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("partial or inconsistent"));
    assert!(!launcher.exists());
}

#[test]
fn writable_or_symlinked_destination_and_writable_active_generation_are_rejected() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "paths");
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    let state = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );
    fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();

    let destination = root.path().join("usr/local/libexec");
    fs::remove_dir(&destination).unwrap();
    std::os::unix::fs::symlink(root.path().join("etc"), &destination).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );

    let root = tempfile::tempdir().unwrap();
    let (bundle, expected) = make_bundle(root.path(), Provider::LinuxBluetooth, "active-mode");
    let result = test_apply(
        root.path(),
        Provider::LinuxBluetooth,
        &bundle,
        &mut TestEffects::passing(),
    );
    assert_eq!(result.state, InstallState::SoftwareVerified);
    let generation = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth/generations")
        .join(expected.generation);
    fs::set_permissions(&generation, fs::Permissions::from_mode(0o777)).unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &bundle,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );
}

#[test]
fn policy_never_grants_installer_runner_or_wildcard_adapter() {
    let _serial = serial();
    let directory = tempfile::tempdir().unwrap();
    let (_, mut bundle) = make_bundle(directory.path(), Provider::LinuxBluetooth, "policy");
    bundle.allowed_bluetooth_adapters = vec!["hci0".into(), "hci2".into()];
    let policy = String::from_utf8(policy_bytes(&bundle).unwrap()).unwrap();
    assert!(policy.contains("--adapter hci0"));
    assert!(policy.contains("--adapter hci2"));
    for adapter in ["hci0", "hci2"] {
        for operation in ["att-parameters", "restore-att-parameters"] {
            assert!(policy.contains(&format!("{operation} --adapter {adapter}\n")));
            assert!(!policy.contains(&format!("{operation} --adapter *")));
        }
        // The runner always names the DTM profile.
        assert!(!policy.contains(&format!("check --adapter {adapter}\n")));
        for version in ["", " --dtm-version v1"] {
            for profile in ["receive-transmit", "transmit", "receive-silence"] {
                assert!(policy.contains(&format!(
                    "check --adapter {adapter}{version} --profile {profile}\n"
                )));
            }
        }
        assert!(policy.contains(&format!(
            "security-failure --adapter {adapter} --peer * --failure missing-key --read-version-before-disconnect\n"
        )));
        assert!(!policy.contains(&format!(
            "security-failure --adapter {adapter} --peer * --failure wrong-key --read-version-before-disconnect"
        )));
        for failure in [
            "missing-key",
            "wrong-key",
            "missing-refresh-key",
            "active-data-mic",
        ] {
            assert!(policy.contains(&format!(
                "security-failure --adapter {adapter} --peer * --failure {failure}\n"
            )));
        }
    }
    assert!(!policy.contains("--failure *"));
    assert!(!policy.contains("--dtm-version *"));
    assert!(!policy.contains("hci*"));
    assert!(!policy.contains("fixture-install"));
    assert!(!policy.contains("cargo"));
    assert!(!policy.contains("hil-runner"));
}

#[test]
fn installed_provider_leases_survive_modeled_run_cleanup() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    for provider in [Provider::LinuxNet, Provider::LinuxBluetooth] {
        let (bundle, _) = make_bundle(root.path(), provider, "persistent-lease");
        let result = test_apply(root.path(), provider, &bundle, &mut TestEffects::passing());
        assert_eq!(result.state, InstallState::SoftwareVerified);
    }

    fs::create_dir_all(root.path().join("run/unrelated-volatile-state")).unwrap();
    fs::write(
        root.path()
            .join("run/unrelated-volatile-state/boot-volatile"),
        b"modeled volatile state",
    )
    .unwrap();
    fs::remove_dir_all(root.path().join("run/unrelated-volatile-state")).unwrap();

    let mut admissions = Vec::new();
    for provider in [Provider::LinuxNet, Provider::LinuxBluetooth] {
        let admission = crate::admission::test_support::admit(root.path(), provider).unwrap();
        let helper_role = match provider {
            Provider::LinuxNet => ArtifactRole::NetworkHelper,
            Provider::LinuxBluetooth => ArtifactRole::BluetoothHelper,
        };
        assert!(admission.artifact(helper_role).unwrap().is_file());
        admissions.push(admission);
    }
    drop(admissions);

    let (upgrade, _) = make_bundle(root.path(), Provider::LinuxNet, "after-run-cleanup");
    let result = test_apply(
        root.path(),
        Provider::LinuxNet,
        &upgrade,
        &mut TestEffects::passing(),
    );
    assert_eq!(result.state, InstallState::SoftwareVerified);
    assert!(!root.path().join("run/unrelated-volatile-state").exists());
}

#[test]
fn admission_rejects_unsafe_persistent_lease_and_parent_metadata() {
    let _serial = serial();
    fn installed_root() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "unsafe");
        test_apply(
            root.path(),
            Provider::LinuxBluetooth,
            &bundle,
            &mut TestEffects::passing(),
        );
        let lock = root
            .path()
            .join("var/lib/open-radio/fixture/linux-bluetooth/session.lock");
        (root, lock)
    }

    let (root, lock) = installed_root();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_err());
    let (upgrade, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "unsafe-upgrade");
    let layout = Layout::test(root.path(), Provider::LinuxBluetooth).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxBluetooth,
            &upgrade,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );
    assert_eq!(
        fs::metadata(&lock).unwrap().permissions().mode() & 0o777,
        0o666
    );

    let (root, lock) = installed_root();
    fs::remove_file(&lock).unwrap();
    std::os::unix::fs::symlink("receipt.json", &lock).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_err());

    let (root, lock) = installed_root();
    fs::remove_file(&lock).unwrap();
    fs::create_dir(&lock).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_err());

    let (root, _) = installed_root();
    let state = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_err());
}

#[test]
fn admission_rejects_pending_or_invalid_committed_state_without_fallback() {
    let _serial = serial();
    fn installed_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let (bundle, _) = make_bundle(root.path(), Provider::LinuxBluetooth, "state");
        test_apply(
            root.path(),
            Provider::LinuxBluetooth,
            &bundle,
            &mut TestEffects::passing(),
        );
        root
    }

    let root = installed_root();
    let state = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth");
    fs::remove_file(state.join("current")).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_err());

    let root = installed_root();
    let state = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth");
    fs::remove_file(state.join("current")).unwrap();
    fs::write(state.join("current"), b"generations/not-a-selector").unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_err());

    let root = installed_root();
    let state = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth");
    std::os::unix::fs::symlink("missing", state.join("transaction.json")).unwrap();
    let error = crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("recovery-required"));

    let root = installed_root();
    let receipt = root
        .path()
        .join("var/lib/open-radio/fixture/linux-bluetooth/receipt.json");
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&receipt, b"{}\n").unwrap();
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o444)).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxBluetooth,).is_err());
}

#[test]
fn policy_never_grants_installer_or_runner() {
    let _serial = serial();
    let directory = tempfile::tempdir().unwrap();
    let (_, bundle) = make_bundle(directory.path(), Provider::LinuxNet, "policy");
    let policy = String::from_utf8(policy_bytes(&bundle).unwrap()).unwrap();
    assert!(!policy.contains("fixture-install"));
    assert!(!policy.contains("cargo"));
    assert!(!policy.contains("hil-runner"));
    assert!(policy.contains("/usr/local/sbin/open-radio-net client-dhcp\n"));
    assert!(!policy.contains("/usr/local/libexec/open-radio-net-client"));
    assert!(!policy.contains("/usr/local/libexec/open-radio-udhcpc"));
}

#[test]
fn station_runtime_and_dhcp_provenance_are_admitted_by_their_generation_bytes() {
    let _serial = serial();
    let target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target")
        .canonicalize()
        .expect("valid station artifact fixture");
    let root = tempfile::tempdir_in(target).expect("valid station artifact fixture");
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "station");
    test_apply(
        root.path(),
        Provider::LinuxNet,
        &bundle,
        &mut TestEffects::passing(),
    );
    let lease = crate::admission::test_support::admit(root.path(), Provider::LinuxNet)
        .expect("valid station artifact fixture");
    for role in [
        ArtifactRole::NetworkClient,
        ArtifactRole::Udhcpc,
        ArtifactRole::UdhcpcProvenance,
    ] {
        let path = lease
            .artifact(role)
            .expect("valid station artifact fixture");
        let original = fs::read(&path).expect("valid station artifact fixture");
        let mode = fs::metadata(&path)
            .expect("valid station artifact fixture")
            .permissions();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("valid station artifact fixture");
        fs::write(&path, b"changed artifact").expect("valid station artifact fixture");
        fs::set_permissions(&path, mode.clone()).expect("valid station artifact fixture");
        assert!(lease.artifact(role).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("valid station artifact fixture");
        fs::write(&path, original).expect("valid station artifact fixture");
        fs::set_permissions(&path, mode).expect("valid station artifact fixture");
        assert!(lease.artifact(role).is_ok());
    }
}
