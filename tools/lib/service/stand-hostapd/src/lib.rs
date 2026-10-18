//! Pinned hostapd preparation for the Linux fixture software; never owns a
//! radio. Built into `target/hil/hostapd` from the recipe in
//! `hil/host/linux-net/hostapd`.
#![forbid(unsafe_code)]

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
use oer_process as process;
use std::{fs, path::Path, process::Command};

const URL: &str = "https://w1.fi/releases/hostapd-2.12.tar.gz";
const SOURCE_SHA256: &str = "f43502561c28ba47ab77e18e1a973d07361c68cc8b14178e619bd5796b70eabd";

fn verify_source(bytes: &[u8]) -> Result<()> {
    if oer_durable::sha256_bytes(bytes) != SOURCE_SHA256 {
        return Err("hostapd archive SHA-256 mismatch".into());
    }
    Ok(())
}

fn command(root: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = oer_process::command(program);
    command.current_dir(root);
    command
}

pub fn build(root: &Path) -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Err("the hostapd fixture build requires Linux".into());
    }
    let output = root.join("target/hil/hostapd");
    fs::create_dir_all(&output)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(output.join("build.lock"))?;
    lock.try_lock()
        .map_err(|_| "another hostapd build owns the output directory")?;
    let recipe = root.join("hil/host/linux-net/hostapd");
    let patch = fs::read(recipe.join("300-noscan.patch"))?;
    let config = fs::read(recipe.join("build.config"))?;
    let compiler = tool_version(command(root, "cc").arg("--version"))?;
    let libraries = tool_version(command(root, "pkg-config").args([
        "--modversion",
        "libnl-3.0",
        "libnl-genl-3.0",
        "openssl",
    ]))?;
    let inputs = serde_json::json!({"schema":1,"source":URL,"source_sha256":SOURCE_SHA256,
        "patch_sha256":oer_durable::sha256_bytes(&patch),"config_sha256":oer_durable::sha256_bytes(&config),"builder_sha256":oer_durable::sha256_bytes(include_bytes!("lib.rs")),
        "compiler":compiler,"libraries":libraries});
    let binary = output.join("hostapd");
    let provenance = output.join("provenance.json");
    if let (Ok(old), Ok(bytes)) = (fs::read(&provenance), fs::read(&binary))
        && let Ok(old) = serde_json::from_slice::<serde_json::Value>(&old)
        && old["inputs"] == inputs
        && old["binary_sha256"] == oer_durable::sha256_bytes(&bytes)
    {
        eprintln!("hostapd build verified: {}", binary.display());
        return Ok(());
    }
    let archive = output.join("hostapd-2.12.tar.gz");
    if !archive.exists() {
        let download = tempfile::NamedTempFile::new_in(&output)?;
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
                .arg(download.path())
                .arg(URL),
        )?;
        verify_source(&fs::read(download.path())?)?;
        download.persist(&archive)?;
    }
    verify_source(&fs::read(&archive)?)?;
    let work = tempfile::tempdir_in(&output)?;
    process::run(
        command(root, "tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(work.path()),
    )?;
    let source = work.path().join("hostapd-2.12");
    process::run(
        command(root, "patch")
            .current_dir(&source)
            .args(["--batch", "--fuzz=0", "-p1", "-i"])
            .arg(recipe.join("300-noscan.patch")),
    )?;
    fs::write(source.join("hostapd/.config"), config)?;
    process::run(
        command(root, "make")
            .current_dir(source.join("hostapd"))
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("CFLAGS")
            .env_remove("CPPFLAGS")
            .env_remove("LDFLAGS")
            .arg(format!("-j{}", std::thread::available_parallelism()?.get()))
            .args(["CC=cc", "hostapd"]),
    )?;
    let built = source.join("hostapd/hostapd");
    verify_policy(root, &built, work.path())?;
    let mut staged = tempfile::NamedTempFile::new_in(&output)?;
    std::io::copy(&mut fs::File::open(&built)?, &mut staged)?;
    fs::set_permissions(staged.path(), fs::metadata(&built)?.permissions())?;
    let hash = oer_durable::sha256_bytes(&fs::read(staged.path())?);
    staged.persist(&binary)?;
    let record = serde_json::json!({"inputs":inputs,"binary_sha256":hash});
    let mut staged = tempfile::NamedTempFile::new_in(&output)?;
    serde_json::to_writer_pretty(&mut staged, &record)?;
    staged.persist(provenance)?;
    eprintln!(
        "hostapd built and policy parser verified: {}",
        binary.display()
    );
    Ok(())
}

fn verify_policy(root: &Path, binary: &Path, directory: &Path) -> Result<()> {
    use oer_process::CommandExt as _;
    let config = directory.join("parser.conf");
    // An intentional final syntax error prevents all driver initialization.
    fs::write(&config, "noscan=1\nht_coex=1\noer_parser_stop=1\n")?;
    let output = command(root, binary).arg(&config).supervised_output()?;
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success()
        || !diagnostic.contains("unknown configuration item 'oer_parser_stop'")
        || !diagnostic.contains("1 errors found")
    {
        return Err(format!("hostapd policy parser verification failed: {diagnostic}").into());
    }
    Ok(())
}

fn tool_version(command: &mut std::process::Command) -> Result<String> {
    let output = process::output(command, Some(std::time::Duration::from_secs(10)))?;
    if !output.status.success() {
        return Err(format!(
            "hostapd build prerequisite failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_corrupt_source_before_extraction() {
        assert!(super::verify_source(b"not the pinned release").is_err());
    }
}
