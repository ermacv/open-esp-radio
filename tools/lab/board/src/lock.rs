//! The device lock: one process owns a board at a time.
//!
//! The key is the board's [`DeviceId`] (its MAC, an Espressif USB
//! Serial/JTAG port's serial number), the file
//! `$XDG_RUNTIME_DIR/open-esp-radio/devices/<MAC>.lock` (the XDG cache
//! directory when `XDG_RUNTIME_DIR` is unset), locked with the foundation's
//! `flock` ([`oer_os_process::lock::FileLock`]). The holder writes its
//! [`Holder`] record into the file, so a refused process names who holds
//! the board. A lock is held for a whole operation: a flash with its reopen
//! and start, a reset, a whole monitor, or a HIL lease with its power cycles.
//! The broker explicitly unlocks it after admitted I/O closes, independently
//! of unrelated descriptor copies inherited by a concurrent fork.
//!
//! An independent lifetime broker owns the lock's file description. Owned
//! guards keep its admission channel alive; delegates carry an explicit
//! [`oer_os_process::Context`] capability for that board. [`DeviceAccess::operation`]
//! atomically admits I/O and retains exclusion until its returned guard drops.
//! After owner loss the broker rejects new operations, stops remaining lifetime
//! holders after bounded cleanup and waits for connection closure and complete
//! exit of commands and processes claimed by cleanup.
//! Ports and their reader threads must close before their operation guard.
//!
//! Every acquisition in one process shares its owning guard through a registry.
//! Delegates never receive the flock descriptor and cannot unlock another I/O.
#![forbid(unsafe_code)]

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

use std::{
    collections::HashMap,
    io::{Read as _, Seek as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};

pub use oer_lab_model::device_id::DeviceId;
use oer_os_process::{
    Context,
    lock::{BrokerOperation, FileLock, LockBroker, Mode},
};
use serde::{Deserialize, Serialize};

/// Who holds a device lock.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Holder {
    pub pid: u32,
    /// The owner's process start time; distinguish exited/recycled PIDs.
    pub owner_started_ticks: u64,
    /// The independently running broker authenticated by SO_PEERCRED.
    pub broker_pid: u32,
    /// What the holder does with the board: its command line or lease.
    pub command: String,
    /// When it took the lock, in Unix milliseconds.
    pub started_unix_millis: u64,
    /// The capability explicitly attached to delegates of this board.
    pub token: String,
}

fn context_key(path: &Path) -> crate::lock::Result<String> {
    Ok(format!(
        "device:{}",
        path.to_str().ok_or("device lock path is not UTF-8")?
    ))
}

fn inherited(path: &Path) -> crate::lock::Result<Option<String>> {
    Ok(Context::current()?
        .get(&context_key(path)?)
        .map(str::to_owned))
}

fn token() -> String {
    oer_os_durable::sha256_bytes(
        format!(
            "{}:{}:{:?}",
            std::process::id(),
            oer_os_durable::unix_millis(),
            std::time::Instant::now()
        )
        .as_bytes(),
    )
}

impl std::fmt::Display for Holder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let age = oer_os_durable::unix_millis().saturating_sub(self.started_unix_millis) / 1000;
        write!(formatter, "pid {} `{}` for {age} s", self.pid, self.command)?;
        if oer_os_process::proc::start_ticks(self.pid) != Some(self.owner_started_ticks) {
            write!(
                formatter,
                "; owner exited or is inaccessible, broker pid {} retains I/O; other UIDs, PID namespaces or restricted /proc can hide holders",
                self.broker_pid
            )?;
        }
        Ok(())
    }
}

/// The directory of the device locks.
pub fn directory() -> crate::lock::Result<PathBuf> {
    oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Runtime, "devices")
}

/// The lock file of the board `id`.
pub fn path(id: &DeviceId) -> crate::lock::Result<PathBuf> {
    Ok(path_in(&directory()?, id))
}

fn path_in(directory: &Path, id: &DeviceId) -> PathBuf {
    directory.join(format!("{}.lock", id.compact()))
}

/// The owning guards of this process, by lock file: one per board.
fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<Owned>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<Owned>>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// The broker owner behind every clone of a [`DeviceGuard`].
#[derive(Debug)]
struct Owned {
    id: DeviceId,
    path: PathBuf,
    /// Taken in `drop` under the registry's mutex, so a concurrent
    /// acquisition in this process never sees the lock still held without
    /// its guard.
    broker: Option<LockBroker>,
    token: String,
}

impl Drop for Owned {
    fn drop(&mut self) {
        let mut registry = registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if registry
            .get(&self.path)
            .is_some_and(|weak| weak.strong_count() == 0)
        {
            registry.remove(&self.path);
        }
        drop(self.broker.take());
    }
}

/// The owning guard of a board: the broker admits operations until the last
/// clone drops, then retains exclusion until admitted I/O ends. Every local
/// acquisition returns a clone of this same guard.
#[derive(Clone, Debug)]
pub struct DeviceGuard {
    owned: Arc<Owned>,
}

impl DeviceGuard {
    pub fn id(&self) -> &DeviceId {
        &self.owned.id
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.owned.path
    }
}

/// A board whose owner explicitly delegated a capability to this process.
/// It admits each operation through the lifetime broker, never by a racy
/// holder-record check. Owner loss retains exclusion throughout I/O cleanup;
/// fresh work is rejected.
#[derive(Clone, Debug)]
pub struct DelegatedDevice {
    id: DeviceId,
    path: PathBuf,
    token: String,
    broker_pid: u32,
}

impl DelegatedDevice {
    pub fn id(&self) -> &DeviceId {
        &self.id
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Exclusion for admitted I/O. Close all ports and child readers before this
/// guard drops. It keeps the broker's operation connection, and for local I/O
/// the owning guard too, without lending a flock descriptor to either caller.
#[derive(Debug)]
pub struct DeviceOperation {
    _broker: BrokerOperation,
    _owner: Option<DeviceGuard>,
}

impl DeviceOperation {
    /// Retain this admitted operation through an external hardware process.
    pub fn lifetime(&self) -> &oer_os_process::IoLifetime {
        self._broker.lifetime()
    }
}

/// Access to a board: owned by this process or delegated by an ancestor.
#[derive(Clone, Debug)]
pub enum DeviceAccess {
    Owned(DeviceGuard),
    Delegated(DelegatedDevice),
}

/// Why a lock was not taken.
#[derive(Debug)]
pub enum Busy {
    /// Another process holds it; its record, when it wrote one.
    Held(Option<Holder>),
}

impl std::fmt::Display for Busy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held(Some(holder)) => write!(formatter, "held by {holder}"),
            Self::Held(None) => write!(formatter, "held by another process"),
        }
    }
}

impl DeviceAccess {
    /// The board `id` for `command`: this process's guard of it, a new
    /// guard, or the delegation of an ancestor; else who holds it.
    pub fn try_acquire(
        id: &DeviceId,
        command: &str,
    ) -> crate::lock::Result<std::result::Result<Self, Busy>> {
        Self::try_acquire_in(&directory()?, id, command)
    }

    /// [`Self::try_acquire`] with the lock files in `directory`: a test's
    /// own.
    pub fn try_acquire_in(
        directory: &Path,
        id: &DeviceId,
        command: &str,
    ) -> crate::lock::Result<std::result::Result<Self, Busy>> {
        let path = path_in(directory, id);
        // Held for the whole decision, so this process never takes a second
        // lock of a board it holds, nor mistakes its own lock for an
        // ancestor's.
        let mut registry = registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(owned) = registry.get(&path).and_then(Weak::upgrade) {
            return Ok(Ok(Self::Owned(DeviceGuard { owned })));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(token) = inherited(&path)? {
            let holder = holder_at(&path)?.ok_or("device lock no longer has an owner")?;
            if holder.token != token {
                return Err("device capability no longer matches its owner".into());
            }
            let delegate = DelegatedDevice {
                id: id.clone(),
                path: path.clone(),
                token,
                broker_pid: holder.broker_pid,
            };
            let _operation = LockBroker::operation(&path, &delegate.token, delegate.broker_pid)?;
            return Ok(Ok(Self::Delegated(delegate)));
        }
        match FileLock::try_acquire(&path, Mode::Exclusive)? {
            Some(lock) => {
                let token = token();
                let mut record_file = lock.file().try_clone()?;
                let broker = LockBroker::start(lock, &token)?;
                record(&mut record_file, command, &token, broker.pid())?;
                let owned = Arc::new(Owned {
                    id: id.clone(),
                    path: path.clone(),
                    broker: Some(broker),
                    token,
                });
                registry.insert(path, Arc::downgrade(&owned));
                Ok(Ok(Self::Owned(DeviceGuard { owned })))
            }
            None => Ok(Err(Busy::Held(holder_at(&path)?))),
        }
    }

    /// The board `id` for `command`; a busy board is an error naming its
    /// holder.
    pub fn acquire(id: &DeviceId, command: &str) -> crate::lock::Result<Self> {
        Self::try_acquire(id, command)?
            .map_err(|busy| format!("device {id} is busy: {busy}").into())
    }

    /// The board `id` for `command`, waiting while another process holds
    /// it; the holder is printed once when the wait starts.
    pub fn wait(id: &DeviceId, command: &str) -> crate::lock::Result<Self> {
        let mut announced = false;
        loop {
            match Self::try_acquire(id, command)? {
                Ok(access) => return Ok(access),
                Err(busy) if !announced => {
                    eprintln!("device {id} is busy ({busy}); waiting");
                    announced = true;
                }
                Err(_) => {}
            }
            oer_os_process::sleep(Duration::from_millis(250))?;
        }
    }

    /// The board.
    pub fn id(&self) -> &DeviceId {
        match self {
            Self::Owned(guard) => guard.id(),
            Self::Delegated(delegate) => delegate.id(),
        }
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        match self {
            Self::Owned(guard) => guard.path(),
            Self::Delegated(delegate) => delegate.path(),
        }
    }

    /// Whether an ancestor holds the lock and this process uses it as its
    /// delegate.
    pub fn is_delegated(&self) -> bool {
        matches!(self, Self::Delegated(_))
    }

    /// Admit I/O while its owner lives and retain exclusion through that I/O.
    /// The guard must outlive ports, resets, flashes and console readers.
    pub fn operation(&self) -> crate::lock::Result<DeviceOperation> {
        let (token, broker_pid, owner) = match self {
            Self::Owned(guard) => (
                &guard.owned.token,
                guard
                    .owned
                    .broker
                    .as_ref()
                    .expect("a live owner holds its broker")
                    .pid(),
                Some(guard.clone()),
            ),
            Self::Delegated(delegate) => (&delegate.token, delegate.broker_pid, None),
        };
        let broker = LockBroker::operation(self.path(), token, broker_pid)
            .map_err(|error| format!("board {}: {error}", self.id()))?;
        Ok(DeviceOperation {
            _broker: broker,
            _owner: owner,
        })
    }

    /// Add only this board's capability to the intended child's context.
    pub fn delegate(&self, context: &mut Context) -> crate::lock::Result<()> {
        let _operation = self.operation()?;
        let token = match self {
            Self::Owned(guard) => &guard.owned.token,
            Self::Delegated(delegate) => &delegate.token,
        };
        context.set(context_key(self.path())?, token);
        Ok(())
    }

    /// Fail unless this access is the board `id`'s.
    pub fn ensure_covers(&self, id: &DeviceId) -> crate::lock::Result<()> {
        if self.id() == id {
            Ok(())
        } else {
            Err(format!("the lock of board {} does not cover board {id}", self.id()).into())
        }
    }
}

/// Write this process's [`Holder`] record after its broker acknowledges startup.
fn record(
    file: &mut std::fs::File,
    command: &str,
    token: &str,
    broker_pid: u32,
) -> crate::lock::Result<()> {
    let holder = Holder {
        pid: std::process::id(),
        owner_started_ticks: oer_os_process::proc::start_ticks(std::process::id())
            .ok_or("cannot identify device lock owner in /proc")?,
        broker_pid,
        command: command.to_owned(),
        started_unix_millis: oer_os_durable::unix_millis(),
        token: token.to_owned(),
    };
    file.set_len(0)?;
    file.rewind()?;
    file.write_all(&serde_json::to_vec(&holder)?)?;
    file.flush()?;
    Ok(())
}

/// Who holds the lock of board `id`, or `None` when it is free.
pub fn holder(id: &DeviceId) -> crate::lock::Result<Option<Holder>> {
    holder_at(&path(id)?)
}

/// The holder record of a held lock at `path`, read strictly: a holder that
/// has not written its record yet is `None`, a record that does not parse
/// is an error (it decides delegation).
fn holder_at(path: &Path) -> crate::lock::Result<Option<Holder>> {
    if !path.exists() {
        return Ok(None);
    }
    // A free lock has no holder, whatever an earlier holder left in it.
    if FileLock::try_acquire(path, Mode::Shared)?.is_some() {
        return Ok(None);
    }
    let mut text = String::new();
    std::fs::File::open(path)?.read_to_string(&mut text)?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&text).map_err(|error| {
        format!("{}: unreadable holder record: {error}", path.display())
    })?))
}

/// Who holds the lock of board `id` when this process may not use it: held,
/// neither by this process nor by the ancestor that delegated to it.
/// `Some(None)` is a holder that wrote no record yet.
pub fn foreign_holder(id: &DeviceId) -> crate::lock::Result<Option<Option<Holder>>> {
    let path = path(id)?;
    if registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&path)
        .is_some_and(|weak| weak.strong_count() > 0)
    {
        return Ok(None);
    }
    if !is_held(id)? {
        return Ok(None);
    }
    Ok(match holder(id)? {
        Some(holder)
            if inherited(&path)?.as_deref() == Some(holder.token.as_str())
                && LockBroker::operation(&path, &holder.token, holder.broker_pid).is_ok() =>
        {
            None
        }
        holder => Some(holder),
    })
}

/// Whether the board `id` is locked by any process.
pub fn is_held(id: &DeviceId) -> crate::lock::Result<bool> {
    let path = path(id)?;
    Ok(path.exists() && FileLock::try_acquire(&path, Mode::Shared)?.is_none())
}
