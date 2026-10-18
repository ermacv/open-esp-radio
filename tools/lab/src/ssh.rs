//! The one way to the stand router: non-interactive SSH.

use std::{fmt, time::Duration};

pub type Result<T> = std::result::Result<T, String>;

/// The longest one remote command may take.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs one command on the router.
pub trait Remote {
    /// `command`'s standard output when it exits 0; otherwise why not,
    /// naming neither the router nor the command.
    fn run(&mut self, command: &str) -> Result<String>;
    /// Wait `duration` between two `iw` reads.
    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// `ssh -o BatchMode=yes -o ConnectTimeout=5 <target> <command>`: never
/// prompts, never waits longer than [`COMMAND_TIMEOUT`].
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

    fn command(&self, command: &str) -> std::process::Command {
        let mut ssh = oer_process::command("ssh");
        ssh.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
            .arg(&self.target)
            .arg(command);
        ssh
    }
}

impl Remote for Ssh {
    fn run(&mut self, command: &str) -> Result<String> {
        let output = oer_process::output(&mut self.command(command), Some(COMMAND_TIMEOUT))
            .map_err(|error| {
                if error.to_string().contains("deadline") {
                    format!("no answer within {} s", COMMAND_TIMEOUT.as_secs())
                } else {
                    format!("cannot run ssh: {error}")
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
            Some(code) => Err(format!("the remote command exited with {code}")),
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
    fn the_real_runner_never_prompts() {
        // Not run: only its command line is checked.
        let ssh = Ssh::new(TARGET).unwrap();
        let command = ssh.command("uci show wireless");
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
                "uci show wireless"
            ]
        );
    }

    #[test]
    fn the_target_is_a_plain_token_and_never_shown() {
        assert!(Ssh::new("-oProxyCommand=x").is_err());
        assert!(Ssh::new("host name").is_err());
        assert!(Ssh::new("user@lab-router.local").is_ok());
        let ssh = Ssh::new(TARGET).unwrap();
        assert!(!format!("{ssh:?}").contains("made-up"));
    }
}
