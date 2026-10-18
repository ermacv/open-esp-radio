//! Unprivileged owner of the installed helper's bounded ATT-parameter lease.
use super::model::Adapter;
use crate::Result;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
};
use std::{
    fs::File,
    io::Read,
    os::fd::AsFd,
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

pub fn require_clean(adapter: Adapter) -> Result<()> {
    let journal = format!("/run/open-radio-bluetooth/{adapter}.att-parameters.json");
    if Path::new(&journal).try_exists()? {
        return Err(format!("unfinished ATT parameter lease: {journal}; recover using sudo /usr/local/libexec/open-radio-bluetooth restore-att-parameters --adapter {adapter}").into());
    }
    Ok(())
}

pub fn preflight(adapter: Adapter) -> Result<()> {
    super::att::preflight(adapter)?;
    super::preflight(adapter)?;
    let output = oer_process::output(
        oer_process::command("sudo").args([
            "-n",
            "-l",
            "/usr/local/libexec/open-radio-bluetooth",
            "att-parameters",
            "--adapter",
            &adapter.to_string(),
        ]),
        Some(Duration::from_secs(5)),
    )?;
    if !output.status.success() {
        return Err("install the ATT parameter helper: cargo stand fixture install --provider linux-bluetooth".into());
    }
    Ok(())
}

pub struct Lease {
    child: Option<oer_process::owned::Child>,
    failure: Option<String>,
}
impl Lease {
    pub fn acquire(adapter: Adapter, output: &Path) -> Result<Self> {
        preflight(adapter)?;
        let mut command = oer_process::command("sudo");
        command
            .args([
                "-n",
                "/usr/local/libexec/open-radio-bluetooth",
                "att-parameters",
                "--adapter",
                &adapter.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(File::create(output.join("att-parameters.stderr"))?);
        let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
            &mut command,
            Duration::from_secs(20),
        )?;
        expect_line(
            child.stdout.as_mut().ok_or("ATT helper stdout absent")?,
            b"ATT_PARAMETERS_READY_V1\n",
        )?;
        Ok(Self {
            child: Some(child),
            failure: None,
        })
    }

    pub fn restore(&mut self) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone().into());
        }
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        drop(child.stdin.take()); // EOF transfers cleanup responsibility back to root.
        let result = (|| -> Result<()> {
            expect_line(
                child.stdout.as_mut().ok_or("ATT helper stdout absent")?,
                b"ATT_PARAMETERS_RESTORED_V1\n",
            )?;
            if !child.wait_timeout(Some(Duration::from_secs(20)))?.success() {
                return Err("ATT parameter restoration helper failed".into());
            }
            Ok(())
        })();
        if let Err(error) = &result {
            self.failure = Some(error.to_string());
        }
        result
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Err(error) = oer_process::cleanup(|| self.restore()) {
            eprintln!("ATT parameter cleanup: {error}");
        }
    }
}

fn expect_line(pipe: &mut (impl Read + AsFd), expected: &[u8]) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    for expected_byte in expected {
        loop {
            oer_process::check_cancelled()?;
            if Instant::now() >= deadline {
                return Err("ATT helper handshake timeout".into());
            }
            let timeout = Timespec::try_from(Duration::from_millis(100))?;
            match poll(&mut [PollFd::new(pipe, PollFlags::IN)], Some(&timeout)) {
                Ok(0) | Err(Errno::INTR) => continue,
                Ok(_) => {}
                Err(error) => return Err(error.into()),
            }
            let mut byte = [0];
            if pipe.read(&mut byte)? != 1 || byte[0] != *expected_byte {
                return Err("ATT helper handshake failed; inspect att-parameters.stderr and recovery journal".into());
            }
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, os::unix::net::UnixStream};
    #[test]
    fn handshake_requires_exact_complete_message() {
        for (message, valid) in [
            (b"READY\n".as_slice(), true),
            (b"READY", false),
            (b"ready\n", false),
        ] {
            let (mut receiver, mut sender) = UnixStream::pair().unwrap();
            sender.write_all(message).unwrap();
            drop(sender);
            assert_eq!(expect_line(&mut receiver, b"READY\n").is_ok(), valid);
        }
    }
}
