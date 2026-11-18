/// NAPOT `tdata2` watching exactly `bottom..bottom + bytes`.
/// The stack policy supplies the IRQ guard size; the task guard watches
/// one four-byte word. Both must be aligned power-of-two regions.
pub(super) const fn napot_guard(bottom: usize, bytes: usize) -> Result<usize, crate::BootError> {
    if bytes < 4 || !bytes.is_power_of_two() || bottom & (bytes - 1) != 0 {
        return Err(crate::BootError::InvalidStackGuard { bottom, bytes });
    }
    Ok(bottom | (bytes / 2 - 1))
}

/// Arm the guard below an interrupt stack. Compiled only in an image that
/// places its guards (`irq_stack_guard`); there the linked guard is the
/// stack policy's, never zero.
#[cfg(irq_stack_guard)]
pub(super) fn arm_interrupt_guard(
    bottom: usize,
    bytes: usize,
    arm: impl FnOnce(usize),
) -> Result<(), crate::BootError> {
    arm(napot_guard(bottom, bytes)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::napot_guard;

    #[cfg(irq_stack_guard)]
    #[test]
    fn a_placed_guard_arms_its_region_and_a_missing_one_is_a_boot_error()
    -> Result<(), crate::BootError> {
        let mut armed = None;
        super::arm_interrupt_guard(0x2000_0000, 4096, |value| armed = Some(value))?;
        assert_eq!(armed, Some(napot_guard(0x2000_0000, 4096)?));
        let mut armed = None;
        assert_eq!(
            super::arm_interrupt_guard(0x2000_0000, 0, |value| armed = Some(value)),
            Err(crate::BootError::InvalidStackGuard {
                bottom: 0x2000_0000,
                bytes: 0
            })
        );
        assert_eq!(armed, None);
        Ok(())
    }

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
