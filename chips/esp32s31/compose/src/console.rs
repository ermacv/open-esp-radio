//! The owned S31 console binding. Images select its interrupt entries.
use crate::Esp32s31;
use oer_esp32s31_hal::{peripherals::usb_serial_jtag as hw, platform::Port};
use oer_espressif_executor::Executor;
use oer_espressif_interrupt_table::{Controller, Entry, InterruptSource, Router};
use oer_espressif_usb_serial_jtag::{Endpoint, Interrupt};

pub struct Pads(hw::UsbPads);
impl Pads {
    pub fn disconnect(self) {
        self.0.disconnect();
    }
}

pub struct Console(Endpoint<Executor<Esp32s31, 0>, hw::ConsoleEndpoint>);
pub struct Binding(Interrupt<Executor<Esp32s31, 0>, hw::ConsoleInterrupt>);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unavailable {
    Empty,
    Busy,
    Taken,
}
impl Binding {
    pub const fn new<I, W>() -> Self
    where
        I: Entry<Part = hw::ConsoleInterrupt>,
        W: Entry,
        W::Part: InterruptSource,
    {
        Self(Interrupt::new::<I, W>())
    }
    pub fn interrupt(&'static self, part: &mut hw::ConsoleInterrupt) {
        self.0.service(part);
    }
    pub fn open<I>(
        &'static self,
        router: &mut Router<impl Controller>,
        token: I,
    ) -> Result<(Console, Pads), Unavailable>
    where
        I: Entry<Part = hw::ConsoleInterrupt>,
    {
        let (endpoint, interrupt, pads) =
            hw::retire_early_writer().map_err(|error| match error {
                hw::RetireError::Empty => Unavailable::Empty,
                hw::RetireError::Busy => Unavailable::Busy,
                hw::RetireError::Taken => Unavailable::Taken,
            })?;
        let bound = router.bind(token, interrupt).unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<Port, _>(error)
        });
        Ok((
            Console(Endpoint::new(endpoint, self.0.signal(), &bound)),
            Pads(pads),
        ))
    }
}
impl embedded_io_async::ErrorType for Console {
    type Error = oer_espressif_usb_serial_jtag::Error;
}
impl embedded_io_async::Read for Console {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self.0.read(bytes).await
    }
}
impl embedded_io_async::Write for Console {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        self.0.write(bytes).await
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush().await
    }
}
pub fn early(note: &oer_diagnostics::Note<'_>) -> bool {
    hw::early_note(note)
}
pub fn early_text(bytes: &[u8]) -> bool {
    hw::early_text(bytes)
}
pub fn take_early_lost() -> u32 {
    hw::take_early_lost()
}
