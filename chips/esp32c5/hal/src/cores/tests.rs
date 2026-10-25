//! The reset's preparation over host memory.

use std::vec;

use oer_esp32c5_pac::platform::{CoreControlRegisters, core_control::HOST_SIZE};

use super::prepare_reset;

#[test]
fn the_reset_opens_uart0_s_clock_for_the_boot_rom() {
    let mut words = vec![0; HOST_SIZE / 4];
    prepare_reset(
        &mut CoreControlRegisters::host(&mut words).expect("host memory holds the block"),
    );
    assert!(words.iter().any(|word| *word != 0), "the clock gate opened");
}
