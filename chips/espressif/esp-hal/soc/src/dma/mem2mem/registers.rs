//! Typed upstream AXI-GDMA register operations and DMA visibility fences.

use super::{
    descriptor::BurstSize,
    status::{AxiGdmaMem2MemRxStatus, AxiGdmaMem2MemStatus, AxiGdmaMem2MemTxStatus},
    transfer::AxiGdmaMem2Mem,
};
use core::{
    arch::asm,
    sync::atomic::{Ordering, compiler_fence},
};
use esp_hal::peripherals::{AXI_GDMA, HP_SYS_CLKRST};

// System address map of the ESP32-S31, from ESP-IDF 4d59230d
// `components/soc/esp32s31/include/soc/soc.h`. The DMA windows describe what
// the AXI-GDMA can address, not a linker region or the board's population.

/// Internal SRAM, all 512 KiB (`SOC_DRAM_LOW..SOC_DRAM_HIGH`).
pub(super) const INTERNAL_SRAM_START: usize = 0x2f00_0000;
pub(super) const INTERNAL_SRAM_END: usize = 0x2f08_0000;

/// External PSRAM address window, 64 MiB (`SOC_EXTRAM_LOW..SOC_EXTRAM_HIGH`).
pub(super) const PSRAM_START: usize = 0x5000_0000;
pub(super) const PSRAM_END: usize = 0x5400_0000;

const CHANNEL: usize = 0;

const M2M_TRIGGER_ID: u8 = 6;

fn channel_status() -> AxiGdmaMem2MemStatus {
    let regs = AXI_GDMA::regs();
    let rx = regs.in_ch(CHANNEL).in_int().raw().read();
    let tx = regs.out_ch(CHANNEL).out_int().raw().read();
    AxiGdmaMem2MemStatus {
        rx: AxiGdmaMem2MemRxStatus {
            done: rx.in_done().bit_is_set(),
            success_eof: rx.in_suc_eof().bit_is_set(),
            error_eof: rx.in_err_eof().bit_is_set(),
            descriptor_error: rx.in_dscr_err().bit_is_set(),
            descriptor_empty: rx.in_dscr_empty().bit_is_set(),
            fifo_overflow: rx.infifo_l1_ovf().bit_is_set(),
            fifo_underflow: rx.infifo_l1_udf().bit_is_set(),
        },
        tx: AxiGdmaMem2MemTxStatus {
            done: tx.out_done().bit_is_set(),
            eof: tx.out_eof().bit_is_set(),
            descriptor_error: tx.out_dscr_err().bit_is_set(),
            total_eof: tx.out_total_eof().bit_is_set(),
            fifo_overflow: tx.outfifo_l1_ovf().bit_is_set(),
            fifo_underflow: tx.outfifo_l1_udf().bit_is_set(),
        },
    }
}

pub(super) fn terminal_status() -> Option<AxiGdmaMem2MemStatus> {
    let status = channel_status();
    status.terminal().then_some(status)
}

/// Enable the completion and failure sources that `terminal_status` reads.
pub(super) fn enable_channel_interrupts() {
    let regs = AXI_GDMA::regs();
    regs.in_ch(CHANNEL).in_int().ena().write(|writer| {
        writer
            .in_suc_eof()
            .set_bit()
            .in_err_eof()
            .set_bit()
            .in_dscr_err()
            .set_bit()
            .in_dscr_empty()
            .set_bit()
    });
    regs.out_ch(CHANNEL)
        .out_int()
        .ena()
        .write(|writer| writer.out_total_eof().set_bit().out_dscr_err().set_bit());
}

pub(super) fn disable_channel_interrupts() {
    let regs = AXI_GDMA::regs();
    regs.in_ch(CHANNEL).in_int().ena().write(|writer| writer);
    regs.out_ch(CHANNEL).out_int().ena().write(|writer| writer);
}

pub(super) fn enable_and_configure_group() {
    let clock = HP_SYS_CLKRST::regs().axi_pdma_ctrl0();
    clock.modify(|_, writer| writer.sys_clk_en().set_bit().rst_en().set_bit());
    clock.modify(|_, writer| writer.rst_en().clear_bit());

    let regs = AXI_GDMA::regs();
    // The channel state machines can consume and write back descriptors
    // while the shared AXI read/write masters remain stale. Reset both
    // layers before publishing any memory window or channel state. This
    // mirrors IDF's group reset and esp-hal's AXI-master initialization.
    regs.misc_conf().modify(|_, writer| {
        writer
            .clk_en()
            .set_bit()
            .axim_rst_rd_inter()
            .set_bit()
            .axim_rst_wr_inter()
            .set_bit()
    });
    regs.misc_conf().modify(|_, writer| {
        writer
            .axim_rst_rd_inter()
            .clear_bit()
            .axim_rst_wr_inter()
            .clear_bit()
    });
    // SAFETY: the four window bounds are the internal SRAM range and the
    // external range from the Flash XIP base (`SOC_IROM_LOW`) through the end
    // of the PSRAM window, which this module's descriptor and payload
    // validation already requires.
    regs.intr_mem_start_addr().write(|writer| unsafe {
        writer
            .access_intr_mem_start_addr()
            .bits(INTERNAL_SRAM_START as u32)
    });
    // SAFETY: see the window bounds above.
    regs.intr_mem_end_addr().write(|writer| unsafe {
        writer
            .access_intr_mem_end_addr()
            .bits((INTERNAL_SRAM_END - 1) as u32)
    });
    // SAFETY: see the window bounds above.
    regs.extr_mem_start_addr().write(|writer| unsafe {
        writer
            .access_extr_mem_start_addr()
            .bits(crate::FLASH_XIP_START as u32)
    });
    // SAFETY: see the window bounds above.
    regs.extr_mem_end_addr()
        .write(|writer| unsafe { writer.access_extr_mem_end_addr().bits(PSRAM_END as u32 - 1) });
}

pub(super) fn dma_fence() {
    compiler_fence(Ordering::SeqCst);
    // SAFETY: a full memory fence only orders accesses; it reads and writes
    // no memory and leaves registers and the stack unchanged.
    unsafe { asm!("fence rw, rw", options(nostack)) };
    compiler_fence(Ordering::SeqCst);
}

impl<'d> AxiGdmaMem2Mem<'d> {
    pub(super) fn configure_channel(&mut self, rx_head: u32, tx_head: u32, burst: BurstSize) {
        self.stop_and_reset_channel();
        disable_channel_interrupts();
        let regs = AXI_GDMA::regs();
        let input = regs.in_ch(CHANNEL);
        let output = regs.out_ch(CHANNEL);

        input.in_int().clr().write(|writer| {
            writer
                .in_done()
                .clear_bit_by_one()
                .in_suc_eof()
                .clear_bit_by_one()
                .in_err_eof()
                .clear_bit_by_one()
                .in_dscr_err()
                .clear_bit_by_one()
                .in_dscr_empty()
                .clear_bit_by_one()
                .infifo_l1_ovf()
                .clear_bit_by_one()
                .infifo_l1_udf()
                .clear_bit_by_one()
        });
        output.out_int().clr().write(|writer| {
            writer
                .out_done()
                .clear_bit_by_one()
                .out_eof()
                .clear_bit_by_one()
                .out_dscr_err()
                .clear_bit_by_one()
                .out_total_eof()
                .clear_bit_by_one()
                .outfifo_l1_ovf()
                .clear_bit_by_one()
                .outfifo_l1_udf()
                .clear_bit_by_one()
        });

        // SAFETY: `BurstSize::register_value` yields only the encoded 16-,
        // 32- or 64-byte burst selector values.
        input.in_conf0().modify(|_, writer| unsafe {
            writer
                .mem_trans_en()
                .set_bit()
                .indscr_burst_en()
                .set_bit()
                .in_burst_size_sel()
                .bits(burst.register_value())
        });
        input
            .in_conf1()
            .modify(|_, writer| writer.in_check_owner().set_bit());
        // SAFETY: trigger 6 selects the memory-to-memory peripheral.
        input
            .in_peri_sel()
            .write(|writer| unsafe { writer.peri_in_sel().bits(M2M_TRIGGER_ID) });
        // SAFETY: callers pass the head of a descriptor chain that they have
        // validated as 8-byte aligned internal SRAM, built and still own.
        input
            .in_link2()
            .write(|writer| unsafe { writer.inlink_addr().bits(rx_head) });

        // SAFETY: see the burst selector above.
        output.out_conf0().modify(|_, writer| unsafe {
            writer
                .out_auto_wrback()
                .set_bit()
                .out_eof_mode()
                .set_bit()
                .outdscr_burst_en()
                .set_bit()
                .out_burst_size_sel()
                .bits(burst.register_value())
        });
        output
            .out_conf1()
            .modify(|_, writer| writer.out_check_owner().set_bit());
        // SAFETY: see the memory-to-memory trigger above.
        output
            .out_peri_sel()
            .write(|writer| unsafe { writer.peri_out_sel().bits(M2M_TRIGGER_ID) });
        // SAFETY: see the validated descriptor chain head above.
        output
            .out_link2()
            .write(|writer| unsafe { writer.outlink_addr().bits(tx_head) });
    }
}

impl<'d> AxiGdmaMem2Mem<'d> {
    pub(super) fn start(&mut self) {
        dma_fence();
        let regs = AXI_GDMA::regs();
        regs.in_ch(CHANNEL)
            .in_link1()
            .modify(|_, writer| writer.inlink_start().set_bit());
        regs.out_ch(CHANNEL)
            .out_link1()
            .modify(|_, writer| writer.outlink_start().set_bit());
    }
}

impl<'d> AxiGdmaMem2Mem<'d> {
    pub(super) fn stop_and_reset_channel(&mut self) {
        let regs = AXI_GDMA::regs();
        let input = regs.in_ch(CHANNEL);
        let output = regs.out_ch(CHANNEL);

        input
            .in_link1()
            .modify(|_, writer| writer.inlink_stop().set_bit());
        output
            .out_link1()
            .modify(|_, writer| writer.outlink_stop().set_bit());
        input
            .in_conf0()
            .modify(|_, writer| writer.in_rst().set_bit());
        input
            .in_conf0()
            .modify(|_, writer| writer.in_rst().clear_bit());
        output
            .out_conf0()
            .modify(|_, writer| writer.out_rst().set_bit());
        output
            .out_conf0()
            .modify(|_, writer| writer.out_rst().clear_bit());
    }
}
