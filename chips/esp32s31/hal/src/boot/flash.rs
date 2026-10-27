//! The flash owner: the SPI1 user command port, the controllers' input
//! timing words and the flash MMU (`FLASH_SPI0`).

use oer_espressif_boot::{
    Access, Busy,
    flash::{
        CHUNK_WORDS, FlashCommandRegisters, FlashReadCommand, FlashReadMode, FlashTimingRegisters,
    },
    mmu::{MmuEntry, MmuRegisters, MmuTarget},
};
use oer_mmio::{Bits, Fixed, Placement};

use oer_esp32s31_pac::{
    instances,
    spi_mem_c::{self as spi0, Spi0Flash},
    spi1_mem_c::{self as spi1, Spi1Flash},
};

/// The flash command port (`FLASH_SPI1`) and the flash cache's controller
/// (`FLASH_SPI0`).
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct FlashRegisters<
    P: Placement = Fixed<{ instances::spi0::BASE }>,
    Q: Placement = Fixed<{ instances::spi1::BASE }>,
> {
    spi0: Spi0Flash<P>,
    spi1: Spi1Flash<Q>,
}

impl FlashRegisters {
    /// Own the bootstrap's flash parts.
    pub(crate) fn new(spi0: Spi0Flash, spi1: Spi1Flash) -> Self {
        Self { spi0, spi1 }
    }
}

/// The SPI1 user command port's read, with the ROM read's effects.
///
/// SOURCE(esp32s31): pinned rev0 ROM, Blobray function records of
/// `_esp_rom_spiflash_read` @0x2f80f0c2: the read mode from `CTRL`
/// (`FREAD_QIO` with `FASTRD_MODE` QIO; otherwise without `FASTRD_MODE`
/// slow read, else `FREAD_DIO` DIO, `FREAD_QUAD` QOUT, `FREAD_DUAL` DOUT,
/// else fast read), `USER.USR_MOSI` cleared, `USER.USR_MISO` set,
/// `USER2` an 8-bit command; `esp_rom_spi_set_rd_cmd_bit_len` @0x2f80e020:
/// `USER.USR_ADDR`/`USER1.USR_ADDR_BITLEN` and `USER.USR_DUMMY`/
/// `USER1.USR_DUMMY_CYCLELEN` (zero dummy cycles leave the length 0);
/// `SPI_read_data` @0x2f80e24a: `ADDR`, `MISO_DLEN` (bits − 1), `CMD.USR`,
/// the data words. The read sets `USER.USR_COMMAND`, which the ROM leaves
/// as the bootloader set it. ESP-IDF 4d80c1b2
/// `components/soc/esp32s31/register/soc/spi1_mem_c_reg.h:16` (`USR`
/// self-clears when the command completes).
impl<P: Placement, Q: Placement> FlashCommandRegisters for FlashRegisters<P, Q> {
    fn read_mode(&self, _: Access<'_>) -> FlashReadMode {
        let ctrl = self.spi1.read_ctrl();
        if ctrl.fread_qio() && ctrl.fastrd_mode() {
            FlashReadMode::Qio
        } else if !ctrl.fastrd_mode() {
            FlashReadMode::SlowRead
        } else if ctrl.fread_dio() {
            FlashReadMode::Dio
        } else if ctrl.fread_quad() {
            FlashReadMode::Qout
        } else if ctrl.fread_dual() {
            FlashReadMode::Dout
        } else {
            FlashReadMode::FastRead
        }
    }

    fn mode_bits_from_status(&self, _: Access<'_>) -> bool {
        self.spi1.read_rd_status().wb_mode_en()
    }

    fn configure_read(&mut self, _: Access<'_>, command: FlashReadCommand) {
        let address = command.address_bits > 0;
        let dummy = command.dummy_cycles > 0;
        self.spi1.modify_user(|w| {
            w.set_usr_mosi(false)
                .set_usr_miso(true)
                .set_usr_command(true)
                .set_usr_addr(address)
                .set_usr_dummy(dummy);
        });
        self.spi1.modify_user2(|w| {
            w.set_usr_command_bitlen(Bits::constant::<7>())
                .set_usr_command_value(u16::from(command.opcode));
        });
        self.spi1.modify_user1(|w| {
            w.set_usr_addr_bitlen(Bits::truncate(u32::from(command.address_bits.max(1)) - 1))
                .set_usr_dummy_cyclelen(Bits::truncate(u32::from(command.dummy_cycles.max(1)) - 1));
        });
    }

    fn set_transfer(&mut self, _: Access<'_>, address: u32, bytes: u32) {
        self.spi1.modify_addr(|w| {
            w.set_usr_addr_value(address);
        });
        // A transfer holds at most sixty-four bytes: 511 bits fit the field.
        self.spi1.modify_miso_dlen(|w| {
            w.set_usr_miso_dbitlen(Bits::truncate(bytes.max(1) * 8 - 1));
        });
    }

    fn start(&mut self, _: Access<'_>) -> Result<(), Busy> {
        self.spi1.start_cmd_usr().map_err(|_| Busy)
    }

    fn busy(&self, _: Access<'_>) -> bool {
        // Every self-clearing command of the word, not only the user one.
        let cmd = self.spi1.read_cmd();
        cmd.flash_pe()
            || cmd.usr()
            || cmd.flash_hpm()
            || cmd.flash_res()
            || cmd.flash_dp()
            || cmd.flash_ce()
            || cmd.flash_be()
            || cmd.flash_se()
            || cmd.flash_pp()
            || cmd.flash_wrsr()
            || cmd.flash_rdsr()
            || cmd.flash_rdid()
            || cmd.flash_wrdi()
            || cmd.flash_wren()
            || cmd.flash_read()
    }

    fn data(&self, _: Access<'_>) -> [u32; CHUNK_WORDS] {
        let spi1 = &self.spi1;
        [
            spi1.read_w0().buf0(),
            spi1.read_w1().buf1(),
            spi1.read_w2().buf2(),
            spi1.read_w3().buf3(),
            spi1.read_w4().buf4(),
            spi1.read_w5().buf5(),
            spi1.read_w6().buf6(),
            spi1.read_w7().buf7(),
            spi1.read_w8().buf8(),
            spi1.read_w9().buf9(),
            spi1.read_w10().buf10(),
            spi1.read_w11().buf11(),
            spi1.read_w12().buf12(),
            spi1.read_w13().buf13(),
            spi1.read_w14().buf14(),
            spi1.read_w15().buf15(),
        ]
    }
}

/// One candidate of the flash input timing: the delay mode and number of
/// every data input, and the extra dummy cycles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlashTimingCandidate {
    pub din_mode: Bits<3>,
    pub din_num: Bits<2>,
    pub extra_dummy: Bits<3>,
}

/// The timing words as the bootloader left them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlashTimingWords {
    spi0_clock: spi0::spi_mem_clock::R,
    spi1_clock: spi1::clock::R,
    spi0_fdummy_rin: bool,
    spi1_fdummy_rin: bool,
    din_mode: spi0::spi_mem_din_mode::R,
    din_num: spi0::spi_mem_din_num::R,
    spi0_timing: spi0::spi_mem_timing_cali::R,
    spi1_timing: spi1::timing_cali::R,
}

/// The controllers' input timing at the sweep's clock: both controllers on
/// the undivided system clock with dummy input on it; the shared delay
/// words latched by `FLASH_SPI0`'s update, the cache's extra dummy changed
/// only for a cache candidate.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/flash/timing.rs:73-152`
/// (`set_flash_clock_120mhz`, `set_flash_timing`) and `:322-336` (the
/// rollback, `FLASH_SPI0`'s update last).
impl<P: Placement, Q: Placement> FlashTimingRegisters for FlashRegisters<P, Q> {
    type Timing = FlashTimingCandidate;
    type Saved = FlashTimingWords;

    fn save_timing(&self, _: Access<'_>) -> FlashTimingWords {
        FlashTimingWords {
            spi0_clock: self.spi0.read_spi_mem_clock(),
            spi1_clock: self.spi1.read_clock(),
            spi0_fdummy_rin: self.spi0.read_spi_mem_ctrl().spi_mem_fdummy_rin(),
            spi1_fdummy_rin: self.spi1.read_ctrl().fdummy_rin(),
            din_mode: self.spi0.read_spi_mem_din_mode(),
            din_num: self.spi0.read_spi_mem_din_num(),
            spi0_timing: self.spi0.read_spi_mem_timing_cali(),
            spi1_timing: self.spi1.read_timing_cali(),
        }
    }

    fn select_tuning_input(&mut self, _: Access<'_>) {
        self.spi0.modify_spi_mem_clock(|w| {
            w.set_spi_mem_clk_equ_sysclk(true)
                .set_spi_mem_clkcnt_n(0)
                .set_spi_mem_clkcnt_h(0)
                .set_spi_mem_clkcnt_l(0);
        });
        self.spi1.modify_clock(|w| {
            w.set_clk_equ_sysclk(true)
                .set_clkcnt_n(0)
                .set_clkcnt_h(0)
                .set_clkcnt_l(0);
        });
        self.spi0.modify_spi_mem_ctrl(|w| {
            w.set_spi_mem_fdummy_rin(true);
        });
        self.spi1.modify_ctrl(|w| {
            w.set_fdummy_rin(true);
        });
    }

    fn apply_timing(&mut self, _: Access<'_>, timing: FlashTimingCandidate, cache: bool) {
        let mode = timing.din_mode;
        self.spi0.modify_spi_mem_din_mode(|w| {
            w.set_spi_mem_din0_mode(mode)
                .set_spi_mem_din1_mode(mode)
                .set_spi_mem_din2_mode(mode)
                .set_spi_mem_din3_mode(mode)
                .set_spi_mem_din4_mode(mode)
                .set_spi_mem_din5_mode(mode)
                .set_spi_mem_din6_mode(mode)
                .set_spi_mem_din7_mode(mode)
                .set_spi_mem_dins_mode(mode);
        });
        let num = timing.din_num;
        self.spi0.modify_spi_mem_din_num(|w| {
            w.set_spi_mem_din0_num(num)
                .set_spi_mem_din1_num(num)
                .set_spi_mem_din2_num(num)
                .set_spi_mem_din3_num(num)
                .set_spi_mem_din4_num(num)
                .set_spi_mem_din5_num(num)
                .set_spi_mem_din6_num(num)
                .set_spi_mem_din7_num(num)
                .set_spi_mem_dins_num(num);
        });
        let calibrated = timing.extra_dummy.get() != 0;
        self.spi0.modify_spi_mem_timing_cali(|w| {
            w.set_spi_mem_timing_clk_ena(true);
            if cache {
                w.set_spi_mem_timing_cali(calibrated)
                    .set_spi_mem_extra_dummy_cyclelen(timing.extra_dummy);
            }
        });
        self.spi0.trigger_spi_mem_timing_cali_update();
        self.spi1.modify_timing_cali(|w| {
            w.set_timing_cali(calibrated)
                .set_extra_dummy_cyclelen(timing.extra_dummy);
        });
        // SPI1 has no update: FLASH_SPI0's latches the shared delay path.
        self.spi0.trigger_spi_mem_timing_cali_update();
    }

    fn restore_timing(&mut self, _: Access<'_>, saved: FlashTimingWords) {
        let clock = saved.spi0_clock;
        self.spi0.modify_spi_mem_clock(|w| {
            w.set_spi_mem_clk_equ_sysclk(clock.spi_mem_clk_equ_sysclk())
                .set_spi_mem_clkcnt_n(clock.spi_mem_clkcnt_n())
                .set_spi_mem_clkcnt_h(clock.spi_mem_clkcnt_h())
                .set_spi_mem_clkcnt_l(clock.spi_mem_clkcnt_l());
        });
        let clock = saved.spi1_clock;
        self.spi1.modify_clock(|w| {
            w.set_clk_equ_sysclk(clock.clk_equ_sysclk())
                .set_clkcnt_n(clock.clkcnt_n())
                .set_clkcnt_h(clock.clkcnt_h())
                .set_clkcnt_l(clock.clkcnt_l());
        });
        self.spi0.modify_spi_mem_ctrl(|w| {
            w.set_spi_mem_fdummy_rin(saved.spi0_fdummy_rin);
        });
        self.spi1.modify_ctrl(|w| {
            w.set_fdummy_rin(saved.spi1_fdummy_rin);
        });
        let mode = saved.din_mode;
        self.spi0.modify_spi_mem_din_mode(|w| {
            w.set_spi_mem_din0_mode(mode.spi_mem_din0_mode())
                .set_spi_mem_din1_mode(mode.spi_mem_din1_mode())
                .set_spi_mem_din2_mode(mode.spi_mem_din2_mode())
                .set_spi_mem_din3_mode(mode.spi_mem_din3_mode())
                .set_spi_mem_din4_mode(mode.spi_mem_din4_mode())
                .set_spi_mem_din5_mode(mode.spi_mem_din5_mode())
                .set_spi_mem_din6_mode(mode.spi_mem_din6_mode())
                .set_spi_mem_din7_mode(mode.spi_mem_din7_mode())
                .set_spi_mem_dins_mode(mode.spi_mem_dins_mode());
        });
        let num = saved.din_num;
        self.spi0.modify_spi_mem_din_num(|w| {
            w.set_spi_mem_din0_num(num.spi_mem_din0_num())
                .set_spi_mem_din1_num(num.spi_mem_din1_num())
                .set_spi_mem_din2_num(num.spi_mem_din2_num())
                .set_spi_mem_din3_num(num.spi_mem_din3_num())
                .set_spi_mem_din4_num(num.spi_mem_din4_num())
                .set_spi_mem_din5_num(num.spi_mem_din5_num())
                .set_spi_mem_din6_num(num.spi_mem_din6_num())
                .set_spi_mem_din7_num(num.spi_mem_din7_num())
                .set_spi_mem_dins_num(num.spi_mem_dins_num());
        });
        let timing = saved.spi1_timing;
        self.spi1.modify_timing_cali(|w| {
            w.set_timing_cali(timing.timing_cali())
                .set_extra_dummy_cyclelen(timing.extra_dummy_cyclelen());
        });
        let timing = saved.spi0_timing;
        self.spi0.modify_spi_mem_timing_cali(|w| {
            w.set_spi_mem_timing_clk_ena(timing.spi_mem_timing_clk_ena())
                .set_spi_mem_timing_cali(timing.spi_mem_timing_cali())
                .set_spi_mem_extra_dummy_cyclelen(timing.spi_mem_extra_dummy_cyclelen());
        });
        self.spi0.trigger_spi_mem_timing_cali_update();
    }
}

/// The flash MMU (`FLASH_SPI0`): an entry's index, then its content: the
/// physical page and the valid flag; a flash entry maps flash alone, so a
/// PSRAM target is written invalid and reads back otherwise.
///
/// SOURCE: ESP-IDF 4d80c1b2
/// `components/esp_hal_cache/esp32s31/include/hal/mmu_ll.h:309-366` (index,
/// then content) and `components/soc/esp32s31/include/soc/ext_mem_defs.h:57,75`
/// (the valid flag, 11 page bits); plaintext flash: `SENSITIVE` stays clear.
impl<P: Placement, Q: Placement> MmuRegisters for FlashRegisters<P, Q> {
    fn read_entry(&mut self, _: Access<'_>, index: u16) -> Option<MmuEntry> {
        self.spi0.modify_spi_mem_mmu_item_index(|w| {
            w.set_spi_mmu_item_index(u32::from(index));
        });
        let content = self.spi0.read_mmu_item_content();
        content.valid().then(|| MmuEntry {
            target: MmuTarget::Flash,
            page: content.paddr().get(),
        })
    }

    fn write_entry(&mut self, _: Access<'_>, index: u16, entry: MmuEntry) {
        let page = match entry.target {
            MmuTarget::Flash => Bits::new(entry.page).ok(),
            MmuTarget::Psram => None,
        };
        self.spi0.modify_spi_mem_mmu_item_index(|w| {
            w.set_spi_mmu_item_index(u32::from(index));
        });
        self.spi0.modify_mmu_item_content(|w| {
            w.set_paddr(page.unwrap_or(Bits::constant::<0>()))
                .set_valid(page.is_some())
                .set_sensitive(false);
        });
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a, 'b> FlashRegisters<oer_mmio::Host<'a>, oer_mmio::Host<'b>> {
    /// The flash owner over host memory from each instance's base.
    pub fn host(spi0: &'a mut [u32], spi1: &'b mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            spi0: Spi0Flash::host(spi0)?,
            spi1: Spi1Flash::host(spi1)?,
        })
    }
}
