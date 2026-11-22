//! A reference peer's line protocol over the console the service runs: each
//! command written once the previous one was answered, its answer the `@OK`
//! or `@ERR` naming it in the peers' one grammar
//! ([`oer_tools_lab_model::peer_line`]). The CLI's `board send` and the HIL
//! harness's stand equipment both speak it through this.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::client::{Input, Result, Running};

/// The peer's console: its lines as they come, and its input.
pub struct PeerLine {
    lines: mpsc::Receiver<Vec<u8>>,
    input: Input,
}

/// How one command ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Answer {
    /// `@OK`.
    Accepted,
    /// `@ERR`, with the line that said it.
    Refused(String),
    /// No answer within the wait.
    Unanswered,
}

impl PeerLine {
    /// The peer on `console`, its lines read by a thread of their own.
    pub fn open(console: Running) -> Self {
        let input = console.input();
        Self {
            lines: lines(console),
            input,
        }
    }

    /// Write `command` and wait up to `timeout` for its answer, printing what
    /// the peer says through `print`. An answer naming another command, such
    /// as noise a peer read while it booted, is only printed.
    ///
    /// # Errors
    /// The console's input could not take the command.
    pub fn send(
        &self,
        command: &str,
        timeout: Duration,
        print: &mut dyn FnMut(&str),
    ) -> Result<Answer> {
        let Some(name) = command.split_whitespace().next() else {
            return Err("an empty command".to_owned());
        };
        if command.contains('\n') {
            return Err(format!("the command `{command}` spans lines"));
        }
        self.input.send(format!("{command}\n").as_bytes())?;
        Ok(answer(&self.lines, name, timeout, print))
    }

    /// Stop the console.
    ///
    /// # Errors
    /// The service could not be told.
    pub fn cancel(&self) -> Result<()> {
        self.input.cancel()
    }
}

/// `console`'s lines, without their line ends, read by a thread until it
/// closes.
pub fn lines(console: Running) -> mpsc::Receiver<Vec<u8>> {
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

/// Print `lines` until the `@OK` or `@ERR` answer to `command`, or nothing
/// after `duration`. An answer naming another command, such as noise a peer
/// read while it booted, is only printed.
pub fn answer(
    lines: &mpsc::Receiver<Vec<u8>>,
    command: &str,
    duration: Duration,
    print: &mut dyn FnMut(&str),
) -> Answer {
    let deadline = Instant::now() + duration;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match lines.recv_timeout(remaining) {
            Ok(line) if line.is_empty() => {}
            Ok(line) => {
                let text = String::from_utf8_lossy(&line);
                print(&text);
                match oer_tools_lab_model::peer_line::parse(text.trim()) {
                    Some(oer_tools_lab_model::peer_line::Line::Ok { command: about })
                        if about.eq_ignore_ascii_case(command) =>
                    {
                        return Answer::Accepted;
                    }
                    Some(oer_tools_lab_model::peer_line::Line::Err { command: about, .. })
                        if about.eq_ignore_ascii_case(command) =>
                    {
                        return Answer::Refused(text.into_owned());
                    }
                    _ => {}
                }
            }
            Err(_) => return Answer::Unanswered,
        }
    }
    Answer::Unanswered
}

#[cfg(test)]
mod tests;
