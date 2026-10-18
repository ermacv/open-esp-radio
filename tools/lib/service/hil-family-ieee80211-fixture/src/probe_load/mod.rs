//! Event-driven ownership of the bounded Linux probe source.
mod air;
use crate::Result;
pub use air::verify as verify_air;
pub use oer_hil_fixture::probe::model;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

pub struct Source {
    child: oer_process::owned::Child,
    events: Receiver<std::io::Result<String>>,
    report: PathBuf,
}
impl Source {
    pub fn prepare(config: &model::Config, output: &Path) -> Result<Self> {
        let mut command = oer_process::command("sudo");
        command.args(["-n", "/usr/local/libexec/open-radio-probe"]);
        Self::prepare_command(&mut command, config, output)
    }
    fn prepare_command(
        command: &mut Command,
        config: &model::Config,
        output: &Path,
    ) -> Result<Self> {
        config.validate()?;
        fs::create_dir_all(output)?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(output.join("probe-source.stderr"))?);
        let mut child =
            oer_process::owned::Child::spawn_with_shutdown_grace(command, Duration::from_secs(7))?
                .with_timeout(Duration::from_secs(45));
        let stdout = child.take_stdout().ok_or("probe source stdout missing")?;
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            // The complete protocol is two short lines, bounded even for a broken helper.
            for line in BufReader::new(stdout.take(4096)).lines().take(3) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut owner = Self {
            child,
            events,
            report: output.join("probe-source.json"),
        };
        let stdin = owner
            .child
            .stdin
            .as_mut()
            .ok_or("probe source stdin missing")?;
        serde_json::to_writer(&mut *stdin, config)?;
        writeln!(stdin)?;
        stdin.flush()?;
        if owner.events.recv_timeout(Duration::from_secs(10))?? != model::READY {
            return Err("probe source did not acknowledge readiness".into());
        }
        Ok(owner)
    }
    pub fn start(&mut self) -> Result<()> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or("probe source stdin missing")?;
        writeln!(stdin, "start")?;
        stdin.flush()?;
        Ok(())
    }
    pub fn finish(mut self) -> Result<()> {
        let line = self.events.recv_timeout(Duration::from_secs(10))??;
        let report: model::Report = serde_json::from_str(&line)?;
        fs::write(&self.report, serde_json::to_vec_pretty(&report)?)?;
        self.child.stdin.take();
        let status = self.child.wait_timeout(Some(Duration::from_secs(5)))?;
        report.validate()?;
        if !status.success() {
            return Err("probe source exited unsuccessfully".into());
        }
        Ok(())
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        // EOF cancels an armed/running source. Its owned process group is the
        // bounded fallback if it fails to observe that event or to clean up.
        self.child.stdin.take();
        oer_hil_workload::fixture::cleanup::record("wait for probe source cleanup", || {
            // Closing stdin is the cooperative stop request. Do not race the
            // helper's cleanup children with the process owner's SIGTERM.
            // The owned child still terminates the group on this deadline.
            self.child.wait_timeout(Some(Duration::from_secs(7)))?;
            Ok(())
        });
    }
}
#[cfg(test)]
mod tests;
