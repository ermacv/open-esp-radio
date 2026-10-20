//! The Wi-Fi interrupt line at the radio interrupt level (P3).
//!
//! One [`WifiIrqLine`] owns the active interrupt epoch: both register banks,
//! the MAC and WDEVPWR fact publishers, and the request word the radio
//! executor (P2) writes. Three handlers of the image run it, one after
//! another and never inside each other (esp-hal runs every source of one
//! priority on one CPU line): the MAC handler calls [`service_mac_line`],
//! the WDEVPWR handler [`service_power_line`] and the radio doorbell's
//! handler [`run_doorbell`]. The image keeps the line in a
//! [`HandlerCell`]; the cell is an ownership state machine, not a lock, so
//! no handler ever shares a lock with a task.
//!
//! P2 never touches an interrupt register while the epoch is live. It asks
//! the line through [`WifiIrqRequests`] and rings the doorbell:
//!
//! 1. P2 sets a request bit (`fetch_or`, Release), then rings
//!    ([`WifiIrqRequests::ring`]).
//! 2. The doorbell handler resets its software interrupt first, then takes
//!    every request at once (`swap(0)`, AcqRel). A ring after the reset
//!    raises the interrupt again, so no request is lost; requests made
//!    before the take coalesce.
//! 3. [`WifiIrqRequest::MaskAll`] wins: unless the banks are masked already,
//!    the line masks both, remembers it, and posts the masked fact
//!    ([`IrqBatch::masked`](oer_esp32s31_ieee80211_mac::irq::consumer::IrqBatch::masked))
//!    once. Otherwise [`WifiIrqRequest::UnmaskRx`] restores the RX delivery
//!    group. An unmask after the mask is ignored and counted.

use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicU32, Ordering},
};
use oer_interrupt_table::signal::SignalExecutor;

use oer_esp32s31_ieee80211_mac::irq::{
    IrqDisposition, MacInterrupt, MacPowerInterrupt, PowerIrqDisposition,
    consumer::{IrqPublisher, PowerIrqPublisher},
    epoch::{ActiveEpoch, Moderated},
};
use oer_memory::{HandlerCell, Installed};

use crate::rx::counters::WifiRxCounters;

/// The radio doorbell, as the radio executor sees it: a software interrupt
/// at the radio interrupt level whose handler calls [`run_doorbell`].
///
/// The image implements it over its `FROM_CPU` 3 doorbell; host tests run
/// the doorbell handler synchronously, as the interrupt would preempt P2.
pub trait RadioDoorbell {
    /// Raise the doorbell. Never waits.
    fn ring(&self);
}

/// The CPU routes of the MAC and WDEVPWR interrupt sources.
///
/// The doorbell's route is enabled at boot and is not part of this pair.
pub trait WifiIrqRoutes {
    /// Route WDEVPWR, then MAC, to the radio interrupt level.
    fn enable(&mut self);
    /// Silence both routes.
    fn silence(&mut self);
}

/// One request from the radio executor to the interrupt line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiIrqRequest {
    /// Restore the RX delivery group after the RX frontier was drained.
    UnmaskRx,
    /// Mask every source of both banks; wins over an unmask.
    MaskAll,
}

impl WifiIrqRequest {
    const fn bit(self) -> u32 {
        match self {
            Self::UnmaskRx => UNMASK_RX,
            Self::MaskAll => MASK_ALL,
        }
    }
}

const UNMASK_RX: u32 = 1;
const MASK_ALL: u32 = 2;

/// Requests the radio executor leaves for the interrupt line.
///
/// Its zero bytes are no request.
#[derive(bytemuck::Zeroable, Default)]
pub struct WifiIrqRequests {
    pending: AtomicU32,
}

impl WifiIrqRequests {
    pub const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
        }
    }

    /// Record `request` (Release) and ring `doorbell`. Never waits.
    pub fn ring(&self, request: WifiIrqRequest, doorbell: &impl RadioDoorbell) {
        self.pending.fetch_or(request.bit(), Ordering::Release);
        doorbell.ring();
    }

    /// Whether a request waits for the doorbell handler.
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) != 0
    }

    fn take(&self) -> u32 {
        self.pending.swap(0, Ordering::AcqRel)
    }

    fn clear(&self) {
        self.pending.store(0, Ordering::Release);
    }
}

/// The owner of one live interrupt epoch, run by the radio interrupt
/// handlers through a [`HandlerCell`].
///
/// `E` is the active epoch: [`ActiveEpoch<Moderated>`] over the PAC banks
/// on the chip, over the host models in tests.
pub struct WifiIrqLine<E, X: SignalExecutor> {
    epoch: Option<E>,
    mac: &'static IrqPublisher<'static, X>,
    power: PowerIrqPublisher<'static, X>,
    requests: &'static WifiIrqRequests,
    counters: &'static WifiRxCounters,
    masked: bool,
}

impl<E, X: SignalExecutor> WifiIrqLine<E, X> {
    const fn new(
        epoch: E,
        mac: &'static IrqPublisher<'static, X>,
        power: PowerIrqPublisher<'static, X>,
        requests: &'static WifiIrqRequests,
        counters: &'static WifiRxCounters,
    ) -> Self {
        Self {
            epoch: Some(epoch),
            mac,
            power,
            requests,
            counters,
            masked: false,
        }
    }

    /// Called only after the runner masked both banks and silenced CPU routes.
    pub(crate) fn take_epoch(&mut self) -> Option<E> {
        if !self.masked {
            return None;
        }
        self.epoch.take()
    }

    /// Reattach a returned epoch while CPU routes are still silent.
    pub(crate) fn put_epoch(&mut self, epoch: E) -> Result<(), E> {
        if self.epoch.is_some() {
            return Err(epoch);
        }
        self.requests.clear();
        self.masked = false;
        self.epoch = Some(epoch);
        Ok(())
    }

    /// Whether the line masked both banks.
    pub const fn masked(&self) -> bool {
        self.masked
    }

    /// The epoch, for a direct mask when the line never reached its cell.
    pub(crate) fn epoch_mut(&mut self) -> &mut E {
        self.epoch.as_mut().expect("the line has its active epoch")
    }

    #[cfg(test)]
    pub(crate) fn epoch(&self) -> &E {
        self.epoch.as_ref().expect("the line has its active epoch")
    }
}

/// The MAC handler's body: sample, mask RX before the acknowledgement,
/// acknowledge, count, then publish and wake after the register borrow
/// ended.
#[inline(always)]
pub fn service_mac_line<X: SignalExecutor, I: MacInterrupt, P: MacPowerInterrupt>(
    line: &mut WifiIrqLine<ActiveEpoch<Moderated, I, P>, X>,
) {
    let Some(epoch) = line.epoch.as_mut() else {
        return;
    };
    let entry = line.mac.begin_mac_entry();
    let (disposition, snapshot) = epoch.service_mac();
    line.counters.mac_interrupt(
        disposition == IrqDisposition::Spurious,
        snapshot.had_auxiliary_event,
        snapshot.had_unhandled_event,
    );
    entry
        .finish(line.mac.publish_snapshot_deferred(snapshot))
        .wake();
}

/// The WDEVPWR handler's body: acknowledge, count and publish the causes.
#[inline(always)]
pub fn service_power_line<X: SignalExecutor, I: MacInterrupt, P: MacPowerInterrupt>(
    line: &mut WifiIrqLine<ActiveEpoch<Moderated, I, P>, X>,
) {
    let Some(epoch) = line.epoch.as_mut() else {
        return;
    };
    let (disposition, snapshot) = epoch.service_power();
    let posted = disposition == PowerIrqDisposition::Posted;
    line.counters.power_interrupt(!posted);
    if posted {
        line.power.publish(snapshot.observation);
    }
}

/// Apply the requests the radio executor left: the doorbell's work once
/// its software interrupt was reset.
#[inline(always)]
pub fn service_doorbell_line<X: SignalExecutor, I: MacInterrupt, P: MacPowerInterrupt>(
    line: &mut WifiIrqLine<ActiveEpoch<Moderated, I, P>, X>,
) {
    let pending = line.requests.take();
    let Some(epoch) = line.epoch.as_mut() else {
        return;
    };
    if pending & MASK_ALL != 0 {
        if !line.masked {
            epoch.mask_all();
            line.masked = true;
            line.mac.notify_masked();
        }
        if pending & UNMASK_RX != 0 {
            line.counters.ignored_unmask();
        }
    } else if pending & UNMASK_RX != 0 {
        if line.masked {
            line.counters.ignored_unmask();
        } else {
            epoch.unmask_rx_after_drain();
            line.counters.doorbell_unmask();
        }
    }
}

/// The doorbell handler: `acknowledge` resets its software interrupt
/// (`exec`'s `acknowledge::<3>` on the chip) before the requests are taken.
///
/// The reset happens even when `cell` holds no line, so a stray raise
/// cannot keep the radio level busy. Returns whether a line ran.
#[inline(always)]
pub fn run_doorbell<X: SignalExecutor, I: MacInterrupt, P: MacPowerInterrupt>(
    cell: &HandlerCell<WifiIrqLine<ActiveEpoch<Moderated, I, P>, X>>,
    acknowledge: impl FnOnce(),
) -> bool {
    acknowledge();
    cell.with(service_doorbell_line).is_some()
}

/// The arena's parts of one interrupt line, before its epoch exists.
pub struct WifiIrqLineParts<'a, E, X: SignalExecutor> {
    pub(crate) mac: &'a IrqPublisher<'a, X>,
    pub(crate) power: PowerIrqPublisher<'a, X>,
    pub(crate) requests: &'a WifiIrqRequests,
    pub(crate) counters: &'a WifiRxCounters,
    pub(crate) slot: &'a mut MaybeUninit<WifiIrqLine<E, X>>,
}

impl<E, X: SignalExecutor> WifiIrqLineParts<'static, E, X> {
    /// Put the line over `epoch` in its arena slot and hand it to the
    /// handlers' `cell`.
    ///
    /// A request left from an earlier epoch is cleared first: the new line
    /// starts with none. When `cell` already holds a line, the new one is
    /// returned, still owned by the caller and never run by a handler.
    pub fn install(
        self,
        epoch: E,
        cell: &'static HandlerCell<WifiIrqLine<E, X>>,
    ) -> Result<Installed<'static, WifiIrqLine<E, X>>, &'static mut WifiIrqLine<E, X>> {
        self.requests.clear();
        let line = WifiIrqLine::new(epoch, self.mac, self.power, self.requests, self.counters);
        cell.install(self.slot.write(line))
    }
}

#[cfg(test)]
mod tests;
