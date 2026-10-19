//! Advisory file locks: the one lock primitive of every repository tool.
//!
//! A [`FileLock`] is the kernel's `flock` on one file, shared or exclusive,
//! released when its owner exits, however it exits, so a crashed owner never
//! leaves a stale lock. Shared locks of one file coexist; an exclusive lock
//! excludes every other lock of it. A held lock converts between the modes
//! in place ([`FileLock::convert`]).
//!
//! Closing one descriptor does not release `flock` while a forked child
//! still holds the open file description until its exec, so a lock is
//! released explicitly when it is dropped, failures included.

use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

use rustix::fs::FlockOperation;

use crate::Result;

#[cfg(target_os = "linux")]
mod broker;
#[cfg(target_os = "linux")]
pub use broker::{BrokerOperation, LockBroker};

/// How a lock shares its file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// Any number of shared holders, no exclusive one.
    Shared,
    /// One holder and nobody else.
    Exclusive,
}

impl Mode {
    fn blocking(self) -> FlockOperation {
        match self {
            Self::Shared => FlockOperation::LockShared,
            Self::Exclusive => FlockOperation::LockExclusive,
        }
    }

    fn nonblocking(self) -> FlockOperation {
        match self {
            Self::Shared => FlockOperation::NonBlockingLockShared,
            Self::Exclusive => FlockOperation::NonBlockingLockExclusive,
        }
    }
}

/// A held lock on one file, released when dropped.
#[derive(Debug)]
pub struct FileLock {
    /// `None` only once [`Self::into_file`] handed the descriptor over.
    file: Option<File>,
    path: PathBuf,
    mode: Mode,
}

impl FileLock {
    /// Open (creating it and its directory) the lock file at `path`.
    fn open(path: &Path) -> Result<File> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| format!("open lock {}: {error}", path.display()).into())
    }

    /// The lock of `path` in `mode`, or `None` while a conflicting lock is
    /// held.
    pub fn try_acquire(path: &Path, mode: Mode) -> Result<Option<Self>> {
        Self::try_lock(Self::open(path)?, path, mode)
    }

    /// The lock of `path` in `mode`, blocking until conflicting holders
    /// release it. The wait is not cancellable; [`Self::wait`] is.
    pub fn acquire(path: &Path, mode: Mode) -> Result<Self> {
        Self::lock(Self::open(path)?, path, mode)
    }

    /// Lock `file`, which the caller opened (with its own flags, such as a
    /// privileged helper's `O_NOFOLLOW` and mode, or an inherited
    /// descriptor), as the lock file `path`; `None` while a conflicting lock
    /// is held.
    pub fn try_lock(file: File, path: &Path, mode: Mode) -> Result<Option<Self>> {
        match rustix::fs::flock(&file, mode.nonblocking()) {
            Ok(()) => Ok(Some(Self {
                file: Some(file),
                path: path.to_owned(),
                mode,
            })),
            Err(rustix::io::Errno::WOULDBLOCK) => Ok(None),
            Err(error) => Err(format!("lock {}: {error}", path.display()).into()),
        }
    }

    /// Lock `file` as [`Self::try_lock`], blocking until conflicting
    /// holders release it.
    pub fn lock(file: File, path: &Path, mode: Mode) -> Result<Self> {
        rustix::fs::flock(&file, mode.blocking())
            .map_err(|error| format!("lock {}: {error}", path.display()))?;
        Ok(Self {
            file: Some(file),
            path: path.to_owned(),
            mode,
        })
    }

    /// The lock of `path` in `mode`, polling until it is free or this
    /// process is cancelled; `waiting` is printed once when the wait starts.
    pub fn wait(path: &Path, mode: Mode, waiting: &str) -> Result<Self> {
        Self::wait_any(&[path.to_owned()], mode, waiting)
    }

    /// The first free lock of `paths` in `mode`, as [`Self::wait`].
    pub fn wait_any(paths: &[PathBuf], mode: Mode, waiting: &str) -> Result<Self> {
        let mut announced = false;
        loop {
            for path in paths {
                if let Some(lock) = Self::try_acquire(path, mode)? {
                    return Ok(lock);
                }
            }
            if !announced {
                eprintln!("{waiting}");
                announced = true;
            }
            crate::sleep(Duration::from_millis(200))?;
        }
    }

    /// Convert the held lock to `mode` in place, blocking while another
    /// holder conflicts with the new mode. `flock` may release the old lock
    /// before granting the new one, so another process can take the file in
    /// between: an owner that needs the state it established must not assume
    /// it survived an upgrade.
    pub fn convert(&mut self, mode: Mode) -> Result<()> {
        rustix::fs::flock(self.held(), mode.blocking())
            .map_err(|error| format!("lock {}: {error}", self.path.display()))?;
        self.mode = mode;
        Ok(())
    }

    /// The mode held.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The lock file, for an owner record written into it.
    pub fn file(&self) -> &File {
        self.held()
    }

    /// The lock file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The descriptor with the lock still held: the lock then lasts as long
    /// as its open file description, for a descriptor handed to a program
    /// this process executes.
    pub fn into_file(mut self) -> File {
        self.file
            .take()
            .expect("a lock holds its file until dropped")
    }

    fn held(&self) -> &File {
        self.file
            .as_ref()
            .expect("a lock holds its file until dropped")
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if let Some(file) = &self.file
            && let Err(error) = rustix::fs::flock(file, FlockOperation::Unlock)
        {
            eprintln!("release lock {}: {error}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_locks_coexist_and_exclude_an_exclusive_one() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/file.lock");
        let first = FileLock::try_acquire(&path, Mode::Shared).unwrap().unwrap();
        let second = FileLock::try_acquire(&path, Mode::Shared).unwrap().unwrap();
        assert_eq!(first.mode(), Mode::Shared);
        assert!(
            FileLock::try_acquire(&path, Mode::Exclusive)
                .unwrap()
                .is_none()
        );
        drop(first);
        assert!(
            FileLock::try_acquire(&path, Mode::Exclusive)
                .unwrap()
                .is_none()
        );
        drop(second);
        let exclusive = FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .unwrap();
        assert!(
            FileLock::try_acquire(&path, Mode::Shared)
                .unwrap()
                .is_none()
        );
        assert!(
            FileLock::try_acquire(&path, Mode::Exclusive)
                .unwrap()
                .is_none()
        );
        drop(exclusive);
        assert!(
            FileLock::try_acquire(&path, Mode::Shared)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn an_exclusive_lock_downgraded_admits_shared_holders() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.lock");
        let mut lock = FileLock::acquire(&path, Mode::Exclusive).unwrap();
        assert!(
            FileLock::try_acquire(&path, Mode::Shared)
                .unwrap()
                .is_none()
        );
        lock.convert(Mode::Shared).unwrap();
        assert_eq!(lock.mode(), Mode::Shared);
        let shared = FileLock::try_acquire(&path, Mode::Shared).unwrap();
        assert!(shared.is_some());
        assert!(
            FileLock::try_acquire(&path, Mode::Exclusive)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_dropped_lock_is_released_while_a_duplicated_descriptor_lives() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.lock");
        let lock = FileLock::acquire(&path, Mode::Exclusive).unwrap();
        // dup shares the open file description as inheritance across fork
        // does; releasing the owner must not depend on it closing.
        let inherited = lock.file().try_clone().unwrap();
        drop(lock);
        assert!(
            FileLock::try_acquire(&path, Mode::Exclusive)
                .unwrap()
                .is_some()
        );
        drop(inherited);
    }

    #[test]
    fn a_wait_takes_the_first_free_lock() {
        let directory = tempfile::tempdir().unwrap();
        let paths = [0, 1].map(|slot| directory.path().join(format!("{slot}.lock")));
        let _held = FileLock::try_acquire(&paths[0], Mode::Exclusive)
            .unwrap()
            .unwrap();
        let lock = FileLock::wait_any(&paths, Mode::Exclusive, "waiting").unwrap();
        assert_eq!(lock.path(), paths[1]);
    }

    #[test]
    fn a_lock_is_released_when_its_holder_drops_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.lock");
        let held = FileLock::acquire(&path, Mode::Exclusive).unwrap();
        let waiter = {
            let path = path.clone();
            std::thread::spawn(move || FileLock::acquire(&path, Mode::Exclusive).map(drop))
        };
        std::thread::sleep(Duration::from_millis(50));
        assert!(!waiter.is_finished());
        drop(held);
        waiter.join().unwrap().unwrap();
    }
}
