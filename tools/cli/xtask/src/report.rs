//! Bounded command output for agents and people alike.
//!
//! The repository checks and `push` print one line per step. The output of
//! the commands they run (Cargo, rustc, the tests) goes to a log under
//! `target/xtask/logs/`; on failure the terminal gets the first diagnostics
//! of that log and its path, so a verdict never hides behind 100 KB of
//! build output. `--verbose` streams everything instead.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use crate::Result;

/// Diagnostic lines a failure shows at most.
pub const DIGEST_LINES: usize = 60;

/// Logs older than this are removed when a new one starts.
const KEEP: Duration = Duration::from_secs(3 * 24 * 3600);

/// The log of one command.
pub struct Log {
    pub path: PathBuf,
}

impl Log {
    /// Starts logging child output of the command `name` under `root`.
    pub fn start(root: &Path, name: &str) -> Result<Self> {
        let directory = root.join("target/xtask/logs");
        fs::create_dir_all(&directory)?;
        let now = SystemTime::now();
        for entry in fs::read_dir(&directory)?.flatten() {
            if entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > KEEP)
            {
                let _ = fs::remove_file(entry.path());
            }
        }
        let stamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        let path = directory.join(format!("{name}-{stamp}-{}.log", std::process::id()));
        oer_process::log_output_to(Some(fs::File::create(&path)?));
        Ok(Self { path })
    }

    /// The failure digest of the log so far.
    pub fn digest(&self) -> String {
        let text = fs::read_to_string(&self.path).unwrap_or_default();
        digest(&text, DIGEST_LINES).join("\n")
    }
}

impl Drop for Log {
    fn drop(&mut self) {
        oer_process::log_output_to(None);
    }
}

/// Whether `line` opens a diagnostic block: a compiler error, a failed
/// test's output, a panic or a failed test summary.
fn opens(line: &str) -> bool {
    line.starts_with("error")
        || line.starts_with("---- ")
        || line.starts_with("test result: FAILED")
        || line.starts_with("Diff in ")
        || (line.starts_with("thread '") && line.contains("panicked"))
}

/// The diagnostic blocks of `text`, each up to its next blank line, at most
/// `limit` lines in all; the last lines when nothing looks like a
/// diagnostic.
pub fn digest(text: &str, limit: usize) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let mut picked = Vec::new();
    let mut index = 0;
    while index < lines.len() && picked.len() < limit {
        if opens(lines[index]) {
            while index < lines.len() && !lines[index].trim().is_empty() && picked.len() < limit {
                picked.push(lines[index]);
                index += 1;
            }
            if picked.len() < limit {
                picked.push("");
            }
        }
        index += 1;
    }
    if picked.is_empty() {
        let start = lines.len().saturating_sub(limit.min(20));
        return lines[start..].to_vec();
    }
    while picked.last() == Some(&"") {
        picked.pop();
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_keeps_the_diagnostics_and_drops_the_progress() {
        let log = "   Compiling a v0.1.0\n   Compiling b v0.1.0\nerror[E0425]: cannot find value `x`\n --> src/lib.rs:1:1\n  |\n\nwarning: build failed\n---- tests::one stdout ----\nthread 'tests::one' panicked at src/lib.rs:9:5:\nassertion failed\n\ntest result: FAILED. 1 passed; 1 failed\n";
        assert_eq!(
            digest(log, 60),
            [
                "error[E0425]: cannot find value `x`",
                " --> src/lib.rs:1:1",
                "  |",
                "",
                "---- tests::one stdout ----",
                "thread 'tests::one' panicked at src/lib.rs:9:5:",
                "assertion failed",
                "",
                "test result: FAILED. 1 passed; 1 failed",
            ]
        );
        assert_eq!(
            digest(log, 2),
            ["error[E0425]: cannot find value `x`", " --> src/lib.rs:1:1"]
        );
    }

    #[test]
    fn without_diagnostics_the_digest_is_the_tail() {
        let log: String = (0..100).map(|n| format!("line {n}\n")).collect();
        let tail = digest(&log, 60);
        assert_eq!(tail.len(), 20);
        assert_eq!(tail[19], "line 99");
    }
}
