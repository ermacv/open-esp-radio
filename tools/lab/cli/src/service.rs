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

/// Run `argv` under `client`'s lease as the service's child, in this
/// command's directory with `environment`; `run_to_end` keeps it running
/// when the lease ends. On a terminal the command gets a pseudo-terminal of
/// its size and this terminal passes every key through until it ends;
/// otherwise its output and input pass through. A cancelled command stops
/// it through the service. The result is its exit code.
pub(crate) fn command(
    client: &mut Client,
    argv: Vec<String>,
    environment: Vec<(String, String)>,
    run_to_end: bool,
) -> Result<serde_json::Value> {
    let terminal = terminal_size();
    let _raw = terminal.and_then(|_| RawTerminal::enter());
    let running = client.start(Operation::Command {
        argv,
        directory: std::env::current_dir()?.display().to_string(),
        environment,
        terminal,
        run_to_end,
    })?;
    let stop = running.input();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watching = done.clone();
    std::thread::spawn(move || {
        while !watching.load(std::sync::atomic::Ordering::SeqCst) {
            if oer_os_process::cancellation_requested() {
                let _ = stop.cancel();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });
    let input = running.input();
    std::thread::spawn(move || {
        use std::io::Read as _;
        let mut stdin = std::io::stdin();
        let mut buffer = vec![0u8; 4096];
        while let Ok(read) = stdin.read(&mut buffer) {
            let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
                break;
            };
            if input.send(chunk).is_err() {
                return;
            }
        }
        let _ = input.close();
    });
    let result = running.finish(&mut std::io::stdout());
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(result?)
}

/// The size of the terminal this command runs on, when its input and output
/// are one.
fn terminal_size() -> Option<oer_lab_client::protocol::TerminalSize> {
    use std::io::IsTerminal as _;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }
    let size = rustix::termios::tcgetwinsize(std::io::stdout()).ok()?;
    Some(oer_lab_client::protocol::TerminalSize {
        rows: size.ws_row,
        columns: size.ws_col,
    })
}

/// This terminal in raw mode until dropped, so the leased command's own
/// terminal handles every key.
struct RawTerminal(rustix::termios::Termios);

impl RawTerminal {
    fn enter() -> Option<Self> {
        let saved = rustix::termios::tcgetattr(std::io::stdin()).ok()?;
        let mut raw = saved.clone();
        raw.make_raw();
        rustix::termios::tcsetattr(
            std::io::stdin(),
            rustix::termios::OptionalActions::Now,
            &raw,
        )
        .ok()?;
        Some(Self(saved))
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = rustix::termios::tcsetattr(
            std::io::stdin(),
            rustix::termios::OptionalActions::Now,
            &self.0,
        );
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
