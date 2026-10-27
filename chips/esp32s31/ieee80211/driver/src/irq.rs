//! The Wi-Fi interrupt line at the radio interrupt level (P3).
//!
//! One entry of the image's interrupt table binds [`WifiIrqPart`] once, at
//! start: both interrupt banks of the MAC (`MODEM_WIFI_MAC` and
//! `MODEM_WIFI_PWR`) and the radio doorbell (`FROM_CPU_INTR3`) route to its
//! one line, and its handler holds the part for the image's life. The
//! handler calls [`service_wifi_line`], which learns what fired from the
//! peripherals themselves and serves one cause per entry: it lowers the
//! doorbell and applies the requests it takes; without one it serves the
//! MAC bank, or else the WDEVPWR bank. The line is level-triggered, so a
//! cause left for later enters the handler again at once.
//!
//! The banks' epoch ([`InterruptEpoch`]) is the handler's alone. The radio
//! executor (P2) never touches an interrupt register and never the CPU
//! route; it asks the line through [`WifiIrqRequests`] and rings the
//! doorbell:
//!
//! 1. P2 sets a request bit (`fetch_or`, Release), then rings
//!    ([`WifiIrqRequests::ring`], or [`WifiIrqRequests::activate`] with the
//!    binding's [`Bound`] proof).
//! 2. The handler lowers the doorbell first, then takes every request at
//!    once (`swap(0)`, AcqRel). A ring after the lowering raises the line
//!    again, so no request is lost; requests made before the take coalesce.
//!    The handler preempts P2, so once [`WifiIrqRequests::is_pending`] reads
//!    `false` the requests it took are applied.
//! 3. Of the requests taken together the safest wins:
//!    [`WifiIrqRequest::Close`] (mask and clear both banks), then
//!    [`WifiIrqRequest::MaskAll`] (mask both, keep the latched events; the
//!    masked fact is posted once,
//!    [`IrqBatch::masked`](oer_esp32s31_ieee80211_mac::irq::consumer::IrqBatch::masked)),
//!    then the activation (clear, then publish the station mask), then
//!    [`WifiIrqRequest::UnmaskRx`] (restore the RX delivery group of an
//!    active epoch). An unmask that loses, or reaches a masked or closed
//!    epoch, is ignored and counted.
//!
//! The handler reaches the receive arena's publishers through the line
//! ([`WifiIrqLine`]), which the task installs once in a [`HandlerCell`]; the
//! cell is an ownership state machine, not a lock, so the handler never
//! shares a lock with a task. Before the line is installed the handler
//! lowers the doorbell and closes a stray bank event, nothing else.

use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
};
use oer_espressif_interrupt_table::{Bound, InterruptSources, signal::SignalExecutor};

use oer_esp32s31_hal::{
    ieee80211::init::MAC_STATION_INTERRUPT_MASK,
    owner::{MacInterruptRegisters, MacPowerInterruptRegisters, WifiInterruptBanks},
    platform::FromCpu3Irq,
    types::MacInterruptEnableState,
};
use oer_esp32s31_ieee80211_mac::irq::{
    IrqDisposition, MacInterrupt, MacPowerInterrupt, PowerIrqDisposition,
    consumer::{IrqPublisher, PowerIrqPublisher},
    epoch::{EpochState, InterruptEpoch, Moderated},
};
use oer_memory::{HandlerCell, Installed};

use crate::rx::counters::WifiRxCounters;

use oer_esp32s31_ieee80211_mac::irq::Fault;

/// The radio doorbell, as the radio executor sees it: a software interrupt
/// at the radio interrupt level, one of [`WifiIrqPart`]'s sources.
///
/// The image implements it over its `FROM_CPU` 3 doorbell; host tests run
/// the handler synchronously, as the interrupt would preempt P2.
pub trait RadioDoorbell {
    const HANDLER: oer_espressif_interrupt_table::Context;
    const EXECUTOR: oer_espressif_interrupt_table::Context;
    /// Raise the doorbell. Never waits.
    fn ring(&self);
}

/// The doorbell's lowering half, which the handler holds in its part
/// (the executor's `DoorbellAcknowledger<3>` on the chip).
pub trait DoorbellAcknowledge {
    /// Lower the doorbell before the requests are taken. Never waits.
    fn acknowledge(&mut self);
}

/// The radio interrupt handler's part: both MAC interrupt banks in their
/// epoch, the doorbell's interrupt part and its lowering half.
///
/// `A` lowers the doorbell; `B`, `I` and `P` are the doorbell's part and
/// the two banks, the chip's by default and host models in tests. The
/// image's interrupt table binds it once, its sources from its type:
///
/// ```no_run
/// use oer_esp32s31_hal::{owner::WifiInterruptBanks, platform::FromCpu3Irq};
/// use oer_esp32s31_ieee80211_driver::irq::{DoorbellAcknowledge, WifiIrqPart};
/// # const LINES: oer_espressif_interrupt_table::Lines =
/// #     oer_espressif_interrupt_table::Lines { first: 16, count: 48, levels: 8, harts: 2, tie_break: oer_espressif_interrupt_table::TieBreak::HighestLine };
/// pub struct Lower;
/// impl DoorbellAcknowledge for Lower {
///     fn acknowledge(&mut self) {}
/// }
/// pub fn on_wifi(_part: &mut WifiIrqPart<Lower>) {
///     // service_wifi_line(&LINE, part)
/// }
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     wifi: WifiToken = WifiIrqPart<Lower> => on_wifi, line 20, context radio, hart 0, Hardware;
/// }
/// fn start(
///     router: &mut oer_espressif_interrupt_table::Router<oer_esp32s31_hal::platform::InterruptController>,
///     token: WifiToken,
///     banks: WifiInterruptBanks,
///     bell: FromCpu3Irq,
/// ) {
///     let _bound = router.bind(token, WifiIrqPart::new(banks, bell, Lower));
/// }
/// # fn main() {}
/// ```
///
/// The part moves into its one binding:
///
/// ```compile_fail,E0382
/// # use oer_esp32s31_ieee80211_driver::irq::{DoorbellAcknowledge, WifiIrqPart};
/// # const LINES: oer_espressif_interrupt_table::Lines =
/// #     oer_espressif_interrupt_table::Lines { first: 16, count: 48, levels: 8, harts: 2, tie_break: oer_espressif_interrupt_table::TieBreak::HighestLine };
/// # pub struct Lower;
/// # impl DoorbellAcknowledge for Lower {
/// #     fn acknowledge(&mut self) {}
/// # }
/// # pub fn on_wifi(_part: &mut WifiIrqPart<Lower>) {}
/// # oer_espressif_interrupt_table::interrupt_table! {
/// #     lines: LINES,
/// #     handler_attributes: [];
/// #     contexts { radio: 3; }
/// #     wifi: WifiToken = WifiIrqPart<Lower> => on_wifi, line 20, context radio, hart 0, Hardware;
/// # }
/// fn start(
///     router: &mut oer_espressif_interrupt_table::Router<oer_esp32s31_hal::platform::InterruptController>,
///     token: WifiToken,
///     part: WifiIrqPart<Lower>,
/// ) {
///     let _bound = router.bind(token, part);
///     let _again = router.bind(token, part);
/// }
/// # fn main() {}
/// ```
pub struct WifiIrqPart<
    A,
    B = FromCpu3Irq,
    I: MacInterrupt = MacInterruptRegisters,
    P: MacPowerInterrupt = MacPowerInterruptRegisters,
> {
    epoch: InterruptEpoch<Moderated, I, P>,
    /// The doorbell's interrupt part: it names a source of the part and
    /// holds no register.
    _bell: B,
    acknowledger: A,
}

impl<A> WifiIrqPart<A> {
    /// The chip's part over the root's banks, closed, and the doorbell's
    /// part and lowering half. No register is touched.
    pub fn new(banks: WifiInterruptBanks, bell: FromCpu3Irq, acknowledger: A) -> Self {
        let WifiInterruptBanks { mac, power } = banks;
        Self {
            epoch: InterruptEpoch::new(mac, power),
            _bell: bell,
            acknowledger,
        }
    }
}

mod sealed {
    pub trait Sealed {}
}

/// A radio handler part of this driver: a [`WifiIrqPart`], whatever lowers
/// its doorbell and whatever its banks are.
pub trait WifiLinePart: sealed::Sealed + InterruptSources {}

impl<A, B, I: MacInterrupt, P: MacPowerInterrupt> sealed::Sealed for WifiIrqPart<A, B, I, P> {}
impl<A, B, I: MacInterrupt, P: MacPowerInterrupt> WifiLinePart for WifiIrqPart<A, B, I, P> where
    Self: InterruptSources
{
}

/// What an activation takes ([`WifiIrqRequests::activate`]): the router's
/// [`Bound`] proof that the image's interrupt table binds a [`WifiLinePart`]
/// (host tests use their own host proof). Sealed: only the router makes one.
pub trait WifiIrqActivation: sealed::Sealed {
    const CONTEXT: oer_espressif_interrupt_table::Context;
}

impl<E: oer_espressif_interrupt_table::Entry> sealed::Sealed for Bound<E> where E::Part: WifiLinePart
{}
impl<E: oer_espressif_interrupt_table::Entry> WifiIrqActivation for Bound<E>
where
    E::Part: WifiLinePart,
{
    const CONTEXT: oer_espressif_interrupt_table::Context = E::CONTEXT;
}

/// The host tests' stand-in for the binding's proof: no table binds a host
/// part, and the driver forbids the `unsafe` a table's entry implements.
#[cfg(test)]
pub(crate) struct HostActivation;

#[cfg(test)]
impl sealed::Sealed for HostActivation {}
#[cfg(test)]
impl WifiIrqActivation for HostActivation {
    const CONTEXT: oer_espressif_interrupt_table::Context =
        oer_espressif_interrupt_table::Context { level: 3, hart: 0 };
}

impl<A> InterruptSources for WifiIrqPart<A> {
    const SOURCES: &'static [u16] = oer_espressif_interrupt_table::interrupt_sources![
        MacInterruptRegisters,
        MacPowerInterruptRegisters,
        FromCpu3Irq
    ];
}

impl<A, B, I: MacInterrupt, P: MacPowerInterrupt> WifiIrqPart<A, B, I, P> {
    /// A part over host banks and a host doorbell, for tests without MMIO.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_host_test(epoch: InterruptEpoch<Moderated, I, P>, bell: B, acknowledger: A) -> Self {
        Self {
            epoch,
            _bell: bell,
            acknowledger,
        }
    }

    /// The banks' epoch, to inspect it in tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn epoch(&self) -> &InterruptEpoch<Moderated, I, P> {
        &self.epoch
    }

    /// The banks' epoch, to latch events in tests as the hardware would.
    #[cfg(any(test, feature = "test-support"))]
    pub fn epoch_mut(&mut self) -> &mut InterruptEpoch<Moderated, I, P> {
        &mut self.epoch
    }
}

/// One request from the radio executor to the interrupt line, besides the
/// activation ([`WifiIrqRequests::activate`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiIrqRequest {
    /// Restore the RX delivery group after the RX frontier was drained.
    UnmaskRx,
    /// Mask every source of both banks; latched events stay latched.
    MaskAll,
    /// Mask both banks and clear every latched event: the epoch closes.
    Close,
}

impl WifiIrqRequest {
    const fn bit(self) -> u32 {
        match self {
            Self::UnmaskRx => UNMASK_RX,
            Self::MaskAll => MASK_ALL,
            Self::Close => CLOSE,
        }
    }
}

const UNMASK_RX: u32 = 1;
const MASK_ALL: u32 = 2;
const ACTIVATE: u32 = 4;
const CLOSE: u32 = 8;

/// The published MAC enable image the line has not read yet.
const ENABLE_UNREAD: u8 = 0;

const fn enable_code(state: MacInterruptEnableState) -> u8 {
    match state {
        MacInterruptEnableState::Disabled => 1,
        MacInterruptEnableState::Station => 2,
        MacInterruptEnableState::Unknown => 3,
    }
}

/// Requests the radio executor leaves for the interrupt line, and the MAC
/// enable image the line last read.
///
/// Its zero bytes are no request and no read image.
#[derive(bytemuck::Zeroable, Default)]
pub struct WifiIrqRequests {
    pending: AtomicU32,
    enable: AtomicU8,
}

impl WifiIrqRequests {
    pub const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
            enable: AtomicU8::new(ENABLE_UNREAD),
        }
    }

    /// Record `request` (Release) and ring `doorbell`. Never waits.
    pub fn ring(&self, request: WifiIrqRequest, doorbell: &impl RadioDoorbell) {
        self.record(request.bit(), doorbell);
    }

    /// Ask the line to activate an epoch: clear both banks, then publish
    /// the station mask. `bound` proves that the image's interrupt table
    /// binds the line's part ([`Bound`] of a [`WifiLinePart`]), so its
    /// handler serves every event the epoch enables. Never waits.
    ///
    /// ```compile_fail,E0061
    /// # use oer_esp32s31_ieee80211_driver::irq::{RadioDoorbell, WifiIrqRequests};
    /// fn start(requests: &WifiIrqRequests, doorbell: &impl RadioDoorbell) {
    ///     // The activation takes the binding's proof.
    ///     requests.activate(doorbell);
    /// }
    /// ```
    ///
    /// ```compile_fail,E0277
    /// # use oer_esp32s31_ieee80211_driver::irq::{RadioDoorbell, WifiIrqRequests};
    /// # use oer_espressif_interrupt_table::Bound;
    /// struct OtherIrq;
    /// impl oer_mmio::InterruptSource for OtherIrq {
    ///     const SOURCE: u16 = 3;
    /// }
    /// fn start<E: oer_espressif_interrupt_table::Entry<Part = OtherIrq>>(requests: &WifiIrqRequests, bound: &Bound<E>, doorbell: &impl RadioDoorbell) {
    ///     // Another part's binding proves nothing about the Wi-Fi banks.
    ///     requests.activate(bound, doorbell);
    /// }
    /// ```
    pub fn activate<A: WifiIrqActivation, D: RadioDoorbell>(&self, bound: &A, doorbell: &D) {
        const {
            assert!(
                oer_espressif_interrupt_table::request_direction(A::CONTEXT, D::EXECUTOR),
                "a request handler must be above its executor on the same hart"
            );
            assert!(
                A::CONTEXT.level == D::HANDLER.level && A::CONTEXT.hart == D::HANDLER.hart,
                "the doorbell must ring the bound handler"
            );
        }
        let _bound = bound;
        self.record(ACTIVATE, doorbell);
    }

    fn record(&self, bits: u32, doorbell: &impl RadioDoorbell) {
        self.pending.fetch_or(bits, Ordering::Release);
        doorbell.ring();
    }

    /// Whether a request waits for the handler: once `false` after a ring,
    /// the handler, which preempts the ringing task, applied it.
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) != 0
    }

    /// The MAC interrupt enable image the line read after the last
    /// requests it applied; `None` before it applied one.
    pub fn mac_interrupt_enable(&self) -> Option<MacInterruptEnableState> {
        match self.enable.load(Ordering::Acquire) {
            1 => Some(MacInterruptEnableState::Disabled),
            2 => Some(MacInterruptEnableState::Station),
            3 => Some(MacInterruptEnableState::Unknown),
            _ => None,
        }
    }

    fn take(&self) -> u32 {
        self.pending.swap(0, Ordering::AcqRel)
    }

    fn clear(&self) {
        self.pending.store(0, Ordering::Release);
    }

    fn publish_enable(&self, state: MacInterruptEnableState) {
        self.enable.store(enable_code(state), Ordering::Release);
    }
}

/// The receive arena's ends of the interrupt line: the MAC and WDEVPWR fact
/// publishers, the request word and the counters. The task installs it once
/// in the image's [`HandlerCell`]; the radio handler lends it per entry.
pub struct WifiIrqLine<X: SignalExecutor> {
    mac: &'static IrqPublisher<'static, X>,
    power: PowerIrqPublisher<'static, X>,
    requests: &'static WifiIrqRequests,
    counters: &'static WifiRxCounters,
}

impl<X: SignalExecutor> WifiIrqLine<X> {
    const fn new(
        mac: &'static IrqPublisher<'static, X>,
        power: PowerIrqPublisher<'static, X>,
        requests: &'static WifiIrqRequests,
        counters: &'static WifiRxCounters,
    ) -> Self {
        Self {
            mac,
            power,
            requests,
            counters,
        }
    }
}

/// The radio handler's body: lower the doorbell, then serve the line over
/// `part`. Without an installed line it closes a stray bank event and
/// returns `Ok(false)`. A fatal MAC assertion returns its original sample after
/// quarantining both banks. The image must retain the owners and record the
/// fault in its never-returning `fatal::reset_recording` path.
#[inline(always)]
pub fn service_wifi_line<A, B, I, P, X>(
    cell: &HandlerCell<WifiIrqLine<X>>,
    part: &mut WifiIrqPart<A, B, I, P>,
) -> Result<bool, Fault>
where
    A: DoorbellAcknowledge,
    I: MacInterrupt,
    P: MacPowerInterrupt,
    X: SignalExecutor,
{
    part.acknowledger.acknowledge();
    if let Some(result) = cell.with(|line| serve(line, part)) {
        result?;
        return Ok(true);
    }
    let _stray = part.epoch.close_stray()?;
    Ok(false)
}

/// One entry over an installed line, one cause: the requests it takes;
/// without one, the MAC bank (sample, moderate RX before the
/// acknowledgement, acknowledge, count, publish and wake after the register
/// borrow ended); with nothing there, the WDEVPWR bank. A closed epoch
/// closes a stray event instead.
#[inline(always)]
fn serve<A, B, I, P, X>(
    line: &mut WifiIrqLine<X>,
    part: &mut WifiIrqPart<A, B, I, P>,
) -> Result<(), Fault>
where
    I: MacInterrupt,
    P: MacPowerInterrupt,
    X: SignalExecutor,
{
    let epoch = &mut part.epoch;
    let pending = line.requests.take();
    if pending != 0 {
        epoch.check_fault()?;
        apply(line, epoch, pending)?;
        line.requests.publish_enable(epoch.mac_interrupt_enable());
        return Ok(());
    }
    if epoch.state() == EpochState::Closed {
        if epoch.close_stray()? {
            line.counters.closed_interrupt();
        }
        return Ok(());
    }
    let entry = line.mac.begin_mac_entry();
    if let Some((disposition, snapshot)) = epoch.service_mac()?
        && disposition != IrqDisposition::Spurious
    {
        line.counters.mac_interrupt(
            false,
            snapshot.had_auxiliary_event,
            snapshot.had_unhandled_event,
        );
        entry
            .finish(line.mac.publish_snapshot_deferred(snapshot))
            .wake();
        return Ok(());
    }
    if let Some((PowerIrqDisposition::Posted, snapshot)) = epoch.service_power() {
        line.counters.power_interrupt();
        line.power.publish(snapshot.observation);
        return Ok(());
    }
    // No request and no enabled event in either bank.
    line.counters.mac_interrupt(true, false, false);
    Ok(())
}

/// Apply the requests taken together: the safest wins.
#[inline(always)]
fn apply<I, P, X>(
    line: &WifiIrqLine<X>,
    epoch: &mut InterruptEpoch<Moderated, I, P>,
    pending: u32,
) -> Result<(), Fault>
where
    I: MacInterrupt,
    P: MacPowerInterrupt,
    X: SignalExecutor,
{
    if pending & CLOSE != 0 {
        epoch.close()?;
    } else if pending & MASK_ALL != 0 {
        if epoch.state() != EpochState::Masked {
            drop(epoch.mask_all());
            line.mac.notify_masked();
        }
    } else if pending & ACTIVATE != 0 {
        epoch.activate(MAC_STATION_INTERRUPT_MASK)?;
    } else if pending & UNMASK_RX != 0 {
        if epoch.unmask_rx_after_drain() {
            line.counters.doorbell_unmask();
        } else {
            line.counters.ignored_unmask();
        }
        return Ok(());
    }
    if pending & UNMASK_RX != 0 {
        line.counters.ignored_unmask();
    }
    Ok(())
}

/// The arena's parts of one interrupt line, before it is installed.
pub struct WifiIrqLineParts<'a, X: SignalExecutor> {
    pub(crate) mac: &'a IrqPublisher<'a, X>,
    pub(crate) power: PowerIrqPublisher<'a, X>,
    pub(crate) requests: &'a WifiIrqRequests,
    pub(crate) counters: &'a WifiRxCounters,
    pub(crate) slot: &'a mut MaybeUninit<WifiIrqLine<X>>,
}

impl<X: SignalExecutor> WifiIrqLineParts<'static, X> {
    /// Put the line in its arena slot and hand it to the handler's `cell`.
    ///
    /// A request left from an earlier line is cleared first: the new line
    /// starts with none. When `cell` already holds a line, the new one is
    /// returned, still owned by the caller and never run by the handler.
    pub fn install(
        self,
        cell: &'static HandlerCell<WifiIrqLine<X>>,
    ) -> Result<Installed<'static, WifiIrqLine<X>>, &'static mut WifiIrqLine<X>> {
        self.requests.clear();
        let line = WifiIrqLine::new(self.mac, self.power, self.requests, self.counters);
        cell.install(self.slot.write(line))
    }
}

#[cfg(test)]
mod tests;
