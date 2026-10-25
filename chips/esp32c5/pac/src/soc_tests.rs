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
        47
    );
    assert_eq!(Interrupt::UART0.source(), 47);
    assert_eq!(Interrupt::from_source(47), Some(Interrupt::UART0));
    assert_eq!(Interrupt::from_source(u16::MAX), None);
}

#[test]
fn the_source_count_is_no_interrupt() {
    // ESP-IDF's ETS_MAX_INTR_SOURCE (84) counts the sources.
    assert_eq!(Interrupt::from_source(84), None);
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
    // TIMG's second timer is reserved in the device layout
    // (`timer_group_struct.h:607-608`).
    for register in ["T1ALARMLO", "T1LOADLO"] {
        assert!(
            report.contains(&std::format!(
                "peripheral = \"TIMG0\"\nregister = \"{register}\"\napi = \"none\""
            )),
            "{register}"
        );
    }
    // PCR.CTRL_CLK_OUT_EN bit 6 is CLK_480M_OEN (`pcr_reg.h:2163`).
    assert!(report.contains(
        "register = \"CTRL_CLK_OUT_EN\"\nsvd = \"CLK_320M_OEN\"\nsource = \"CLK_480M_OEN\""
    ));
}

#[test]
fn a_user_command_and_its_program_erase_qualifier_start_in_one_write() {
    // SPI_MEM_FLASH_PE goes with SPI_MEM_USR (`spi1_mem_reg.h:33`), as
    // `spimem_flash_ll_user_start` writes both (`spimem_flash_ll.h:453`).
    let mut words = [0_u32; crate::soc::spi1::SIZE / 4];
    let mut spi = crate::soc::spi1::Spi1::host(&mut words).expect("memory holds the block");
    assert_eq!(spi.start_spi_mem_cmd_spi_mem_usr(true), Ok(()));
    // A second start while the first runs refuses: it would clear the
    // qualifier of the running command.
    assert_eq!(
        spi.start_spi_mem_cmd_spi_mem_usr(false),
        Err(oer_mmio::CommandBusy {
            running: (1 << 18) | (1 << 17)
        })
    );
    drop(spi);
    assert_eq!(words[0], (1 << 18) | (1 << 17));
    words[0] = 0;
    let mut spi = crate::soc::spi1::Spi1::host(&mut words).expect("memory holds the block");
    assert_eq!(spi.start_spi_mem_cmd_spi_mem_usr(false), Ok(()));
    drop(spi);
    assert_eq!(
        words[0],
        1 << 18,
        "a read or write command has no qualifier"
    );
}
