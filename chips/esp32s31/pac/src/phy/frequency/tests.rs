use super::{
    ChannelCbwFields, PhyFrequencyI2cNumberAddresses, bss_tx_offset, channel_cbw_fields,
    nrx_frequency_quotient, publish_i2c_number_addresses, publish_nrx_frequency, rv32_signed_div,
};
use crate::generated::phy_frequency_channel_oracle::{
    self as frequency, PhyFrequencyChannelOracle,
};
use std::{vec, vec::Vec};

fn changed(before: &[u32], after: &[u32]) -> Vec<u32> {
    before
        .iter()
        .zip(after)
        .map(|(before, after)| before ^ after)
        .filter(|bits| *bits != 0)
        .collect()
}

#[test]
fn bss_offsets_retain_complete_rom_branches() {
    assert_eq!(bss_tx_offset(0), 0);
    assert_eq!(bss_tx_offset(1), 0);
    assert_eq!(bss_tx_offset(2), 2);
    assert_eq!(bss_tx_offset(3), 1);
    assert_eq!(bss_tx_offset(4), 0);
}

#[test]
fn channel_cbw_derivation_retains_both_complete_rom_paths() {
    assert_eq!(
        channel_cbw_fields(0),
        ChannelCbwFields {
            tx_offset: 0,
            control_0: 0,
            control_1_high: 0,
            control_1_low: 0,
        }
    );
    assert_eq!(
        channel_cbw_fields(2),
        ChannelCbwFields {
            tx_offset: 0,
            control_0: 0,
            control_1_high: 1,
            control_1_low: 1,
        }
    );
    assert_eq!(
        channel_cbw_fields(0x50),
        ChannelCbwFields {
            tx_offset: 0,
            control_0: 0,
            control_1_high: 4,
            control_1_low: 0,
        }
    );
    assert_eq!(
        channel_cbw_fields(0x53),
        ChannelCbwFields {
            tx_offset: 3,
            control_0: 3,
            control_1_high: 4,
            control_1_low: 0,
        }
    );
    assert_eq!(channel_cbw_fields(0x1_010), channel_cbw_fields(0x10));
    assert_eq!(channel_cbw_fields(0x1_00f).tx_offset, 0x0f);
}

#[test]
fn nrx_division_matches_the_complete_rv32_input_domain() {
    assert_eq!(rv32_signed_div(80, 0), -1);
    assert_eq!(rv32_signed_div(i32::MIN, -1), i32::MIN);
    assert_eq!(rv32_signed_div(-1_610_612_736, 7), -230_087_533);

    assert_eq!(nrx_frequency_quotient(0, 0), 0x00ff_ffff);
    assert_eq!(nrx_frequency_quotient(0x19, 7), 0x0049_2493);
}

#[test]
fn the_i2c_number_prefix_is_one_rmw_and_the_address_words_are_written_whole() {
    let addresses = PhyFrequencyI2cNumberAddresses::new([0; 11]).expect("zero is an address");
    let before = vec![u32::MAX; frequency::SIZE / 4];
    let mut after = before.clone();
    let mut registers =
        PhyFrequencyChannelOracle::host(&mut after).expect("memory holds the block");
    publish_i2c_number_addresses(&mut registers, &addresses);
    let control = registers.read_i2c_number_control();
    assert_eq!(control.bits_8_12_opaque().get(), 0);
    assert_eq!(control.bits_13_17_opaque().get(), 0);
    assert_eq!(
        control.memory_read_mode().get(),
        3,
        "the RMW keeps the read mode"
    );
    drop(registers);
    let difference = changed(&before, &after);
    // The prefix word and the three address words, each from all ones.
    assert_eq!(difference.len(), 4);
    assert_eq!(
        difference.iter().filter(|bits| **bits == u32::MAX).count(),
        3
    );
    assert_eq!(
        difference
            .iter()
            .filter(|bits| bits.count_ones() == 10)
            .count(),
        1
    );
}

#[test]
fn the_i2c_number_prefix_takes_the_first_two_addresses() {
    let mut values = [0; 11];
    values[0] = 0x15;
    values[1] = 0x0a;
    let addresses = PhyFrequencyI2cNumberAddresses::new(values).expect("five-bit addresses");
    let mut memory = vec![0; frequency::SIZE / 4];
    let mut registers =
        PhyFrequencyChannelOracle::host(&mut memory).expect("memory holds the block");
    publish_i2c_number_addresses(&mut registers, &addresses);
    let control = registers.read_i2c_number_control();
    assert_eq!(control.bits_8_12_opaque().get(), 0x15);
    assert_eq!(control.bits_13_17_opaque().get(), 0x0a);
}

#[test]
fn the_nrx_frequency_keeps_the_high_fields_it_read_and_stores_the_quotient() {
    for fill in [0, u32::MAX, 0x2300_0000] {
        let mut memory = vec![fill; frequency::SIZE / 4];
        let mut registers =
            PhyFrequencyChannelOracle::host(&mut memory).expect("memory holds the block");
        let read = registers.read_nrx_frequency_control();
        let shift = read.bits_24_28_opaque().get() + read.bits_29_31_opaque().get() * 0x20;
        publish_nrx_frequency(&mut registers, 2412);
        let written = registers.read_nrx_frequency_control();
        assert_eq!(written.bits_24_28_opaque(), read.bits_24_28_opaque());
        assert_eq!(written.bits_29_31_opaque(), read.bits_29_31_opaque());
        assert_eq!(
            written.bits_0_23_opaque().get(),
            nrx_frequency_quotient(shift, 2412)
        );
    }
}
