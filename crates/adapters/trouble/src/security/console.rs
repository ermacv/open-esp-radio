//! Bounded manual Numeric Comparison console. Never owns HCI or bond keys.
//!
//! A decision names the current boot, request ID and six-digit number. The user
//! must compare the independently displayed peer number. No timeout, reconnect,
//! parser recovery or transport closure supplies an affirmative response.

use super::{
    comparison::{Challenge, NumericComparison},
    gatt::Observation,
};
use core::{convert::Infallible, fmt::Write};
use embassy_futures::select::{Either3, select3};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embedded_io_async::{Read, Write as IoWrite};

#[derive(Debug)]
pub enum ConsoleError<E> {
    Io(E),
    Closed,
}

/// Last-value diagnostics, not a lossless qualification/evidence transport.
pub type Observations = Signal<NoopRawMutex, Observation>;

#[derive(Default)]
struct Input {
    line: heapless::Vec<u8, 96>,
    overflow: bool,
    last_cr: bool,
}

impl Input {
    fn byte(&mut self, byte: u8, boot: u64, comparison: &NumericComparison) -> Option<bool> {
        if byte == b'\n' && self.last_cr {
            self.last_cr = false;
            return None;
        }
        self.last_cr = byte == b'\r';
        if byte != b'\n' && byte != b'\r' {
            if self.line.push(byte).is_err() {
                self.overflow = true;
            }
            return None;
        }
        let result = !self.overflow && decision(&self.line, boot, comparison);
        self.line.clear();
        self.overflow = false;
        Some(result)
    }
}

fn decision(line: &[u8], boot: u64, comparison: &NumericComparison) -> bool {
    let Ok(line) = core::str::from_utf8(line) else {
        return false;
    };
    let mut fields = line.split_ascii_whitespace();
    if fields.next() != Some("confirm") {
        return false;
    }
    let Some(epoch) = fields.next() else {
        return false;
    };
    if epoch.len() != 16 || u64::from_str_radix(epoch, 16) != Ok(boot) {
        return false;
    }
    let Some(id) = fields.next().and_then(|s| s.parse::<u64>().ok()) else {
        return false;
    };
    let Some(number) = fields
        .next()
        .filter(|s| s.len() == 6 && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u32>().ok())
    else {
        return false;
    };
    let accept = match fields.next() {
        Some("yes") => true,
        Some("no") => false,
        _ => return false,
    };
    if fields.next().is_some() {
        return false;
    }
    comparison.respond(Challenge { id, number }, accept).is_ok()
}

/// Run with one exclusively owned byte stream (USB in the standalone image).
///
/// Transport failure returns without confirming a prompt. The outer composition
/// cancels the application future and continues owning Host/Controller cleanup.
pub async fn run<T: Read + IoWrite>(
    io: &mut T,
    boot: u64,
    comparison: &NumericComparison,
    observations: &Observations,
) -> Result<Infallible, ConsoleError<T::Error>> {
    let mut line = heapless::String::<192>::new();
    writeln!(
        &mut line,
        "secure GATT boot={boot:016x}; compare BOTH peer numbers before confirming."
    )
    .expect("bounded banner");
    io.write_all(line.as_bytes())
        .await
        .map_err(ConsoleError::Io)?;
    let mut input = Input::default();
    loop {
        let mut byte = [0];
        line.clear();
        match select3(
            io.read(&mut byte),
            comparison.wait_changed(),
            observations.wait(),
        )
        .await
        {
            Either3::First(Ok(1)) => {
                let Some(ok) = input.byte(byte[0], boot, comparison) else {
                    continue;
                };
                line.push_str(if ok {
                    "decision received; pairing completion is separate\n"
                } else {
                    "invalid or stale decision\n"
                })
                .expect("bounded reply");
            }
            Either3::First(Ok(_)) => return Err(ConsoleError::Closed),
            Either3::First(Err(error)) => return Err(ConsoleError::Io(error)),
            Either3::Second(()) => {
                if let Some(challenge) = comparison.pending() {
                    writeln!(
                        &mut line,
                        "compare {:06}; reply: confirm {boot:016x} {} {:06} yes|no",
                        challenge.number, challenge.id, challenge.number
                    )
                    .expect("bounded prompt");
                } else {
                    line.push_str("comparison no longer pending\n")
                        .expect("bounded reply");
                }
            }
            Either3::Third(event) => {
                writeln!(&mut line, "{event:?}").expect("bounded value-only observation");
            }
        }
        io.write_all(line.as_bytes())
            .await
            .map_err(ConsoleError::Io)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decision_requires_exact_boot_request_number_and_explicit_choice() {
        let comparison = NumericComparison::new();
        let _prompt = comparison.begin(123).unwrap();
        for line in [
            "yes",
            "confirm 0000000000000002 1 000123 yes",
            "confirm 0000000000000001 2 000123 yes",
            "confirm 0000000000000001 1 000124 yes",
            "confirm 0000000000000001 1 123 yes",
            "confirm 0000000000000001 1 000123 yes extra",
        ] {
            assert!(!decision(line.as_bytes(), 1, &comparison));
            assert!(comparison.pending().is_some());
        }
        assert!(decision(
            b"confirm 0000000000000001 1 000123 no",
            1,
            &comparison
        ));
        assert!(!decision(
            b"confirm 0000000000000001 1 000123 yes",
            1,
            &comparison
        ));
    }
    #[test]
    fn oversized_line_cannot_be_reinterpreted_as_valid_suffix() {
        let comparison = NumericComparison::new();
        let _prompt = comparison.begin(123).unwrap();
        let mut input = Input::default();
        for _ in 0..100 {
            input.byte(b' ', 1, &comparison);
        }
        for b in b"confirm 0000000000000001 1 000123 yes" {
            input.byte(*b, 1, &comparison);
        }
        assert_eq!(input.byte(b'\n', 1, &comparison), Some(false));
        assert!(comparison.pending().is_some());
        for b in b"confirm 0000000000000001 1 000123 yes" {
            input.byte(*b, 1, &comparison);
        }
        assert_eq!(input.byte(b'\n', 1, &comparison), Some(true));
    }

    #[test]
    fn terminal_cr_and_crlf_deliver_one_decision() {
        let comparison = NumericComparison::new();
        let _prompt = comparison.begin(123).unwrap();
        let mut input = Input::default();
        for b in b"confirm 0000000000000001 1 000123 no" {
            input.byte(*b, 1, &comparison);
        }
        assert_eq!(input.byte(b'\r', 1, &comparison), Some(true));
        assert_eq!(input.byte(b'\n', 1, &comparison), None);
    }
}
