//! Real nested launches with an isolated arbiter and no board I/O.

use std::{
    fs,
    io::Write as _,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use oer_stand_arbiter::{Arbiter, Request};
use oer_stand_claims::Claim;

use crate::launch::Runner;

const ROLE: &str = "OER_EXPERIMENT_LEASE_TEST_ROLE";
const DIRECTORY: &str = "OER_EXPERIMENT_LEASE_TEST_DIRECTORY";
const MAC: &str = "00:11:22:33:44:66";

fn child(test: &str, role: &str) -> Command {
    let mut command = oer_process::command(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--test-threads=1"])
        .env(ROLE, role);
    command
}

/// Re-exec the test below a live grant, then let its actual launcher start a
/// fake runner that must join the parent and admit delegated device I/O.
pub fn within_enclosing_lease(test: &str, experiment: impl FnOnce(&Path, Runner)) {
    match std::env::var(ROLE).ok().as_deref() {
        None => {
            let directory = tempfile::tempdir().unwrap();
            let stand = directory.path().join("stand.toml");
            fs::write(
                &stand,
                "schema = 1\n[stand]\nid = 'test'\nair = 'exclusive'\n\
                [[board]]\nid = 'dut'\nusb-serial = '00:11:22:33:44:66'\nchip = 'esp32c5'\n\
                radios = ['ble']\nroles = ['dut']\nreset = ['jtag']\n",
            )
            .unwrap();
            fs::set_permissions(&stand, fs::Permissions::from_mode(0o600)).unwrap();
            let mut owner = child(test, "owner");
            owner
                .env(DIRECTORY, directory.path())
                .env("XDG_RUNTIME_DIR", directory.path())
                .env("XDG_CACHE_HOME", directory.path().join("cache"))
                .env("XDG_DATA_HOME", directory.path().join("data"))
                .env(oer_stand_file::paths::STAND_FILE_ENV, stand)
                .env(
                    oer_stand_file::paths::ARBITER_ENV,
                    directory.path().join("arbiter"),
                );
            oer_process::run_with_timeout(&mut owner, Duration::from_secs(15)).unwrap();
        }
        Some("owner") => {
            let grant = Arbiter::open()
                .unwrap()
                .acquire(&request("lease-parent"))
                .unwrap();
            let mut nested = child(test, "experiment");
            grant
                .context()
                .unwrap()
                .with(oer_stand_arbiter::jobs::JOB_KEY, "parent-job")
                .apply(&mut nested)
                .unwrap();
            oer_process::run_with_timeout(&mut nested, Duration::from_secs(10)).unwrap();
        }
        Some("experiment") => {
            let directory = PathBuf::from(std::env::var_os(DIRECTORY).unwrap());
            let executable = directory.join("runner");
            fs::write(&executable, format!(
                "#!/bin/sh\nexport {ROLE}=runner\nexec \"{}\" --exact {test} --test-threads=1\n",
                std::env::current_exe().unwrap().display(),
            )).unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            experiment(
                &directory,
                Runner {
                    executable,
                    receipt: None,
                },
            );
        }
        Some("runner") => {
            let context = oer_process::Context::current().unwrap();
            assert!(context.get(oer_stand_arbiter::LEASE_KEY).is_some());
            assert_eq!(
                context.get(oer_stand_owners::OWNER_KEY),
                Some("experiment-test")
            );
            assert!(context.get(oer_stand_arbiter::jobs::JOB_KEY).is_some());
            assert_ne!(
                context.get(oer_stand_arbiter::jobs::JOB_KEY),
                Some("parent-job")
            );
            assert_eq!(
                oer_stand_owners::from_environment().unwrap().as_str(),
                "experiment-test"
            );
            let grant = Arbiter::open()
                .unwrap()
                .acquire(&request("experiment-test"))
                .unwrap();
            assert!(
                grant.is_nested(),
                "runner queued behind its enclosing lease"
            );
            let access = grant.device(&MAC.parse().unwrap()).unwrap();
            assert!(access.is_delegated());
            let _operation = access.operation().unwrap();
            let receipt = std::env::var_os(oer_hil_run_bundle::receipt::ENV).unwrap();
            writeln!(
                fs::OpenOptions::new().append(true).open(receipt).unwrap(),
                "7"
            )
            .unwrap();
        }
        other => panic!("unknown fixture role {other:?}"),
    }
}

fn request(owner: &str) -> Request {
    Request {
        owner: owner.into(),
        work: "nested experiment".into(),
        scenarios: Vec::new(),
        run: None,
        claims: vec![Claim::stand()],
    }
}

pub fn repository(directory: &Path) -> PathBuf {
    let root = directory.join("repository");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        oer_process::git::run(&root, args).unwrap();
    }
    root
}
