use super::{
    ChannelCbwFields, PhyFrequencyI2cNumberAddresses, bss_tx_offset, channel_cbw_fields,
    configure_i2c_number_addresses, configure_nrx_frequency,
    configure_nrx_frequency_between_samples, nrx_frequency_quotient, residue_of_eight,
    residue_of_sixteen, rv32_signed_div,
};
use crate::radio::phy::registers::FrequencyRegisters;
use oer_chips_esp32s31_pac::Bits;
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
    assert_eq!(bss_tx_offset(0).get(), 0);
    assert_eq!(bss_tx_offset(1).get(), 0);
    assert_eq!(bss_tx_offset(2).get(), 2);
    assert_eq!(bss_tx_offset(3).get(), 1);
    assert_eq!(bss_tx_offset(4).get(), 0);
}

#[test]
fn channel_cbw_derivation_retains_both_complete_rom_paths() {
    assert_eq!(channel_cbw_fields(0), cbw_fields(0, 0, 0, 0));
    assert_eq!(channel_cbw_fields(2), cbw_fields(0, 0, 1, 1));
    assert_eq!(channel_cbw_fields(0x50), cbw_fields(0, 0, 4, 0));
    assert_eq!(channel_cbw_fields(0x53), cbw_fields(3, 3, 4, 0));
    assert_eq!(channel_cbw_fields(0x1_010), channel_cbw_fields(0x10));
    assert_eq!(channel_cbw_fields(0x1_00f).tx_offset.get(), 0x0f);
}

#[test]
fn nrx_division_matches_the_complete_rv32_input_domain() {
    assert_eq!(rv32_signed_div(80, 0), -1);
    assert_eq!(rv32_signed_div(i32::MIN, -1), i32::MIN);
    assert_eq!(rv32_signed_div(-1_610_612_736, 7), -230_087_533);

    assert_eq!(nrx_frequency_quotient(0, 0), 0xffff_ffff);
    // The whole quotient; the field keeps its low 24 bits, 0x49_2493.
    assert_eq!(nrx_frequency_quotient(0x19, 7), 0xf249_2493);
}

#[test]
fn the_i2c_number_prefix_is_one_rmw_and_the_address_words_are_written_whole() {
    let addresses = PhyFrequencyI2cNumberAddresses::new([0; 11]).expect("zero is an address");
    let before = vec![u32::MAX; 31976 / 4];
    let mut after = before.clone();
    let mut registers = FrequencyRegisters::host(&mut after).expect("memory holds the block");
    configure_i2c_number_addresses(&mut registers, addresses);
    let control = registers.fe.read_i2c_number_control();
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
    let mut memory = vec![0; 31976 / 4];
    let mut registers = FrequencyRegisters::host(&mut memory).expect("memory holds the block");
    configure_i2c_number_addresses(&mut registers, addresses);
    let control = registers.fe.read_i2c_number_control();
    assert_eq!(control.bits_8_12_opaque().get(), 0x15);
    assert_eq!(control.bits_13_17_opaque().get(), 0x0a);
}

#[test]
fn the_nrx_frequency_keeps_the_high_fields_it_read_and_stores_the_quotient() {
    for fill in [0, u32::MAX, 0x2300_0000] {
        let mut memory = vec![fill; 31976 / 4];
        let mut registers = FrequencyRegisters::host(&mut memory).expect("memory holds the block");
        let read = registers.wifi_bb.read_nrx_frequency_control();
        let shift = read.frequency_numerator_shift().get() + read.bits_29_31_opaque().get() * 0x20;
        configure_nrx_frequency(&mut registers, 2412);
        let written = registers.wifi_bb.read_nrx_frequency_control();
        assert_eq!(
            written.frequency_numerator_shift(),
            read.frequency_numerator_shift()
        );
        assert_eq!(written.bits_29_31_opaque(), read.bits_29_31_opaque());
        assert_eq!(
            written.bits_0_23_opaque().get(),
            nrx_frequency_quotient(shift, 2412) & 0x00ff_ffff
        );
    }
}

#[test]
fn the_nrx_shift_comes_from_the_first_sample_and_the_high_fields_from_the_second() {
    // The first sample: shift selector 0x03 + 1 * 0x20.
    let mut memory = vec![0x2300_0000; 31976 / 4];
    let mut registers = FrequencyRegisters::host(&mut memory).expect("memory holds the block");
    let first_shift = 0x03 + 0x20;
    // The word the second sample reads: high fields 0x05 and 0.
    let second_low = oer_chips_esp32s31_pac::Bits::<5>::new(0x05).expect("five bits");
    let second_high = oer_chips_esp32s31_pac::Bits::<3>::new(0).expect("three bits");
    let second_shift = 0x05;
    assert_ne!(
        nrx_frequency_quotient(first_shift, 2412),
        nrx_frequency_quotient(second_shift, 2412)
    );
    let mut between_samples = 0;
    configure_nrx_frequency_between_samples(&mut registers, 2412, |registers| {
        between_samples += 1;
        registers.wifi_bb.modify_nrx_frequency_control(|w| {
            w.set_bits_0_23_opaque(oer_chips_esp32s31_pac::Bits::constant::<0>())
                .set_frequency_numerator_shift(second_low)
                .set_bits_29_31_opaque(second_high);
        });
    });
    assert_eq!(between_samples, 1);
    let written = registers.wifi_bb.read_nrx_frequency_control();
    assert_eq!(written.frequency_numerator_shift(), second_low);
    assert_eq!(written.bits_29_31_opaque(), second_high);
    assert_eq!(
        written.bits_0_23_opaque().get(),
        nrx_frequency_quotient(first_shift, 2412) & 0x00ff_ffff
    );
}

/// The four fields' values, each checked to fit its field.
fn cbw_fields(
    tx_offset: u32,
    control_0: u32,
    control_1_high: u32,
    control_1_low: u32,
) -> ChannelCbwFields {
    ChannelCbwFields {
        tx_offset: Bits::new(tx_offset).unwrap(),
        control_0: Bits::new(control_0).unwrap(),
        control_1_high: Bits::new(control_1_high).unwrap(),
        control_1_low: Bits::new(control_1_low).unwrap(),
    }
}

#[test]
fn every_residue_is_the_value_modulo_the_field_s_power_of_two() {
    for value in 0..=u8::MAX {
        assert_eq!(
            crate::radio::phy::residue_of_four(u16::from(value)).get(),
            u32::from(value % 4)
        );
        assert_eq!(residue_of_eight(value).get(), u32::from(value % 8));
        assert_eq!(residue_of_sixteen(value).get(), u32::from(value % 16));
    }
}
