//! The vendor SVD's blocks under the conservative defaults, over host memory.

use crate::soc::{Interrupt, cache::Cache, efuse::Efuse, soc_etm::SocEtm, uart0::Uart0};

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
fn a_cache_sync_command_starts_once_and_refuses_while_one_runs() {
    let mut words = [0_u32; crate::soc::cache::SIZE / 4];
    // SYNC_RGID (bits 8:5) is 3; the overlay's `[[commands]]` entry makes
    // SYNC_CTRL a command register written only by its starts.
    let offset = crate::soc::cache::sync_ctrl::OFFSET / 4;
    words[offset] = 3 << 5;
    let mut cache = Cache::host(&mut words).expect("memory holds the block");
    // CACHE.SYNC_CTRL's four sync commands are mutually exclusive
    // (`cache_reg.h:1576`): the hardware runs one at a time.
    assert_eq!(cache.start_sync_ctrl_writeback_ena(), Ok(()));
    assert!(cache.sync_ctrl_writeback_ena_busy());
    assert!(!cache.sync_ctrl_invalidate_ena_busy());
    assert_eq!(
        cache.start_sync_ctrl_invalidate_ena(),
        Err(oer_mmio::CommandBusy { running: 1 << 2 })
    );
    let sync = cache.read_sync_ctrl();
    assert!(!sync.invalidate_ena(), "a refused start writes nothing");
    assert_eq!(sync.sync_rgid().get(), 3, "a start keeps the group id read");
    drop(cache);
    assert_eq!(words[offset], (3 << 5) | (1 << 2));
}

#[test]
fn a_flash_user_command_waits_for_every_self_clearing_bit_of_its_register() {
    let mut words = [0_u32; crate::soc::spi1::SIZE / 4];
    // FLASH_RDSR (bit 27) still runs: a debugging command the application
    // never starts (`spi1_mem_c_reg.h:125`), held at zero by every start.
    words[0] = 1 << 27;
    let mut spi = crate::soc::spi1::Spi1::host(&mut words).expect("memory holds the block");
    assert_eq!(
        spi.start_spi_mem_cmd_spi_mem_usr(),
        Err(oer_mmio::CommandBusy { running: 1 << 27 })
    );
    drop(spi);
    assert_eq!(words[0], 1 << 27, "a refused start writes nothing");
    words[0] = 0;
    let mut spi = crate::soc::spi1::Spi1::host(&mut words).expect("memory holds the block");
    assert_eq!(spi.start_spi_mem_cmd_spi_mem_usr(), Ok(()));
    assert!(spi.spi_mem_cmd_spi_mem_usr_busy());
    drop(spi);
    assert_eq!(words[0], 1 << 18);
}

#[test]
fn a_word_of_write_triggers_alone_is_written_from_zero_and_never_read() {
    let mut words = [0_u32; crate::soc::pmu::SIZE / 4];
    let offset = crate::soc::pmu::imm_hp_ck_power_1::OFFSET / 4;
    // Whatever the word reads, a trigger writes its field alone.
    words[offset] = u32::MAX;
    let mut pmu = crate::soc::pmu::Pmu::host(&mut words).expect("memory holds the block");
    pmu.trigger_imm_hp_ck_power_1_tie_high_xpd_pll(oer_mmio::Bits::constant::<0b1010>());
    drop(pmu);
    assert_eq!(words[offset], 0b1010 << 27);
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

#[test]
fn peripherals_with_semantics_described_only_in_text_have_no_block() {
    // GMAC, SDHOST and USB_OTG_HS mix write-one-to-clear flags, destructive
    // reads and self-clearing bits into `R/W` fields: the generated module
    // renders none of their registers until a typed API exists.
    let generated = include_str!("generated.rs");
    for module in [
        "\n    pub mod gmac {",
        "\n    pub mod sdhost {",
        "\n    pub mod usb_otg_hs {",
    ] {
        assert!(!generated.contains(module), "{module}");
    }
}
