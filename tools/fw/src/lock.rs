//! One process at a time per board: an exclusive `flock` on
//! `$XDG_RUNTIME_DIR/oer-fw/<MAC>.lock`, shared by every checkout of the
//! repository. The holder writes its pid and command into the file, so a
//! refused process can name it; the value of `--expect-bssid` is masked
//! there.

use std::{io::Write as _, path::PathBuf};

use oer_process::lock::{FileLock, Mode};

use crate::{Result, w1};

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
    let command = masked_command(&std::env::args().collect::<Vec<_>>());
    writeln!(file, "pid {} ({command})", std::process::id())?;
    Ok(BoardLock { _lock: lock })
}

/// The flag whose value the lock file never shows whole.
const MASKED_FLAG: &str = "--expect-bssid";

/// `value` masked: the last two octets of a BSSID, or nothing of a value
/// that is none.
fn mask(value: &str) -> String {
    w1::parse_bssid(value).map_or_else(|_| "xx:xx:xx:xx:xx:xx".to_owned(), |bssid| bssid.masked())
}

/// The command line `args` with the value after `--expect-bssid` (or in
/// `--expect-bssid=<value>`) masked.
pub fn masked_command(args: &[String]) -> String {
    let mut shown = Vec::with_capacity(args.len());
    let mut mask_next = false;
    for arg in args {
        if mask_next {
            shown.push(mask(arg));
            mask_next = false;
        } else if arg == MASKED_FLAG {
            shown.push(arg.clone());
            mask_next = true;
        } else if let Some(value) = arg
            .strip_prefix(MASKED_FLAG)
            .and_then(|rest| rest.strip_prefix('='))
        {
            shown.push(format!("{MASKED_FLAG}={}", mask(value)));
        } else {
            shown.push(arg.clone());
        }
    }
    shown.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn the_expected_bssid_is_masked_in_the_command() {
        assert_eq!(
            masked_command(&args(
                "fw run w1 --board b1 --expect-bssid 02:ab:cd:ef:f6:d0 --channel 6"
            )),
            "fw run w1 --board b1 --expect-bssid xx:xx:xx:xx:F6:D0 --channel 6"
        );
        assert_eq!(
            masked_command(&args("fw run w1 --expect-bssid=02-AB-CD-EF-F6-D0")),
            "fw run w1 --expect-bssid=xx:xx:xx:xx:F6:D0"
        );
        // A value that is no BSSID shows nothing of itself.
        assert_eq!(
            masked_command(&args("fw run w1 --expect-bssid 02:AB:CD:EF:F6")),
            "fw run w1 --expect-bssid xx:xx:xx:xx:xx:xx"
        );
        assert_eq!(
            masked_command(&args("fw run w1 --expect-bssid")),
            "fw run w1 --expect-bssid"
        );
        assert_eq!(
            masked_command(&args("fw run w0 --board 02:00:00:00:00:01")),
            "fw run w0 --board 02:00:00:00:00:01"
        );
    }

    #[test]
    fn a_second_holder_is_refused_with_the_first_named() {
        let directory = tempfile::tempdir().unwrap();
        let first = acquire_in(directory.path(), "30:ed:a0:f3:f6:d0").unwrap();
        let path = path_in(directory.path(), "30:ED:A0:F3:F6:D0");
        assert!(path.ends_with("30EDA0F3F6D0.lock"));
        // `flock` excludes another open file description, even in this process.
        let error = acquire_in(directory.path(), "30:ED:A0:F3:F6:D0").unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("pid {}", std::process::id())),
            "{error}"
        );
        drop(first);
        acquire_in(directory.path(), "30:ED:A0:F3:F6:D0").unwrap();
    }
}
