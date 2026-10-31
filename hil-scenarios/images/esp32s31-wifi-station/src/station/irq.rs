//! The Wi-Fi interrupt line and the radio executor's signals.
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_wifi::{InterruptTypes, Interrupts};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use oer_espressif_executor::InterruptExecutor;
use oer_espressif_interrupt_table::signal::{FromHandler, FromTask, IrqSignal};

type RadioExecutor = InterruptExecutor<Esp32s31, 2>;
oer_memory::zeroed_static! {
    pub(crate) static LINE: Interrupts = zeroed in ".critical.bss.wifi_rx_line";
}
oer_espressif_staged_runtime::placed_static! {
    pub(crate) static MAC_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<(FromHandler<crate::WifiToken>, FromTask<oer_espressif_executor::Executor<Esp32s31, 0>, crate::ExecutorWake>), crate::RadioToken>(), in critical_data "wifi_station";
    pub(crate) static POWER_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<FromHandler<crate::WifiToken>, crate::RadioToken>(), in critical_data "wifi_station";
}
const RING_SAMPLES: usize = 4096;
oer_espressif_staged_runtime::placed_static! {
    static RING: [AtomicU32; RING_SAMPLES] = [const { AtomicU32::new(0) }; RING_SAMPLES], in critical_data "samples";
    static KEPT: AtomicU32 = AtomicU32::new(0), in critical_data "samples";
    static INTERRUPTS: AtomicU32 = AtomicU32::new(0), in critical_data "samples";
    static ARMED: AtomicBool = AtomicBool::new(false), in critical_data "samples";
}

/// Start (clearing the ring) or stop sampling the interrupted program
/// counters.
pub(crate) fn arm(on: bool) {
    if on {
        KEPT.store(0, Ordering::Relaxed);
        INTERRUPTS.store(0, Ordering::Relaxed);
    }
    ARMED.store(on, Ordering::Relaxed);
}

/// The kept samples from `from`.
pub(crate) fn samples(from: u16) -> oer_services_load::Samples {
    let kept = KEPT.load(Ordering::Relaxed).min(RING_SAMPLES as u32);
    oer_services_load::Samples {
        kept: kept as u16,
        interrupts: INTERRUPTS.load(Ordering::Relaxed),
        pcs: RING
            .iter()
            .take(kept as usize)
            .skip(usize::from(from))
            .take(oer_services_load::MAX_SAMPLES)
            .map(|slot| slot.load(Ordering::Relaxed))
            .collect(),
    }
}

pub(crate) type WifiPart = <Interrupts as InterruptTypes>::Part;

/// The radio handler (P3): the doorbell's requests, then the MAC bank, then
/// the WDEVPWR bank, one cause per entry. A MAC fatal assertion is recorded
/// and resets the chip.
/// The application executor's alarm, sampling the interrupted program
/// counter first.
pub(crate) fn on_radio_alarm(part: &mut oer_esp32s31_hal::peripherals::system_timer::SystimerAlarm0) {
    sample();
    oer_espressif_executor::alarm_interrupt::<crate::station::RadioTasks, 0, _>(part);
}

fn sample() {
    if ARMED.load(Ordering::Relaxed) {
        let pc = oer_riscv::read_mepc() as u32;
        INTERRUPTS.fetch_add(1, Ordering::Relaxed);
        let ra = match oer_espressif_staged_runtime::stacks::outermost_frame_ra() {
            Some((ra, mepc)) if mepc == pc => ra,
            Some(_) => 1,
            None => 0,
        };
        let at = KEPT.load(Ordering::Relaxed);
        if let (Some(slot), Some(next)) = (RING.get(at as usize), RING.get(at as usize + 1)) {
            slot.store(pc, Ordering::Relaxed);
            next.store(ra, Ordering::Relaxed);
            KEPT.store(at.wrapping_add(2), Ordering::Relaxed);
        }
    }
}

pub(crate) fn on_wifi(part: &mut WifiPart) {
    match LINE.service(part) {
        Ok(_installed) => {}
        Err(raw) => oer_esp32s31_compose::wifi_fault::reset(raw),
    }
}
