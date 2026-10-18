//! The image's reset-retained event trace and its drain for the host.
//!
//! One `oer_trace` trace lives in RTC fast memory beside the post-mortem
//! record, so it survives the resets the post-mortem does. After a clean
//! boot it records at once, every channel on; the host narrows the channels
//! per session. When the previous boot froze its trace (a hang, a panic, or
//! a driver's own trigger), the new boot holds it untouched until the host
//! has paged it out and starts recording, or for [`HOLD_LIMIT`] at most.

use embassy_time::{Duration, Timer};
use oer_hil_protocol::{
    telemetry::TRACE_ENTRY_PAGE, telemetry::TRACE_SNAPSHOT_PAGE, telemetry::TraceControl,
    telemetry::TraceEntries, telemetry::TraceEntry, telemetry::TraceSnapshotPage,
    telemetry::TraceStatus,
};
use oer_trace::{Retained, Trace};

const ENTRIES: usize = 512;
/// Snapshot slots and their words: only images that capture register
/// windows (`trace-snapshots`) spend RTC memory on them.
#[cfg(feature = "trace-snapshots")]
const SLOTS: usize = 2;
#[cfg(feature = "trace-snapshots")]
const WORDS: usize = 1024;
#[cfg(not(feature = "trace-snapshots"))]
const SLOTS: usize = 0;
#[cfg(not(feature = "trace-snapshots"))]
const WORDS: usize = 0;

#[unsafe(link_section = ".rtc_fast.persistent")]
static RETAINED: Retained<ENTRIES, SLOTS, WORDS> = Retained::new();
#[unsafe(link_section = ".critical.data.panic_trace")]
static TRACE: Trace = Trace::new(&RETAINED);

oer_trace::clock!(oer_time_embassy::now_micros);

/// Every channel records until the host chooses its own.
const DEFAULT_MASK: u64 = u64::MAX;
/// How long a boot keeps the previous boot's frozen trace for the host.
const HOLD_LIMIT: Duration = Duration::from_secs(30);

/// Install the trace; start it unless the previous boot left a frozen one.
pub(crate) fn install() {
    let boot = oer_trace::install(&TRACE);
    if !boot
        .previous
        .is_some_and(|previous| previous.trigger.is_some())
    {
        oer_trace::start(DEFAULT_MASK);
    }
}

/// Start recording once [`HOLD_LIMIT`] passed with the previous boot's trace
/// still held.
#[embassy_executor::task]
pub(crate) async fn hold_limit_task() {
    Timer::after(HOLD_LIMIT).await;
    if holding() {
        oer_trace::start(DEFAULT_MASK);
    }
}

fn holding() -> bool {
    oer_trace::installed().is_some_and(|trace| !trace.is_running() && !trace.is_frozen())
}

/// Apply `control` and report the trace's state.
pub(crate) fn control(control: TraceControl) -> TraceStatus {
    match control {
        TraceControl::Status => {}
        TraceControl::Start { mask } => oer_trace::start(mask),
        TraceControl::SetMask { mask } => oer_trace::set_mask(mask),
    }
    status()
}

fn status() -> TraceStatus {
    let Some(trace) = oer_trace::installed() else {
        return TraceStatus {
            installed: false,
            entries: 0,
            snapshot_slots: 0,
            snapshot_words: 0,
            running: false,
            frozen: false,
            mask: 0,
            trigger: None,
            holding_previous: false,
            stored_entries: 0,
            stored_snapshots: 0,
        };
    };
    let geometry = trace.geometry();
    TraceStatus {
        installed: true,
        entries: geometry.entries as u16,
        snapshot_slots: geometry.slots as u8,
        snapshot_words: geometry.words_per_slot as u16,
        running: trace.is_running(),
        frozen: trace.is_frozen(),
        mask: oer_trace::mask(),
        trigger: trace.trigger().map(|trigger| (trigger.kind, trigger.tag)),
        holding_previous: holding(),
        stored_entries: trace.records().count() as u16,
        stored_snapshots: trace.snapshots().count() as u8,
    }
}

/// The complete entries of storage slots `first..`, one page.
pub(crate) fn entries(first: u16) -> TraceEntries {
    let mut page = TraceEntries {
        first,
        next: first,
        entries: Default::default(),
    };
    let Some(trace) = oer_trace::installed() else {
        return page;
    };
    let total = trace.geometry().entries as u16;
    while page.next < total && !page.entries.is_full() {
        if let Some(record) = trace.entry(usize::from(page.next)) {
            let _ = page.entries.push(TraceEntry {
                tag: record.tag,
                kind: record.kind,
                t_us: record.t_us,
                words: record.words,
            });
        }
        page.next += 1;
    }
    page
}

/// Words `offset..` of the snapshot in `slot`, one page; `None` when the
/// slot holds none, or another one by the time it was read.
pub(crate) fn snapshot(slot: u8, offset: u16) -> Option<TraceSnapshotPage> {
    let trace = oer_trace::installed()?;
    let snapshot = trace.snapshot(usize::from(slot))?;
    let mut words = [0_u32; TRACE_SNAPSHOT_PAGE];
    let count = trace.read_snapshot(&snapshot, usize::from(offset), &mut words)?;
    Some(TraceSnapshotPage {
        slot,
        point: snapshot.point,
        tag: snapshot.tag,
        t_us: snapshot.t_us,
        len: snapshot.len as u16,
        truncated: snapshot.truncated,
        offset,
        words: words[..count].iter().copied().collect(),
    })
}

const _: () = assert!(TRACE_ENTRY_PAGE <= u16::MAX as usize);
