//! The LAB service as the stand's commands use it: every lease and every
//! hardware operation goes through it.

use std::sync::mpsc;

use oer_lab_arbiter::claims::Claim;
use oer_lab_service::{
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
