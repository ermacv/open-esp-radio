//! A client of the service: the connection a command holds its lease by.
//! Dropping it ends the lease the way a lost client does.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    sync::{Arc, Mutex, PoisonError},
};

use oer_lab_arbiter::claims::Claim;

use crate::protocol::{Frame, LeaseRequest, Operation, PROTOCOL, Reply, Request, Status};

pub type Result<T> = std::result::Result<T, String>;

pub struct Client {
    reader: UnixStream,
    writer: Arc<Mutex<UnixStream>>,
    /// Why the service ended the lease, once it did.
    revoked: Option<String>,
    /// Why divisible work should yield, once the service asked.
    yield_reason: Option<String>,
    next_operation: u64,
}

/// A lease the client holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Granted {
    pub lease: u64,
    /// Lets a command inside the lease join it.
    pub token: String,
}

impl Client {
    /// Greet the service on `stream` as `client`; another protocol is
    /// refused with the service's reason.
    pub fn connect(stream: UnixStream, client: &str) -> Result<Self> {
        let writer = stream.try_clone().map_err(|error| error.to_string())?;
        let mut this = Self {
            reader: stream,
            writer: Arc::new(Mutex::new(writer)),
            revoked: None,
            yield_reason: None,
            next_operation: 1,
        };
        this.send(&Request::Hello {
            protocol: PROTOCOL,
            client: client.to_owned(),
        })?;
        match this.reply()? {
            Reply::Welcome { .. } => Ok(this),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to hello: {other:?}")),
        }
    }

    /// Why the service ended the lease, if it did.
    pub fn revoked(&self) -> Option<&str> {
        self.revoked.as_deref()
    }

    /// Why divisible work should release its lease at the next boundary,
    /// if the service asked it to.
    pub fn yield_requested(&self) -> Option<&str> {
        self.yield_reason.as_deref()
    }

    /// End held lease `lease` for `reason`, on behalf of `by`.
    pub fn preempt(&mut self, lease: u64, by: &str, reason: &str) -> Result<()> {
        self.send(&Request::Preempt {
            lease,
            by: by.to_owned(),
            reason: reason.to_owned(),
        })?;
        match self.reply()? {
            Reply::Waiting { .. } => Ok(()),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to preempt: {other:?}")),
        }
    }

    /// Queue for a lease and wait for it; `waiting` sees each report.
    pub fn acquire(
        &mut self,
        lease: LeaseRequest,
        waiting: &mut dyn FnMut(&str),
    ) -> Result<Granted> {
        self.send(&Request::Acquire { lease })?;
        loop {
            match self.reply()? {
                Reply::Waiting { message, .. } => waiting(&message),
                Reply::Granted { lease, token } => return Ok(Granted { lease, token }),
                Reply::Refused { reason } => return Err(reason),
                other => return Err(format!("unexpected reply to acquire: {other:?}")),
            }
        }
    }

    /// Work under the lease `token` names, for `claims` it holds.
    pub fn join(&mut self, token: &str, claims: Vec<Claim>) -> Result<Granted> {
        self.send(&Request::Join {
            token: token.to_owned(),
            claims,
        })?;
        match self.reply()? {
            Reply::Granted { lease, token } => Ok(Granted { lease, token }),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to join: {other:?}")),
        }
    }

    /// End the lease once its operations stopped.
    pub fn release(&mut self) -> Result<()> {
        self.send(&Request::Release)?;
        self.ended()
    }

    /// Wait until the lease ended.
    pub(crate) fn ended(&mut self) -> Result<()> {
        loop {
            match self.reply()? {
                Reply::Ended => return Ok(()),
                Reply::Refused { reason } => return Err(reason),
                _ => {}
            }
        }
    }

    pub fn status(&mut self) -> Result<Status> {
        self.send(&Request::Status)?;
        loop {
            if let Reply::Status { status } = self.reply()? {
                return Ok(status);
            }
        }
    }

    /// Ask the service to recover unavailable `resource` again.
    pub fn return_resource(&mut self, resource: &str) -> Result<()> {
        self.send(&Request::Return {
            resource: resource.to_owned(),
        })?;
        match self.reply()? {
            Reply::Waiting { .. } => Ok(()),
            Reply::Refused { reason } => Err(reason),
            other => Err(format!("unexpected reply to return: {other:?}")),
        }
    }

    /// Run `operation` under the lease: its output goes to `output`, `input`
    /// feeds it until its end. Lost output is reported on `output`'s side as
    /// an error line, never hidden.
    pub fn run(
        &mut self,
        operation: Operation,
        output: &mut dyn Write,
        input: Option<Box<dyn Read + Send>>,
    ) -> Result<serde_json::Value> {
        let id = self.next_operation;
        self.next_operation += 1;
        self.send(&Request::Start { id, operation })?;
        if let Some(mut input) = input {
            let writer = self.writer.clone();
            std::thread::spawn(move || {
                let mut buffer = vec![0u8; 16 * 1024];
                loop {
                    let read = match input.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    };
                    let frame = Frame::Data {
                        operation: id,
                        bytes: buffer.get(..read).unwrap_or_default().to_vec(),
                    };
                    let mut writer = writer.lock().unwrap_or_else(PoisonError::into_inner);
                    if frame.write(&mut *writer).is_err() {
                        return;
                    }
                }
                if let Ok(frame) = Frame::message(&Request::CloseInput { id }) {
                    let mut writer = writer.lock().unwrap_or_else(PoisonError::into_inner);
                    let _ = frame.write(&mut *writer);
                }
            });
        }
        let mut lost = 0u64;
        loop {
            match self.frame()? {
                Frame::Data { operation, bytes } if operation == id => {
                    output
                        .write_all(&bytes)
                        .map_err(|error| error.to_string())?;
                }
                Frame::Data { .. } => {}
                Frame::Message(json) => match parse(&json)? {
                    Reply::Lost { id: lost_id, bytes } if lost_id == id => lost += bytes,
                    Reply::Finished { id: done, result } if done == id => {
                        if lost > 0 {
                            let _ = writeln!(
                                output,
                                "lab-service: {lost} bytes of output were lost to a slow reader"
                            );
                        }
                        return result;
                    }
                    Reply::Revoked { reason } => self.revoked = Some(reason),
                    Reply::Yield { reason } => self.yield_reason = Some(reason),
                    _ => {}
                },
            }
        }
    }

    pub(crate) fn send(&self, request: &Request) -> Result<()> {
        let frame = Frame::message(request).map_err(|error| error.to_string())?;
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        frame
            .write(&mut *writer)
            .map_err(|error| format!("the LAB service is unreachable: {error}"))
    }

    pub(crate) fn frame(&mut self) -> Result<Frame> {
        match Frame::read(&mut self.reader) {
            Ok(Some(frame)) => Ok(frame),
            Ok(None) => Err("the LAB service closed the connection".to_owned()),
            Err(error) => Err(format!("the LAB service connection failed: {error}")),
        }
    }

    /// The next reply; a revocation is kept and reading goes on.
    fn reply(&mut self) -> Result<Reply> {
        loop {
            if let Frame::Message(json) = self.frame()? {
                match parse(&json)? {
                    Reply::Revoked { reason } => self.revoked = Some(reason),
                    Reply::Yield { reason } => self.yield_reason = Some(reason),
                    reply => return Ok(reply),
                }
            }
        }
    }
}

pub(crate) fn parse(json: &[u8]) -> Result<Reply> {
    serde_json::from_slice(json).map_err(|error| format!("unreadable reply: {error}"))
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.reader.shutdown(std::net::Shutdown::Both);
    }
}
