//! Deterministic byte-stream evidence shared by host and target HIL peers.

const STREAM_PATTERN_PERIOD: usize = 256;

const STREAM_PATTERN: [u8; STREAM_PATTERN_PERIOD] = {
    let mut pattern = [0; STREAM_PATTERN_PERIOD];
    let mut remaining = pattern.as_mut_slice();
    let mut phase = 0_u8;
    while let Some((byte, rest)) = remaining.split_first_mut() {
        *byte = stream_pattern_byte(phase as u64);
        phase = phase.wrapping_add(1);
        remaining = rest;
    }
    pattern
};

/// Return the byte expected at one absolute stream offset.
///
/// The full-period affine map catches dropped, duplicated and shifted bytes
/// without storing a second reference stream. TCP still supplies ordering and
/// retransmission; this pattern proves that both applications observed the
/// same byte positions rather than merely the same total length.
pub const fn stream_pattern_byte(offset: u64) -> u8 {
    (offset as u8).wrapping_mul(31).wrapping_add(0x5a)
}

pub fn fill_stream_pattern(buffer: &mut [u8], offset: u64) {
    for (byte, expected) in buffer.iter_mut().zip(pattern_at(offset)) {
        *byte = expected;
    }
}

pub fn stream_pattern_matches(buffer: &[u8], offset: u64) -> bool {
    buffer
        .iter()
        .copied()
        .zip(pattern_at(offset))
        .all(|(byte, expected)| byte == expected)
}

fn pattern_at(offset: u64) -> impl Iterator<Item = u8> {
    STREAM_PATTERN
        .iter()
        .copied()
        .cycle()
        .skip(usize::from(offset as u8))
}

#[cfg(test)]
mod tests;
