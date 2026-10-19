#![no_main]
#![no_std]

//! Takes the radio root from esp-hal and requests and releases each platform
//! clock once through the radio's provider, then prints
//! `OER-RESULT: pass radio-root`. It powers no RF, PHY or MAC.

use core::fmt::Write as _;

use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use oer_adapters_esp_hal_radio::{RadioSingletons, take_radio};
use oer_esp32s31_hal::power::{PlatformClock, PlatformClockProvider as _};
use oer_platform_esp32s31_board as board;
use oer_platform_staged_runtime::panic;

oer_platform_staged_runtime::interrupt_table! {}

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    // SAFETY: `runtime_main` runs once on CPU0, right after `_runtime_start`,
    // with interrupts disabled and the bootstrap's mapping intact, which
    // nothing has reset or remapped; this image never enables interrupts.
    let psram = unsafe {
        oer_platform_staged_runtime::adopt(INTERRUPT_TABLE, || {
            board::adopt_psram_mapping(peripherals.PSRAM)
        })
    };
    let mut console = UsbSerialJtag::new(peripherals.USB_DEVICE);
    if let Some(record) = previous {
        let _ = write!(console, "OER-PANIC previous {record}\r\n");
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
    let verdict = match take_radio(singletons, &psram) {
        Err(error) => {
            let _ = write!(
                console,
                "OER-RESULT: fail radio-root take_radio={error:?}\r\n"
            );
            None
        }
        Ok((hardware, clocks)) => {
            let _ = console.write(b"OER radio-root handoff=ok\r\n");
            let mut passed = true;
            for clock in PlatformClock::ALL {
                match clocks.acquire(clock) {
                    Ok(guard) => {
                        guard.release();
                        let _ = write!(
                            console,
                            "OER radio-root clock={clock:?} acquired released\r\n"
                        );
                    }
                    Err(error) => {
                        passed = false;
                        let _ = write!(
                            console,
                            "OER radio-root clock={clock:?} refused={error:?}\r\n"
                        );
                    }
                }
            }
            Some((hardware, passed))
        }
    };
    if let Some((_hardware, true)) = &verdict {
        let _ = console.write(b"OER-RESULT: pass radio-root\r\n");
    } else if verdict.is_some() {
        let _ = console.write(b"OER-RESULT: fail radio-root clock\r\n");
    }
    loop {
        // SAFETY: `wfi` only waits; with interrupts disabled it parks the
        // hart, which is all the image does from here on.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
