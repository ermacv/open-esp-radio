//! The probe's arithmetic and its console lines, checked on the host.

/// The slow RC oscillator's nominal rate, which the LP timer counts.
///
/// SOURCE: ESP-IDF 4d80c1b2 `components/soc/esp32s31/include/soc/clk_tree_defs.h:52`
/// (`SOC_CLK_RC_SLOW_FREQ_APPROX` 136 kHz).
pub const NOMINAL_SLOW_HZ: u64 = 136_000;

/// LP-timer cycles of `millis` at the nominal slow rate.
pub fn nominal_ticks(millis: u64) -> Option<u64> {
    millis.checked_mul(NOMINAL_SLOW_HZ)?.checked_div(1_000)
}

/// The slow clock's rate from `ticks` LP-timer cycles over `micros`
/// system-timer microseconds; `None` for an empty span.
pub fn rate_hz(ticks: u64, micros: u64) -> Option<u64> {
    ticks.checked_mul(1_000_000)?.checked_div(micros)
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

    /// `value` as `0x` and eight hexadecimal digits.
    pub fn hex(&mut self, value: u32) -> &mut Self {
        self.text("0x");
        for shift in (0..8).rev() {
            let nibble = (value >> (shift * 4)) & 0xf;
            let digit = match nibble {
                0..=9 => b'0'.wrapping_add(nibble as u8),
                _ => b'a'.wrapping_add((nibble as u8).wrapping_sub(10)),
            };
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
