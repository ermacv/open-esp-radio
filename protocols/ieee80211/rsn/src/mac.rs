//! RFC 2104 key normalization for infallible, block-sized HMAC constructors.
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

pub(crate) fn sha1(key: &[u8]) -> Hmac<Sha1> {
    let mut block = [0; 64];
    if key.len() > block.len() {
        let mut digest = Sha1::digest(key);
        for (target, source) in block.iter_mut().zip(digest.iter()) {
            *target = *source;
        }
        digest.zeroize();
    } else {
        for (target, source) in block.iter_mut().zip(key) {
            *target = *source;
        }
    }
    let mac = <Hmac<Sha1> as Mac>::new((&block).into());
    block.zeroize();
    mac
}

pub(crate) fn sha256(key: &[u8]) -> Hmac<Sha256> {
    let mut block = [0; 64];
    if key.len() > block.len() {
        let mut digest = Sha256::digest(key);
        for (target, source) in block.iter_mut().zip(digest.iter()) {
            *target = *source;
        }
        digest.zeroize();
    } else {
        for (target, source) in block.iter_mut().zip(key) {
            *target = *source;
        }
    }
    let mac = <Hmac<Sha256> as Mac>::new((&block).into());
    block.zeroize();
    mac
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;

    #[test]
    fn normalized_hmac_keys_match_the_variable_length_constructors() {
        for length in [0, 1, 16, 32, 63, 64, 65, 128] {
            let key: std::vec::Vec<_> = (0..length).map(|byte| byte as u8).collect();
            let mut actual = sha1(&key);
            let mut expected = <Hmac<Sha1> as Mac>::new_from_slice(&key).unwrap();
            actual.update(b"RSN key normalization");
            expected.update(b"RSN key normalization");
            assert_eq!(
                actual.finalize().into_bytes(),
                expected.finalize().into_bytes()
            );
            let mut actual = sha256(&key);
            let mut expected = <Hmac<Sha256> as Mac>::new_from_slice(&key).unwrap();
            actual.update(b"SAE key normalization");
            expected.update(b"SAE key normalization");
            assert_eq!(
                actual.finalize().into_bytes(),
                expected.finalize().into_bytes()
            );
        }
    }
}
