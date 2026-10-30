//! Install `cargo stand` as a prebuilt tool from `main`, and the LAB
//! service it carries as a systemd user unit.
//!
//! An update first asks a running service to drain: it grants nothing more
//! and answers once no lease is held. Then the unit restarts on the new
//! binary. A checkout never replaces the service by itself.
//!
//! `cargo stand` otherwise builds `oer-tool-lab-cli` from the caller's working tree
//! first: a rebuild after every edit of a stand package, unavailable while
//! that code does not compile, and six checkouts on six commits writing the
//! one shared arbiter state with six different binaries. `cargo stand
//! install` builds the `oer-tool-lab-cli` of `origin/main` once, in its own
//! clone, and installs `oer-tool-lab-cli` on the user's PATH: a wrapper that runs
//! that binary against the caller's checkout (`--root` is the caller's Git
//! top level, so owners and local paths stay the caller's).
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::Result;
use oer_os_process as process;
use oer_os_process::Checkout;

/// The package, and binary, the tool installs: `cargo stand`.
const PACKAGE: &str = "oer-tool-lab-cli";

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is required to install the stand tool".into())
}

/// Where the tool's clone, build and binary live.
pub fn tool_directory() -> Result<PathBuf> {
    oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Data, "stand-tool")
}

/// The systemd user unit of the LAB service.
const UNIT: &str = "oer-lab.service";

/// The unit running `binary`'s service, with the chip profiles of the
/// clone at `root` and the tools on `path`. Every process a hardware
/// command starts stays in the unit's control group, which a stop or a
/// crash empties before the service starts again.
pub fn unit(binary: &Path, root: &Path, path: &str) -> String {
    format!(
        "[Unit]\n\
         Description=open-esp-radio LAB service: the stand's one owner\n\
         \n\
         [Service]\n\
         ExecStart='{}' --root '{}' __serve\n\
         Environment=\"PATH={path}\"\n\
         KillMode=control-group\n\
         Restart=on-failure\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        binary.display(),
        root.display()
    )
}

/// Run `systemctl --user` with `arguments`.
fn systemctl(arguments: &[&str]) -> Result<()> {
    process::run(
        oer_os_process::command("systemctl")
            .arg("--user")
            .args(arguments),
    )
}

/// Install the service's unit for `binary` and restart it, after a running
/// service drained its leases.
fn install_service(binary: &Path, root: &Path) -> Result<()> {
    if let Ok(mut client) = oer_lab_service::client::Client::open("cargo stand install") {
        println!("waiting for the running LAB service to finish its leases");
        client.administer(oer_lab_service::protocol::Request::Drain)?;
    }
    let directory = home()?.join(".config/systemd/user");
    fs::create_dir_all(&directory)?;
    let path = std::env::var("PATH").unwrap_or_default();
    fs::write(directory.join(UNIT), unit(binary, root, &path))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", UNIT])?;
    systemctl(&["restart", UNIT])?;
    println!("the LAB service runs as the systemd user unit {UNIT}");
    Ok(())
}

/// The wrapper script: the installed binary against the caller's checkout.
pub fn wrapper(binary: &Path, fallback_root: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         # Installed by `cargo stand install`; `cargo stand`\n\
         # without building the caller's tree. Runs against the caller's checkout.\n\
         root=$(git rev-parse --show-toplevel 2>/dev/null) || root='{}'\n\
         exec '{}' --root \"$root\" \"$@\"\n",
        fallback_root.display(),
        binary.display()
    )
}

pub fn run(ctx: &Checkout) -> Result<()> {
    let tool = tool_directory()?;
    let source = tool.join("src");
    let remote = String::from_utf8(
        process::capture(
            oer_os_process::git::command(&ctx.root).args(["remote", "get-url", "origin"]),
        )?
        .stdout,
    )?;
    let remote = remote.trim();
    if !source.join(".git").is_dir() {
        fs::create_dir_all(&tool)?;
        process::run(
            oer_os_process::git::command(&ctx.root)
                .args(["clone", "--quiet", remote])
                .arg(&source),
        )?;
    }
    process::run(
        oer_os_process::git::command(&source).args(["fetch", "--quiet", "origin", "main"]),
    )?;
    process::run(oer_os_process::git::command(&source).args([
        "checkout",
        "--quiet",
        "--detach",
        "FETCH_HEAD",
    ]))?;
    let git_text = |args: &[&str]| -> Result<String> {
        Ok(String::from_utf8(
            process::capture(oer_os_process::git::command(&source).args(args))?.stdout,
        )?
        .trim()
        .to_owned())
    };
    let commit = git_text(&["rev-parse", "HEAD"])?;
    let short = &commit[..12.min(commit.len())];
    let bin = tool.join("bin");
    let installed = bin.join(PACKAGE);
    let previous = fs::read_to_string(tool.join("installed-commit")).ok();
    if installed.is_file()
        && let Some(previous) = previous.as_deref().map(str::trim)
    {
        // Rebuild only when a package the tool is built from changed.
        let mut diff = vec!["diff", "--name-only", previous, "HEAD", "--"];
        let inputs = build_inputs(ctx, &source)?;
        diff.extend(inputs.iter().map(String::as_str));
        if git_text(&diff).is_ok_and(|changed| changed.is_empty()) {
            fs::write(tool.join("installed-commit"), &commit)?;
            println!("oer-tool-lab-cli is up to date at {short}");
            return install_service(&installed, &source);
        }
    }
    // The tool's work is file and device I/O: the incremental dev profile
    // rebuilds in seconds where the release profile took minutes.
    process::run(
        oer_os_toolchain::cargo_in(&ctx.root)
            .current_dir(&source)
            .args(["build", "--locked", "-p", PACKAGE]),
    )?;
    fs::create_dir_all(&bin)?;
    let staged = bin.join(format!(".{PACKAGE}.new"));
    fs::copy(source.join("target/debug").join(PACKAGE), &staged)?;
    fs::rename(&staged, &installed)?;
    let script_directory = home()?.join(".local/bin");
    fs::create_dir_all(&script_directory)?;
    let script = script_directory.join("oer-tool-lab-cli");
    fs::write(&script, wrapper(&installed, &source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    }
    fs::write(tool.join("installed-commit"), &commit)?;
    println!(
        "installed oer-tool-lab-cli from main {short} ({})",
        script.display()
    );
    install_service(&installed, &source)
}

/// The files the tool is built from, relative to its clone: the workspace
/// manifest, lock file and toolchain, and the directory of every path
/// package the tool's package depends on.
fn build_inputs(ctx: &Checkout, source: &Path) -> Result<Vec<String>> {
    let output = process::capture(
        oer_os_toolchain::cargo_in(&ctx.root)
            .current_dir(source)
            .args(["metadata", "--format-version", "1", "--locked"]),
    )?;
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let mut inputs = path_package_closure(&metadata, PACKAGE, source)?;
    inputs.extend(["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo"].map(String::from));
    Ok(inputs)
}

/// The directories, relative to `root`, of the path packages `package`
/// reaches through `metadata`'s resolved graph, itself included.
fn path_package_closure(
    metadata: &serde_json::Value,
    package: &str,
    root: &Path,
) -> Result<Vec<String>> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata lists no packages")?;
    let start = packages
        .iter()
        .find(|candidate| candidate["name"] == package && candidate["source"].is_null())
        .and_then(|candidate| candidate["id"].as_str())
        .ok_or_else(|| format!("no path package {package}"))?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("cargo metadata has no resolved graph")?;
    let mut seen = std::collections::BTreeSet::from([start.to_owned()]);
    let mut pending = vec![start.to_owned()];
    while let Some(id) = pending.pop() {
        let Some(node) = nodes.iter().find(|node| node["id"] == id.as_str()) else {
            continue;
        };
        for dependency in node["deps"].as_array().into_iter().flatten() {
            if let Some(next) = dependency["pkg"].as_str()
                && seen.insert(next.to_owned())
            {
                pending.push(next.to_owned());
            }
        }
    }
    let root = root.canonicalize()?;
    Ok(packages
        .iter()
        .filter(|candidate| {
            candidate["source"].is_null()
                && candidate["id"].as_str().is_some_and(|id| seen.contains(id))
        })
        .filter_map(|candidate| {
            let manifest = Path::new(candidate["manifest_path"].as_str()?);
            let directory = manifest.parent()?.canonicalize().ok()?;
            Some(
                directory
                    .strip_prefix(&root)
                    .ok()?
                    .to_string_lossy()
                    .into_owned(),
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapper_runs_the_installed_binary_against_the_callers_checkout() {
        let text = wrapper(Path::new("/tool/bin/oer-stand"), Path::new("/tool/src"));
        assert!(text.starts_with("#!/bin/sh\n"));
        assert!(
            text.contains("root=$(git rev-parse --show-toplevel 2>/dev/null) || root='/tool/src'")
        );
        assert!(text.contains("exec '/tool/bin/oer-stand' --root \"$root\" \"$@\""));
    }

    #[test]
    fn the_unit_serves_from_the_installed_binary_and_owns_every_descendant() {
        let text = unit(
            Path::new("/tool/bin/oer-tool-lab-cli"),
            Path::new("/tool/src"),
            "/usr/bin:/bin",
        );
        assert!(text.contains("ExecStart='/tool/bin/oer-tool-lab-cli' --root '/tool/src' __serve"));
        assert!(text.contains("KillMode=control-group"));
        assert!(text.contains("Environment=\"PATH=/usr/bin:/bin\""));
        assert!(text.contains("WantedBy=default.target"));
    }
}
