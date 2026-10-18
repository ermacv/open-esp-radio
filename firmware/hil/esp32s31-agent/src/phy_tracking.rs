//! Wire control of the shared PHY's periodic tracking timer.
//!
//! The timer runs from boot. A suspension ends it between ticks, so a started
//! tick always finishes; a resumption starts it again. The counts cover every
//! tick since boot, for comparing traffic with and without tracking.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use oer_esp32s31_phy::{ConcurrentPhyTrackingError, ConcurrentTrackingTick};
#[cfg(feature = "phy-tracking-hold")]
use oer_hil_protocol::{
    base::RejectReason, phy::PhyTrackingHoldEvidence, phy::PhyTrackingHoldRequest,
};
use oer_hil_protocol::{phy::PhyTrackingCommand, phy::PhyTrackingEvidence};

static SUSPENDED: AtomicBool = AtomicBool::new(false);
static CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static TRACKED: AtomicU32 = AtomicU32::new(0);
static NOT_DUE: AtomicU32 = AtomicU32::new(0);
static SKIPPED: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "phy-tracking-hold")]
static HOLD_REQUEST: Signal<CriticalSectionRawMutex, PhyTrackingHoldRequest> = Signal::new();
#[cfg(feature = "phy-tracking-hold")]
static HOLD_MEASURED: Signal<CriticalSectionRawMutex, PhyTrackingHoldEvidence> = Signal::new();
/// A task serves hold requests: the image's tracking task, or an IEEE
/// 802.15.4 session, which runs instead of it.
#[cfg(feature = "phy-tracking-hold")]
static HOLD_SERVED: AtomicBool = AtomicBool::new(false);

/// Registers its holder as the server of hold requests until dropped.
#[cfg(feature = "phy-tracking-hold")]
pub(crate) struct HoldServer(());

#[cfg(feature = "phy-tracking-hold")]
impl HoldServer {
    pub(crate) fn register() -> Self {
        HOLD_SERVED.store(true, Ordering::Release);
        Self(())
    }
}

#[cfg(feature = "phy-tracking-hold")]
impl Drop for HoldServer {
    fn drop(&mut self) {
        HOLD_SERVED.store(false, Ordering::Release);
    }
}

/// Time one forced tick through the suspended tracking task.
///
/// # Errors
///
/// The periodic timer runs (suspend it first), or no task serves hold
/// requests in this image mode.
#[cfg(feature = "phy-tracking-hold")]
pub(crate) async fn measure_hold(
    request: PhyTrackingHoldRequest,
) -> Result<PhyTrackingHoldEvidence, RejectReason> {
    if !suspended() || !HOLD_SERVED.load(Ordering::Acquire) {
        return Err(RejectReason::InvalidState);
    }
    HOLD_MEASURED.reset();
    HOLD_REQUEST.signal(request);
    Ok(HOLD_MEASURED.wait().await)
}

/// The next hold measurement the suspended task must run.
#[cfg(feature = "phy-tracking-hold")]
pub(crate) async fn hold_requested() -> PhyTrackingHoldRequest {
    HOLD_REQUEST.wait().await
}

/// Hand a measured tick back to its request.
#[cfg(feature = "phy-tracking-hold")]
pub(crate) fn hold_measured(evidence: PhyTrackingHoldEvidence) {
    HOLD_MEASURED.signal(evidence);
}

/// Apply one wire command and report the timer.
pub(crate) fn control(command: PhyTrackingCommand) -> oer_hil_protocol::phy::TrackingState {
    match command {
        PhyTrackingCommand::Resume => set_suspended(false),
        PhyTrackingCommand::Suspend => set_suspended(true),
        PhyTrackingCommand::Status => {}
    }
    oer_hil_protocol::phy::TrackingState(PhyTrackingEvidence {
        running: !SUSPENDED.load(Ordering::Acquire),
        tracked: TRACKED.load(Ordering::Relaxed),
        not_due: NOT_DUE.load(Ordering::Relaxed),
        skipped: SKIPPED.load(Ordering::Relaxed),
    })
}

fn set_suspended(suspended: bool) {
    SUSPENDED.store(suspended, Ordering::Release);
    CHANGED.signal(());
}

/// Whether the timer is suspended now.
pub(crate) fn suspended() -> bool {
    SUSPENDED.load(Ordering::Acquire)
}

/// Wait for the next suspension or resumption.
pub(crate) async fn changed() {
    CHANGED.wait().await;
}

/// Complete once the timer is suspended.
pub(crate) async fn until_suspended() {
    while !suspended() {
        changed().await;
    }
}

/// Count one tick's result.
pub(crate) fn record(tick: &Result<ConcurrentTrackingTick, ConcurrentPhyTrackingError>) {
    let counter = match tick {
        Ok(ConcurrentTrackingTick::Tracked(_)) => &TRACKED,
        Ok(ConcurrentTrackingTick::NotDue) => &NOT_DUE,
        Ok(ConcurrentTrackingTick::Unavailable(_)) => &SKIPPED,
        Err(_) => return,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}
