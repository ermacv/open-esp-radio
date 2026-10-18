//! One process at a time per board: an exclusive `flock` on
//! `$XDG_RUNTIME_DIR/oer-fw/<MAC>.lock`, shared by every checkout of the
//! repository. The holder writes its pid and command into the file, so a
//! refused process can name it. No argument of this tool carries a stand
//! secret, so the command is written as given.

use std::{io::Write as _, path::PathBuf};

use oer_process::lock::{FileLock, Mode};

use crate::Result;

/// The lock directory: `$XDG_RUNTIME_DIR/oer-fw`.
pub fn directory() -> Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|runtime| !runtime.is_empty())
        .ok_or("XDG_RUNTIME_DIR is not set: the board locks live there")?;
    Ok(PathBuf::from(runtime).join("oer-fw"))
}

/// The lock file of the board `mac`.
pub fn path_in(directory: &std::path::Path, mac: &str) -> PathBuf {
    directory.join(format!("{}.lock", mac.replace(':', "").to_uppercase()))
}

/// The held lock of one board.
#[derive(Debug)]
pub struct BoardLock {
    _lock: FileLock,
}

/// Take board `mac`'s lock, or fail naming its holder.
pub fn acquire(mac: &str) -> Result<BoardLock> {
    acquire_in(&directory()?, mac)
}

pub fn acquire_in(directory: &std::path::Path, mac: &str) -> Result<BoardLock> {
    let path = path_in(directory, mac);
    let Some(lock) = FileLock::try_acquire(&path, Mode::Exclusive)? else {
        let holder = std::fs::read_to_string(&path).unwrap_or_default();
        let holder = holder.trim();
        return Err(format!(
            "board {mac} is busy: {} holds {}",
            if holder.is_empty() {
                "another process"
            } else {
                holder
            },
            path.display()
        )
        .into());
    };
    let mut file = lock.file();
    file.set_len(0)?;
    let command = std::env::args().collect::<Vec<_>>().join(" ");
    writeln!(file, "pid {} ({command})", std::process::id())?;
    Ok(BoardLock { _lock: lock })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_is_refused_with_the_first_named() {
        let directory = tempfile::tempdir().unwrap();
        let first = acquire_in(directory.path(), "02:00:00:00:01:d0").unwrap();
        let path = path_in(directory.path(), "02:00:00:00:01:D0");
        assert!(path.ends_with("0200000001D0.lock"));
        // `flock` excludes another open file description, even in this process.
        let error = acquire_in(directory.path(), "02:00:00:00:01:D0").unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("pid {}", std::process::id())),
            "{error}"
        );
        drop(first);
        acquire_in(directory.path(), "02:00:00:00:01:D0").unwrap();
    }
}
