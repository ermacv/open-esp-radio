//! The pinned inputs of Blobray's RISC-V executor conformance test
//! (`tools/analysis/cli/tests/riscv_conformance/inputs.toml`): the
//! riscv-arch-test checkout at its pinned revision, holding only the test
//! environment and the suites the pin lists, and the Sail release archive
//! verified against its SHA-256. Both live in the host-wide store
//! ([`crate::artifacts::store`]) like every other pinned input.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::artifacts::Result;

/// The pin, relative to the repository root.
pub const INPUTS: &str = "tools/analysis/cli/tests/riscv_conformance/inputs.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    schema: u32,
    #[serde(rename = "arch-test")]
    arch_test: ArchTest,
    sail: Sail,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchTest {
    repository: String,
    tag: String,
    revision: String,
    suite: Vec<Suite>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Suite {
    name: String,
    #[allow(dead_code, reason = "the test assembles each suite for it")]
    march: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sail {
    url: String,
    sha256: String,
    binary: String,
}

/// Where the fetched inputs lie.
#[derive(Debug, Eq, PartialEq)]
pub struct Inputs {
    /// The riscv-arch-test checkout.
    pub arch_test: PathBuf,
    /// The `sail_riscv_sim` executable.
    pub sail: PathBuf,
}

fn pin(root: &Path) -> Result<Pin> {
    let path = root.join(INPUTS);
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let pin: Pin = toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    if pin.schema != 1 {
        return Err(format!("{}: schema {}, expected 1", path.display(), pin.schema).into());
    }
    Ok(pin)
}

/// The paths the inputs of `pin` take in `store`.
fn located(store: &Path, pin: &Pin) -> Inputs {
    let sail = store.join("sail-riscv").join(&pin.sail.sha256);
    Inputs {
        arch_test: store.join("riscv-arch-test").join(&pin.arch_test.revision),
        sail: sail.join(&pin.sail.binary),
    }
}

/// The fetched inputs, or an error naming `cargo pins conformance` when
/// either is missing.
pub fn inputs(root: &Path) -> Result<Inputs> {
    let inputs = located(&crate::artifacts::store()?, &pin(root)?);
    if !inputs.arch_test.join(".git").is_dir() || !inputs.sail.is_file() {
        return Err("the conformance inputs are not fetched: run `cargo pins conformance`".into());
    }
    Ok(inputs)
}

fn git(directory: &Path) -> std::process::Command {
    let mut command = oer_os_process::command("git");
    command.arg("-C").arg(directory);
    command
}

fn run(command: &mut std::process::Command, what: &str) -> Result<()> {
    if !command.status()?.success() {
        return Err(format!("{what} failed").into());
    }
    Ok(())
}

/// Check out the pinned revision with only the test environment and the
/// pinned suites into `destination`, through a staging directory.
fn fetch_arch_test(arch: &ArchTest, destination: &Path) -> Result<()> {
    let staging = destination.with_extension("partial");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    run(git(&staging).arg("init").arg("--quiet"), "git init")?;
    run(
        git(&staging).args(["remote", "add", "origin", &arch.repository]),
        "git remote add",
    )?;
    let mut paths = vec!["riscv-test-suite/env".to_owned()];
    paths.extend(
        arch.suite
            .iter()
            .map(|suite| format!("riscv-test-suite/rv32i_m/{}", suite.name)),
    );
    run(
        git(&staging)
            .args(["sparse-checkout", "set", "--no-cone"])
            .args(&paths),
        "git sparse-checkout",
    )?;
    run(
        git(&staging)
            .args([
                "fetch",
                "--quiet",
                "--depth",
                "1",
                "--filter=blob:none",
                "origin",
            ])
            .arg(&arch.revision),
        "git fetch of the arch-test revision",
    )?;
    run(
        git(&staging).args(["checkout", "--quiet", "FETCH_HEAD"]),
        "git checkout",
    )?;
    let head = oer_os_process::capture(git(&staging).args(["rev-parse", "HEAD"]))?;
    let head = String::from_utf8_lossy(&head.stdout);
    if head.trim() != arch.revision {
        return Err(format!(
            "riscv-arch-test {} resolved to {}, pinned {}",
            arch.tag,
            head.trim(),
            arch.revision
        )
        .into());
    }
    std::fs::rename(&staging, destination)?;
    Ok(())
}

/// Download the Sail release archive, verify it and unpack it into
/// `directory`.
fn fetch_sail(sail: &Sail, directory: &Path) -> Result<()> {
    let staging = directory.with_extension("partial");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let archive = staging.join("sail.tar.gz");
    crate::artifacts::download(&sail.url, &archive)?;
    let actual = oer_os_durable::sha256_file(&archive)?;
    if actual != sail.sha256 {
        return Err(format!("{}: sha256 {actual}, pinned {}", sail.url, sail.sha256).into());
    }
    run(
        oer_os_process::command("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&staging),
        "unpacking the Sail archive",
    )?;
    std::fs::remove_file(&archive)?;
    std::fs::rename(&staging, directory)?;
    Ok(())
}

/// Fetch whichever input the store lacks and return where both lie.
pub fn fetch(root: &Path) -> Result<Inputs> {
    let pin = pin(root)?;
    let store = crate::artifacts::store()?;
    let inputs = located(&store, &pin);
    if !inputs.arch_test.join(".git").is_dir() {
        std::fs::create_dir_all(store.join("riscv-arch-test"))?;
        fetch_arch_test(&pin.arch_test, &inputs.arch_test)?;
    }
    if !inputs.sail.is_file() {
        fetch_sail(&pin.sail, &store.join("sail-riscv").join(&pin.sail.sha256))?;
        if !inputs.sail.is_file() {
            return Err(format!("the Sail archive holds no {}", pin.sail.binary).into());
        }
    }
    Ok(inputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tracked_pin_parses_and_locates_its_inputs_by_revision_and_hash() {
        let pin: Pin = toml::from_str(include_str!(
            "../../../../analysis/cli/tests/riscv_conformance/inputs.toml"
        ))
        .unwrap();
        assert!(!pin.arch_test.suite.is_empty());
        let inputs = located(Path::new("/store"), &pin);
        assert_eq!(
            inputs.arch_test,
            Path::new("/store/riscv-arch-test").join(&pin.arch_test.revision)
        );
        assert!(
            inputs
                .sail
                .starts_with(Path::new("/store/sail-riscv").join(&pin.sail.sha256))
        );
    }
}
