//! A source of the image's interrupt table, silent until its owner enables
//! it: system timer alarm 2, armed by the HIL interrupt-table probe.

use core::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};

use critical_section::Mutex;
use esp_hal::{Blocking, timer::OneShotTimer};

/// How many times the handler ran.
static FIRED: AtomicU32 = AtomicU32::new(0);

static TIMER: Mutex<RefCell<Option<OneShotTimer<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));

/// The table's handler of `SYSTIMER_TARGET2`: count, then quiet the alarm.
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub(crate) fn on_alarm() {
    FIRED.fetch_add(1, Ordering::Relaxed);
    critical_section::with(|cs| {
        if let Some(timer) = TIMER.borrow_ref_mut(cs).as_mut() {
            timer.unlisten();
            timer.clear_interrupt();
        }
    });
}

pub(crate) use probe::{install, probe};

mod probe {
    use super::{FIRED, Ordering, TIMER};
    use crate::SourceGateToken;
    use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
    use embassy_time::Timer;
    use esp_hal::{Blocking, timer::OneShotTimer};
    use oer_hil_protocol::system::SourceGated;

    /// The alarm's token, while the probe owns the source.
    static TOKEN: Mutex<CriticalSectionRawMutex, Option<SourceGateToken>> = Mutex::new(None);

    /// Own the alarm and its source's token.
    pub(crate) fn install(timer: OneShotTimer<'static, Blocking>, token: SourceGateToken) {
        critical_section::with(|cs| TIMER.borrow_ref_mut(cs).replace(timer));
        TOKEN
            .try_lock()
            .expect("the probe installs its token once at boot")
            .replace(token);
    }

    /// Fire the alarm once with its source silent, once enabled and once
    /// disabled again; `None` before [`install`].
    pub(crate) async fn probe() -> Option<SourceGated> {
        let token = TOKEN.lock().await;
        let token = token.as_ref()?;
        FIRED.store(0, Ordering::Relaxed);
        fire();
        Timer::after_millis(5).await;
        let before_enable = FIRED.load(Ordering::Relaxed);
        // Still pending: enabling the source delivers it.
        if oer_espressif_interrupt_table_esp_hal::enable(token).is_err() {
            quiet();
            return None;
        }
        Timer::after_millis(5).await;
        let enabled = FIRED.load(Ordering::Relaxed) - before_enable;
        oer_espressif_interrupt_table_esp_hal::disable(token);
        fire();
        Timer::after_millis(5).await;
        let after_disable = FIRED.load(Ordering::Relaxed) - before_enable - enabled;
        quiet();
        Some(SourceGated {
            before_enable,
            enabled,
            after_disable,
        })
    }

    /// Arm the alarm for one millisecond from now, with its peripheral
    /// interrupt on.
    fn fire() {
        critical_section::with(|cs| {
            if let Some(timer) = TIMER.borrow_ref_mut(cs).as_mut() {
                timer.clear_interrupt();
                timer
                    .schedule(esp_hal::time::Duration::from_millis(1))
                    .expect("one millisecond fits the alarm");
                timer.listen();
            }
        });
    }

    /// Leave the alarm stopped, its interrupt off and clear.
    fn quiet() {
        critical_section::with(|cs| {
            if let Some(timer) = TIMER.borrow_ref_mut(cs).as_mut() {
                timer.unlisten();
                timer.stop();
                timer.clear_interrupt();
            }
        });
    }
}
