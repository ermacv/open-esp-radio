//! The IPC protocol between the service and its clients over one Unix
//! stream socket.
//!
//! Every frame is a big-endian `u32` length of the rest, one kind byte and
//! the payload: a JSON [`Request`] or [`Reply`], or the bytes of an
//! operation's stream (its output from the service, its input from the
//! client) after the operation's big-endian `u64` identity. A frame longer
//! than [`MAX_FRAME`] ends the connection. The first request is
//! [`Request::Hello`]: a client of another [`PROTOCOL`] gets
//! [`Reply::Refused`] with the reason, and the connection ends.

use std::io::{self, Read, Write};

use oer_lab_arbiter::claims::Claim;
use oer_lab_arbiter::readiness::Readiness;
use serde::{Deserialize, Serialize};

/// The protocol version; another one is refused.
pub const PROTOCOL: u32 = 1;
/// The longest frame either side sends or accepts.
pub const MAX_FRAME: usize = 1 << 20;

const MESSAGE: u8 = 0;
const DATA: u8 = 1;

/// One frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    /// A JSON request or reply.
    Message(Vec<u8>),
    /// Bytes of operation `operation`'s stream.
    Data { operation: u64, bytes: Vec<u8> },
}

impl Frame {
    pub fn message(message: &impl Serialize) -> io::Result<Self> {
        Ok(Self::Message(serde_json::to_vec(message)?))
    }

    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        let (kind, head, body): (u8, &[u8], &[u8]) = match self {
            Self::Message(json) => (MESSAGE, &[], json),
            Self::Data { operation, bytes } => (DATA, &operation.to_be_bytes(), bytes),
        };
        let length = 1 + head.len() + body.len();
        if length > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("a frame of {length} bytes exceeds {MAX_FRAME}"),
            ));
        }
        let length = u32::try_from(length).map_err(io::Error::other)?;
        let mut frame = Vec::with_capacity(4 + 1 + head.len() + body.len());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.push(kind);
        frame.extend_from_slice(head);
        frame.extend_from_slice(body);
        out.write_all(&frame)?;
        out.flush()
    }

    /// The next frame, `None` at the end of the stream between frames.
    pub fn read(input: &mut impl Read) -> io::Result<Option<Self>> {
        let mut length = [0u8; 4];
        match input.read_exact(&mut length) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
        }
        let length = usize::try_from(u32::from_be_bytes(length)).map_err(io::Error::other)?;
        if length == 0 || length > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("a frame of {length} bytes is outside 1..={MAX_FRAME}"),
            ));
        }
        let mut payload = vec![0u8; length];
        input.read_exact(&mut payload)?;
        let Some((&kind, rest)) = payload.split_first() else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "an empty frame"));
        };
        match kind {
            MESSAGE => Ok(Some(Self::Message(rest.to_vec()))),
            DATA => {
                let Some((operation, bytes)) = rest.split_first_chunk::<8>() else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "a data frame without its operation",
                    ));
                };
                Ok(Some(Self::Data {
                    operation: u64::from_be_bytes(*operation),
                    bytes: bytes.to_vec(),
                }))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown frame kind {other}"),
            )),
        }
    }
}

/// What a lease is requested for.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LeaseRequest {
    pub owner: String,
    pub work: String,
    #[serde(default)]
    pub scenarios: Vec<String>,
    #[serde(default)]
    pub run: Option<String>,
    /// None claims the whole stand.
    pub claims: Vec<Claim>,
    /// Stand maintenance and its reason: served first, preempting the
    /// holders it conflicts with.
    #[serde(default)]
    pub maintenance: Option<String>,
}

/// A request of a client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "request", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    /// The first request: the client's protocol and a description of it.
    Hello { protocol: u32, client: String },
    /// Queue for a lease; [`Reply::Granted`] follows.
    Acquire { lease: LeaseRequest },
    /// Join the lease `token` names, for `claims` it covers: a command
    /// inside a lease uses its resources without queueing.
    Join { token: String, claims: Vec<Claim> },
    /// End this connection's lease; [`Reply::Ended`] follows once its
    /// operations stopped.
    Release,
    /// Start operation `id` under the lease; [`Reply::Finished`] follows.
    Start { id: u64, operation: Operation },
    /// Stop operation `id`.
    Cancel { id: u64 },
    /// The input of operation `id` ended.
    CloseInput { id: u64 },
    /// The service's state.
    Status,
    /// A person returns an unavailable resource: the service recovers it
    /// again.
    Return { resource: String },
}

/// Work on the leased resources, run by the service's drivers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Operation {
    /// A command the service runs as its child under the lease, with the
    /// client's arguments, directory and environment.
    Command {
        argv: Vec<String>,
        directory: String,
        environment: Vec<(String, String)>,
    },
}

/// A reply or notice of the service.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "reply", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Reply {
    Welcome {
        protocol: u32,
    },
    /// The request is refused; after a refused `Hello` the connection ends.
    Refused {
        reason: String,
    },
    /// The lease request waits as ticket `ticket`.
    Waiting {
        ticket: u64,
        message: String,
    },
    /// The lease is granted. `token` lets commands inside it join it.
    Granted {
        lease: u64,
        token: String,
    },
    /// The service ended the lease for `reason`; its operations are being
    /// stopped.
    Revoked {
        reason: String,
    },
    /// The lease ended and its operations stopped.
    Ended,
    /// Operation `id` finished.
    Finished {
        id: u64,
        result: Result<serde_json::Value, String>,
    },
    /// `bytes` of operation `id`'s output were dropped for a slow reader.
    Lost {
        id: u64,
        bytes: u64,
    },
    Status {
        status: Status,
    },
}

/// The service's state, for `cargo stand queue` and the tests.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Status {
    pub holders: Vec<HolderStatus>,
    pub waiting: Vec<u64>,
    pub resources: Vec<(String, Readiness)>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HolderStatus {
    pub lease: u64,
    pub owner: String,
    pub work: String,
    /// The lease ended and its operations are being stopped.
    pub ending: bool,
    /// Its running operations.
    pub operations: usize,
}

#[cfg(test)]
mod tests;
