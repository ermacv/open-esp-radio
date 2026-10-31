//! Install `cargo stand` as a prebuilt tool from `main`, and the LAB
//! service (`oer-tool-lab-service`) as two systemd user units: `oer-lab.socket`
//! holds the service's socket and hands it over, `oer-lab.service` runs the
//! service.
//!
//! An update builds the new version in a worktree of its own (`stage`) and
//! stages its binaries beside the installed ones; nothing installed changes
//! yet. Then it asks a running service to drain: it grants nothing more and
//! answers once no lease is held. A service of another protocol cannot be
//! asked: the update stops there, the installation as it was, and says how
//! to replace the service by hand. Only then does the clone the service
//! reads move to the new commit, the staged binaries replace the installed
//! ones and the unit restart. Before the service first
//! starts, the restrictions and balances the file-locked arbiter kept are
//! imported (`oer_lab_arbiter::import`) and a schema 1 stand file is
//! rewritten to schema 2, the schema 1 file kept beside it. A checkout never
//! replaces the service by itself.
//!
//! `cargo stand` otherwise builds `oer-tool-lab-cli` from the caller's working tree
//! first: a rebuild after every edit of a stand package, unavailable while
//! that code does not compile, and six checkouts on six commits writing the
//! one shared arbiter state with six different binaries. `cargo stand
//! install` builds the `oer-tool-lab-cli` of `origin/main` once, beside its
//! own clone, and installs `oer-tool-lab-cli` on the user's PATH: a wrapper that runs
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
/// board is parked before anyone asks. A stop or a crash ends every program
/// a driver started before the service starts again.
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

/// Install the service's units for `binary` and restart it, once a running
/// service drained its leases: the stand's earlier records are imported and
/// a schema 1 stand file rewritten first.
fn install_service(binary: &Path, root: &Path) -> Result<()> {
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
    // The clone the running service reads its chip profiles from stays as
    // it is until the service drained: the new version is built in a
    // worktree of its own and staged beside the installed binaries.
    let source = tool.join("src");
    let stage = tool.join("stage");
    let remote = String::from_utf8(
        process::capture(
            oer_os_process::git::command(&ctx.root).args(["remote", "get-url", "origin"]),
        )?
        .stdout,
    )?;
    let remote = remote.trim();
    if !source.join(".git").exists() {
        fs::create_dir_all(&tool)?;
        process::run(
            oer_os_process::git::command(&ctx.root)
                .args(["clone", "--quiet", "--no-checkout", remote])
                .arg(&source),
        )?;
    }
    process::run(
        oer_os_process::git::command(&source).args(["fetch", "--quiet", "origin", "main"]),
    )?;
    let commit = String::from_utf8(
        process::capture(oer_os_process::git::command(&source).args(["rev-parse", "FETCH_HEAD"]))?
            .stdout,
    )?
    .trim()
    .to_owned();
    if stage.join(".git").exists() {
        process::run(
            oer_os_process::git::command(&stage).args(["checkout", "--quiet", "--detach", &commit]),
        )?;
    } else {
        process::run(
            oer_os_process::git::command(&source)
                .args(["worktree", "add", "--quiet", "--detach"])
                .arg(&stage)
                .arg(&commit),
        )?;
    }
    let short = &commit[..12.min(commit.len())];
    // The tool's work is file and device I/O: the incremental dev profile
    // rebuilds in seconds where the release profile took minutes.
    let target = tool.join("target");
    process::run(
        oer_os_toolchain::cargo_in(&ctx.root)
            .current_dir(&stage)
            .env("CARGO_TARGET_DIR", &target)
            .args(["build", "--locked", "-p", PACKAGE, "-p", SERVICE_PACKAGE]),
    )?;
    let bin = tool.join("bin");
    fs::create_dir_all(&bin)?;
    let installed = bin.join(PACKAGE);
    let service = bin.join(SERVICE_BINARY);
    let staged = [(BINARY, &installed), (SERVICE_BINARY, &service)].map(|(built, live)| {
        (
            target.join("debug").join(built),
            bin.join(format!(".{built}.new")),
            live,
        )
    });
    for (built, staged, _) in &staged {
        fs::copy(built, staged)?;
    }
    // Nothing installed has changed yet: a service that cannot drain stops
    // the update here.
    drain()?;
    process::run(
        oer_os_process::git::command(&source).args(["checkout", "--quiet", "--detach", &commit]),
    )?;
    for (_, staged, live) in &staged {
        fs::rename(staged, live)?;
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
