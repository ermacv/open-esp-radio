//! A variant path as a retained record's text: the names joined by `/`,
//! then each number as ` 0x` and eight hex digits, then ` +` when the path
//! kept less than its value had. Text past the room is cut, and its last
//! two bytes become ` +`.
use oer_portable_diagnostics::DiagPath;

/// Write `prefix`, a space when `path` holds anything, then `path` into
/// `out`; the bytes written.
pub fn write(prefix: &[u8], path: &DiagPath, out: &mut [u8]) -> usize {
    let mut length = 0usize;
    let mut cut = false;
    let mut put = |bytes: &[u8]| {
        for byte in bytes {
            match out.get_mut(length) {
                Some(slot) => {
                    *slot = *byte;
                    length = length.saturating_add(1);
                }
                None => cut = true,
            }
        }
    };
    put(prefix);
    let empty = path.names().is_empty() && path.words().is_empty() && !path.overflowed();
    if !prefix.is_empty() && !empty {
        put(b" ");
    }
    for (index, name) in path.names().iter().enumerate() {
        if index != 0 {
            put(b"/");
        }
        put(name.as_bytes());
    }
    for word in path.words() {
        put(b" 0x");
        put(&hex(*word));
    }
    if path.overflowed() {
        put(b" +");
    }
    if cut {
        let end = out.iter_mut().take(length).rev().take(2).rev();
        for (slot, byte) in end.zip(b" +") {
            *slot = *byte;
        }
    }
    length
}

/// `word` as eight lowercase hex digits.
fn hex(mut word: u32) -> [u8; 8] {
    let mut digits = [b'0'; 8];
    for digit in digits.iter_mut().rev() {
        let nibble = (word & 0xf) as u8;
        *digit = if nibble < 10 {
            b'0'.wrapping_add(nibble)
        } else {
            b'a'.wrapping_add(nibble.wrapping_sub(10))
        };
        word >>= 4;
    }
    digits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reason_and_its_path_share_one_line_and_a_cut_says_so() {
        let mut path = DiagPath::new();
        for name in ["Lifecycle", "StopUnconfirmed", "Arena", "Watchdog"] {
            path.name(name);
        }
        let mut out = [0; 64];
        let length = write(b"wifi-abandoned", &path, &mut out);
        assert_eq!(
            &out[..length],
            b"wifi-abandoned Lifecycle/StopUnconfirmed/Arena/Watchdog"
        );
        let length = write(b"wifi-abandoned", &DiagPath::new(), &mut out);
        assert_eq!(&out[..length], b"wifi-abandoned");
        let mut short = [0; 20];
        let length = write(b"wifi-abandoned", &path, &mut short);
        assert_eq!(length, 20);
        assert_eq!(&short, b"wifi-abandoned Lif +");
    }
}
