//! The ESP32-S31 placement of the post-mortem record.
//!
//! The record lives in `.rtc_fast.persistent`, RTC fast memory that no reset
//! entry initializes, so what one boot writes is there for the next unless
//! the chip lost power. [`begin`] runs once, early in every boot: it takes
//! what the previous boot left and starts this boot's ring. The record is
//! written only inside the critical section, except by the panic handler,
//! which must not wait for another hart.

use core::cell::{RefCell, UnsafeCell};

use critical_section::Mutex;
use oer_hil_agent::postmortem::{Previous, RateMonitor, Record};

struct Retained(UnsafeCell<Record>);

// SAFETY: every access goes through `with_record`, inside the critical
// section, or through the panic path documented at `record_panic`.
#[allow(
    unsafe_code,
    reason = "a static in retained memory needs interior mutability"
)]
unsafe impl Sync for Retained {}

// Every bit pattern is a `Record`, so memory no reset initialized is a value;
// the initializer is never loaded into this NOLOAD section.
#[allow(
    unsafe_code,
    reason = "placing the record in retained memory needs a link section"
)]
#[unsafe(link_section = ".rtc_fast.persistent")]
static RECORD: Retained = Retained(UnsafeCell::new(Record::EMPTY));

static PREVIOUS: Mutex<RefCell<Option<Previous>>> = Mutex::new(RefCell::new(None));
static RATE: Mutex<RefCell<RateMonitor>> = Mutex::new(RefCell::new(RateMonitor::new()));

fn with_record<T>(access: impl FnOnce(&mut Record) -> T) -> T {
    critical_section::with(|_| {
        // SAFETY: the critical section excludes every other access but the
        // panic path.
        #[allow(unsafe_code, reason = "the retained record is a raw static")]
        let record = unsafe { &mut *RECORD.0.get() };
        access(record)
    })
}

fn uptime_ms() -> u32 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis() as u32
}

fn hart() -> u8 {
    esp_hal::system::Cpu::current() as u8
}

/// Take what the previous boot left and start this boot's record.
#[cfg(feature = "system-watchdog")]
pub(crate) fn begin() {
    let previous = with_record(Record::begin_boot);
    critical_section::with(|cs| *PREVIOUS.borrow_ref_mut(cs) = previous);
}

/// The previous boot's record, as boot evidence reports it.
pub(crate) fn previous<T>(read: impl FnOnce(Option<&Previous>) -> T) -> T {
    critical_section::with(|cs| read(PREVIOUS.borrow_ref(cs).as_ref()))
}

/// Record that this boot passed `name`. For named phase transitions, not
/// hot loops: debug builds log a second with too many checkpoints.
#[allow(dead_code, reason = "each image checkpoints its own phases")]
pub(crate) fn checkpoint(name: &str, arg: u32) {
    let now = uptime_ms();
    with_record(|record| record.checkpoint(name, arg, now, hart()));
    let excess = critical_section::with(|cs| RATE.borrow_ref_mut(cs).record(now));
    let _ = excess;
}

/// Record the panic being handled. The other hart may hold the critical
/// section while this one panics, so the record is written without it; a
/// checkpoint racing this write can at worst fail its own CRC.
#[cfg(feature = "system-watchdog")]
#[unsafe(link_section = ".flash.critical.text.panic_hook")]
pub(crate) fn record_panic(details: &oer_espressif_staged_runtime::panic::PanicDetails<'_>) {
    // SAFETY: see above; the halted boot never reads the record again.
    #[allow(unsafe_code, reason = "a panic cannot wait for the critical section")]
    let record = unsafe { &mut *RECORD.0.get() };
    record.record_panic(details.file, details.line, details.message.unwrap_or(""));
}
