//! The checks re-exec is the same admitted CLI operation through Cargo.
#![cfg(not(feature = "checks"))]

use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf};

const TEST: &str = "checks_reexec_preserves_the_operation_context";
const DIRECTORY: &str = "OER_FW_CHECKS_TEST_DIRECTORY";

#[test]
fn checks_reexec_preserves_the_operation_context() {
    if std::env::var_os("OER_FW_CHECKS_TEST_CHILD").is_some() {
        let context = oer_process::Context::current().unwrap();
        assert_eq!(context.get("stand.lease"), Some("enclosing-lease"));
        assert_eq!(context.get("stand.job"), Some("same-job"));
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let context = oer_process::Context::default()
        .with("stand.lease", "enclosing-lease")
        .with("stand.job", "same-job");
    let cargo = directory.path().join("cargo");
    fs::write(
        &cargo,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"${DIRECTORY}/arguments\"\n\
         exec \"{}\" --exact {TEST}\n",
            std::env::current_exe().unwrap().display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700)).unwrap();
    let mut fw = oer_process::command(env!("CARGO_BIN_EXE_oer-fw"));
    fw.args(["compare", "old.elf", "new.elf"])
        .env("CARGO", &cargo)
        .env(DIRECTORY, directory.path())
        .env("OER_FW_CHECKS_TEST_CHILD", "1");
    context.apply(&mut fw).unwrap();
    oer_process::run_with_timeout(&mut fw, std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(
        fs::read_to_string(PathBuf::from(directory.path()).join("arguments")).unwrap(),
        "run\n--quiet\n-p\noer-fw\n--features\nchecks\n--\ncompare\nold.elf\nnew.elf\n"
    );
}
