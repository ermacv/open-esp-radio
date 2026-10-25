//! The vendor SVD's blocks under the conservative defaults, over host memory.

use crate::soc::{Interrupt, efuse::Efuse, soc_etm::SocEtm, uart0::Uart0};

#[test]
fn a_fully_declared_plain_register_is_read_and_written_in_place() {
    let mut words = [0_u32; crate::soc::efuse::SIZE / 4];
    words[3] = 0x5555_5555;
    let mut efuse = Efuse::host(&mut words).expect("memory holds the block");
    let index = oer_mmio::Index::new(2).expect("PGM_DATA has eight words");
    efuse.modify_pgm_data(index, |data| {
        data.set_pgm_data(0xdead_beef);
    });
    assert_eq!(efuse.read_pgm_data(index).pgm_data(), 0xdead_beef);
    drop(efuse);
    assert_eq!(words[2], 0xdead_beef);
    // The neighbouring word keeps its value.
    assert_eq!(words[3], 0x5555_5555);
}

#[test]
fn a_register_its_header_does_not_confirm_is_only_read() {
    let mut words = [0_u32; crate::soc::efuse::SIZE / 4];
    words[0x30 / 4] = 0x8000_0001;
    let efuse = Efuse::host(&mut words).expect("memory holds the block");
    // The header does not confirm every bit of RD_REPEAT_DATA0: its owner
    // reads it and has no write (`soc.report.toml` lists it as `read`).
    let _: crate::soc::efuse::rd_repeat_data0::R = efuse.read_rd_repeat_data0();
}

#[test]
fn a_block_names_its_interrupt_source_and_the_enum_round_trips() {
    assert_eq!(
        <Uart0<oer_mmio::Host<'_>> as oer_mmio::InterruptSource>::SOURCE,
        9
    );
    assert_eq!(Interrupt::UART0.source(), 9);
    assert_eq!(Interrupt::from_source(9), Some(Interrupt::UART0));
    assert_eq!(Interrupt::from_source(u16::MAX), None);
}

#[test]
fn the_source_count_is_no_interrupt() {
    // ESP-IDF's ETS_MAX_INTR_SOURCE (169) counts the sources.
    assert_eq!(Interrupt::from_source(169), None);
}

#[test]
fn a_write_one_to_clear_status_acknowledges_exactly_its_sample() {
    let mut words = [0_u32; crate::soc::soc_etm::SIZE / 4];
    words.fill(u32::MAX);
    let etm = SocEtm::host(&mut words).expect("memory holds the block");
    let sample = etm.sample_evt_st0();
    drop(etm);
    // No event is pending any more; the acknowledgement writes the sample,
    // not the register's current state.
    words.fill(0);
    let mut etm = SocEtm::host(&mut words).expect("memory holds the block");
    etm.acknowledge_evt_st0(sample);
    drop(etm);
    assert_eq!(words.iter().filter(|word| **word != 0).count(), 1);
}

#[test]
fn the_report_lists_the_header_s_narrowings() {
    let report = include_str!("../../registers/published/soc.report.toml");
    // GMAC's remote wake-up filter port is read and written in sequence
    // (`gmac_reg.h:916`); its PMT and LPI status bits are cleared by a read
    // (`gmac_reg.h:971`, `:1037`).
    for register in [
        "REGISTER10_REMOTEWAKEUPFRAMEFILTERREGISTER",
        "REGISTER11_PMTCONTROLANDSTATUSREGISTER",
        "REGISTER12_LPICONTROLANDSTATUSREGISTER",
        "REGISTER458_TIMESTAMPSTATUSREGISTER",
    ] {
        assert!(
            report.contains(&std::format!(
                "peripheral = \"GMAC\"\nregister = \"{register}\"\napi = \"none\""
            )),
            "{register}"
        );
    }
}
