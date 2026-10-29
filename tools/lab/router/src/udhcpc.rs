//! Build only the pinned BusyBox udhcpc applet, without root or radio access.
use oer_os_process as process;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use crate::Result;

// SOURCE: Buildroot's upstream-derived busybox.hash, BusyBox 1.38.0.
// https://raw.githubusercontent.com/buildroot/buildroot/master/package/busybox/busybox.hash
const SHA256: &str = "34f9ea6ff8636f2c9241153b9114eefa9e65674a45318ae1ef95bb5f31c53bb2";
const URL: &str = "https://sources.buildroot.net/busybox/busybox-1.38.0.tar.bz2";

fn command(root: &Path, program: &str) -> Command {
    let mut c = process::command(program);
    c.current_dir(root);
    c
}

pub fn verify(bytes: &[u8]) -> Result<()> {
    if oer_os_durable::sha256_bytes(bytes) != SHA256 {
        return Err("BusyBox archive SHA-256 mismatch".into());
    }
    Ok(())
}

pub fn build(root: &Path) -> Result<()> {
    let out = root.join("target/hil/udhcpc");
    fs::create_dir_all(&out)?;
    let _lock =
        process::lock::FileLock::acquire(&out.join("build.lock"), process::lock::Mode::Exclusive)?;
    let seed = fs::read(root.join("tools/lab/linux-net/udhcpc.config"))?;
    let compiler = process::output(
        command(root, "cc").arg("--version"),
        Some(std::time::Duration::from_secs(10)),
    )?;
    if !compiler.status.success() {
        return Err("cannot inspect udhcpc compiler".into());
    }
    let inputs = serde_json::json!({"source":URL,"source_sha256":SHA256,"version":"1.38.0","license":"GPL-2.0-only","config":std::str::from_utf8(&seed)?,"compiler":String::from_utf8(compiler.stdout)?,"builder_sha256":oer_os_durable::sha256_bytes(include_bytes!("udhcpc.rs"))});
    let binary = out.join("busybox");
    let record = out.join("provenance.json");
    if let (Ok(bytes), Ok(old)) = (fs::read(&binary), fs::read(&record))
        && let Ok(old) = serde_json::from_slice::<serde_json::Value>(&old)
        && old.get("inputs") == Some(&inputs)
        && old.get("binary_sha256").and_then(serde_json::Value::as_str)
            == Some(oer_os_durable::sha256_bytes(&bytes).as_str())
    {
        return Ok(());
    }
    let archive = out.join("busybox-1.38.0.tar.bz2");
    if !archive.exists() {
        let tmp = tempfile::NamedTempFile::new_in(&out)?;
        process::run(
            command(root, "curl")
                .args([
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "120",
                    "--output",
                ])
                .arg(tmp.path())
                .arg(URL),
        )?;
        verify(&fs::read(tmp.path())?)?;
        tmp.persist(&archive)?;
    }
    verify(&fs::read(&archive)?)?;
    let work = tempfile::tempdir_in(&out)?;
    process::run(
        command(root, "tar")
            .arg("-xjf")
            .arg(&archive)
            .arg("-C")
            .arg(work.path()),
    )?;
    let source = work.path().join("busybox-1.38.0");
    let config = work.path().join("seed.config");
    fs::write(&config, seed)?;
    let make = || {
        let mut c = command(&source, "make");
        c.env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("CFLAGS")
            .env_remove("CPPFLAGS")
            .env_remove("LDFLAGS")
            .env_remove("CROSS_COMPILE")
            .stdin(Stdio::null());
        c
    };
    process::run(make().arg("allnoconfig"))?;
    let allno = fs::read_to_string(source.join(".config"))?;
    let configured = with_seed(&allno, std::str::from_utf8(&fs::read(&config)?)?);
    fs::write(source.join(".config"), configured)?;
    process::run(make().arg("oldconfig"))?;
    process::run(
        make()
            .arg(format!("-j{}", std::thread::available_parallelism()?.get()))
            .args(["CC=cc", "busybox"]),
    )?;
    // Single-applet BusyBox treats its first argument as the applet name;
    // --list is not an inspection command in that build. Inspect the actual
    // compiler-generated dispatch table without executing DHCP at build time.
    let table = fs::read_to_string(source.join("include/applet_tables.h"))?;
    if !only_udhcpc(&table) {
        return Err("DHCP build must contain only udhcpc".into());
    }
    let bytes = fs::read(source.join("busybox"))?;
    let provenance = serde_json::json!({"inputs":inputs,"binary_sha256":oer_os_durable::sha256_bytes(&bytes),"applets":["udhcpc"]});
    let mut staged = tempfile::NamedTempFile::new_in(&out)?;
    std::io::Write::write_all(&mut staged, &bytes)?;
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o755))?;
    staged.persist(binary)?;
    fs::write(record, serde_json::to_vec_pretty(&provenance)?)?;
    eprintln!("udhcpc built and applet set verified");
    Ok(())
}

fn only_udhcpc(table: &str) -> bool {
    [
        "#define NUM_APPLETS 1",
        "#define SINGLE_APPLET_STR \"udhcpc\"",
        "#define SINGLE_APPLET_MAIN udhcpc_main",
    ]
    .into_iter()
    .all(|line| table.lines().any(|l| l == line))
}

fn with_seed(allno: &str, seed: &str) -> String {
    let keys = seed
        .lines()
        .filter_map(|s| s.split_once('=').map(|(key, _)| key))
        .collect::<Vec<_>>();
    let mut result = allno
        .lines()
        .filter(|line| {
            !keys.contains(&line.split('=').next().unwrap_or(""))
                && !line
                    .strip_prefix("# ")
                    .and_then(|s| s.split_whitespace().next())
                    .is_some_and(|key| keys.contains(&key))
        })
        .map(|s| format!("{s}\n"))
        .collect::<String>();
    result.push_str(seed);
    result.push('\n');
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn corrupt_sources_are_rejected_before_extraction() {
        assert!(super::verify(b"wrong archive").is_err());
    }
    #[test]
    fn applet_verification_never_executes_the_dhcp_client() {
        let good = "#define NUM_APPLETS 1\n#define SINGLE_APPLET_STR \"udhcpc\"\n#define SINGLE_APPLET_MAIN udhcpc_main\n";
        assert!(super::only_udhcpc(good));
        assert!(!super::only_udhcpc(
            &good.replace("NUM_APPLETS 1", "NUM_APPLETS 2")
        ));
        assert!(!super::only_udhcpc(&good.replace("\"udhcpc\"", "\"sh\"")));
    }
}
