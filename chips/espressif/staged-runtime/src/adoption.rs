//! Stage-two adoption, over the same driver API isolated verifiers use.
use crate::{Adopted, BootError};
use oer_espressif_boot::adoption::{self as driver, Port};
use oer_espressif_interrupt_table::Controller;

/// Chip composition of the driver's hardware output and the runtime's controller.
pub trait Binding {
    type Platform: oer_espressif_platform_port::EarlyText;
    type Driver: Port;
    type Hardware;
    type Controller: Controller;

    fn split(parts: <Self::Driver as Port>::Output) -> (Self::Hardware, Self::Controller);
}

/// Initialized hardware held before mapping/context adoption. Images can
/// report their startup diagnostics now, with the console already installed.
/// The owners remain private and this phase cannot publish a ready runtime.
#[must_use = "prepared owners still require mapping/context adoption"]
pub struct Prepared<B: Binding> {
    parts: <B::Driver as Port>::Output,
}

/// Run the shared driver, retaining its exact owners until adoption. This
/// phase lets image-owned reporting surround initialization without copying
/// the driver's sequence or issuing a ready `Runtime` early.
pub fn prepare<B: Binding>(
    owners: driver::Owners<B::Driver>,
) -> Result<Prepared<B>, driver::InitError> {
    driver::initialize(owners).map(|parts| Prepared { parts })
}

impl<B: Binding> Prepared<B> {
    /// Complete stage-two adoption after any image startup diagnostics.
    #[cfg(target_arch = "riscv32")]
    pub fn adopt<Board: crate::AdoptsBootstrapMapping, H>(
        self,
        handoff: crate::Handoff<B::Platform>,
        table: &'static crate::InterruptTable,
        board: Board,
        previous_records: impl FnOnce(),
        before_adoption: impl FnOnce(B::Hardware) -> H,
    ) -> Result<Runtime<H, B::Controller, Board::Mapping>, BootError> {
        self.finish(before_adoption, |controller| {
            handoff.adopt(table, controller, board, previous_records)
        })
    }

    fn finish<H, M>(
        self,
        before_adoption: impl FnOnce(B::Hardware) -> H,
        adopt: impl FnOnce(B::Controller) -> Result<(M, Adopted<B::Controller>), BootError>,
    ) -> Result<Runtime<H, B::Controller, M>, BootError> {
        let (hardware, controller) = B::split(self.parts);
        let hardware = before_adoption(hardware);
        let (mapping, interrupts) = adopt(controller)?;
        Ok(Runtime {
            hardware,
            mapping,
            interrupts,
        })
    }
}

/// A fully adopted runtime. Only the runner constructs one, after driver
/// initialization and mapping/context adoption have both succeeded.
#[must_use = "the runtime retains the adopted mapping and platform owners"]
pub struct Runtime<H, C: Controller, M> {
    hardware: H,
    mapping: M,
    interrupts: Adopted<C>,
}

impl<H, C: Controller, M> Runtime<H, C, M> {
    pub fn into_parts(self) -> (H, M, Adopted<C>) {
        (self.hardware, self.mapping, self.interrupts)
    }
}

/// An unchanged driver installation error or an interrupt-admission refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Initialization(driver::InitError),
    Context(BootError),
}

impl Error {
    pub const fn name(self) -> oer_diagnostics::Name<32> {
        match self {
            Self::Initialization(driver::InitError::ConsoleInstalled) => {
                oer_diagnostics::name!("console-installed"; 32)
            }
            Self::Initialization(driver::InitError::CoreControlInstalled) => {
                oer_diagnostics::name!("core-control-installed"; 32)
            }
            Self::Context(error) => error.name(),
        }
    }
}

/// Adopt hardware, then report retained records, accept the existing PSRAM
/// mapping and install the image's interrupt context. Interrupts stay disabled.
#[cfg(target_arch = "riscv32")]
pub fn run<B: Binding, Board: crate::AdoptsBootstrapMapping, H>(
    handoff: crate::Handoff<B::Platform>,
    owners: driver::Owners<B::Driver>,
    table: &'static crate::InterruptTable,
    board: Board,
    previous_records: impl FnOnce(),
    before_adoption: impl FnOnce(B::Hardware) -> H,
) -> Result<Runtime<H, B::Controller, Board::Mapping>, Error> {
    complete::<B, _, _>(owners, before_adoption, |controller| {
        handoff.adopt(table, controller, board, previous_records)
    })
}

// The injectable continuation is private: only the real Handoff path above
// can publish a Runtime outside this crate. Host tests exercise this same order.
fn complete<B: Binding, H, M>(
    owners: driver::Owners<B::Driver>,
    before_adoption: impl FnOnce(B::Hardware) -> H,
    adopt: impl FnOnce(B::Controller) -> Result<(M, Adopted<B::Controller>), BootError>,
) -> Result<Runtime<H, B::Controller, M>, Error> {
    prepare::<B>(owners)
        .map_err(Error::Initialization)?
        .finish(before_adoption, adopt)
        .map_err(Error::Context)
}

#[cfg(test)]
mod tests;
