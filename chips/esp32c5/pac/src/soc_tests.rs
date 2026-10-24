//! The vendor SVD's blocks under the conservative defaults, over host memory.

use crate::soc::{Interrupt, efuse::Efuse, uart0::Uart0};

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
fn a_register_with_undeclared_bits_is_only_read() {
    let mut words = [0_u32; crate::soc::efuse::SIZE / 4];
    words[0x30 / 4] = 0x8000_0001;
    let efuse = Efuse::host(&mut words).expect("memory holds the block");
    // RD_REPEAT_DATA0 has undeclared bits: its owner reads it and has no
    // write (`soc.report.toml` lists it as `read`).
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
