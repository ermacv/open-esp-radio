//! IEEE Std 802.11-2020 12.7.1.6.2 SHA-256 KDF.
use hmac::Mac;
use zeroize::Zeroize;

pub(crate) fn kdf_sha256<const N: usize>(
    key: &[u8],
    label: &[u8],
    context: &[u8],
    output: &mut [u8; N],
) {
    // The output's Length in bits, counted at build: eight a octet.
    let bits = const {
        let mut bits: u16 = 0;
        let mut octets = N;
        while octets > 0 {
            bits = match bits.checked_add(8) {
                Some(bits) => bits,
                None => panic!("KDF output bit length must fit u16"),
            };
            octets = octets.saturating_sub(1);
        }
        bits
    };
    let mut remaining = output.iter_mut().peekable();
    for counter in 1_u16..=u16::MAX {
        if remaining.peek().is_none() {
            break;
        }
        let mut mac = crate::mac::sha256(key);
        mac.update(&KdfField::of(counter).to_bytes());
        mac.update(label);
        mac.update(context);
        mac.update(&KdfField::of(bits).to_bytes());
        let mut block = mac.finalize().into_bytes();
        for source in &block {
            if let Some(target) = remaining.next() {
                *target = *source;
            }
        }
        block.zeroize();
    }
}

oer_portable_wire::bitfield! {
    /// The KDF's counter i and its Length: 16-bit little-endian integers.
    /// SOURCE: IEEE Std 802.11-2020, 12.7.1.7.2 (Key derivation function).
    struct KdfField[2] (little) {
        value, with_value: {u16} = 0..16;
    }
}

impl KdfField {
    const fn of(value: u16) -> Self {
        Self::from_bytes([0; 2]).with_value(value)
    }
}
