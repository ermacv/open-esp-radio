//! The core-control owner over host memory: each test fills the blocks with
//! one pattern and checks what the owner reads back, never where a word
//! lies.

use std::{vec, vec::Vec};

use super::{CoreControlRegisters, HOST_SIZES, HpCore};

struct Memory([Vec<u32>; 4]);

impl Memory {
    fn filled(fill: u32) -> Self {
        Self(HOST_SIZES.map(|size| vec![fill; size / 4]))
    }

    fn registers(
        &mut self,
    ) -> CoreControlRegisters<
        oer_mmio::Host<'_>,
        oer_mmio::Host<'_>,
        oer_mmio::Host<'_>,
        oer_mmio::Host<'_>,
    > {
        let [stall, status, clocks, reset] = &mut self.0;
        CoreControlRegisters::host(stall, status, clocks, reset)
            .expect("host memory holds the blocks")
    }
}

#[test]
fn a_stall_request_names_one_core_and_keeps_the_other_core_s_request() {
    let mut memory = Memory::filled(0);
    let mut registers = memory.registers();
    registers.request_stall(HpCore::Core1);
    assert!(registers.stall_requested(HpCore::Core1));
    assert!(!registers.stall_requested(HpCore::Core0));
    registers.request_stall(HpCore::Core0);
    registers.request_run(HpCore::Core1);
    assert!(registers.stall_requested(HpCore::Core0));
    assert!(!registers.stall_requested(HpCore::Core1));
}

#[test]
fn the_stall_status_is_the_hardware_s_and_not_the_request() {
    let mut memory = Memory::filled(0);
    let mut registers = memory.registers();
    registers.request_stall(HpCore::Core1);
    // Nothing reports the stall yet: the request alone is no confirmation.
    assert!(!registers.is_stalled(HpCore::Core1));
    drop(registers);
    let mut memory = Memory::filled(u32::MAX);
    let registers = memory.registers();
    assert!(registers.is_stalled(HpCore::Core0) && registers.is_stalled(HpCore::Core1));
}

#[test]
fn holding_core_1_gates_its_clocks_and_asserts_its_reset_and_releasing_undoes_it() {
    let mut memory = Memory::filled(0);
    let mut registers = memory.registers();
    assert!(!registers.core1_held());
    registers.hold_core1();
    assert!(registers.core1_held());
    let control = registers.clocks.read_hpcore1_ctrl0();
    assert!(!control.core1_clic_clk_en() && !control.core1_force_norst());
    registers.release_core1();
    assert!(!registers.core1_held());
    let control = registers.clocks.read_hpcore1_ctrl0();
    assert!(control.core1_cpu_clk_en() && control.core1_clic_clk_en());
}

#[test]
fn a_core_reset_pulses_only_that_core_s_word() {
    let mut memory = Memory::filled(0);
    memory.registers().reset_core(HpCore::Core1);
    let changed: Vec<u32> = memory.0[3]
        .iter()
        .copied()
        .filter(|word| *word != 0)
        .collect();
    assert_eq!(changed.len(), 1, "one word holds the pulse");
    let mut memory_core0 = Memory::filled(0);
    memory_core0.registers().reset_core(HpCore::Core0);
    assert_ne!(memory.0[3], memory_core0.0[3], "each core has its own word");
}

#[test]
fn uart0_s_clock_returns_to_undivided_xtal_and_core_1_s_word_keeps_its_bits() {
    let mut memory = Memory::filled(u32::MAX);
    let mut registers = memory.registers();
    registers.restore_uart0_clock();
    let clock = registers.clocks.read_uart0_ctrl0();
    assert_eq!(clock.clk_src_sel().get(), 0);
    assert_eq!(
        (
            clock.sclk_div_num(),
            clock.sclk_div_numerator(),
            clock.sclk_div_denominator()
        ),
        (0, 0, 0)
    );
    assert!(clock.clk_en());
    // Core 1's word beside it keeps every bit.
    let control = registers.clocks.read_hpcore1_ctrl0();
    assert!(control.core1_cpu_clk_en() && control.core1_global_rst_en());
}
