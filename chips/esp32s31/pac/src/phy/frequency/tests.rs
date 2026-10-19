use super::{
    ChannelCbwFields, bss_tx_offset, channel_cbw_fields, nrx_frequency_quotient, rv32_signed_div,
};

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
