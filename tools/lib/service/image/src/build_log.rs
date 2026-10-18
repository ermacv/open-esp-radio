//! The log of an image build: each step's standard error, kept beside the
//! bundle so a failure's cause survives the caller's terminal.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::Result;

/// The log of one image build: every step's standard error, as the terminal
/// also shows it, so a failure's cause survives the caller's terminal.
pub struct BuildLog {
    path: PathBuf,
}

/// A build step that failed, with the line that says why when one does, and
/// the build log holding its whole output.
#[derive(Debug)]
pub struct BuildStepFailed {
    pub step: String,
    pub cause: String,
    pub log: PathBuf,
}

impl std::fmt::Display for BuildStepFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.step, self.cause)
    }
}

impl Error for BuildStepFailed {}

impl BuildLog {
    /// Start an empty log at `path`.
    pub fn create(path: &Path) -> Result<Self> {
        fs::write(path, b"")?;
        Ok(Self {
            path: path.to_owned(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn append(&self, text: &str) -> Result<()> {
        use std::io::Write as _;
        let mut file = fs::OpenOptions::new().append(true).open(&self.path)?;
        file.write_all(text.as_bytes())?;
        Ok(())
    }

    /// Run `command` as the step `description`, copying its standard error
    /// to the terminal and the log.
    pub fn run(&self, command: &mut Command, description: &str) -> Result<()> {
        use std::io::BufRead as _;
        eprintln!("==> {description}");
        self.append(&format!("==> {description}\n"))?;
        command.stderr(Stdio::piped());
        let mut child = oer_process::owned::Child::spawn(command)?;
        let stderr = child
            .take_stderr()
            .ok_or("the build step has no standard error")?;
        let log = self.path.clone();
        let copier = std::thread::spawn(move || -> Vec<String> {
            use std::io::Write as _;
            let mut file = fs::OpenOptions::new().append(true).open(&log).ok();
            let mut lines = Vec::new();
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(std::io::Result::ok)
            {
                eprintln!("{line}");
                if let Some(file) = file.as_mut() {
                    let _ = writeln!(file, "{line}");
                }
                lines.push(line);
            }
            lines
        });
        let status = child.wait_timeout(Some(std::time::Duration::from_secs(30 * 60)))?;
        let lines = copier.join().unwrap_or_default();
        if !status.success() {
            return Err(BuildStepFailed {
                step: description.to_owned(),
                cause: decisive_line(&lines).unwrap_or_else(|| format!("exit {status}")),
                log: self.path.clone(),
            }
            .into());
        }
        Ok(())
    }

    /// Record an in-process step's failure in the log and name the log.
    pub fn failed(
        &self,
        step: &str,
        error: Box<dyn Error + Send + Sync>,
    ) -> Box<dyn Error + Send + Sync> {
        let _ = self.append(&format!("==> {step}\n{error}\n"));
        BuildStepFailed {
            step: step.to_owned(),
            cause: error.to_string(),
            log: self.path.clone(),
        }
        .into()
    }
}

/// The line of a failed step's standard error that says why: an espflash
/// error id, else the last `error` line, else the last non-empty line.
pub fn decisive_line(lines: &[String]) -> Option<String> {
    let trimmed = || {
        lines
            .iter()
            .rev()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
    };
    trimmed()
        .find(|line| line.contains("espflash::"))
        .or_else(|| {
            trimmed().find(|line| {
                let lower = line.to_ascii_lowercase();
                lower.starts_with("error") || lower.contains(" error:") || lower.contains("× ")
            })
        })
        .or_else(|| trimmed().next())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_decisive_line_prefers_the_espflash_error_id() {
        let lines = [
            "   Compiling x",
            "Error: espflash::image_too_big",
            "  × Image size 4210448B exceeds partition size 4194304B",
            "",
        ]
        .map(String::from);
        assert_eq!(
            decisive_line(&lines).as_deref(),
            Some("Error: espflash::image_too_big")
        );
        let cargo = ["warning: x", "error: could not compile `y`", "  note"].map(String::from);
        assert_eq!(
            decisive_line(&cargo).as_deref(),
            Some("error: could not compile `y`")
        );
        assert_eq!(decisive_line(&["tail".into()]).as_deref(), Some("tail"));
        assert_eq!(decisive_line(&[]), None);
    }

    #[test]
    fn a_failed_step_keeps_its_output_in_the_log() {
        let directory = tempfile::tempdir().unwrap();
        let log = BuildLog::create(&directory.path().join("build.log")).unwrap();
        let error = log
            .run(
                oer_process::command("sh")
                    .args(["-c", "echo noise >&2; echo 'error: broken' >&2; exit 3"]),
                "a step",
            )
            .unwrap_err();
        let failed = error.downcast_ref::<BuildStepFailed>().unwrap();
        assert_eq!(failed.cause, "error: broken");
        let text = fs::read_to_string(log.path()).unwrap();
        assert!(
            text.contains("==> a step\nnoise\nerror: broken\n"),
            "{text}"
        );
    }
}
