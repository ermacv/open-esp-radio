//! The one way to the stand router: non-interactive SSH running a script
//! that goes over stdin, so no value of the stand file but the target
//! itself is ever in an argument list (local or remote `ps`).

use std::{fmt, io::Write as _, process::Stdio, time::Duration};

pub type Result<T> = std::result::Result<T, String>;

/// The longest one script may take, connection included.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs one script on the router.
pub trait Remote {
    /// `script`'s standard output when it exits 0; otherwise why not,
    /// naming neither the router nor the script.
    fn run(&mut self, script: &str) -> Result<String>;
    /// Wait `duration` between two reads of the router.
    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// `ssh -o BatchMode=yes -o ConnectTimeout=5 <target> sh -s`, the script
/// on its stdin: never prompts, never waits longer than
/// [`COMMAND_TIMEOUT`].
pub struct Ssh {
    target: String,
}

impl fmt::Debug for Ssh {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Ssh(<target hidden>)")
    }
}

impl Ssh {
    /// SSH to `target`: letters, digits, `.`, `_`, `-` and `@`, not starting
    /// with `-`.
    pub fn new(target: &str) -> Result<Self> {
        if !token(target) {
            return Err(
                "[station_fixture].ssh_target holds characters other than letters, digits, \
                 `.`, `_`, `-` and `@`, or starts with `-`"
                    .to_owned(),
            );
        }
        Ok(Self {
            target: target.to_owned(),
        })
    }

    /// The command; the script is not in it.
    fn command(&self) -> std::process::Command {
        let mut ssh = oer_os_process::command("ssh");
        ssh.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
            .arg(&self.target)
            .args(["sh", "-s"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        ssh
    }
}

impl Remote for Ssh {
    fn run(&mut self, script: &str) -> Result<String> {
        let mut child = oer_os_process::owned::Child::spawn(&mut self.command())
            .map_err(|error| format!("cannot run ssh: {error}"))?;
        // A script far below the pipe's capacity: the write never waits for
        // ssh to read.
        child
            .stdin
            .take()
            .ok_or("ssh has no stdin")?
            .write_all(script.as_bytes())
            .map_err(|error| format!("cannot hand ssh the script: {error}"))?;
        let output = child
            .wait_with_output_timeout(Some(COMMAND_TIMEOUT))
            .map_err(|error| {
                if error.to_string().contains("deadline") {
                    format!("no answer within {} s", COMMAND_TIMEOUT.as_secs())
                } else {
                    format!("ssh failed: {error}")
                }
            })?;
        match output.status.code() {
            Some(0) => String::from_utf8(output.stdout)
                .map_err(|_| "the router's answer is not UTF-8".to_owned()),
            Some(255) => Err(
                "ssh could not reach the router or was refused (exit 255; BatchMode needs a \
                 loaded key)"
                    .to_owned(),
            ),
            Some(code) => Err(format!("the remote script exited with {code}")),
            None => Err("ssh was killed by a signal".to_owned()),
        }
    }
}

/// Letters, digits, `.`, `_`, `-` and `@`, not empty, not starting with `-`.
pub(crate) fn token(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('-')
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'@'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "made-up-router.lab";

    #[test]
    fn the_argument_list_holds_the_target_and_no_other_value() {
        // Not run: only its command line is checked.
        let command = Ssh::new(TARGET).unwrap().command();
        assert_eq!(command.get_program(), "ssh");
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                TARGET,
                "sh",
                "-s"
            ]
        );
    }

    #[test]
    fn the_target_is_a_plain_token_and_never_shown() {
        assert!(Ssh::new("-oProxyCommand=x").is_err());
        assert!(Ssh::new("host name").is_err());
        assert!(Ssh::new("").is_err());
        assert!(Ssh::new("user@lab-router.local").is_ok());
        let ssh = Ssh::new(TARGET).unwrap();
        assert!(!format!("{ssh:?}").contains("made-up"));
        let error = Ssh::new("made-up router").unwrap_err();
        assert!(!error.contains("made-up"), "{error}");
    }
}
