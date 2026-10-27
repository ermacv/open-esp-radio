//! The core-control owner over host memory: each test fills the blocks with
//! one pattern and checks what the owner reads back and which words it
//! changed.

use std::{vec, vec::Vec};

use super::{CoreControlRegisters, HOST_SIZES, Hart, UART0_HOST_SIZE, Uart0ClockRegisters};

struct Memory([Vec<u32>; 5]);

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
        oer_mmio::Host<'_>,
    > {
        let [stall, status, clocks, reset, system] = &mut self.0;
        CoreControlRegisters::host(stall, status, clocks, reset, system)
            .expect("host memory holds the blocks")
    }
}

#[test]
fn a_stall_request_names_one_core_and_keeps_the_other_core_s_request() {
    let mut memory = Memory::filled(0);
    let mut registers = memory.registers();
    registers.request_stall(Hart::SECONDARIES[0].hart());
    assert!(registers.stall_requested(Hart::SECONDARIES[0].hart()));
    assert!(!registers.stall_requested(crate::platform::BOOT_HART));
    registers.request_stall(crate::platform::BOOT_HART);
    registers.request_run(Hart::SECONDARIES[0].hart());
    assert!(registers.stall_requested(crate::platform::BOOT_HART));
    assert!(!registers.stall_requested(Hart::SECONDARIES[0].hart()));
}

#[test]
fn the_stall_status_is_the_hardware_s_and_not_the_request() {
    let mut memory = Memory::filled(0);
    let mut registers = memory.registers();
    registers.request_stall(Hart::SECONDARIES[0].hart());
    // Nothing reports the stall yet: the request alone is no confirmation.
    assert!(!registers.is_stalled(Hart::SECONDARIES[0].hart()));
    drop(registers);
    let mut memory = Memory::filled(u32::MAX);
    let registers = memory.registers();
    assert!(
        registers.is_stalled(crate::platform::BOOT_HART)
            && registers.is_stalled(Hart::SECONDARIES[0].hart())
    );
}

#[test]
fn holding_core_1_gates_its_clocks_and_asserts_its_reset() {
    let mut memory = Memory::filled(0);
    let mut registers = memory.registers();
    assert!(!registers.held(Hart::SECONDARIES[0].hart()));
    registers.hold(Hart::SECONDARIES[0]);
    assert!(registers.held(Hart::SECONDARIES[0].hart()));
    let control = registers
        .clocks
        .read_hpcore_ctrl0(Hart::SECONDARIES[0].hart().index());
    assert!(!control.reg_core_clic_clk_en() && !control.reg_core_force_norst());
}

#[test]
fn a_core_reset_pulses_only_that_core_s_word() {
    let mut memory = Memory::filled(0);
    memory.registers().reset_core(Hart::SECONDARIES[0].hart());
    let changed: Vec<u32> = memory.0[3]
        .iter()
        .copied()
        .filter(|word| *word != 0)
        .collect();
    assert_eq!(changed.len(), 1, "one word holds the pulse");
    let mut memory_core0 = Memory::filled(0);
    memory_core0
        .registers()
        .reset_core(crate::platform::BOOT_HART);
    assert_ne!(memory.0[3], memory_core0.0[3], "each core has its own word");
}

#[test]
fn uart0_s_clock_returns_to_undivided_xtal_with_its_gate_open() {
    let mut words = vec![u32::MAX; UART0_HOST_SIZE / 4];
    let mut registers = Uart0ClockRegisters::host(&mut words).expect("host memory holds the block");
    registers.restore_uart0_clock();
    let clock = registers.0.read_uart0_ctrl0();
    assert_eq!(clock.reg_uart0_clk_src_sel().get(), 0);
    assert_eq!(
        (
            clock.reg_uart0_sclk_div_num(),
            clock.reg_uart0_sclk_div_numerator(),
            clock.reg_uart0_sclk_div_denominator()
        ),
        (0, 0, 0)
    );
    assert!(clock.reg_uart0_clk_en());
    assert!(
        clock.reg_uart0_apb_clk_en() && clock.reg_uart0_core_rst_en(),
        "the bus clock and reset bits are kept"
    );
}

#[test]
fn the_boot_address_store_replaces_the_whole_word_including_zero() {
    let mut memory = Memory::filled(u32::MAX);
    let before = memory.0.clone();
    let entry = 0x2f01_2400;
    {
        let mut registers = memory.registers();
        registers.set_boot_address(Hart::SECONDARIES[0], entry);
        assert_eq!(
            registers
                .system
                .read_boot_addr_hp_core(Hart::SECONDARIES[0].index())
                .boot_addr_hp_core(),
            entry
        );
        registers.set_boot_address(Hart::SECONDARIES[0], 0);
        assert_eq!(
            registers
                .system
                .read_boot_addr_hp_core(Hart::SECONDARIES[0].index())
                .boot_addr_hp_core(),
            0
        );
    }
    // SOURCE: pinned lp_system_reg.h:754-764, the boot handoff data word.
    let mut expected = before;
    expected[4][0x164 / 4] = 0;
    assert_eq!(memory.0, expected, "only the boot word was written");
}

#[test]
fn the_reset_trigger_keeps_configuration_without_acknowledging_wakeup() {
    // Host memory records the bus store, rather than emulating WTC hardware:
    // zero here proves the wakeup flag was not acknowledged.
    // SOURCE: pinned lp_system_reg.h:47-120 (WT 1/29, WTC 30).
    for initial in [0, u32::MAX, 0xd55a_5aa5, 0x2000_0000, 0x4000_0000] {
        let mut memory = Memory::filled(initial);
        let mut expected = memory.0.clone();
        expected[4][0x8 / 4] = (initial & !0x6000_0002) | 2;
        memory.registers().request_system_reset();
        assert_eq!(
            memory.0, expected,
            "ordinary and unassigned bits kept; WTC/other WT zero"
        );
    }
}
