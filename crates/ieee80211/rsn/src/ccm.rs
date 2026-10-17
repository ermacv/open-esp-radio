//! Shared CCMP-128 CCM transform (8-byte MIC, 13-byte nonce, two-byte length).
use crate::ccmp_mpdu::Aad;
use aes::{
    Aes128,
    cipher::{BlockEncrypt, generic_array::GenericArray},
};
use zeroize::Zeroize;

pub(crate) const MIC_LEN: usize = 8;
pub(crate) const NONCE_LEN: usize = 13;

pub(crate) fn crypt(cipher: &Aes128, nonce: &[u8; NONCE_LEN], body: &mut [u8]) {
    for (index, chunk) in body.chunks_mut(16).enumerate() {
        // Callers validate the two-byte CCM body length before reaching here.
        let mut stream = counter_block(nonce, index as u16 + 1);
        cipher.encrypt_block(&mut stream);
        for (byte, key) in chunk.iter_mut().zip(stream.iter()) {
            *byte ^= key;
        }
        stream.zeroize();
    }
}

pub(crate) fn authenticate(
    cipher: &Aes128,
    nonce: &[u8; NONCE_LEN],
    aad: &Aad,
    body: &[u8],
) -> [u8; MIC_LEN] {
    let mut mac = GenericArray::from([0_u8; 16]);
    mac[0] = 0x40 | (((MIC_LEN as u8 - 2) / 2) << 3) | 1;
    mac[1..14].copy_from_slice(nonce);
    mac[14..16].copy_from_slice(&(body.len() as u16).to_be_bytes());
    cipher.encrypt_block(&mut mac);
    let aad = aad.bytes();
    let mut block = [0; 16];
    block[..2].copy_from_slice(&(aad.len() as u16).to_be_bytes());
    let mut rest = aad.iter().copied();
    for byte in &mut block[2..] {
        *byte = rest.next().unwrap_or(0);
    }
    absorb(cipher, &mut mac, &block);
    // Aad admits only 22/24/28/30-byte MAC-header shapes; this consumes all of it.
    for byte in &mut block {
        *byte = rest.next().unwrap_or(0);
    }
    if aad.len() > 14 {
        absorb(cipher, &mut mac, &block);
    }
    for chunk in body.chunks(16) {
        absorb(cipher, &mut mac, chunk);
    }
    let mut first = counter_block(nonce, 0);
    cipher.encrypt_block(&mut first);
    let mut mic = [0; MIC_LEN];
    for ((out, mac), stream) in mic.iter_mut().zip(mac.iter()).zip(first.iter()) {
        *out = mac ^ stream;
    }
    mac.zeroize();
    first.zeroize();
    block.zeroize();
    mic
}

fn counter_block(
    nonce: &[u8; NONCE_LEN],
    counter: u16,
) -> GenericArray<u8, aes::cipher::consts::U16> {
    let mut block = GenericArray::from([0; 16]);
    block[0] = 1;
    block[1..14].copy_from_slice(nonce);
    block[14..16].copy_from_slice(&counter.to_be_bytes());
    block
}

fn absorb(cipher: &Aes128, mac: &mut GenericArray<u8, aes::cipher::consts::U16>, block: &[u8]) {
    for (state, byte) in mac.iter_mut().zip(block.iter()) {
        *state ^= byte;
    }
    cipher.encrypt_block(mac);
}
