use core::mem::size_of;
use oer_esp32s31_hal::platform::Port;
use oer_esp32s31_layout::LAYOUT;
use oer_espressif_boot::flash;
use oer_espressif_staged_bootstrap::{self as staged, startup};
use static_cell::ConstStaticCell;

// The bootstrap's reports and panic go through the HAL's early writer.
staged::panic_handler!(Port);

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
static FLASH_TUNING_REFERENCE: [u32; FLASH_TUNING_REFERENCE_WORDS] = const {
    let mut words = [0_u32; FLASH_TUNING_REFERENCE_WORDS];
    let mut state = 0x31a5_c33c_u32;
    let mut index = 0_usize;
    let mut remaining: &mut [u32] = &mut words;
    while let [word, rest @ ..] = remaining {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *word = state ^ (index as u32).rotate_left((index & 31) as u32);
        remaining = rest;
        index += 1;
    }
    words
};

// The full-page sweep reference is owned static SRAM, never the boot stack.
static FLASH_TUNING_SCRATCH: ConstStaticCell<[u32; flash::TUNING_SCRATCH_WORDS]> =
    ConstStaticCell::new([0; flash::TUNING_SCRATCH_WORDS]);

// SAFETY: the reset entry calls this symbol after stack/BSS initialization.
#[unsafe(no_mangle)]
extern "C" fn bootstrap_main() -> ! {
    startup::run::<oer_esp32s31_boot::S31, RUNTIME_ENTRY>(startup::Resources {
        layout: &LAYOUT,
        payload: &RUNTIME_PAYLOAD,
        tuning_reference: &FLASH_TUNING_REFERENCE,
        flash_capacity: FLASH_CAPACITY_BYTES,
        scratch: || FLASH_TUNING_SCRATCH.try_take().map(|words| words.as_mut_slice()),
    })
}
