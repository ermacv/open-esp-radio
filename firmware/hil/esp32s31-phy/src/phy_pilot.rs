//! Image binding of the SAR/PWDET pilot to the measured P2 line pair.
use oer_esp32s31_compose::Esp32s31;
use crate::pilot_timing::{Queue, Reader, Sample, Sender};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use oer_esp32s31_hal::domain::transaction::{Fault, FaultReason, SarSetupFault, SarSetupStorage};
use oer_esp32s31_hal::platform::timg0_alarm::{Timg0AlarmInterrupt, Timg0AlarmParts};
use oer_esp32s31_phy::sar_pilot;
use oer_espressif_executor::InterruptExecutor;
use oer_espressif_interrupt_table::{
    Controller, Router,
    signal::{FromExecutorLevel, IrqSignal},
};
use oer_memory::HandlerCell;
use static_cell::StaticCell;

type X = InterruptExecutor<Esp32s31, 2>;
pub(crate) type Client =
    sar_pilot::Client<oer_espressif_interrupt_table::Bound<crate::PhyToken>, X>;
struct Handler {
    pilot: sar_pilot::Interrupt<oer_espressif_interrupt_table::Bound<crate::PhyToken>, X>,
    samples: Sender,
}
oer_espressif_staged_runtime::placed_static! {
    static CELLS: StaticCell<sar_pilot::Cells<oer_espressif_interrupt_table::Bound<crate::PhyToken>>> = StaticCell::new(), in critical_data "phy_pilot";
    static STATE: StaticCell<Handler> = StaticCell::new(), in critical_data "phy_pilot";
    static HANDLER: HandlerCell<Handler> = HandlerCell::new(), in critical_data "phy_pilot";
    static REPLY: IrqSignal<X> = IrqSignal::new::<FromExecutorLevel<crate::PhyToken>, crate::RadioToken>(), in critical_data "phy_pilot";
    static SAMPLES: StaticCell<Queue> = StaticCell::new(), in critical_data "phy_pilot";
    static MEASURING: AtomicBool = AtomicBool::new(false), in critical_data "phy_pilot";
    static LOST: AtomicU32 = AtomicU32::new(0), in critical_data "phy_pilot";
}
/// Install once while interrupts are disabled. `split` masks and CLR-clears
/// stale RAW before any request can enable peripheral delivery.
pub(crate) fn init(
    parts: Timg0AlarmParts,
    router: &mut Router<impl Controller>,
    token: crate::PhyToken,
) -> (Client, Reader) {
    let mut interrupt = parts.interrupt;
    let Some(cells) = CELLS.try_init_with(sar_pilot::Cells::new) else {
        reset(SarSetupFault::Storage(SarSetupStorage::Cells));
    };
    let Some(storage) = SAMPLES.try_uninit() else {
        reset(SarSetupFault::Storage(SarSetupStorage::Cells));
    };
    let (samples, reader) = Queue::init_in_place(storage).split();
    let (client, pilot) = cells.split(parts.program, &mut interrupt, &REPLY);
    let Some(handler) = STATE.try_init_with(|| Handler { pilot, samples }) else {
        reset(SarSetupFault::Storage(SarSetupStorage::State));
    };
    if HANDLER.install(handler).is_err() {
        reset(SarSetupFault::Storage(SarSetupStorage::Handler));
    }
    let bound =
        router.bind(token, interrupt).unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<
                oer_esp32s31_hal::platform::Port,
                _,
            >(error)
        });
    (client.bind(bound), reader)
}
/// P2 task/IRQ windows never overlap: these contexts have the same level/hart.
pub(crate) fn begin_timing() {
    LOST.store(0, Ordering::Relaxed);
    MEASURING.store(true, Ordering::Relaxed);
}
pub(crate) fn end_timing() -> u32 {
    MEASURING.store(false, Ordering::Relaxed);
    LOST.load(Ordering::Relaxed)
}
pub(crate) fn interrupt(part: &mut Timg0AlarmInterrupt) {
    let entry = oer_riscv::cycle64();
    let measuring = MEASURING.load(Ordering::Relaxed);
    match HANDLER.with(|handler| {
        if !measuring {
            return handler.pilot.service(part, &oer_embassy_time::EmbassyClock);
        }
        let call = oer_riscv::cycle64();
        let result = handler.pilot.service(part, &oer_embassy_time::EmbassyClock);
        let returned = oer_riscv::cycle64();
        let empty_start = oer_riscv::cycle64();
        let empty_end = oer_riscv::cycle64();
        crate::pilot_timing::publish(
            &handler.samples,
            Sample {
                entry,
                call,
                returned,
                empty: empty_end.wrapping_sub(empty_start),
            },
            &LOST,
        );
        result
    }) {
        Some(Ok(())) => {}
        Some(Err(fault)) => oer_esp32s31_compose::phy_fault::reset(Fault {
            reason: FaultReason::SarAlarmWithoutBundle(fault),
            registration: None,
        }),
        None => reset(SarSetupFault::Storage(SarSetupStorage::Handler)),
    }
}

fn reset(fault: SarSetupFault) -> ! {
    oer_esp32s31_compose::phy_fault::reset(Fault {
        reason: FaultReason::SarSetup(fault),
        registration: None,
    })
}
