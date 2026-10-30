//! The service process: one per host, on the socket of the stand model's
//! `paths::socket`, started by its systemd user unit (`cargo stand
//! install`). A lock in the stand's directory keeps a second instance out;
//! a socket file without a live service behind it is replaced.

use std::{os::unix::net::UnixListener, path::PathBuf, sync::Arc};

use oer_lab_arbiter::Arbiter;
use oer_lab_model::Stand;
use oer_os_process::lock::{FileLock, Mode};

use crate::{service::Service, stand::StandDrivers};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Serve the stand until the process ends. `root` is the repository the
/// service was built from: its chip profiles describe the boards.
pub fn serve(root: PathBuf) -> Result<()> {
    let arbiter = Arbiter::open()?;
    let _instance =
        FileLock::try_acquire(&arbiter.directory().join("service.lock"), Mode::Exclusive)?
            .ok_or("another LAB service runs for this stand")?;
    let socket = oer_lab_model::paths::socket()?;
    if let Some(directory) = socket.parent() {
        std::fs::create_dir_all(directory)?;
    }
    // The instance lock is held: a socket file left is a dead service's.
    match std::fs::remove_file(&socket) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let listener = UnixListener::bind(&socket)?;
    let stand = Stand::load(arbiter.stand_file())?;
    let drivers = Arc::new(StandDrivers::new(
        root,
        arbiter.stand_file().to_owned(),
        arbiter.journal(),
    ));
    let service = Service::open(
        arbiter,
        StandDrivers::resources(&stand),
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
