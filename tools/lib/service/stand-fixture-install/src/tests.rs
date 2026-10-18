use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest as _, Sha256};

use super::*;
use crate::transaction::test_support::{
    Effects, LINUX_NET_OPERATIONS, Layout, Stage, apply, policy_bytes,
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
    let entries: Vec<(ArtifactRole, &str, &str, u32, Vec<u8>)> = match provider {
        Provider::LinuxNet => vec![
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

fn make_network_race_bundle(directory: &Path, suffix: &str, effect: &Path) -> (PathBuf, Bundle) {
    let (bundle_path, mut bundle) = make_bundle(directory, Provider::LinuxNet, suffix);
    let helper = bundle
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.role == ArtifactRole::NetworkHelper)
        .unwrap();
    let script = format!(
        "#!/bin/sh\nif test \"$1\" = capabilities; then printf '%s\\n' '{}'; exit 0; fi\nprintf '{suffix}/' >'{}'\nGENERATION_DIR=$(/usr/bin/dirname -- \"$0\")\n/bin/cat \"$GENERATION_DIR/open-radio-hostapd\" >>'{}'\n# generation {suffix}\n",
        Provider::LinuxNet.runtime_contract(),
        effect.display(),
        effect.display(),
    );
    let helper_path = bundle_path.join("artifacts").join(&helper.file_name);
    fs::set_permissions(&helper_path, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(&helper_path, script.as_bytes()).unwrap();
    fs::set_permissions(&helper_path, fs::Permissions::from_mode(helper.mode)).unwrap();
    helper.size_bytes = script.len() as u64;
    helper.sha256 = format!("{:x}", Sha256::digest(script.as_bytes()));
    bundle.generation = bundle_hash(&bundle);
    fs::write(
        bundle_path.join("bundle.json"),
        serde_json::to_vec_pretty(&bundle).unwrap(),
    )
    .unwrap();
    (bundle_path, bundle)
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
fn plans_are_deterministic_and_offline() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let net = build_plan(root.path(), Provider::LinuxNet).unwrap();
    assert_eq!(net, build_plan(root.path(), Provider::LinuxNet).unwrap());
    assert_eq!(net.provider, Provider::LinuxNet);
    assert!(net.build_steps.iter().any(|step| step.contains("hostapd")));
    assert!(
        net.artifacts
            .iter()
            .all(|item| item.state == ArtifactState::Missing)
    );
    assert!(!net.automatic_hardware_checks);
}

#[test]
fn fresh_repeat_and_upgrade_use_complete_generations() {
    let _serial = serial();
    use std::os::unix::fs::MetadataExt as _;

    let root = tempfile::tempdir().unwrap();
    let (first_path, first) = make_bundle(root.path(), Provider::LinuxNet, "first");
    let result = test_apply(
        root.path(),
        Provider::LinuxNet,
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
        .join("var/lib/open-radio/fixture/linux-net/session.lock");
    let lease_inode = fs::metadata(&lease_path).unwrap().ino();
    let stable = root.path().join("usr/local/sbin/open-radio-net");
    assert_eq!(
        fs::read(&stable).unwrap(),
        artifact_declarations(Provider::LinuxNet, "first")[1].1
    );

    let repeat = test_apply(
        root.path(),
        Provider::LinuxNet,
        &first_path,
        &mut TestEffects::passing(),
    );
    assert_eq!(repeat.state, InstallState::SoftwareVerified);
    assert!(!repeat.changed);
    assert_eq!(fs::metadata(&lease_path).unwrap().ino(), lease_inode);

    let (second_path, second) = make_bundle(root.path(), Provider::LinuxNet, "second");
    let upgrade = test_apply(
        root.path(),
        Provider::LinuxNet,
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
        artifact_declarations(Provider::LinuxNet, "second")[1].1
    );
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-net/generations")
            .join(first.generation)
            .exists()
    );
}

#[test]
fn post_install_dispatch_contains_only_policy_bytes_and_capabilities_checks() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "software-only");
    let mut effects = TestEffects::passing();
    let result = test_apply(root.path(), Provider::LinuxNet, &bundle, &mut effects);
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
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "policy-conflict");
    let mut effects = EffectiveReject { effective_calls: 0 };
    let result = apply(
        &layout,
        Provider::LinuxNet,
        &bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut effects,
    )
    .unwrap();
    assert_eq!(result.state, InstallState::RolledBack);
    assert!(result.primary_error.unwrap().contains("effective policy"));
    assert_eq!(effects.effective_calls, 2);
    assert!(!root.path().join("etc/sudoers.d/open-radio-net").exists());
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
        let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "failure");
        let mut effects = TestEffects::passing();
        effects.fail = Some(stage);
        let result = test_apply(root.path(), Provider::LinuxNet, &bundle, &mut effects);
        assert_eq!(result.state, InstallState::RolledBack, "stage {stage:?}");
        assert!(
            !root.path().join("usr/local/sbin/open-radio-net").exists(),
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
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "rollback");
    let result = apply(
        &layout,
        Provider::LinuxNet,
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
            .join("var/lib/open-radio/fixture/linux-net/transaction.json")
            .exists()
    );
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_err());
}

#[test]
fn abrupt_process_loss_is_recovered_from_persisted_journal() {
    let _serial = serial();
    for stage in [Stage::PolicyPublished, Stage::Activated] {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
        let (bundle, expected) = make_bundle(root.path(), Provider::LinuxNet, "crash");
        let mut crashing = TestEffects::passing();
        crashing.panic = Some(stage);
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = apply(
                &layout,
                Provider::LinuxNet,
                &bundle,
                OPERATOR,
                rustix::process::geteuid().as_raw(),
                &mut crashing,
            );
        }));
        assert!(crashed.is_err());
        assert!(
            root.path()
                .join("var/lib/open-radio/fixture/linux-net/transaction.json")
                .exists()
        );
        assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_err());

        let recovered = apply(
            &layout,
            Provider::LinuxNet,
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
                    .join("var/lib/open-radio/fixture/linux-net/current")
            )
            .unwrap(),
            PathBuf::from("generations").join(expected.generation)
        );
        assert!(
            !root
                .path()
                .join("var/lib/open-radio/fixture/linux-net/transaction.json")
                .exists()
        );
    }
}

#[test]
fn receipt_failure_after_commit_preserves_truth_and_next_apply_finishes_recovery() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    let (bundle, expected) = make_bundle(root.path(), Provider::LinuxNet, "receipt");
    let mut effects = TestEffects::passing();
    let receipts = root
        .path()
        .join("var/lib/open-radio/fixture/linux-net/receipts");
    effects.make_receipt_unwritable = Some(receipts.clone());
    let result = apply(
        &layout,
        Provider::LinuxNet,
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
                .join("var/lib/open-radio/fixture/linux-net/current")
        )
        .unwrap(),
        PathBuf::from("generations").join(&expected.generation)
    );
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-net/transaction.json")
            .exists()
    );
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_err());

    fs::set_permissions(&receipts, fs::Permissions::from_mode(0o755)).unwrap();
    let recovered = apply(
        &layout,
        Provider::LinuxNet,
        &bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut TestEffects::passing(),
    )
    .unwrap();
    assert_eq!(recovered.state, InstallState::SoftwareVerified);
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-net/receipt.json")
            .exists()
    );
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_ok());
}

#[test]
fn source_replacement_symlink_and_partial_installation_are_rejected() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "source");
    let replaced = bundle.join("artifacts/open-radio-net");
    fs::set_permissions(&replaced, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(replaced, b"replaced").unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
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

    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "symlink");
    let original = bundle.join("artifacts/open-radio-net");
    fs::remove_file(&original).unwrap();
    std::os::unix::fs::symlink("/bin/true", &original).unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
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
    let (bundle_path, mut bundle) = make_bundle(root.path(), Provider::LinuxNet, "manifest");
    bundle.artifacts[0].target = PathBuf::from("/usr/local/libexec/../escape");
    bundle.generation = bundle_hash(&bundle);
    fs::write(
        bundle_path.join("bundle.json"),
        serde_json::to_vec(&bundle).unwrap(),
    )
    .unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxNet,
            &bundle_path,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut TestEffects::passing(),
        )
        .is_err()
    );

    let root = tempfile::tempdir().unwrap();
    let (bundle_path, _) = make_bundle(root.path(), Provider::LinuxNet, "operator");
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxNet,
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
            Provider::LinuxNet,
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
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    let stable = root.path().join("usr/local/sbin/open-radio-net");
    fs::write(&stable, b"unmanaged helper").unwrap();
    fs::set_permissions(&stable, fs::Permissions::from_mode(0o555)).unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "new");
    let error = apply(
        &layout,
        Provider::LinuxNet,
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
            .join("var/lib/open-radio/fixture/linux-net/current")
            .exists()
    );
}

#[test]
fn active_session_and_installation_locks_refuse_an_upgrade() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (net_bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "net");
    test_apply(
        root.path(),
        Provider::LinuxNet,
        &net_bundle,
        &mut TestEffects::passing(),
    );
    assert!(root.path().join("usr/local/sbin/open-radio-net").exists());

    let lock_path = root
        .path()
        .join("var/lib/open-radio/fixture/linux-net/session.lock");
    let lock = oer_process::lock::FileLock::try_lock(
        File::open(&lock_path).unwrap(),
        &lock_path,
        oer_process::lock::Mode::Shared,
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
    let install_lock = oer_process::lock::FileLock::try_lock(
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&install_lock_path)
            .unwrap(),
        &install_lock_path,
        oer_process::lock::Mode::Exclusive,
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
    drop(install_lock);
}

#[test]
fn missing_launcher_is_incomplete_not_an_implicit_upgrade_path() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (first, _) = make_bundle(root.path(), Provider::LinuxNet, "installed");
    test_apply(
        root.path(),
        Provider::LinuxNet,
        &first,
        &mut TestEffects::passing(),
    );
    let launcher = root
        .path()
        .join("usr/local/libexec/open-radio-net-launcher");
    fs::remove_file(&launcher).unwrap();
    let (upgrade, _) = make_bundle(root.path(), Provider::LinuxNet, "new");
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    let error = apply(
        &layout,
        Provider::LinuxNet,
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
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "paths");
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    let state = root.path().join("var/lib/open-radio/fixture/linux-net");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o777)).unwrap();
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
    fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();

    let destination = root.path().join("usr/local/libexec");
    fs::remove_dir(&destination).unwrap();
    std::os::unix::fs::symlink(root.path().join("etc"), &destination).unwrap();
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

    let root = tempfile::tempdir().unwrap();
    let (bundle, expected) = make_bundle(root.path(), Provider::LinuxNet, "active-mode");
    let result = test_apply(
        root.path(),
        Provider::LinuxNet,
        &bundle,
        &mut TestEffects::passing(),
    );
    assert_eq!(result.state, InstallState::SoftwareVerified);
    let generation = root
        .path()
        .join("var/lib/open-radio/fixture/linux-net/generations")
        .join(expected.generation);
    fs::set_permissions(&generation, fs::Permissions::from_mode(0o777)).unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
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
fn policy_never_grants_installer_or_runner() {
    let _serial = serial();
    let directory = tempfile::tempdir().unwrap();
    let (_, bundle) = make_bundle(directory.path(), Provider::LinuxNet, "policy");
    let policy = String::from_utf8(policy_bytes(&bundle).unwrap()).unwrap();
    assert!(!policy.contains("fixture-install"));
    assert!(!policy.contains("cargo"));
    assert!(!policy.contains("hil-runner"));
}

#[test]
fn genuine_visudo_parses_generated_policy_when_available() {
    let _serial = serial();
    let Some(visudo) = ["/usr/sbin/visudo", "/usr/bin/visudo"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
    else {
        eprintln!("visudo unavailable: genuine parser check skipped");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let (_, mut bundle) = make_bundle(directory.path(), Provider::LinuxNet, "visudo");
    bundle.operator = String::from_utf8(
        Command::new("/usr/bin/id")
            .arg("-un")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    let main = directory.path().join("sudoers.main");
    fs::write(&main, b"Defaults env_reset\n").unwrap();
    let candidate = directory.path().join("sudoers.candidate");
    fs::write(&candidate, policy_bytes(&bundle).unwrap()).unwrap();
    let policy_path = directory.path().join("sudoers.composite");
    fs::write(
        &policy_path,
        format!(
            "@include {}\n@include {}\n",
            main.display(),
            candidate.display()
        ),
    )
    .unwrap();
    let output = Command::new(&visudo)
        .args(["-c", "-f"])
        .arg(&policy_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::write(&candidate, b"this is not sudoers syntax {\n").unwrap();
    let invalid = Command::new(&visudo)
        .args(["-c", "-f"])
        .arg(&policy_path)
        .output()
        .unwrap();
    assert!(!invalid.status.success());
}

fn initialize_test_repository(root: &Path) {
    fs::write(root.join(".gitignore"), "/target\n").unwrap();
    fs::write(root.join("tracked.txt"), "clean\n").unwrap();
    for args in [
        &["init", "-q"][..],
        &["add", ".gitignore", "tracked.txt"][..],
        &[
            "-c",
            "user.name=Fixture Test",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
        ][..],
    ] {
        assert!(
            Command::new("git")
                .current_dir(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
}

/// Write every linux-net artifact `prepare` imports, with hostapd provenance
/// naming the written hostapd; returns the network helper.
fn write_network_prepare_input(root: &Path, suffix: &str) -> PathBuf {
    let network = root.join("hil/host/linux-net/open-radio-net");
    fs::create_dir_all(network.parent().unwrap()).unwrap();
    executable(
        &network,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' '{}'\n# {suffix}\n",
            Provider::LinuxNet.runtime_contract()
        ),
    );
    fs::create_dir_all(root.join("target/debug")).unwrap();
    for launcher in ["open-radio-net-launcher", "open-radio-probe-launcher"] {
        executable(
            &root.join("target/debug").join(launcher),
            "#!/bin/sh\nexit 1\n",
        );
    }
    executable(
        &root.join("target/debug/open-radio-probe"),
        "#!/bin/sh\nexit 0\n",
    );
    let hostapd = root.join("target/hil/hostapd/hostapd");
    fs::create_dir_all(hostapd.parent().unwrap()).unwrap();
    executable(&hostapd, "hostapd bytes");
    let hostapd_sha256 = format!("{:x}", Sha256::digest(fs::read(&hostapd).unwrap()));
    fs::write(
        root.join("target/hil/hostapd/provenance.json"),
        serde_json::json!({"binary_sha256": hostapd_sha256}).to_string(),
    )
    .unwrap();
    network
}

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn preparation_captures_locked_artifacts_and_dirty_source_identity() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    initialize_test_repository(root.path());
    write_network_prepare_input(root.path(), "prepared");
    fs::write(root.path().join("tracked.txt"), "dirty\n").unwrap();

    let directory = prepare(root.path(), Provider::LinuxNet, OPERATOR).unwrap();
    let bundle: Bundle =
        serde_json::from_slice(&fs::read(directory.join("bundle.json")).unwrap()).unwrap();
    assert!(bundle.source.dirty);
    assert_eq!(bundle.artifacts.len(), 6);
    assert!(
        bundle
            .artifacts
            .iter()
            .any(|artifact| artifact.role == ArtifactRole::HostapdProvenance)
    );
}

#[test]
fn preparation_rejects_missing_stale_or_modified_artifacts() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    initialize_test_repository(root.path());
    assert!(prepare(root.path(), Provider::LinuxNet, OPERATOR).is_err());

    let helper = write_network_prepare_input(root.path(), "stale");
    executable(&helper, "#!/bin/sh\necho schema=1\n");
    assert!(
        prepare(root.path(), Provider::LinuxNet, OPERATOR)
            .unwrap_err()
            .to_string()
            .contains("incompatible runtime contract")
    );

    write_network_prepare_input(root.path(), "valid");
    let bundle = prepare(root.path(), Provider::LinuxNet, OPERATOR).unwrap();
    let installed = bundle.join("artifacts/open-radio-net");
    fs::set_permissions(&installed, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(&installed, b"corrupt cached bundle").unwrap();
    assert!(
        prepare(root.path(), Provider::LinuxNet, OPERATOR)
            .unwrap_err()
            .to_string()
            .contains("existing bundle artifact mismatch")
    );
}

#[test]
fn network_preparation_rejects_hostapd_provenance_mismatch() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    initialize_test_repository(root.path());
    write_network_prepare_input(root.path(), "provenance");
    fs::write(
        root.path().join("target/hil/hostapd/provenance.json"),
        serde_json::json!({"binary_sha256": "0".repeat(64)}).to_string(),
    )
    .unwrap();
    assert!(
        prepare(root.path(), Provider::LinuxNet, OPERATOR)
            .unwrap_err()
            .to_string()
            .contains("provenance")
    );
}

#[test]
fn installed_provider_leases_survive_modeled_run_cleanup() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "persistent-lease");
    let result = test_apply(
        root.path(),
        Provider::LinuxNet,
        &bundle,
        &mut TestEffects::passing(),
    );
    assert_eq!(result.state, InstallState::SoftwareVerified);

    fs::create_dir_all(root.path().join("run/unrelated-volatile-state")).unwrap();
    fs::write(
        root.path()
            .join("run/unrelated-volatile-state/boot-volatile"),
        b"modeled volatile state",
    )
    .unwrap();
    fs::remove_dir_all(root.path().join("run/unrelated-volatile-state")).unwrap();

    let admission = crate::admission::test_support::admit(root.path(), Provider::LinuxNet).unwrap();
    assert!(
        admission
            .artifact(ArtifactRole::NetworkHelper)
            .unwrap()
            .is_file()
    );
    drop(admission);

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
fn persistent_operational_lease_blocks_upgrade_until_its_owner_releases() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let effect = root.path().join("runner-effect");
    let (first, _) = make_network_race_bundle(root.path(), "held-A", &effect);
    test_apply(
        root.path(),
        Provider::LinuxNet,
        &first,
        &mut TestEffects::passing(),
    );
    let lease = crate::admission::test_support::admit(root.path(), Provider::LinuxNet).unwrap();
    assert!(
        fs::read_to_string(lease.artifact(ArtifactRole::NetworkHelper).unwrap())
            .unwrap()
            .contains("held-A")
    );
    let nested = crate::launcher::test_support::run_at(
        root.path(),
        crate::launcher::LaunchTarget::Network,
        &["identity"],
    )
    .unwrap();
    assert!(nested.success());
    assert_eq!(
        fs::read_to_string(&effect).unwrap(),
        "held-A/hostapd-held-A"
    );
    let (upgrade, expected) = make_bundle(root.path(), Provider::LinuxNet, "held-B");
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
    drop(lease);
    let result = apply(
        &layout,
        Provider::LinuxNet,
        &upgrade,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut TestEffects::passing(),
    )
    .unwrap();
    assert_eq!(result.generation, expected.generation);
}

#[test]
fn admission_rejects_unsafe_persistent_lease_and_parent_metadata() {
    let _serial = serial();
    fn installed_root() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "unsafe");
        test_apply(
            root.path(),
            Provider::LinuxNet,
            &bundle,
            &mut TestEffects::passing(),
        );
        let lock = root
            .path()
            .join("var/lib/open-radio/fixture/linux-net/session.lock");
        (root, lock)
    }

    let (root, lock) = installed_root();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_err());
    let (upgrade, _) = make_bundle(root.path(), Provider::LinuxNet, "unsafe-upgrade");
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    assert!(
        apply(
            &layout,
            Provider::LinuxNet,
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
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_err());

    let (root, lock) = installed_root();
    fs::remove_file(&lock).unwrap();
    fs::create_dir(&lock).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_err());

    let (root, _) = installed_root();
    let state = root.path().join("var/lib/open-radio/fixture/linux-net");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_err());
}

#[test]
fn admission_rejects_pending_or_invalid_committed_state_without_fallback() {
    let _serial = serial();
    fn installed_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "state");
        test_apply(
            root.path(),
            Provider::LinuxNet,
            &bundle,
            &mut TestEffects::passing(),
        );
        root
    }

    let root = installed_root();
    let state = root.path().join("var/lib/open-radio/fixture/linux-net");
    fs::remove_file(state.join("current")).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_err());

    let root = installed_root();
    let state = root.path().join("var/lib/open-radio/fixture/linux-net");
    fs::remove_file(state.join("current")).unwrap();
    fs::write(state.join("current"), b"generations/not-a-selector").unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_err());

    let root = installed_root();
    let state = root.path().join("var/lib/open-radio/fixture/linux-net");
    std::os::unix::fs::symlink("missing", state.join("transaction.json")).unwrap();
    let error = crate::admission::test_support::admit(root.path(), Provider::LinuxNet)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("recovery-required"));

    let root = installed_root();
    let receipt = root
        .path()
        .join("var/lib/open-radio/fixture/linux-net/receipt.json");
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&receipt, b"{}\n").unwrap();
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o444)).unwrap();
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_err());
}

#[test]
fn lifecycle_isolated_child_entry() {
    use std::io::{Read as _, Write as _};

    let _serial = serial();

    let Ok(mode) = std::env::var("OPEN_RADIO_LIFECYCLE_CHILD") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("OPEN_RADIO_TEST_ROOT").unwrap());
    match mode.as_str() {
        "interrupt" => {
            struct ExitAtPolicyPublished;
            impl Effects for ExitAtPolicyPublished {
                fn checkpoint(&mut self, stage: Stage) -> crate::Result<()> {
                    if stage == Stage::PolicyPublished {
                        std::process::exit(86);
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
            let layout = Layout::test(&root, Provider::LinuxNet).unwrap();
            let bundle = PathBuf::from(std::env::var_os("OPEN_RADIO_TEST_BUNDLE").unwrap());
            let _ = apply(
                &layout,
                Provider::LinuxNet,
                &bundle,
                OPERATOR,
                rustix::process::geteuid().as_raw(),
                &mut ExitAtPolicyPublished,
            );
            std::process::exit(87);
        }
        "launch" => {
            let (stable, target, arguments): (PathBuf, _, &[&str]) = (
                root.join("usr/local/sbin/open-radio-net"),
                crate::launcher::LaunchTarget::Network,
                &["identity"],
            );
            let mut loaded_helper = File::open(stable).unwrap();
            let mut loaded = String::new();
            loaded_helper.read_to_string(&mut loaded).unwrap();
            assert!(loaded.contains("generation A"));
            fs::write(root.join("loaded"), b"A").unwrap();
            let ready = root.join("ready.fifo");
            let resume = root.join("resume.fifo");
            let mut ready_writer = OpenOptions::new().write(true).open(ready).unwrap();
            ready_writer.write_all(b"ready").unwrap();
            drop(ready_writer);
            let mut resume_reader = File::open(resume).unwrap();
            let mut token = String::new();
            resume_reader.read_to_string(&mut token).unwrap();
            assert_eq!(token, "continue\n");
            let status = crate::launcher::test_support::run_at(&root, target, arguments).unwrap();
            std::process::exit(if status.success() { 0 } else { 88 });
        }
        _ => panic!("unknown lifecycle child mode"),
    }
}

#[test]
fn process_loss_leaves_admission_closed_until_authorized_recovery() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let (bundle, _) = make_bundle(root.path(), Provider::LinuxNet, "process-loss");
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::lifecycle_isolated_child_entry",
            "--nocapture",
        ])
        .env("OPEN_RADIO_LIFECYCLE_CHILD", "interrupt")
        .env("OPEN_RADIO_TEST_ROOT", root.path())
        .env("OPEN_RADIO_TEST_BUNDLE", &bundle)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(86));
    let external_effect = root.path().join("hardware-effect");
    let admission = crate::admission::test_support::admit(root.path(), Provider::LinuxNet);
    if admission.is_ok() {
        fs::write(&external_effect, b"called").unwrap();
    }
    let error = admission.err().unwrap().to_string();
    assert!(error.contains("recovery-required"));
    assert!(!external_effect.exists());

    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    let recovered = apply(
        &layout,
        Provider::LinuxNet,
        &bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut TestEffects::passing(),
    )
    .unwrap();
    assert_eq!(recovered.state, InstallState::SoftwareVerified);
    assert!(crate::admission::test_support::admit(root.path(), Provider::LinuxNet,).is_ok());
}

#[test]
fn interrupted_installation_rejects_a_new_operational_consumer() {
    let _serial = serial();
    let root = tempfile::tempdir().unwrap();
    let layout = Layout::test(root.path(), Provider::LinuxNet).unwrap();
    let external_effect = root.path().join("hardware-effect");
    let (bundle, _) = make_network_race_bundle(root.path(), "interrupted", &external_effect);
    let mut crashing = TestEffects::passing();
    crashing.panic = Some(Stage::PolicyPublished);
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = apply(
            &layout,
            Provider::LinuxNet,
            &bundle,
            OPERATOR,
            rustix::process::geteuid().as_raw(),
            &mut crashing,
        );
    }));
    assert!(crashed.is_err());
    assert!(
        root.path()
            .join("var/lib/open-radio/fixture/linux-net/transaction.json")
            .exists()
    );

    let launch = crate::launcher::test_support::run_at(
        root.path(),
        crate::launcher::LaunchTarget::Network,
        &["identity"],
    );
    assert!(
        !external_effect.exists(),
        "the current production admission admits an operation while recovery is required"
    );
    assert!(
        launch
            .err()
            .unwrap()
            .to_string()
            .contains("recovery-required")
    );

    let recovered = apply(
        &layout,
        Provider::LinuxNet,
        &bundle,
        OPERATOR,
        rustix::process::geteuid().as_raw(),
        &mut TestEffects::passing(),
    )
    .unwrap();
    assert_eq!(recovered.state, InstallState::SoftwareVerified);
    let launched = crate::launcher::test_support::run_at(
        root.path(),
        crate::launcher::LaunchTarget::Network,
        &["identity"],
    )
    .unwrap();
    assert!(launched.success());
    assert_eq!(
        fs::read_to_string(external_effect).unwrap(),
        "interrupted/hostapd-interrupted"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn direct_stable_launch_cannot_mix_loaded_and_selected_generations() {
    let _serial = serial();
    use std::io::{Read as _, Write as _};

    {
        let provider = Provider::LinuxNet;
        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready.fifo");
        let resume = root.path().join("resume.fifo");
        for fifo in [&ready, &resume] {
            rustix::fs::mknodat(
                rustix::fs::CWD,
                fifo.as_path(),
                rustix::fs::FileType::Fifo,
                rustix::fs::Mode::from_raw_mode(0o600),
                0,
            )
            .unwrap();
        }
        let effect = root.path().join("effect");

        let (first_path, _) = make_network_race_bundle(root.path(), "A", &effect);
        test_apply(
            root.path(),
            provider,
            &first_path,
            &mut TestEffects::passing(),
        );

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::lifecycle_isolated_child_entry",
                "--nocapture",
            ])
            .env("OPEN_RADIO_LIFECYCLE_CHILD", "launch")
            .env("OPEN_RADIO_TEST_ROOT", root.path())
            .spawn()
            .unwrap();
        let mut ready_reader = File::open(&ready).unwrap();
        let mut signal = String::new();
        ready_reader.read_to_string(&mut signal).unwrap();
        assert_eq!(signal, "ready");
        assert_eq!(fs::read_to_string(root.path().join("loaded")).unwrap(), "A");

        let (second_path, _) = make_network_race_bundle(root.path(), "B", &effect);
        let upgraded = test_apply(
            root.path(),
            provider,
            &second_path,
            &mut TestEffects::passing(),
        );
        assert_eq!(upgraded.state, InstallState::SoftwareVerified);

        let mut resume_writer = OpenOptions::new().write(true).open(&resume).unwrap();
        resume_writer.write_all(b"continue\n").unwrap();
        drop(resume_writer);
        assert!(child.wait().unwrap().success());

        let launched = fs::read_to_string(&effect).unwrap();
        assert_eq!(launched, "B/hostapd-B");
        assert_ne!(launched, "A/hostapd-B");
    }
}

#[test]
fn the_network_policy_grants_exactly_the_helper_s_operations() {
    let _serial = serial();
    let directory = tempfile::tempdir().unwrap();
    let (_, bundle) = make_bundle(directory.path(), Provider::LinuxNet, "operations");
    let policy = String::from_utf8(policy_bytes(&bundle).unwrap()).unwrap();
    // Every `case` of the helper's dispatch is one operation; the runner
    // calls each through non-interactive sudo, so a new one the policy
    // misses stops every scenario that needs it.
    let helper = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../hil/host/linux-net/open-radio-net"),
    )
    .unwrap();
    let dispatch = helper.split("case \"$1\" in\n").nth(1).unwrap();
    let mut operations: Vec<&str> = dispatch
        .lines()
        .filter_map(|line| line.strip_prefix("    ")?.strip_suffix(')'))
        .filter(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .collect();
    operations.sort_unstable();
    let mut allowed = LINUX_NET_OPERATIONS.to_vec();
    allowed.sort_unstable();
    assert_eq!(operations, allowed);
    for operation in LINUX_NET_OPERATIONS {
        assert!(policy.contains(&format!(
            "NOPASSWD: /usr/local/sbin/open-radio-net {operation}\n"
        )));
    }
}
