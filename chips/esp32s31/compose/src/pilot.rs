//! Permanent SAR/PWDET binding; image entries retain priority and hart policy.
use crate::Esp32s31;
use oer_esp32s31_hal::{
    domain::transaction::{Fault, FaultReason, PhyTransaction, SarSetupFault, SarSetupStorage},
    peripherals::timg0_alarm::{Timg0AlarmInterrupt, Timg0AlarmParts},
};
use oer_esp32s31_phy::sar_pilot::{self, Service};
use oer_espressif_executor::InterruptExecutor;
use oer_espressif_interrupt_table::{
    Bound, Controller, Entry, Router,
    signal::{FromExecutorLevel, IrqSignal},
};
use oer_memory::HandlerCell;
use static_cell::StaticCell;
type Executor = InterruptExecutor<Esp32s31, 2>;
type Handler<I> = sar_pilot::Interrupt<Bound<I>, Executor>;
pub struct Client<I: Entry<Part = Timg0AlarmInterrupt> + 'static> {
    inner: sar_pilot::Client<Bound<I>, Executor>,
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static> Service for Client<I> {
    async fn sample(
        &mut self,
        phy: PhyTransaction,
        clear_tone: bool,
    ) -> (PhyTransaction, Result<sar_pilot::Sample, sar_pilot::Error>) {
        self.inner.sample(phy, clear_tone).await
    }
}
/// Place once in critical data. Payloads retain their affine ownership.
pub struct Storage<I: Entry<Part = Timg0AlarmInterrupt> + 'static> {
    cells: StaticCell<sar_pilot::Cells<Bound<I>>>,
    state: StaticCell<Handler<I>>,
    handler: HandlerCell<Handler<I>>,
    reply: IrqSignal<Executor>,
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static> Storage<I> {
    pub const fn new<R: Entry>() -> Self
    where
        R::Part: oer_espressif_interrupt_table::InterruptSource,
    {
        Self {
            cells: StaticCell::new(),
            state: StaticCell::new(),
            handler: HandlerCell::new(),
            reply: IrqSignal::new::<FromExecutorLevel<I>, R>(),
        }
    }
    /// Stop/mask and discard stale RAW through the PHY driver before bind.
    pub fn init(
        &'static self,
        parts: Timg0AlarmParts,
        router: &mut Router<impl Controller>,
        token: I,
    ) -> Client<I> {
        let mut interrupt = parts.interrupt;
        let Some(cells) = self.cells.try_init_with(sar_pilot::Cells::new) else {
            reset(SarSetupStorage::Cells);
        };
        let (client, handler) = cells.split(parts.program, &mut interrupt, &self.reply);
        let Some(handler) = self.state.try_init_with(|| handler) else {
            reset(SarSetupStorage::State);
        };
        if self.handler.install(handler).is_err() {
            reset(SarSetupStorage::Handler);
        }
        let bound = router.bind(token, interrupt).unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<
                oer_esp32s31_hal::platform::Port,
                _,
            >(error)
        });
        Client {
            inner: client.bind(bound),
        }
    }
    pub fn interrupt(&self, part: &mut Timg0AlarmInterrupt) {
        match self
            .handler
            .with(|handler| handler.service(part, &oer_embassy_time::EmbassyClock))
        {
            Some(Ok(())) => {}
            Some(Err(fault)) => crate::phy_fault::reset(Fault {
                reason: FaultReason::SarAlarmWithoutBundle(fault),
                registration: None,
            }),
            None => reset(SarSetupStorage::Handler),
        }
    }
}
fn reset(storage: SarSetupStorage) -> ! {
    crate::phy_fault::reset(Fault {
        reason: FaultReason::SarSetup(SarSetupFault::Storage(storage)),
        registration: None,
    })
}
