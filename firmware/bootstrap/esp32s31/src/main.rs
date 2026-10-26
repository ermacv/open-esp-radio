#![no_main]
#![no_std]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

//! S31 cold boot: one reset root, bounded boot sequences, then stage two.
use core::mem::size_of;
use oer_esp32s31_hal::{self as hal, boot};
use oer_esp32s31_layout::LAYOUT;
use oer_espressif_boot::{BootFailure, cache, clock, flash, mmu, watchdog::BootWatchdogs};
use oer_espressif_staged_bootstrap::{self as staged, fail, print};
use static_cell::ConstStaticCell;

staged::app_descriptor!(
    "0.1.0",
    "oer-firmware-bootstrap-esp32s31",
    LAYOUT.mmu_page_bytes
);

const FLASH_TUNING_REFERENCE_WORDS: usize =
    LAYOUT.flash_tuning_reference_bytes as usize / size_of::<u32>();
const RUNTIME_ENTRY: usize = staged::runtime_entry(include_bytes!(env!("PSRAM_RUNTIME_BIN")));
const FLASH_CAPACITY_BYTES: u32 = include!(concat!(env!("OUT_DIR"), "/flash-capacity.rs"));

// SAFETY: the linker owns this immutable payload section in flash.
#[used]
#[unsafe(link_section = ".psram.runtime.payload")]
static RUNTIME_PAYLOAD: [u8; include_bytes!(env!("PSRAM_RUNTIME_BIN")).len()] =
    *include_bytes!(env!("PSRAM_RUNTIME_BIN"));

// SAFETY: a disjoint, cold flash span, never executed or read after tuning.
#[used]
#[unsafe(link_section = ".flash.tuning.reference")]
static FLASH_TUNING_REFERENCE: [u32; FLASH_TUNING_REFERENCE_WORDS] = flash_tuning_reference();

// The full-page sweep reference is owned static SRAM, never the boot stack.
static FLASH_TUNING_SCRATCH: ConstStaticCell<[u32; flash::TUNING_SCRATCH_WORDS]> =
    ConstStaticCell::new([0; flash::TUNING_SCRATCH_WORDS]);

// SAFETY: the reset entry calls this symbol after stack/BSS initialization.
// Its code must remain in SRAM while the flash timing changes.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".rwtext.bootstrap_main")]
extern "C" fn bootstrap_main() -> ! {
    // The root claims the chip once and installs the console's parts for the
    // early writer, so it comes before any output; the boot-hart check of
    // `start` prints nothing before it fails.
    let root = hal::root::Root::take();
    staged::start();
    let root = root.unwrap_or_else(|| fail("OER_BOOT bootstrap=FAIL reason=root-taken\r\n"));
    let mut hardware = root
        .into_bootstrap()
        .unwrap_or_else(|_| fail("OER_BOOT bootstrap=FAIL reason=monotonic\r\n"));
    let deadline = boot::BootDeadline::start(&hardware.monotonic, boot::BOOT_BUDGET);
    hardware
        .soc
        .initialize(&deadline)
        .unwrap_or_else(|_| fail("OER_BOOT bootstrap=FAIL reason=soc-init\r\n"));
    let watchdogs = BootWatchdogs::take_over(
        hardware.main_watchdog0,
        hardware.rtc_watchdog,
        hardware.super_watchdog,
    );
    hardware
        .brownout
        .apply_idf_policy(&deadline)
        .unwrap_or_else(|_| fail("OER_BOOT bootstrap=FAIL reason=brownout-policy\r\n"));
    print("OER_BOOT bootstrap=BROWNOUT\r\n");

    let rates = checked(clock::bring_up(
        &mut hardware.clock,
        &mut hardware.active_power,
        boot::CpuProfile::Mhz160.clock(),
        boot::CLOCK_WAITS,
        &deadline,
    ));
    print("OER_BOOT bootstrap=INIT\r\n");
    let memory = checked(hardware.psram.into_region(
        &mut hardware.clock,
        &mut hardware.active_power,
        &mut hardware.cache,
        LAYOUT.psram.length(),
        &deadline,
    ));
    if memory.as_ptr() as usize != LAYOUT.psram.origin() as usize {
        fail("OER_BOOT bootstrap=FAIL reason=psram-init\r\n");
    }
    let image = staged::stage(
        &LAYOUT,
        &RUNTIME_PAYLOAD,
        memory,
        |progress| match progress {
            staged::StageProgress::Psram => {
                print("OER_BOOT bootstrap=PSRAM\r\n");
                let source_address = RUNTIME_PAYLOAD.as_ptr() as usize;
                let source_end = source_address
                    .checked_add(RUNTIME_PAYLOAD.len())
                    .unwrap_or_else(|| fail("OER_BOOT bootstrap=FAIL reason=source-overflow\r\n"));
                let xip = LAYOUT.flash_xip;
                if !(xip.origin() as usize..xip.end() as usize).contains(&source_address)
                    || source_end > xip.end() as usize
                {
                    fail("OER_BOOT bootstrap=FAIL reason=source-not-xip\r\n");
                }
            }
            staged::StageProgress::SourceCrc => print("OER_BOOT bootstrap=SOURCE_CRC\r\n"),
            staged::StageProgress::DestinationCrc => {
                print("OER_BOOT bootstrap=DESTINATION_CRC\r\n")
            }
        },
    )
    .unwrap_or_else(|error| staged::fail_stage(error));
    let tuning_virtual = FLASH_TUNING_REFERENCE.as_ptr() as u32;
    let tuning_physical = checked(mmu::physical_address(
        &mut hardware.flash,
        hardware.flash_window,
        mmu::MmuTarget::Flash,
        tuning_virtual,
    ));
    checked(flash::tune(
        flash::TuningOwners {
            flash: &mut hardware.flash,
            clock: &mut hardware.clock,
            cache: &mut hardware.cache,
            xip: &boot::XipRegion::new(&FLASH_TUNING_REFERENCE),
        },
        boot::flash_geometry(FLASH_CAPACITY_BYTES),
        flash::TuningRegion {
            physical_start: tuning_physical,
            virtual_start: tuning_virtual,
            bytes: size_of::<[u32; FLASH_TUNING_REFERENCE_WORDS]>() as u32,
        },
        &boot::FLASH_TIMING_CANDIDATES,
        FLASH_TUNING_SCRATCH.take(),
        boot::TUNING_WAITS,
        &deadline,
    ));
    print("OER_BOOT bootstrap=FLASH_TUNED\r\n");
    let published = checked(cache::publish_code(
        &mut hardware.cache,
        boot::CACHE_GEOMETRY,
        image.load_address() as u32,
        image.payload_len() as u32,
        boot::CACHE_SYNC,
        &deadline,
    ));
    staged::hand_off::<RUNTIME_ENTRY>(image, published, || {
        let _stopped = watchdogs.hand_off();
    })
}

fn checked<T>(result: Result<T, BootFailure>) -> T {
    result.unwrap_or_else(|error| staged::fail_boot(error))
}

const fn flash_tuning_reference() -> [u32; FLASH_TUNING_REFERENCE_WORDS] {
    let mut words = [0_u32; FLASH_TUNING_REFERENCE_WORDS];
    let mut state = 0x31a5_c33c_u32;
    let mut index = 0;
    while index < words.len() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        words[index] = state ^ (index as u32).rotate_left((index & 31) as u32);
        index += 1;
    }
    words
}
