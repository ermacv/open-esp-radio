//! The ESP32-S31 placement of the post-mortem record.
//!
//! The record lives in `.rtc_fast.persistent`, RTC fast memory that no reset
//! entry initializes, so what one boot writes is there for the next unless
//! the chip lost power. [`begin`] runs once, early in every boot: it takes
//! what the previous boot left and starts this boot's ring. The checkpoint
//! ring and the fault are separate retained cells: checkpoints are written
//! inside the critical section, the fault by the panic handler, which must
//! not wait for another hart and so never shares a cell with a checkpoint.

use core::cell::RefCell;

use critical_section::Mutex;
use oer_hil_agent::postmortem::{FaultRecord, Previous, RateMonitor, Record};

oer_espressif_staged_runtime::retained_static! {
    /// The checkpoint ring, taken only inside the critical section.
    static RECORD: Record;
}
oer_espressif_staged_runtime::retained_static! {
    /// The fault, taken by the panic handler without waiting: while the
    /// other hart records its own panic, this one's is skipped.
    static FAULT: FaultRecord;
}

static PREVIOUS: Mutex<RefCell<Option<Previous>>> = Mutex::new(RefCell::new(None));
static RATE: Mutex<RefCell<RateMonitor>> = Mutex::new(RefCell::new(RateMonitor::new()));

/// The ring, inside the critical section. Only the critical section takes
/// the ring, so `None` is unreachable but for a panic inside `access`.
fn with_record<T>(access: impl FnOnce(&mut Record) -> T) -> Option<T> {
    critical_section::with(|_| RECORD.try_lock().map(|mut record| access(&mut record)))
}

fn uptime_ms() -> u32 {
    embassy_time::Instant::now().as_millis() as u32
}

fn hart() -> u8 {
    oer_espressif_staged_runtime::trap::TrapRegisters::read().hart as u8
}

/// Take what the previous boot left and start this boot's record.
#[cfg(feature = "system-watchdog")]
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

/// Record that this boot passed `name`. For named phase transitions, not
/// hot loops: debug builds log a second with too many checkpoints.
#[allow(dead_code, reason = "each image checkpoints its own phases")]
pub(crate) fn checkpoint(name: &str, arg: u32) {
    let now = uptime_ms();
    with_record(|record| {
        if let Err(fault) = record.checkpoint(name, arg, now, hart()) {
            oer_espressif_staged_runtime::panic::reset_fault::<oer_esp32s31_hal::platform::Port>(
                oer_espressif_staged_runtime::panic::FaultReason::HilCheckpoint,
                [fault.head, fault.len, arg, now],
            );
        }
    });
    critical_section::with(|cs| {
        let mut rate = RATE.borrow_ref_mut(cs);
        if let Err(fault) = rate.record(now) {
            oer_espressif_staged_runtime::panic::reset_fault::<oer_esp32s31_hal::platform::Port>(
                oer_espressif_staged_runtime::panic::FaultReason::HilRateOverflow,
                [fault.second, fault.count, now, 0],
            );
        }
    });
}

/// Record the panic being handled. The other hart may hold the critical
/// section while this one panics, so the fault is taken without it, from
/// its own cell that no checkpoint takes.
#[cfg(feature = "system-watchdog")]
pub(crate) fn record_panic(details: &oer_espressif_staged_runtime::panic::PanicDetails<'_>) {
    if let Some(mut fault) = FAULT.try_lock() {
        fault.record_panic(details.file, details.line, details.message.unwrap_or(""));
    }
}
