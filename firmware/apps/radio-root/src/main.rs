#![no_main]
#![no_std]
#![forbid(unsafe_code)]

//! Takes the radio root from esp-hal and requests and releases each platform
//! clock once through the radio's provider. The host renders its typed
//! verdict as `OER-RESULT: pass radio-root`. It powers no RF, PHY or MAC.

use oer_esp32s31_board as board;
use oer_esp32s31_hal::power::{PlatformClock, PlatformClockProvider as _};
use oer_esp32s31_hal::usb_serial_jtag;
use oer_espressif_esp_hal_radio::{RadioSingletons, diagnostics::RadioRoot, take_radio};
use oer_espressif_staged_runtime::diagnostics::{self, Console};
use oer_espressif_staged_runtime::panic;

oer_espressif_staged_runtime::interrupt_table! {}

oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (psram, _adopted) =
        handoff.adopt(INTERRUPT_TABLE, board::BootstrapPsram(peripherals.PSRAM));
    // No executor exists: the early writer waits for room within its budget
    // and counts what it drops.
    let mut diagnostics = Console::new(|bytes| {
        if usb_serial_jtag::early_text(bytes) {
            Ok(())
        } else {
            Err(())
        }
    });
    if let Some(record) = previous {
        let _ = diagnostics::previous_panic(&mut diagnostics, &record);
    }
    let singletons = RadioSingletons {
        i2c_ana_mst: peripherals.I2C_ANA_MST,
        modem_syscon: peripherals.MODEM_SYSCON,
        modem_lpcon: peripherals.MODEM_LPCON,
        wifi: peripherals.WIFI,
        bt: peripherals.BT,
        ieee802154: peripherals.IEEE802154,
        lp_tsens: peripherals.LP_TSENS,
    };
    let (_hardware, clocks) = take_radio(
        singletons,
        &psram,
        todo!("integration PR: bootstrap root provides PllPowerReleased"),
    );
    let _ = diagnostics.event(&RadioRoot::Handoff);
    let mut passed = true;
    for clock in PlatformClock::ALL {
        match clocks.acquire(clock) {
            Ok(guard) => {
                guard.release();
                let _ = diagnostics.event(&RadioRoot::ClockReleased(clock));
            }
            Err(oer_esp32s31_hal::power::PlatformClockError) => {
                passed = false;
                let _ = diagnostics.event(&RadioRoot::ClockRefused(clock));
            }
        }
    }
    let _ = diagnostics.event(if passed {
        &RadioRoot::Passed
    } else {
        &RadioRoot::ClockFailed
    });
    oer_espressif_staged_runtime::idle()
}
