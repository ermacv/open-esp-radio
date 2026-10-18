//! Board observation of the ESP32-S31 boot state the esp-hal fork relies on.
//!
//! 1. PMA: every `pmacfg`/`pmaaddr` pair as the bootloader left it, then after
//!    `esp_hal::init`, which programs entry 7 for the external-memory aperture.
//! 2. Flash MMU entry width: an unused (invalid) entry is written with each
//!    candidate page-number width and read back, then restored. The PAC
//!    publishes a 10-bit `PADDR`; ESP-IDF's `ext_mem_defs.h` gives 11 bits
//!    (`SOC_MMU_FLASH_VALID_VAL_MASK`) and 32768 pages.
//! 3. `PMU.IMM_HP_CK_POWER_1`: whether the tie-high bits read back latched
//!    after a write that keeps CPLL powered, as the CPU already runs from it.
//! 4. Last, because a failure resets the chip: the TIMG0 watchdog is armed
//!    with a 1 s system-reset stage, disabled through esp-hal, and left for
//!    3 s. A second `PROBE-BOOT` line before `PROBE-DONE` means the disable did
//!    not apply.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::{
    delay::Delay,
    main,
    peripherals::{LP_PERI, PMU, RNG, SPI0, TIMG0},
    time::Duration,
    timer::timg::{MwdtStage, TimerGroup},
};
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

/// The last Flash MMU entry: the probe image maps far fewer pages.
const UNUSED_MMU_ENTRY: u32 = 1023;
/// Flash MMU valid (bit 12) and PSRAM valid (bit 11) flags of an entry.
const MMU_VALID_FLAGS: u32 = (1 << 12) | (1 << 11);

macro_rules! read_csr {
    ($csr:literal) => {{
        let value: u32;
        // SAFETY: reading a PMA CSR has no side effect.
        unsafe { core::arch::asm!(concat!("csrr {0}, ", $csr), out(reg) value) };
        value
    }};
}

fn pma() -> [(u32, u32); 16] {
    [
        (read_csr!("0xbc0"), read_csr!("0xbd0")),
        (read_csr!("0xbc1"), read_csr!("0xbd1")),
        (read_csr!("0xbc2"), read_csr!("0xbd2")),
        (read_csr!("0xbc3"), read_csr!("0xbd3")),
        (read_csr!("0xbc4"), read_csr!("0xbd4")),
        (read_csr!("0xbc5"), read_csr!("0xbd5")),
        (read_csr!("0xbc6"), read_csr!("0xbd6")),
        (read_csr!("0xbc7"), read_csr!("0xbd7")),
        (read_csr!("0xbc8"), read_csr!("0xbd8")),
        (read_csr!("0xbc9"), read_csr!("0xbd9")),
        (read_csr!("0xbca"), read_csr!("0xbda")),
        (read_csr!("0xbcb"), read_csr!("0xbdb")),
        (read_csr!("0xbcc"), read_csr!("0xbdc")),
        (read_csr!("0xbcd"), read_csr!("0xbdd")),
        (read_csr!("0xbce"), read_csr!("0xbde")),
        (read_csr!("0xbcf"), read_csr!("0xbdf")),
    ]
}

/// The NAPOT region a `pmaaddr` encodes, as `(base, size)`.
fn napot(address: u32) -> (u32, u64) {
    let ones = address.trailing_ones();
    let size = 1u64 << (ones + 3);
    let base = (address & !((1u32 << ones).wrapping_sub(1))) << 2;
    (base, size)
}

fn print_pma(stage: &str, entries: &[(u32, u32); 16]) {
    for (index, (config, address)) in entries.iter().enumerate() {
        let (base, size) = napot(*address);
        println!(
            "PROBE pma {stage} {index:2}: cfg {config:#010x} addr {address:#010x} napot base {base:#010x} size {size:#x}"
        );
    }
}

fn probe_mmu() {
    let spi0 = SPI0::regs();
    // SAFETY: the index field takes any entry number; the probe is the only
    // user of the selector.
    spi0.mmu_item_index()
        .write(|w| unsafe { w.mmu_item_index().bits(UNUSED_MMU_ENTRY) });
    let original = spi0.mmu_item_content().read().bits();
    if original & MMU_VALID_FLAGS != 0 {
        println!("PROBE INCOMPLETE mmu entry {UNUSED_MMU_ENTRY} is mapped: {original:#010x}");
        return;
    }
    for pattern in [0x0000_03ff_u32, 0x0000_07ff, 0x0000_7fff & !MMU_VALID_FLAGS] {
        // SAFETY: the entry stays invalid, so no access translates through it.
        spi0.mmu_item_content()
            .write(|w| unsafe { w.bits(pattern) });
        let observed = spi0.mmu_item_content().read().bits();
        println!(
            "PROBE mmu entry {UNUSED_MMU_ENTRY}: wrote {pattern:#010x} read {observed:#010x} (pac paddr {})",
            spi0.mmu_item_content().read().paddr().bits()
        );
    }
    // SAFETY: restores the entry the probe found.
    spi0.mmu_item_content()
        .write(|w| unsafe { w.bits(original) });
}

fn probe_pmu(delay: &Delay) {
    let power = PMU::regs().imm_hp_ck_power_1();
    let before = power.read().bits();
    // The CPU runs from CPLL: tying it high keeps the state it has.
    power.modify(|_, w| {
        w.tie_high_xpd_pll().set_bit();
        w.tie_high_xpd_pll_i2c().set_bit()
    });
    let at_once = power.read().bits();
    delay.delay_micros(10);
    let later = power.read().bits();
    println!(
        "PROBE pmu imm_hp_ck_power_1: before {before:#010x} after write {at_once:#010x} after 10 us {later:#010x}"
    );
}

fn print_trng(stage: &str) {
    let trng = RNG::regs();
    println!(
        "PROBE trng {stage}: rng_ctrl {:#010x} date {:#010x} conf {:#010x} debug_conf {:#010x} int_raw {:#010x}",
        LP_PERI::regs().rng_ctrl().read().bits(),
        trng.date().read().bits(),
        trng.conf().read().bits(),
        trng.debug_conf().read().bits(),
        trng.int_raw().read().bits()
    );
}

fn probe_watchdog(timg0: TIMG0<'static>, delay: &Delay) {
    let mut wdt = TimerGroup::new(timg0).wdt;
    wdt.set_timeout(MwdtStage::Stage0, Duration::from_secs(1));
    wdt.enable();
    wdt.feed();
    delay.delay_millis(100);
    wdt.disable();
    let enabled = TIMG0::regs().wdtconfig0().read().wdt_en().bit();
    println!("PROBE wdt disabled through esp-hal: wdt_en reads {enabled}; waiting 3 s");
    delay.delay_millis(3000);
    println!("PROBE MATCH wdt disable held for 3 s past its 1 s timeout");
}

#[main]
fn main() -> ! {
    println!("PROBE-BOOT");
    let bootloader_pma = pma();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    print_pma("bootloader", &bootloader_pma);
    print_pma("esp-hal", &pma());
    probe_mmu();
    probe_pmu(&delay);
    print_trng("after init");
    delay.delay_millis(100);
    print_trng("after 100 ms");
    for _ in 0..4 {
        delay.delay_micros(100);
        println!(
            "PROBE trng crc_sync_data {:#010x}",
            RNG::regs().crc_sync_data().read().bits()
        );
    }
    probe_watchdog(peripherals.TIMG0, &delay);

    println!("PROBE-DONE");
    loop {
        delay.delay_millis(1000);
    }
}
