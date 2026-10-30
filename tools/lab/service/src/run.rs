//! The service process: one per host. Its systemd user units (`cargo stand
//! install`) run it: `oer-lab.socket` holds the stand model's socket
//! (`paths::socket`) and hands it over, so a client that connects while the
//! service restarts waits for it, and `oer-lab.service` delegates its
//! control group to it for the commands it runs. A lock in the stand's
//! directory keeps a second instance out. Started by hand, without a socket
//! handed over, it binds the socket itself.

use std::{os::unix::net::UnixListener, path::PathBuf, sync::Arc};

use oer_lab_arbiter::Arbiter;
use oer_os_process::{
    launch_handoff::LaunchHandoff,
    lock::{FileLock, Mode},
};

use crate::{service::Service, stand::StandDrivers};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The listening socket systemd hands over (`sd_listen_fds`): descriptor 3,
/// for the process `LISTEN_PID` names.
const ACTIVATION: LaunchHandoff = LaunchHandoff {
    descriptor: 3,
    marker: "LISTEN_PID",
    environment: &["LISTEN_PID", "LISTEN_FDS", "LISTEN_FDNAMES"],
};

/// The socket systemd handed over, when it handed one.
fn activated() -> Result<Option<UnixListener>> {
    if std::env::var_os(ACTIVATION.marker).is_none() {
        return Ok(None);
    }
    if std::env::var("LISTEN_FDS").as_deref() != Ok("1") {
        return Err("systemd must hand the service exactly one socket (LISTEN_FDS=1)".into());
    }
    let socket = ACTIVATION.adopt(&std::process::id().to_string())?;
    Ok(Some(UnixListener::from(std::os::fd::OwnedFd::from(socket))))
}

/// Serve the stand until the process ends. `root` is the repository the
/// service was built from: its chip profiles describe the boards. Call it
/// first in `main`, before any thread starts: it takes the socket systemd
/// handed over.
pub fn serve(root: PathBuf) -> Result<()> {
    let handed = activated()?;
    let _signals = oer_os_process::install_signal_handlers()?;
    let arbiter = Arbiter::open()?;
    let _instance =
        FileLock::try_acquire(&arbiter.directory().join("service.lock"), Mode::Exclusive)?
            .ok_or("another LAB service runs for this stand")?;
    let socket = oer_lab_model::paths::socket()?;
    let listener = match handed {
        Some(listener) => listener,
        None => {
            if let Some(directory) = socket.parent() {
                std::fs::create_dir_all(directory)?;
            }
            // The instance lock is held: a socket file left is a dead
            // service's.
            match std::fs::remove_file(&socket) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(error.into());
                }
                _ => {}
            }
            UnixListener::bind(&socket)?
        }
    };
    // Every command runs in a control group of the service's own, which
    // its unit delegates to it.
    let commands = oer_os_process::cgroup::Delegated::take()?;
    let drivers = Arc::new(StandDrivers::new(
        root,
        arbiter.stand_file().to_owned(),
        arbiter.directory().to_owned(),
        arbiter.journal(),
        Some(commands),
    ));
    // Before `cargo stand init` wrote the stand file the stand has no
    // resource; `Request::Reload` reads it once it exists.
    let resources = crate::service::Drivers::resources(&*drivers)?;
    let service = Service::open(
        arbiter,
        resources,
        drivers,
        Box::new(oer_os_durable::unix_millis),
    )?;
    eprintln!("lab-service: serving {}", socket.display());
    // A stop (SIGTERM from systemd) ends the service at once: its state is
    // persisted with every change, and the unit stops what it started.
    std::thread::spawn(|| {
        while !oer_os_process::cancellation_requested() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        eprintln!("lab-service: stopping");
        std::process::exit(0);
    });
    service.serve(listener)?;
    Ok(())
}
