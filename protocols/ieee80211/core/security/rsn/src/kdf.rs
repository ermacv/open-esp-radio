//! IEEE Std 802.11-2020 12.7.1.6.2 SHA-256 KDF.
use hmac::Mac;
use zeroize::Zeroize;

pub(crate) fn kdf_sha256<const N: usize>(
    key: &[u8],
    label: &[u8],
    context: &[u8],
    output: &mut [u8; N],
) {
    const {
        if N > (u16::MAX as usize) / 8 {
            panic!("KDF output bit length must fit u16");
        }
    }
    let bits = const { (N * 8) as u16 };
    let mut remaining = output.iter_mut().peekable();
    for counter in 1_u16..=u16::MAX {
        if remaining.peek().is_none() {
            break;
        }
        let mut mac = crate::mac::sha256(key);
        mac.update(&counter.to_le_bytes());
        mac.update(label);
        mac.update(context);
        mac.update(&bits.to_le_bytes());
        let mut block = mac.finalize().into_bytes();
        for source in &block {
            if let Some(target) = remaining.next() {
                *target = *source;
            }
        }
        block.zeroize();
    }
}
