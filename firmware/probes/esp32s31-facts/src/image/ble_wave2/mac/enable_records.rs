//! Bank-zero enable diagnostics, one current model field per output value.
use oer_esp32s31_pac::bt_mac::irq_enable_0;

pub(super) fn bank0(a: irq_enable_0::R) -> [[u32; 3]; 5] {
    [
        [
            u32::from(a.source_0()),
            u32::from(a.source_1()),
            u32::from(a.source_8()),
        ],
        [
            u32::from(a.source_13()),
            u32::from(a.source_15()),
            u32::from(a.source_16()),
        ],
        [
            u32::from(a.source_21()),
            a.sources_27_28().get(),
            u32::from(a.iso_cig_reset_source()),
        ],
        [
            a.bits_9_12_opaque().get(),
            u32::from(a.bit_14_opaque()),
            a.bits_17_20_opaque().get(),
        ],
        [
            a.bits_22_26_opaque().get(),
            a.bits_29_31_opaque().get(),
            a.bits_3_7_opaque().get(),
        ],
    ]
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "host register fixture construction")]
mod tests {
    use super::*;
    use oer_esp32s31_pac::{bt_mac::BluetoothInterruptBank, instances};
    use oer_mmio::Bits;

    #[test]
    fn split_fields_remain_independent_without_displacing_other_observations() {
        let mut memory = vec![u32::MAX; instances::bt_mac::SIZE / size_of::<u32>()];
        let mut bank = BluetoothInterruptBank::host(&mut memory).expect("host aperture");
        for (cig, opaque) in [(true, 0), (false, 31), (true, 21), (false, 0)] {
            bank.modify_irq_enable_0(|w| {
                w.set_iso_cig_reset_source(cig)
                    .set_bits_3_7_opaque(Bits::new(opaque).expect("five-bit observation"));
            });
            assert_eq!(
                bank0(bank.read_irq_enable_0()),
                [
                    [1, 1, 1],
                    [1, 1, 1],
                    [1, 3, u32::from(cig)],
                    [15, 1, 15],
                    [31, 7, opaque],
                ]
            );
        }
    }
}
