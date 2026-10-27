//! Image binding of the SAR/PWDET pilot to the measured P2 line pair.
use oer_esp32s31_hal::domain::transaction::{Fault, FaultReason, SarSetupFault, SarSetupStorage};
use oer_esp32s31_hal::platform::timg0_alarm::{Timg0AlarmInterrupt, Timg0AlarmParts};
use oer_esp32s31_phy::sar_pilot;
use oer_espressif_executor::InterruptExecutor;
use oer_interrupt_table::{
    Controller, Router,
    signal::{FromExecutorLevel, IrqSignal},
};
use oer_memory::HandlerCell;
use static_cell::StaticCell;

type X = InterruptExecutor<2>;
pub(crate) type Client = sar_pilot::Client<oer_interrupt_table::Bound<crate::PhyToken>, X>;
type Handler = sar_pilot::Interrupt<oer_interrupt_table::Bound<crate::PhyToken>, X>;
oer_espressif_staged_runtime::placed_static! {
    static CELLS: StaticCell<sar_pilot::Cells<oer_interrupt_table::Bound<crate::PhyToken>>> = StaticCell::new(), in critical_data "phy_pilot";
    static STATE: StaticCell<Handler> = StaticCell::new(), in critical_data "phy_pilot";
    static HANDLER: HandlerCell<Handler> = HandlerCell::new(), in critical_data "phy_pilot";
    static REPLY: IrqSignal<X> = IrqSignal::new::<FromExecutorLevel<crate::PhyToken>, crate::RadioToken>(), in critical_data "phy_pilot";
}
/// Install once while interrupts are disabled. `split` masks and CLR-clears
/// stale RAW before any request can enable peripheral delivery.
pub(crate) fn init(
    parts: Timg0AlarmParts,
    router: &mut Router<impl Controller>,
    token: crate::PhyToken,
) -> Client {
    let mut interrupt = parts.interrupt;
    let Some(cells) = CELLS.try_init_with(sar_pilot::Cells::new) else {
        reset(SarSetupFault::Storage(SarSetupStorage::Cells));
    };
    let (client, handler) = cells.split(parts.program, &mut interrupt, &REPLY);
    let Some(handler) = STATE.try_init_with(|| handler) else {
        reset(SarSetupFault::Storage(SarSetupStorage::State));
    };
    if HANDLER.install(handler).is_err() {
        reset(SarSetupFault::Storage(SarSetupStorage::Handler));
    }
    let bound = router
        .bind(token, interrupt)
        .unwrap_or_else(|error| oer_espressif_staged_runtime::interrupts::reset_binding(error));
    client.bind(bound)
}
pub(crate) fn interrupt(part: &mut Timg0AlarmInterrupt) {
    match HANDLER.with(|handler| handler.service(part, &oer_embassy_time::EmbassyClock)) {
        Some(Ok(())) => {}
        Some(Err(fault)) => oer_espressif_staged_runtime::phy_fault::reset(Fault {
            reason: FaultReason::SarAlarmWithoutBundle(fault),
            registration: None,
        }),
        None => reset(SarSetupFault::Storage(SarSetupStorage::Handler)),
    }
}

fn reset(fault: SarSetupFault) -> ! {
    oer_espressif_staged_runtime::phy_fault::reset(Fault {
        reason: FaultReason::SarSetup(fault),
        registration: None,
    })
}
