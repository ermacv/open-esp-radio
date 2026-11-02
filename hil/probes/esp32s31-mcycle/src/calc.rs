//! The probe's arithmetic and its console lines, checked on the host.

/// Counter cycles per 10 ms of the system timer, from a span of `micros`
/// timer microseconds that took `cycles`; `None` for an empty span.
pub fn per_10ms(cycles: u64, micros: u64) -> Option<u64> {
    cycles.checked_mul(10_000)?.checked_div(micros)
}

/// The cycles hart 1's counter leads hart 0's by, from one sample on each
/// hart: `(c1 - c0) - rate * (t1 - t0)`, with `rate` the counter's cycles
/// per 10 ms and `t` in microseconds. `None` when a term overflows.
pub fn offset(hart0: (u64, u64), hart1: (u64, u64), rate_per_10ms: u64) -> Option<i64> {
    let (t0, c0) = hart0;
    let (t1, c1) = hart1;
    let counters = i128::from(c1).checked_sub(i128::from(c0))?;
    let micros = i128::from(t1).checked_sub(i128::from(t0))?;
    let elapsed = micros
        .checked_mul(i128::from(rate_per_10ms))?
        .checked_div(10_000)?;
    i64::try_from(counters.checked_sub(elapsed)?).ok()
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

    pub fn signed(&mut self, value: i64) -> &mut Self {
        if value < 0 {
            self.push(b'-');
        }
        self.number(value.unsigned_abs())
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
