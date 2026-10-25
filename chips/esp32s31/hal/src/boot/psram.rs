//! The PSRAM owner: the cache's controller (`PSRAM_MSPI0`), the command port
//! (`PSRAM_MSPI1`), the PSRAM pads and the PSRAM MMU.

use oer_espressif_boot::{
    Access, Busy,
    mmu::{MmuEntry, MmuRegisters, MmuTarget},
    psram::{
        CacheAccess, ChipSelectTiming, PsramCommand, PsramCommandRegisters,
        PsramControllerRegisters,
    },
};
use oer_mmio::{Bits, Fixed, Index, Placement};

use oer_esp32s31_pac::soc::{
    iomux_mspi_pin::{self, IomuxMspiPin},
    memspi2::{self, Memspi2},
    memspi3::{self, Memspi3},
};

/// The PSRAM controllers, their pads and the PSRAM MMU.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct PsramRegisters<
    P: Placement = Fixed<{ memspi2::BASE }>,
    Q: Placement = Fixed<{ memspi3::BASE }>,
    R: Placement = Fixed<{ iomux_mspi_pin::BASE }>,
> {
    memspi2: Memspi2<P>,
    memspi3: Memspi3<Q>,
    pins: IomuxMspiPin<R>,
}

impl<P: Placement, Q: Placement, R: Placement> PsramRegisters<P, Q, R> {
    /// Own the bootstrap's generated psram parts.
    pub fn new(parts: oer_esp32s31_pac::boot::PsramRegisters<P, Q, R>) -> Self {
        Self {
            memspi2: parts.memspi2,
            memspi3: parts.memspi3,
            pins: parts.pins,
        }
    }
}

/// The pads' drive strength.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_psram/device/esp_psram_impl_ap_oct.c:430`
/// (`mspi_timing_ll_pin_drv_set(2)`).
const PAD_DRIVE: Bits<3> = Bits::constant::<2>();

/// The cache's 32-bit address phase, as its length − 1.
/// SOURCE: ESP-IDF 4d59230d `psram_ctrlr_ll.h:70-362`.
const ADDRESS_BITLEN: Bits<6> = Bits::constant::<31>();
/// The cache's 16-bit synchronous read and write commands, the lengths as
/// length − 1.
/// SOURCE: ESP-IDF 4d59230d `psram_ctrlr_ll.h:70-362`.
const COMMAND_BITLEN: Bits<4> = Bits::constant::<15>();
/// SOURCE: ESP-IDF 4d59230d `psram_ctrlr_ll.h:70-362`: synchronous read/write commands.
const SYNC_READ: u16 = 0x0000;
/// SOURCE: ESP-IDF 4d59230d `psram_ctrlr_ll.h:70-362`: synchronous write command.
const SYNC_WRITE: u16 = 0x8080;

/// The controllers' bus clock, DLL, pads, chip select and the cache's octal
/// DDR access.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_mspi/esp32s31/include/hal/psram_ctrlr_ll.h:487-503`
/// (the bus divider: N = div − 1, H = div / 2 − 1, L = div − 1, or the
/// undivided clock), `:513-518` (the DLL), `:535-618` (chip select:
/// setup, hold, hold delay, split transfers), `:70-362` (the cache's
/// commands 0x0000/0x8080, 32-bit addresses, dummies, DDR, octal lines,
/// AXI and splices), `components/esp_hal_mspi/esp32s31/include/hal/mspi_ll.h:157-185`
/// (pad drive and the data strobe pad); esp-hal 1f646c5
/// `esp-hal/src/psram/oct_hex.rs:62-139,189-200,258-288` (the same words,
/// the cache's dummy lengths as the controller holds them).
impl<P: Placement, Q: Placement, R: Placement> PsramControllerRegisters
    for PsramRegisters<P, Q, R>
{
    fn set_bus_divider(&mut self, _: Access<'_>, divider: u8) {
        let undivided = divider <= 1;
        let (n, h) = if undivided {
            (0, 0)
        } else {
            (divider - 1, divider / 2 - 1)
        };
        self.memspi2.modify_sram_clk(|w| {
            w.set_sclk_equ_sysclk(undivided)
                .set_sclkcnt_n(n)
                .set_sclkcnt_h(h)
                .set_sclkcnt_l(n);
        });
        self.memspi3.modify_clock(|w| {
            w.set_clk_equ_sysclk(undivided)
                .set_clkcnt_n(n)
                .set_clkcnt_h(h)
                .set_clkcnt_l(n);
        });
    }

    fn enable_dll(&mut self, _: Access<'_>) {
        self.memspi2.modify_timing_cali(|w| {
            w.set_dll_timing_cali(true);
        });
        self.memspi2.modify_smem_timing_cali(|w| {
            w.set_dll_timing_cali(true);
        });
    }

    fn configure_pads(&mut self, _: Access<'_>) {
        let pins = &mut self.pins;
        pins.modify_psram_d_pin0(|w| {
            w.set_reg_psram_d_drv(PAD_DRIVE);
        });
        pins.modify_psram_q_pin0(|w| {
            w.set_reg_psram_q_drv(PAD_DRIVE);
        });
        pins.modify_psram_wp_pin0(|w| {
            w.set_reg_psram_wp_drv(PAD_DRIVE);
        });
        pins.modify_psram_hold_pin0(|w| {
            w.set_reg_psram_hold_drv(PAD_DRIVE);
        });
        pins.modify_psram_dq4_pin0(|w| {
            w.set_reg_psram_dq4_drv(PAD_DRIVE);
        });
        pins.modify_psram_dq5_pin0(|w| {
            w.set_reg_psram_dq5_drv(PAD_DRIVE);
        });
        pins.modify_psram_dq6_pin0(|w| {
            w.set_reg_psram_dq6_drv(PAD_DRIVE);
        });
        pins.modify_psram_dq7_pin0(|w| {
            w.set_reg_psram_dq7_drv(PAD_DRIVE);
        });
        pins.modify_psram_dqs_0_pin0(|w| {
            w.set_reg_psram_dqs_0_drv(PAD_DRIVE)
                .set_reg_psram_dqs_0_xpd(true);
        });
        pins.modify_psram_ck_pin0(|w| {
            w.set_reg_psram_ck_drv(PAD_DRIVE);
        });
        pins.modify_psram_cs_pin0(|w| {
            w.set_reg_psram_cs_drv(PAD_DRIVE);
        });
    }

    fn set_chip_select_timing(&mut self, _: Access<'_>, timing: ChipSelectTiming) {
        let cycles = |n: u8| u32::from(n.max(1)) - 1;
        self.memspi2.modify_smem_ac(|w| {
            w.set_cs_setup(true)
                .set_cs_hold(true)
                .set_split_trans_en(true)
                .set_cs_setup_time(Bits::truncate(cycles(timing.setup)))
                .set_cs_hold_time(Bits::truncate(cycles(timing.hold)))
                .set_cs_hold_delay(Bits::truncate(cycles(timing.hold_delay)));
        });
    }

    fn configure_cache_access(&mut self, _: Access<'_>, cache: CacheAccess) {
        let spi = &mut self.memspi2;
        spi.modify_cache_sctrl(|w| {
            w.set_cache_usr_saddr_4byte(true)
                .set_usr_wr_sram_dummy(true)
                .set_usr_rd_sram_dummy(true)
                .set_cache_sram_usr_rcmd(true)
                .set_sram_rdummy_cyclelen(Bits::truncate(u32::from(cache.read_dummy_cyclelen)))
                .set_sram_wdummy_cyclelen(Bits::truncate(u32::from(cache.write_dummy_cyclelen)))
                .set_sram_addr_bitlen(ADDRESS_BITLEN)
                .set_cache_sram_usr_wcmd(true)
                .set_sram_oct(true);
        });
        spi.modify_sram_cmd(|w| {
            w.set_sdin_oct(true)
                .set_sdout_oct(true)
                .set_saddr_oct(true)
                .set_scmd_oct(true)
                .set_sdummy_wout(true)
                .set_sdin_hex(false)
                .set_sdout_hex(false);
        });
        spi.modify_sram_drd_cmd(|w| {
            w.set_cache_sram_usr_rd_cmd_bitlen(COMMAND_BITLEN)
                .set_cache_sram_usr_rd_cmd_value(SYNC_READ);
        });
        spi.modify_sram_dwr_cmd(|w| {
            w.set_cache_sram_usr_wr_cmd_bitlen(COMMAND_BITLEN)
                .set_cache_sram_usr_wr_cmd_value(SYNC_WRITE);
        });
        spi.modify_ctrl1(|w| {
            w.set_ar_splice_en(true).set_aw_splice_en(true);
        });
        spi.modify_smem_ddr(|w| {
            w.set_var_dummy(true)
                .set_rdat_swp(false)
                .set_wdat_swp(false)
                .set_en(true);
        });
        spi.modify_cache_fctrl(|w| {
            w.set_axi_req_en(true).set_close_axi_inf_en(false);
        });
    }
}

/// The command port's first data word.
const FIRST_WORD: Index<16> = Index::constant::<0>();

/// The command port (`PSRAM_MSPI1`), with the effects of the ROM's
/// `esp_rom_spi_set_op_mode(3, OPI_DTR)` and `esp_rom_spi_cmd_config(3, …)`.
///
/// SOURCE(esp32s31): pinned rev0 ROM (esp-rom-elfs 20260528, SHA-256
/// a52ad751...ab87), Blobray function records of `esp_rom_spi_set_op_mode`
/// @0x2f811334 (mode 7: the default `esp_rom_spi_reset_rw_mode` with flag 0,
/// then `CTRL` octal command, address and data, fast read; `DDR` enabled,
/// its command DDR not disabled), `esp_rom_spi_reset_rw_mode` @0x2f811304
/// (flag 0: `CTRL`'s quad, dual, DIO, QIO and octal read modes and
/// `USER`'s write modes cleared) and `esp_rom_spi_cmd_config` @0x2f8114b6
/// (each phase enabled with its length − 1, or disabled with length 0;
/// the address written when an address phase is set; the data words not
/// read by the ROM beyond the 16 bits sent). Mode 7 is `OPI_DTR`: ESP-IDF
/// 4d59230d `components/esp_rom/include/esp_rom_spiflash.h:24-37` and
/// `components/esp_hal_mspi/esp32s31/include/hal/psram_ctrlr_ll.h:741`,
/// esp-hal 1f646c5 `esp-hal/src/psram/oct_hex.rs:332`. The chip select:
/// `components/esp_hal_mspi/esp32s31/include/hal/psram_ctrlr_ll.h:859-861`
/// and esp-hal `oct_hex.rs:362-366` (chip select 1 the PSRAM's). The ROM's
/// four-byte address flag (`CACHE_FCTRL.CACHE_USR_ADDR_4BYTE`) only sets
/// the cache's address length, which this port does not serve: it is not
/// written.
impl<P: Placement, Q: Placement, R: Placement> PsramCommandRegisters for PsramRegisters<P, Q, R> {
    fn set_octal_ddr_mode(&mut self, _: Access<'_>) {
        let spi = &mut self.memspi3;
        spi.modify_ctrl(|w| {
            w.set_fcmd_quad(false)
                .set_fread_dual(false)
                .set_fread_quad(false)
                .set_fread_dio(false)
                .set_fread_qio(false)
                .set_fdout_oct(true)
                .set_fdin_oct(true)
                .set_faddr_oct(true)
                .set_fcmd_oct(true)
                .set_fastrd_mode(true);
        });
        spi.modify_user(|w| {
            w.set_fwrite_dual(false)
                .set_fwrite_quad(false)
                .set_fwrite_dio(false)
                .set_fwrite_qio(false);
        });
        spi.modify_ddr(|w| {
            w.set_cmd_dis(false).set_en(true);
        });
    }

    fn configure(&mut self, _: Access<'_>, command: PsramCommand) {
        let length = |bits: u32| bits.max(1) - 1;
        let spi = &mut self.memspi3;
        spi.modify_user2(|w| {
            w.set_usr_command_bitlen(Bits::truncate(length(u32::from(command.command_bits))))
                .set_usr_command_value(command.command);
        });
        spi.modify_user(|w| {
            w.set_usr_command(command.command_bits > 0)
                .set_usr_addr(command.address_bits > 0)
                .set_usr_mosi(command.output_bits > 0)
                .set_usr_miso(command.input_bits > 0)
                .set_usr_dummy(command.dummy_bits > 0);
        });
        spi.modify_user1(|w| {
            w.set_usr_addr_bitlen(Bits::truncate(length(u32::from(command.address_bits))))
                .set_usr_dummy_cyclelen(Bits::truncate(length(u32::from(command.dummy_bits))));
        });
        if command.address_bits > 0 {
            spi.modify_addr(|w| {
                w.set_usr_addr_value(command.address);
            });
        }
        spi.modify_mosi_dlen(|w| {
            w.set_usr_mosi_dbitlen(Bits::truncate(length(u32::from(command.output_bits))));
        });
        spi.modify_miso_dlen(|w| {
            w.set_usr_miso_dbitlen(Bits::truncate(length(u32::from(command.input_bits))));
        });
    }

    fn set_output(&mut self, _: Access<'_>, word: u32) {
        self.memspi3.write_w(FIRST_WORD, word);
    }

    fn select_psram(&mut self, _: Access<'_>) {
        self.memspi3.modify_misc(|w| {
            w.set_cs1_dis(false).set_cs0_dis(true);
        });
    }

    fn start(&mut self, _: Access<'_>) -> Result<(), Busy> {
        self.memspi3.start_cmd_usr().map_err(|_| Busy)
    }

    fn busy(&self, _: Access<'_>) -> bool {
        self.memspi3.cmd_usr_busy()
    }

    fn input(&self, _: Access<'_>) -> u32 {
        self.memspi3.read_w(FIRST_WORD).buf()
    }
}

/// The PSRAM MMU (`PSRAM_MSPI0`): an entry's index, then its content: the
/// physical page, the PSRAM access flag and the valid flag; a PSRAM entry
/// maps PSRAM alone, so a flash target is written invalid and reads back
/// otherwise.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_cache/esp32s31/include/hal/mmu_ll.h:321-344` and
/// `components/soc/esp32s31/include/soc/ext_mem_defs.h:59-76`; plaintext
/// PSRAM: `SENSITIVE` stays clear (esp-hal 1f646c5
/// `esp-hal/src/psram/oct_hex.rs:24-33`).
impl<P: Placement, Q: Placement, R: Placement> MmuRegisters for PsramRegisters<P, Q, R> {
    fn read_entry(&mut self, _: Access<'_>, index: u16) -> Option<MmuEntry> {
        self.memspi2.modify_mmu_item_index(|w| {
            w.set_mmu_item_index(u32::from(index));
        });
        let content = self.memspi2.read_mmu_item_content();
        (content.valid() && content.access_psram()).then(|| MmuEntry {
            target: MmuTarget::Psram,
            page: content.paddr().get(),
        })
    }

    fn write_entry(&mut self, _: Access<'_>, index: u16, entry: MmuEntry) {
        let page = match entry.target {
            MmuTarget::Psram => Bits::new(entry.page).ok(),
            MmuTarget::Flash => None,
        };
        self.memspi2.modify_mmu_item_index(|w| {
            w.set_mmu_item_index(u32::from(index));
        });
        self.memspi2.modify_mmu_item_content(|w| {
            w.set_paddr(page.unwrap_or(Bits::constant::<0>()))
                .set_access_psram(true)
                .set_valid(page.is_some())
                .set_sensitive(false);
        });
    }
}

#[cfg(test)]
impl<'a, 'b, 'c> PsramRegisters<oer_mmio::Host<'a>, oer_mmio::Host<'b>, oer_mmio::Host<'c>> {
    /// The PSRAM owner over host memory.
    pub(crate) fn host(
        memspi2: &'a mut [u32],
        memspi3: &'b mut [u32],
        pins: &'c mut [u32],
    ) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            memspi2: Memspi2::host(memspi2)?,
            memspi3: Memspi3::host(memspi3)?,
            pins: IomuxMspiPin::host(pins)?,
        })
    }
}
