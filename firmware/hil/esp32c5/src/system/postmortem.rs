//! The ESP32-C5 placement of the post-mortem record.
//!
//! The record lives in `.rtc_fast.persistent`, LP RAM that no reset entry
//! initializes (the staged runtime's retained region), so what one boot writes is there for the next unless
//! the chip lost power. [`begin`] runs once, early in every boot: it takes
//! what the previous boot left and starts this boot's ring. The checkpoint
//! ring is written inside the critical section, the fault by the panic
//! handler, each in its own retained cell.

use core::cell::RefCell;

use critical_section::Mutex;
use oer_hil_agent::postmortem::{FaultRecord, Previous, Record};

oer_espressif_staged_runtime::retained_static! {
    /// The checkpoint ring, taken only inside the critical section.
    static RECORD: Record;
}
oer_espressif_staged_runtime::retained_static! {
    /// The fault, taken by the panic handler without waiting, from a cell no
    /// checkpoint takes.
    static FAULT: FaultRecord;
}

static PREVIOUS: Mutex<RefCell<Option<Previous>>> = Mutex::new(RefCell::new(None));

/// The ring, inside the critical section. Only the critical section takes
/// the ring, so `None` is unreachable but for a panic inside `access`.
fn with_record<T>(access: impl FnOnce(&mut Record) -> T) -> Option<T> {
    critical_section::with(|_| RECORD.try_lock().map(|mut record| access(&mut record)))
}

fn uptime_ms() -> u32 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis() as u32
}

/// Take what the previous boot left and start this boot's record.
pub(crate) fn begin() {
    let previous = critical_section::with(|_| {
        let mut fault = FAULT.try_lock()?;
        RECORD.try_lock()?.begin_boot(&mut fault)
    });
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

/// Record the panic being handled, from the platform's panic entry. The
/// fault has its own cell, so the panic takes it without the critical
/// section, which the panicking code may hold; the chip resets next.
/// Formatting would put `core::fmt` on every context's panic path: a
/// message with arguments is recorded empty.
pub(crate) fn record_panic(details: &oer_espressif_staged_runtime::panic::PanicDetails<'_>) {
    if let Some(mut fault) = FAULT.try_lock() {
        fault.record_panic(details.file, details.line, details.message.unwrap_or(""));
    }
}
