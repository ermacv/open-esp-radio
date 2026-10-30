//! Install `cargo stand` as a prebuilt tool from `main`, and the LAB
//! service (`oer-tool-lab-service`) as two systemd user units: `oer-lab.socket`
//! holds the service's socket and hands it over, `oer-lab.service` runs the
//! service with its control group delegated to it.
//!
//! An update first asks a running service to drain: it grants nothing more
//! and answers once no lease is held. A service of another protocol cannot
//! be asked: the update stops there and says how to replace it by hand.
//! Then the unit restarts on the new binary. Before the service first
//! starts, the restrictions and balances the file-locked arbiter kept are
//! imported (`oer_lab_arbiter::import`) and a schema 1 stand file is
//! rewritten to schema 2, the schema 1 file kept beside it. A checkout never
//! replaces the service by itself.
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

/// The package the tool installs, `cargo stand`, and the binary it builds.
const PACKAGE: &str = "oer-tool-lab-cli";
const BINARY: &str = "stand";
/// The LAB service's package and binary.
const SERVICE_PACKAGE: &str = "oer-tool-lab-service";
const SERVICE_BINARY: &str = "oer-tool-lab-service";

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

/// The systemd user units of the LAB service.
const UNIT: &str = "oer-lab.service";
const SOCKET_UNIT: &str = "oer-lab.socket";

/// The socket unit: it holds the service's socket in the runtime
/// directory, so a client that connects while the service restarts waits.
pub fn socket_unit() -> String {
    "[Unit]\n\
     Description=open-esp-radio LAB service socket\n\
     \n\
     [Socket]\n\
     ListenStream=%t/open-esp-radio/lab.sock\n\
     SocketMode=0600\n\
     DirectoryMode=0700\n\
     \n\
     [Install]\n\
     WantedBy=sockets.target\n"
        .to_owned()
}

/// The service unit running `binary` with the chip profiles of the clone
/// at `root` and the tools on `path`. It starts at login too, so every
/// board is parked before anyone asks. Its control group is delegated to
/// it: every command it runs gets a group of its own there. A stop or a
/// crash empties the unit's group before the service starts again.
pub fn unit(binary: &Path, root: &Path, path: &str) -> String {
    format!(
        "[Unit]\n\
         Description=open-esp-radio LAB service: the stand's one owner\n\
         Requires={SOCKET_UNIT}\n\
         After={SOCKET_UNIT}\n\
         \n\
         [Service]\n\
         ExecStart='{}' --root '{}'\n\
         Environment=\"PATH={path}\"\n\
         Delegate=yes\n\
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

/// Ask a running service to drain: wait until it holds no lease. Nothing to
/// ask when no service listens; a service of another protocol cannot be
/// asked, and the update stops.
fn drain() -> Result<()> {
    let socket = oer_lab_model::paths::socket()?;
    let stream = match std::os::unix::net::UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(());
        }
        Err(error) => return Err(format!("cannot reach {}: {error}", socket.display()).into()),
    };
    let mut client = oer_lab_client::client::Client::connect(stream, "cargo stand install")
        .map_err(|refused| {
            format!(
                "the running LAB service cannot be asked to drain: {refused}. Wait until \
                 `cargo stand queue` from a checkout of its version shows no lease, stop it \
                 with `systemctl --user stop {UNIT}`, and install again"
            )
        })?;
    println!("waiting for the running LAB service to finish its leases");
    client.administer(oer_lab_client::protocol::Request::Drain)?;
    Ok(())
}

/// Install the service's units for `binary` and restart it, after a
/// running service drained its leases and the stand's earlier records were
/// imported.
fn install_service(binary: &Path, root: &Path) -> Result<()> {
    drain()?;
    if std::env::var_os(oer_lab_model::paths::ARBITER_ENV).is_none() {
        let previous = oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Cache, "arbiter")?;
        if let Some(imported) =
            oer_lab_arbiter::import::previous(&previous, &oer_lab_arbiter::Arbiter::open()?)?
        {
            println!(
                "imported the stand's earlier records: {} balance(s), {} board(s) under \
                 maintenance, {} quarantined, {}",
                imported.balances,
                imported.maintenance,
                imported.quarantined,
                imported.records.join(", ")
            );
        }
    }
    // Every client switches to this service now: its stand file too.
    let stand_file = oer_lab_model::paths::stand_file()?;
    if fs::read_to_string(&stand_file).is_ok_and(|text| oer_lab_model::convert::is_schema_1(&text))
    {
        crate::stand_file::convert(&stand_file, &stand_file)?;
    }
    let directory = home()?.join(".config/systemd/user");
    fs::create_dir_all(&directory)?;
    let path = std::env::var("PATH").unwrap_or_default();
    fs::write(directory.join(SOCKET_UNIT), socket_unit())?;
    fs::write(directory.join(UNIT), unit(binary, root, &path))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", SOCKET_UNIT, UNIT])?;
    systemctl(&["start", SOCKET_UNIT])?;
    systemctl(&["restart", UNIT])?;
    println!("the LAB service runs as the systemd user units {SOCKET_UNIT} and {UNIT}");
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
    let service = bin.join(SERVICE_BINARY);
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
            return install_service(&service, &source);
        }
    }
    // The tool's work is file and device I/O: the incremental dev profile
    // rebuilds in seconds where the release profile took minutes.
    process::run(
        oer_os_toolchain::cargo_in(&ctx.root)
            .current_dir(&source)
            .args(["build", "--locked", "-p", PACKAGE, "-p", SERVICE_PACKAGE]),
    )?;
    fs::create_dir_all(&bin)?;
    for (built, target) in [(BINARY, &installed), (SERVICE_BINARY, &service)] {
        let staged = bin.join(format!(".{built}.new"));
        fs::copy(source.join("target/debug").join(built), &staged)?;
        fs::rename(&staged, target)?;
    }
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
    install_service(&service, &source)
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
    inputs.extend(path_package_closure(&metadata, SERVICE_PACKAGE, source)?);
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
            Path::new("/tool/bin/oer-tool-lab-service"),
            Path::new("/tool/src"),
            "/usr/bin:/bin",
        );
        assert!(text.contains("ExecStart='/tool/bin/oer-tool-lab-service' --root '/tool/src'"));
        assert!(text.contains("Requires=oer-lab.socket"));
        assert!(text.contains("Delegate=yes"));
        assert!(text.contains("KillMode=control-group"));
        assert!(text.contains("Environment=\"PATH=/usr/bin:/bin\""));
        assert!(text.contains("WantedBy=default.target"));
    }

    #[test]
    fn the_socket_unit_holds_the_service_socket_in_the_runtime_directory() {
        let text = socket_unit();
        assert!(text.contains("ListenStream=%t/open-esp-radio/lab.sock"));
        assert!(text.contains("SocketMode=0600"));
        assert!(text.contains("WantedBy=sockets.target"));
    }
}
