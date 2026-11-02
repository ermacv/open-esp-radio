//! The USB Serial/JTAG console as the application's duplex byte stream
//! (`embedded_io_async::Read` and `Write`), its interrupt in the
//! application's context.

use oer_esp32s31_compose::console::Binding;
use oer_esp32s31_hal::peripherals::usb_serial_jtag::ConsoleInterrupt;

use super::{Boot, ConsoleToken, Error, WakeToken};

/// The console's byte stream.
pub use oer_esp32s31_compose::console::Console as UsbConsole;

oer_espressif_staged_runtime::placed_static! {
    static BINDING: Binding = Binding::new::<ConsoleToken, WakeToken>(), in critical_data "sdk_console";
}

/// The console's interrupt raises its endpoint's signal.
pub(crate) fn on_usb(part: &mut ConsoleInterrupt) {
    BINDING.interrupt(part);
}

/// The console's owner: [`Console::open`] takes it.
pub struct ConsolePeripheral(pub(crate) ConsoleToken);

/// The console.
pub struct Console;

impl Console {
    /// Open the console over the block the early writer used until now:
    /// its interrupt is bound in the application's context. From here on
    /// [`println!`](crate::println) writes nothing and returns `false`; a
    /// fatal note still reaches the host.
    ///
    /// # Errors
    /// [`Error::Console`] when the early writer could not retire its block.
    pub fn open(console: ConsolePeripheral, boot: &mut Boot) -> Result<UsbConsole, Error> {
        let (stream, _pads) = BINDING
            .open(boot.router(), console.0)
            .map_err(|_| Error::Console)?;
        Ok(stream)
    }
}
