//! Image binding of the SAR/PWDET pilot to the measured P2 line pair.
use oer_chips_esp32s31_hal::peripherals::timg0_alarm::{Timg0AlarmInterrupt, Timg0AlarmParts};
use oer_chips_esp32s31_hal::radio::domain::transaction::{
    Fault, FaultReason, SarSetupFault, SarSetupStorage,
};
use oer_chips_esp32s31_platform_runtime::Esp32s31;
use oer_chips_esp32s31_radio_phy::sar_pilot;
use oer_chips_family_platform_executor::InterruptExecutor;
use oer_chips_family_platform_interrupt_table::{
    Controller, Router,
    signal::{FromExecutorLevel, IrqSignal},
};
use oer_portable_memory::HandlerCell;
use static_cell::StaticCell;

type X = InterruptExecutor<Esp32s31, 2>;
pub(crate) type Client =
    sar_pilot::Client<oer_chips_family_platform_interrupt_table::Bound<crate::PhyToken>, X>;
type Handler =
    sar_pilot::Interrupt<oer_chips_family_platform_interrupt_table::Bound<crate::PhyToken>, X>;
oer_chips_family_platform_runtime::placed_static! {
    static CELLS: StaticCell<sar_pilot::Cells<oer_chips_family_platform_interrupt_table::Bound<crate::PhyToken>>> = StaticCell::new(), in critical_data "phy_pilot";
    static STATE: StaticCell<Handler> = StaticCell::new(), in critical_data "phy_pilot";
    static HANDLER: HandlerCell<Handler> = HandlerCell::new(), in critical_data "phy_pilot";
    static REPLY: IrqSignal<X> = IrqSignal::new::<FromExecutorLevel<crate::PhyToken>, crate::RadioToken>(), in critical_data "phy_pilot";
}
/// Install once while interrupts are disabled. `split` masks and CLR-clears
/// stale RAW before any request can enable peripheral delivery.
pub(crate) fn init<const H: u8>(
    parts: Timg0AlarmParts,
    router: &mut Router<impl Controller, H>,
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
    let bound = router.bind(token, interrupt).unwrap_or_else(|error| {
        oer_chips_family_platform_runtime::interrupts::reset_binding::<
            oer_chips_esp32s31_hal::system::platform::Port,
            _,
        >(error)
    });
    client.bind(bound)
}
pub(crate) fn interrupt(part: &mut Timg0AlarmInterrupt) {
    match HANDLER.with(|handler| handler.service(part, &oer_portable_embassy_time::EmbassyClock)) {
        Some(Ok(())) => {}
        Some(Err(fault)) => oer_chips_esp32s31_platform_runtime::phy_fault::reset(Fault {
            reason: FaultReason::SarAlarmWithoutBundle(fault),
            registration: None,
        }),
        None => reset(SarSetupFault::Storage(SarSetupStorage::Handler)),
    }
}

fn reset(fault: SarSetupFault) -> ! {
    oer_chips_esp32s31_platform_runtime::phy_fault::reset(Fault {
        reason: FaultReason::SarSetup(fault),
        registration: None,
    })
}
