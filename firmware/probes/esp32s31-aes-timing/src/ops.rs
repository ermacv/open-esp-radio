//! The protocol layer's AES operations over any block primitive, so that one
//! code path runs over the software block and over each hardware mode, and
//! the statistics and report lines of the measurements.
//!
//! CCM is CCMP-128's (IEEE 802.11-2020 12.5.3: eight-byte MIC, thirteen-byte
//! nonce, two-byte length field, RFC 3610), CMAC is BIP's AES-128-CMAC
//! (RFC 4493). The host tests check both against the RustCrypto crates the
//! product's software path uses.

use oer_crypto::block::{Block, BlockCipher};

/// CCMP's MIC length.
pub const MIC: usize = 8;
/// CCMP's nonce length.
pub const NONCE: usize = 13;

/// RFC 3610 B0 flags for CCMP: Adata, M = 8, L = 2.
const B0_FLAGS: u8 = 0x40 | (((MIC as u8 - 2) / 2) << 3) | (2 - 1);
/// RFC 3610 A_i flags: L = 2.
const CTR_FLAGS: u8 = 2 - 1;

/// Seal `body` in place under CCM with `nonce` and `aad`, and return the MIC.
/// `aad` is at most 30 bytes (CCMP's AAD) and `body` at most 65535 bytes.
pub fn ccm_seal<C: BlockCipher>(
    cipher: &mut C,
    nonce: &[u8; NONCE],
    aad: &[u8],
    body: &mut [u8],
) -> [u8; MIC] {
    let length = u16::try_from(body.len()).unwrap_or(u16::MAX);
    // CBC-MAC over B0, the encoded AAD and the plaintext.
    let mut mac = block(B0_FLAGS, nonce, length);
    cipher.encrypt_block(&mut mac);
    let aad = aad.get(..30).unwrap_or(aad);
    let aad_length = u16::try_from(aad.len()).unwrap_or(u16::MAX);
    let mut encoded = [0_u8; 32];
    for (out, byte) in encoded
        .iter_mut()
        .zip(aad_length.to_be_bytes().iter().chain(aad))
    {
        *out = *byte;
    }
    for chunk in encoded
        .chunks(16)
        .take(aad.len().saturating_add(2).div_ceil(16))
    {
        xor_into(&mut mac, chunk);
        cipher.encrypt_block(&mut mac);
    }
    for chunk in body.chunks(16) {
        xor_into(&mut mac, chunk);
        cipher.encrypt_block(&mut mac);
    }
    // CTR: A_0 masks the MIC, A_1.. the body.
    let counter = |index: u16| block(CTR_FLAGS, nonce, index);
    for (index, chunk) in (1_u16..).zip(body.chunks_mut(16)) {
        let mut stream = counter(index);
        cipher.encrypt_block(&mut stream);
        xor_into_slice(chunk, &stream);
    }
    let mut s0 = counter(0);
    cipher.encrypt_block(&mut s0);
    let mut mic = [0; MIC];
    for ((out, tag), mask) in mic.iter_mut().zip(mac).zip(s0) {
        *out = tag ^ mask;
    }
    mic
}

/// A CCM block: `flags`, the nonce and a two-byte big-endian `field`.
fn block(flags: u8, nonce: &[u8; NONCE], field: u16) -> Block {
    let mut out: Block = [0; 16];
    for (slot, byte) in out.iter_mut().zip(
        core::iter::once(&flags)
            .chain(nonce)
            .chain(&field.to_be_bytes()),
    ) {
        *slot = *byte;
    }
    out
}

/// AES-CMAC (RFC 4493) of `message`.
pub fn cmac<C: BlockCipher>(cipher: &mut C, message: &[u8]) -> Block {
    let mut l: Block = [0; 16];
    cipher.encrypt_block(&mut l);
    let k1 = double(&l);
    let k2 = double(&k1);
    let blocks = message.len().div_ceil(16).max(1);
    let mut mac: Block = [0; 16];
    let mut chunks = message.chunks(16);
    for chunk in chunks.by_ref().take(blocks.saturating_sub(1)) {
        xor_into(&mut mac, chunk);
        cipher.encrypt_block(&mut mac);
    }
    let tail = chunks.next().unwrap_or(&[]);
    let mut last: Block = [0; 16];
    xor_into(&mut last, tail);
    if tail.len() == 16 {
        xor_into(&mut last, &k1);
    } else {
        if let Some(pad) = last.get_mut(tail.len()) {
            *pad = 0x80;
        }
        xor_into(&mut last, &k2);
    }
    xor_into(&mut mac, &last);
    cipher.encrypt_block(&mut mac);
    mac
}

/// RFC 4493's subkey doubling in GF(2^128).
fn double(block: &Block) -> Block {
    let value = u128::from_be_bytes(*block);
    let doubled = (value << 1) ^ if value >> 127 == 1 { 0x87 } else { 0 };
    doubled.to_be_bytes()
}

fn xor_into(block: &mut Block, bytes: &[u8]) {
    for (out, byte) in block.iter_mut().zip(bytes) {
        *out ^= byte;
    }
}

fn xor_into_slice(bytes: &mut [u8], block: &Block) {
    for (out, byte) in bytes.iter_mut().zip(block) {
        *out ^= byte;
    }
}

/// The cycle counts of one case's runs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    pub runs: u32,
    pub min: u32,
    pub max: u32,
    pub total: u64,
}

impl Stats {
    /// Count one run of `cycles`.
    pub fn add(&mut self, cycles: u32) {
        if self.runs == 0 || cycles < self.min {
            self.min = cycles;
        }
        self.max = self.max.max(cycles);
        self.runs = self.runs.saturating_add(1);
        self.total = self.total.saturating_add(u64::from(cycles));
    }

    /// The mean cycles of a run; zero without runs.
    pub fn mean(&self) -> u64 {
        self.total.checked_div(u64::from(self.runs)).unwrap_or(0)
    }
}

/// A report line built in place, cut at its capacity.
pub struct Line {
    bytes: [u8; 160],
    length: usize,
}

impl Line {
    pub const fn new() -> Self {
        Self {
            bytes: [0; 160],
            length: 0,
        }
    }

    pub fn text(&mut self, text: &str) -> &mut Self {
        for &byte in text.as_bytes() {
            if let Some(slot) = self.bytes.get_mut(self.length) {
                *slot = byte;
                self.length += 1;
            }
        }
        self
    }

    pub fn number(&mut self, value: u64) -> &mut Self {
        let mut digits = [0_u8; 20];
        let mut rest = value;
        let mut count = 0;
        loop {
            if let Some(digit) = digits.get_mut(count) {
                *digit = b'0'.wrapping_add((rest % 10) as u8);
            }
            count += 1;
            rest /= 10;
            if rest == 0 || count == digits.len() {
                break;
            }
        }
        for &digit in digits.iter().take(count).rev() {
            if let Some(slot) = self.bytes.get_mut(self.length) {
                *slot = digit;
                self.length += 1;
            }
        }
        self
    }

    /// One case's line: `OER-PROBE aes-timing <mode> <operation> bytes=<n>
    /// runs=<n> min=<cycles> mean=<cycles> max=<cycles>`.
    pub fn case(mode: &str, operation: &str, bytes: usize, stats: &Stats) -> Self {
        let mut line = Self::new();
        line.text("OER-PROBE aes-timing ")
            .text(mode)
            .text(" ")
            .text(operation)
            .text(" bytes=")
            .number(bytes as u64)
            .text(" runs=")
            .number(u64::from(stats.runs))
            .text(" min=")
            .number(u64::from(stats.min))
            .text(" mean=")
            .number(stats.mean())
            .text(" max=")
            .number(u64::from(stats.max))
            .text("\r\n");
        line
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or(&self.bytes)
    }
}

#[cfg(test)]
mod tests;
