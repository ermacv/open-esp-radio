// Share the actual PAC write with the host regression, which supplies a PAC
// register in ordinary memory instead of the peripheral's MMIO register.
macro_rules! clear_alarm {
    ($register:expr, $channel:expr) => {
        $register.write(|w| w.target($channel).clear_bit_by_one())
    };
}
#[cfg(feature = "chip")]
pub(crate) use clear_alarm;

#[cfg(test)]
#[allow(
    unsafe_code,
    reason = "exercise real PAC volatile writes in local memory"
)]
mod tests {
    #[test]
    fn acknowledgement_overwrites_only_its_w1c_bit_on_both_chips() {
        macro_rules! check_register {
            ($register:ty, $channel:literal, $expected:literal) => {{
                // SAFETY: these PAC registers contain only a u32 VolatileCell;
                // zero is valid and the local register is properly aligned.
                let register: $register = unsafe { core::mem::zeroed() };
                // SAFETY: as_ptr addresses this local register's live u32.
                // Seed every bit to catch an accidental read-modify-write.
                unsafe { register.as_ptr().write_volatile(u32::MAX) };
                clear_alarm!(register, $channel);
                // SAFETY: the pointer still addresses the local u32. Real
                // INT_CLR is write-only; this observes the bus write value.
                assert_eq!(unsafe { register.as_ptr().read_volatile() }, $expected);
            }};
        }
        check_register!(esp32s31_pac::systimer::INT_CLR, 0, 1);
        check_register!(esp32s31_pac::systimer::INT_CLR, 1, 2);
        check_register!(esp32s31_pac::systimer::INT_CLR, 2, 4);
        check_register!(esp32c5_pac::systimer::INT_CLR, 0, 1);
        check_register!(esp32c5_pac::systimer::INT_CLR, 1, 2);
        check_register!(esp32c5_pac::systimer::INT_CLR, 2, 4);
    }
}
