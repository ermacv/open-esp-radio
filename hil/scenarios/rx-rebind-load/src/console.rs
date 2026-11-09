//! The probe console's owner: it reads lines from the board's reset until
//! the rx-rebind result line, keeps every line it read, and on every exit
//! (a result, a timeout, a failed log write, an early return) cancels the
//! console operation and joins its reader.

use crate::result::{ProbeResult, parse};
use std::{
    io::{BufRead as _, BufReader, Read, Write},
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub struct ConsoleSession {
    lines: mpsc::Receiver<String>,
    cancel: Option<Box<dyn FnOnce() + Send>>,
    reader: Option<JoinHandle<()>>,
}

impl ConsoleSession {
    /// Own `stream`; `cancel` asks its source to end it, which ends the
    /// stream and with it the reader.
    pub fn new(stream: impl Read + Send + 'static, cancel: impl FnOnce() + Send + 'static) -> Self {
        let (send, lines) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stream).split(b'\n') {
                let Ok(line) = line else { break };
                let line = String::from_utf8_lossy(&line).trim_end().to_owned();
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            lines,
            cancel: Some(Box::new(cancel)),
            reader: Some(reader),
        }
    }

    /// Copy lines to `log` until the probe's result line or `timeout`.
    pub fn result(&self, log: &mut dyn Write, timeout: Duration) -> Result<ProbeResult, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err("no rx-rebind result line within the console timeout".to_owned());
            };
            let line = match self.lines.recv_timeout(left) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err("no rx-rebind result line within the console timeout".to_owned());
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("the console ended without an rx-rebind result line".to_owned());
                }
            };
            writeln!(log, "{line}").map_err(|error| format!("console log: {error}"))?;
            if let Some(result) = parse(&line)? {
                return Ok(result);
            }
        }
    }

    /// Cancel the console and join its reader.
    fn end(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }

    pub fn finish(mut self) {
        self.end();
    }
}

impl Drop for ConsoleSession {
    fn drop(&mut self) {
        self.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A console that yields `text`, then blocks until it is cancelled,
    /// then ends, as the LAB service's console operation does. Dropped with
    /// the reader thread that owns it, which reports it then.
    struct Console {
        text: Vec<u8>,
        at: usize,
        cancelled: mpsc::Receiver<()>,
        ended: mpsc::Sender<()>,
    }

    impl Read for Console {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if let Some(rest) = self.text.get(self.at..).filter(|rest| !rest.is_empty()) {
                let count = rest.len().min(buffer.len());
                for (to, from) in buffer.iter_mut().zip(rest.iter()).take(count) {
                    *to = *from;
                }
                self.at += count;
                return Ok(count);
            }
            // Blocks until the cancel drops its sender.
            let _ = self.cancelled.recv();
            Ok(0)
        }
    }

    impl Drop for Console {
        fn drop(&mut self) {
            let _ = self.ended.send(());
        }
    }

    /// A session over `text`; the receiver reports the reader's end.
    fn open(text: &str) -> (ConsoleSession, mpsc::Receiver<()>) {
        let (cancel, cancelled) = mpsc::channel::<()>();
        let (ended, reader_ended) = mpsc::channel();
        let console = Console {
            text: text.as_bytes().to_vec(),
            at: 0,
            cancelled,
            ended,
        };
        (
            ConsoleSession::new(console, move || drop(cancel)),
            reader_ended,
        )
    }

    #[test]
    fn the_result_line_ends_the_read_and_finish_cancels_and_joins() {
        let (session, reader_ended) =
            open("REBIND boot\nSEG 0 arm=fixed\nOER-RESULT: pass rx-rebind\n");
        let mut log = Vec::new();
        assert_eq!(
            session.result(&mut log, Duration::from_secs(3600)),
            Ok(ProbeResult::Pass)
        );
        session.finish();
        // Joined: the reader's stream was dropped before `finish` returned.
        assert_eq!(reader_ended.try_recv(), Ok(()));
        assert_eq!(
            String::from_utf8(log).unwrap(),
            "REBIND boot\nSEG 0 arm=fixed\nOER-RESULT: pass rx-rebind\n"
        );
    }

    #[test]
    fn a_failed_log_write_still_cancels_and_joins_on_drop() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (session, reader_ended) = open("REBIND boot\n");
        assert!(
            session
                .result(&mut Broken, Duration::from_secs(3600))
                .is_err()
        );
        drop(session);
        assert_eq!(reader_ended.try_recv(), Ok(()));
    }

    #[test]
    fn a_foreign_result_is_an_error_and_still_ends_the_console() {
        let (session, reader_ended) = open("OER-RESULT: pass rx-chain\n");
        assert!(
            session
                .result(&mut Vec::new(), Duration::from_secs(3600))
                .is_err()
        );
        session.finish();
        assert_eq!(reader_ended.try_recv(), Ok(()));
    }

    #[test]
    fn a_console_without_a_result_times_out_and_still_ends() {
        let (session, reader_ended) = open("REBIND boot\n");
        assert!(session.result(&mut Vec::new(), Duration::ZERO).is_err());
        session.finish();
        assert_eq!(reader_ended.try_recv(), Ok(()));
    }
}
