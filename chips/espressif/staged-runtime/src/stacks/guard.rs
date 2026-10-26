/// NAPOT `tdata2` watching exactly `bottom..bottom + bytes`.
/// The chip contract supplies the IRQ guard size; the task guard watches
/// one four-byte word. Both must be aligned power-of-two regions.
pub(super) const fn napot_guard(bottom: usize, bytes: usize) -> Result<usize, crate::BootError> {
    if bytes < 4 || !bytes.is_power_of_two() || bottom & (bytes - 1) != 0 {
        return Err(crate::BootError::InvalidStackGuard { bottom, bytes });
    }
    Ok(bottom | (bytes / 2 - 1))
}

#[cfg(test)]
mod tests {
    use super::napot_guard;

    #[test]
    fn covers_exactly_the_guard_at_the_stack_bottom() {
        for (bottom, bytes) in [
            (0x2000_0000, 4096),
            (0x2000_8000, 1024),
            (0x4080_0000, 4096),
            (0x2000_0000, 4),
            (0x2000_0000, 8),
        ] {
            let encoded = napot_guard(bottom, bytes).expect("aligned guard");
            // NAPOT ignores the trailing ones and the bit above them.
            let mask = (1usize << (encoded.trailing_ones() + 1)) - 1;
            let start = encoded & !mask;
            assert_eq!(start..start + mask + 1, bottom..bottom + bytes);
            for address in [bottom - 1, bottom, bottom + bytes - 1, bottom + bytes] {
                assert_eq!(
                    address & !mask == start,
                    (bottom..bottom + bytes).contains(&address)
                );
            }
        }
    }

    #[test]
    fn refuses_an_unaligned_bottom_or_invalid_size() {
        for (bottom, bytes) in [
            (0x2000_0440, 4096),
            (0x2000_0000, 0),
            (0x2000_0000, 2),
            (0x2000_0000, 48),
        ] {
            assert_eq!(
                napot_guard(bottom, bytes),
                Err(crate::BootError::InvalidStackGuard { bottom, bytes })
            );
        }
    }
}
