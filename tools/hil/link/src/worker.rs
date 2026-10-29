//! The link's worker: it owns the serial line, writes queued frames, decodes
//! every device frame into the shared state and renders diagnostic lines.

use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};

use oer_hil_protocol::{DecodeError, FrameDecoder, WireKind};
use zeroize::Zeroizing;

use crate::{LinkError, Received, Shared, notes::Notes, transport::events::EventPoll};

/// How long one frame may take to leave the host.
const WRITE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// The serial line as a nonblocking file the worker polls.
pub(crate) fn nonblocking(line: Box<dyn crate::SerialLine>) -> Result<fs::File, LinkError> {
    let file = fs::File::from(line.into_owned_descriptor());
    rustix::fs::fcntl_getfl(&file)
        .and_then(|flags| rustix::fs::fcntl_setfl(&file, flags | rustix::fs::OFlags::NONBLOCK))
        .map_err(|error| LinkError::Transport(format!("cannot make the serial line nonblocking: {error}")))?;
    Ok(file)
}

/// Serve `serial` until `stop`, then record why it ended.
pub(crate) fn run(
    mut serial: fs::File,
    mut poll: EventPoll,
    shared: &Shared,
    stop: &AtomicBool,
    commands: mpsc::Receiver<Zeroizing<Vec<u8>>>,
) {
    let result = serve(&mut serial, &mut poll, shared, stop, &commands);
    let mut state = shared.lock();
    match result {
        Ok(()) => state.fail(LinkError::Transport("the link stopped".to_owned())),
        Err(error) => state.fail(error),
    }
    shared.changed.notify_all();
}

fn serve(
    serial: &mut fs::File,
    poll: &mut EventPoll,
    shared: &Shared,
    stop: &AtomicBool,
    commands: &mpsc::Receiver<Zeroizing<Vec<u8>>>,
) -> Result<(), LinkError> {
    let transport = |error: std::io::Error| LinkError::Transport(error.to_string());
    poll.register(serial, false).map_err(transport)?;
    let mut decoder = FrameDecoder::new();
    let mut notes = Notes::new();
    let mut buffer = [0_u8; 2048];
    let mut pending = VecDeque::<Zeroizing<Vec<u8>>>::new();
    let mut offset = 0;
    let mut write_deadline = None;
    let mut writable_interest = false;
    let mut readable = false;
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        pending.extend(commands.try_iter());
        while let Some(frame) = pending.front() {
            let deadline = *write_deadline.get_or_insert_with(|| Instant::now() + WRITE_DEADLINE);
            if Instant::now() >= deadline {
                return Err(LinkError::Transport("a frame missed its write deadline".to_owned()));
            }
            let Some(rest) = frame.get(offset..) else {
                return Err(LinkError::Transport("the frame writer lost its place".to_owned()));
            };
            match serial.write(rest) {
                Ok(0) => return Err(LinkError::Transport("the serial writer made no progress".to_owned())),
                Ok(length) => {
                    offset += length;
                    if offset == frame.len() {
                        pending.pop_front();
                        offset = 0;
                        write_deadline = None;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(LinkError::Transport(format!("serial write failed: {error}"))),
            }
        }
        let wants_write = !pending.is_empty();
        if wants_write != writable_interest {
            poll.interest(serial, wants_write).map_err(transport)?;
            writable_interest = wants_write;
        }
        if !readable {
            readable = poll.wait(write_deadline).map_err(transport)?.readable;
            continue;
        }
        let length = match serial.read(&mut buffer) {
            Ok(0) => return Err(LinkError::Transport("the serial line closed".to_owned())),
            Ok(length) => length,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                readable = false;
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(LinkError::Transport(format!("serial read failed: {error}"))),
        };
        let chunk = buffer.get(..length).unwrap_or_default();
        let mut state = shared.lock();
        let boots = state.health.boots;
        decoder.feed(WireKind::Event, chunk, |frame| match frame {
            Ok(frame) => state.receive(Received::from_frame(&frame)),
            // Another framing is an image of another protocol.
            Err(error @ DecodeError::FramingVersion) => {
                state.fail(LinkError::Protocol(error.to_string()));
            }
            // Boot text and torn frames show in the counters.
            Err(_) => {}
        });
        if state.health.boots != boots {
            notes.restart();
        }
        notes.feed(chunk, &mut state.notes);
        state.health.counters = decoder.counters();
        shared.changed.notify_all();
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
    }
}
