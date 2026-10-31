//! The LAB service as the stand's commands use it: every lease and every
//! hardware operation goes through it.

use std::sync::mpsc;

use oer_lab_arbiter::claims::Claim;
use oer_lab_client::{
    client::{Client, Running},
    protocol::{LeaseRequest, Operation},
};

use crate::Result;

/// This command's connection to the service.
pub(crate) fn client() -> Result<Client> {
    Ok(Client::open(
        &std::env::args().collect::<Vec<_>>().join(" "),
    )?)
}

/// A lease of `claims` for `owner`'s `work`; it ends when the client drops.
pub(crate) fn lease(owner: String, work: String, claims: Vec<Claim>) -> Result<Client> {
    let mut client = client()?;
    client.acquire(
        LeaseRequest {
            owner,
            work,
            scenarios: Vec::new(),
            run: None,
            claims,
            maintenance: None,
            divisible: false,
        },
        &mut |message| eprintln!("stand: {message}"),
    )?;
    Ok(client)
}

/// How a leased command ended.
pub(crate) enum Ended {
    /// It exited with this code.
    Exited(std::process::ExitCode, bool),
    /// The service ended the lease, for this reason, and the command was
    /// stopped.
    Revoked(String),
    /// This command was cancelled and stopped its child.
    Cancelled,
}

/// Run `argv` as this command's child under `client`'s lease: it gets the
/// lease's token and `owner`, so the stand tools it runs join the lease
/// instead of queueing. It is stopped when the service ends the lease or
/// this command is cancelled. On a terminal it is the terminal's foreground
/// job with this command: it reads the terminal and Ctrl-C is its own
/// (`oer_os_process::foreground`). Otherwise it owns a process group, and
/// stopping it stops what it started.
pub(crate) fn command(
    client: &Client,
    ctx: &oer_os_process::Checkout,
    argv: &[std::ffi::OsString],
    owner: &str,
) -> Result<Ended> {
    use std::io::IsTerminal as _;
    let (program, arguments) = argv.split_first().ok_or("the command is empty")?;
    let mut command = ctx.command(program);
    command
        .args(arguments)
        .env(oer_lab_arbiter::owner::OWNER_ENV, owner)
        .env(
            oer_lab_client::protocol::LEASE_ENV,
            client.token().ok_or("the lease has no token")?,
        );
    let mut child = if std::io::stdin().is_terminal() {
        Child::Foreground(oer_os_process::foreground::Child::spawn(&mut command)?)
    } else {
        Child::Owned(oer_os_process::owned::Child::spawn_with_shutdown_grace(
            &mut command,
            GRACE,
        )?)
    };
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Ended::Exited(
                oer_os_process::exit_code(status),
                status.success(),
            ));
        }
        let ended = match client.revoked() {
            Some(reason) => Ended::Revoked(reason),
            None if oer_os_process::cancellation_requested() => Ended::Cancelled,
            None => {
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
        };
        child.stop()?;
        return Ok(ended);
    }
}

/// How long a stopped command has to end before it is killed.
const GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// A command run under a lease.
enum Child {
    Foreground(oer_os_process::foreground::Child),
    Owned(oer_os_process::owned::Child),
}

impl Child {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self {
            Self::Foreground(child) => child.try_wait(),
            Self::Owned(child) => child.try_wait(),
        }
    }

    fn stop(&mut self) -> Result<()> {
        match self {
            Self::Foreground(child) => {
                child.terminate(GRACE)?;
            }
            Self::Owned(child) => child.kill()?,
        }
        Ok(())
    }
}

/// Run `operation` under `client`'s lease to its result.
pub(crate) fn run(client: &mut Client, operation: Operation) -> Result<serde_json::Value> {
    Ok(client.run(operation, &mut std::io::sink(), None)?)
}

/// The lines of a console stream, each without its line ending; the channel
/// ends with the stream.
pub(crate) fn lines(console: Running) -> mpsc::Receiver<Vec<u8>> {
    let (send, lines) = mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        for line in std::io::BufReader::new(console).split(b'\n') {
            let Ok(mut line) = line else {
                break;
            };
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if send.send(line).is_err() {
                break;
            }
        }
    });
    lines
}
