//! The ESP32-C5 placement of the post-mortem record.
//!
//! The record lives in `.rtc_fast.persistent`, LP RAM that no reset entry
//! initializes (the staged runtime's retained region), so what one boot writes is there for the next unless
//! the chip lost power. [`begin`] runs once, early in every boot: it takes
//! what the previous boot left and starts this boot's ring. The record is
//! written only inside the critical section, except by the panic handler.

use core::cell::{RefCell, UnsafeCell};

use critical_section::Mutex;
use oer_hil_agent::postmortem::{Previous, Record};

struct Retained(UnsafeCell<Record>);

// SAFETY: every access goes through `with_record`, inside the critical
// section, or through the panic path documented at `record_panic`.
unsafe impl Sync for Retained {}

// Every bit pattern is a `Record`, so memory no reset initialized is a value;
// the initializer is never loaded into this NOLOAD section.
#[unsafe(link_section = ".rtc_fast.persistent")]
static RECORD: Retained = Retained(UnsafeCell::new(Record::EMPTY));

static PREVIOUS: Mutex<RefCell<Option<Previous>>> = Mutex::new(RefCell::new(None));

fn with_record<T>(access: impl FnOnce(&mut Record) -> T) -> T {
    critical_section::with(|_| {
        // SAFETY: the critical section excludes every other access but the
        // panic path.
        let record = unsafe { &mut *RECORD.0.get() };
        access(record)
    })
}

fn uptime_ms() -> u32 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis() as u32
}

/// Take what the previous boot left and start this boot's record.
pub(crate) fn begin() {
    let previous = with_record(Record::begin_boot);
    critical_section::with(|cs| *PREVIOUS.borrow_ref_mut(cs) = previous);
}

/// The previous boot's record, as boot evidence reports it.
pub(crate) fn previous<T>(read: impl FnOnce(Option<&Previous>) -> T) -> T {
    critical_section::with(|cs| read(PREVIOUS.borrow_ref(cs).as_ref()))
}

/// Record that this boot passed `name`, for named phase transitions. The
/// single hart is 0.
pub(crate) fn checkpoint(name: &str, arg: u32) {
    let now = uptime_ms();
    with_record(|record| record.checkpoint(name, arg, now, 0));
}

/// Record the panic being handled, from the platform's panic entry. It is
/// written without the critical section, which the panicking code may hold;
/// the chip resets next and never reads the record again in this boot.
/// Formatting would put `core::fmt` on every context's panic path: a message
/// with arguments is recorded empty.
#[unsafe(link_section = ".flash.critical.text.panic_hook")]
pub(crate) fn record_panic(details: &oer_espressif_staged_runtime::panic::PanicDetails<'_>) {
    // SAFETY: see above.
    let record = unsafe { &mut *RECORD.0.get() };
    record.record_panic(details.file, details.line, details.message.unwrap_or(""));
}
