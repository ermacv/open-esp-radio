//! The probe's tallies and its console lines, checked on the host.

/// One trial's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Everything the selection expects happened, in its order.
    Held,
    /// Something else happened.
    Broke,
}

/// One selection's tally over its trials: how many ran, how many held, and
/// the least and the most of a measured span (cycles) where it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tally {
    pub trials: u32,
    pub held: u32,
    /// `(least, most)` of the measured spans; `None` before the first.
    pub span: Option<(u32, u32)>,
}

impl Tally {
    pub const fn new() -> Self {
        Self {
            trials: 0,
            held: 0,
            span: None,
        }
    }

    /// Count one trial and its outcome.
    pub fn trial(&mut self, outcome: Outcome) {
        self.trials = self.trials.saturating_add(1);
        if outcome == Outcome::Held {
            self.held = self.held.saturating_add(1);
        }
    }

    /// Take one measured span into the least and the most.
    pub fn span(&mut self, cycles: u32) {
        self.span = Some(match self.span {
            None => (cycles, cycles),
            Some((least, most)) => (least.min(cycles), most.max(cycles)),
        });
    }

    /// Every trial held, and there was one.
    pub const fn holds(&self) -> bool {
        self.trials != 0 && self.held == self.trials
    }
}

/// One console line, cut at its capacity.
pub struct Line {
    bytes: [u8; 192],
    length: usize,
}

impl Line {
    pub const fn new() -> Self {
        Self {
            bytes: [0; 192],
            length: 0,
        }
    }

    pub fn text(&mut self, text: &str) -> &mut Self {
        for &byte in text.as_bytes() {
            self.push(byte);
        }
        self
    }

    pub fn number(&mut self, value: u64) -> &mut Self {
        let mut digits = [0_u8; 20];
        let mut rest = value;
        let mut count = 0_usize;
        loop {
            if let Some(digit) = digits.get_mut(count) {
                *digit = b'0'.wrapping_add((rest % 10) as u8);
            }
            count = count.saturating_add(1);
            rest /= 10;
            if rest == 0 || count == digits.len() {
                break;
            }
        }
        for &digit in digits.iter().take(count).rev() {
            self.push(digit);
        }
        self
    }

    /// The line ended with CR LF.
    pub fn end(&mut self) -> &[u8] {
        self.push(b'\r');
        self.push(b'\n');
        self.as_bytes()
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or(&[])
    }

    fn push(&mut self, byte: u8) {
        if let Some(slot) = self.bytes.get_mut(self.length) {
            *slot = byte;
            self.length = self.length.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests;
