//! The core-control owner over host memory.

use std::vec;

use super::{CoreControlRegisters, HOST_SIZE};

#[test]
fn uart0_s_clock_returns_to_undivided_xtal_with_its_gate_open() {
    let mut words = vec![u32::MAX; HOST_SIZE / 4];
    let mut registers =
        CoreControlRegisters::host(&mut words).expect("host memory holds the block");
    registers.restore_uart0_clock();
    let clock = registers.0.read_uart0_sclk_conf();
    assert_eq!(clock.uart0_sclk_sel().get(), 0);
    assert_eq!(
        (
            clock.uart0_sclk_div_a().get(),
            clock.uart0_sclk_div_b().get(),
            clock.uart0_sclk_div_num()
        ),
        (0, 0, 0)
    );
    assert!(clock.uart0_sclk_en());
}
