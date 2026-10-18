//! One process at a time per stand router: an exclusive `flock` on
//! `<directory>/router-<key>.lock`, whose key is a hash of the SSH target,
//! so the file name never shows the target. The holder writes its pid and
//! command line (which carries no stand value) into the file, so a refused
//! process can name it.

use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

use oer_tool_base_process::lock::{FileLock, Mode};
use sha2::{Digest as _, Sha256};

use crate::Result;

/// The lock file of the router `target` in `directory`.
pub fn router_path(directory: &Path, target: &str) -> PathBuf {
    let digest = Sha256::digest(target.as_bytes());
    let key = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    directory.join(format!("router-{key}.lock"))
}

/// The held lock of the stand router.
#[derive(Debug)]
pub struct RouterLock {
    _lock: FileLock,
}

/// Take the lock of the router `target` in `directory`, or fail naming its
/// holder.
pub fn router(directory: &Path, target: &str) -> Result<RouterLock> {
    let path = router_path(directory, target);
    let Some(lock) = FileLock::try_acquire(&path, Mode::Exclusive)? else {
        let holder = std::fs::read_to_string(&path).unwrap_or_default();
        let holder = holder.trim();
        return Err(format!(
            "the stand router is busy: {} holds {}",
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
    Ok(RouterLock { _lock: lock })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "made-up-router.lab";

    #[test]
    fn a_second_holder_is_refused_with_the_first_named() {
        let directory = tempfile::tempdir().unwrap();
        let first = router(directory.path(), TARGET).unwrap();
        // `flock` excludes another open file description, even in this process.
        let error = router(directory.path(), TARGET).unwrap_err().to_string();
        assert!(
            error.contains(&format!("pid {}", std::process::id())),
            "{error}"
        );
        assert!(!error.contains("made-up"), "{error}");
        // Another router is another lock.
        let other = router(directory.path(), "other-made-up-router").unwrap();
        drop((first, other));
        router(directory.path(), TARGET).unwrap();
    }

    #[test]
    fn the_file_name_does_not_show_the_target() {
        let path = router_path(Path::new("/run/open-esp-radio/routers"), TARGET);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("router-") && name.ends_with(".lock"),
            "{name}"
        );
        assert_eq!(name.len(), "router-".len() + 16 + ".lock".len());
        assert!(!name.contains("made-up"));
        assert_eq!(
            path,
            router_path(Path::new("/run/open-esp-radio/routers"), TARGET)
        );
        assert_ne!(
            path,
            router_path(Path::new("/run/open-esp-radio/routers"), "made-up-other")
        );
    }
}
