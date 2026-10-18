#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Executor-independent IEEE 802.15.4 radio runtime for the ESP32-S31.
//!
//! [`IRuntime`] holds the radio role — the ported ESP-IDF MAC
//! engine behind the portable radio contract — together with the MAC owners
//! in one blocking mutex over a caller-chosen raw mutex, as the vendor driver
//! holds its state under `i_enter_critical`. The platform interrupt
//! handler and every command run inside that lock. Portable events leave the
//! lock as owned values through a bounded queue that any executor may await.
//! [`IRuntime::install`] returns the installed radio as the IEEE
//! 802.15.4 port's backend ([`IRuntimeBackend`], the owner shape of
//! `oer_i::owner`), which the composition hands to `run_port`; the
//! port's end masks the MAC route and returns the engine and the hardware.
//! The engine stays behind the runtime's blocking mutex, shared with the
//! MAC interrupt, an interim the owner allowed until .
//!
//! CSMA-CA backoffs and the delays before retries run on the runtime's
//! [`oer_time::Timer`] in the port's joined driver, as ESP-IDF's OpenThread
//! `SubMac` runs them on its tasklet timer; the events consumer only takes
//! events.
//!
//! The bounded event queue keeps a slot free for the next operation's
//! terminal, which the operation permit admits, and for the fault that may
//! end an enabled period: a terminal event is never lost. Only received
//! frames are dropped when the queue is full, reported as [`EventsLost`] in
//! place of the first dropped frame.
//!
//! A radio fault poisons the port: the events consumer hands out the
//! failed operation's terminal and the events before it, then `Poisoned`
//! at every later call.
//!
//! Shared-PHY tracking runs with the MAC receiving, as ESP-IDF's
//! `phy_common.c` does: the runtime never pauses for it, and an operation
//! that loses RF ends with its ordinary terminal event.

#[cfg(test)]
extern crate std;

mod backend;
mod trace;

use core::{
    cell::{Cell, RefCell},
    convert::Infallible,
};

use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    channel::Channel,
    signal::Signal,
};
use oer_espressif_ieee802154_engine::engine::Ieee802154Engine;
pub use oer_espressif_ieee802154_engine::engine::{
    Ieee802154RxAbortStatistics, Ieee802154RxStatistics, Ieee802154TxAbortStatistics,
    Ieee802154TxRxStatistics, Ieee802154TxStatistics,
};
use oer_espressif_ieee802154_engine::ll::Ieee802154LowLevel;
use oer_espressif_ieee802154_engine::pib::Ieee802154PibDefaults;
use oer_espressif_ieee802154_radio::{
    Ieee802154Csl, Ieee802154EnhancedAckGenerator, Ieee802154Radio, Ieee802154RadioSink,
    writes_registers,
};

use oer_ieee802154::{
    AcceptedCommand, AppliedSecurity, CancelError, CommandError, EventsLost, Frame,
    FrameCounterUpdate, Interface, LifecycleEvent, MacKeys, RadioCommand, RadioEvent, RadioFault,
    RadioSetting, RadioState, ReceivedFrame, RequestId, RxMetadata, SettingError, TxStatus,
};
use oer_time::{Duration, Instant, Timer};

pub use backend::{
    Ieee802154PortEvent, Ieee802154RouteDetach, Ieee802154RuntimeBackend, Ieee802154RuntimeCaps,
    NoRoute, RUNTIME_INTERFACES, RuntimeClient, RuntimeClients, RuntimeConfigured, RuntimeControl,
    RuntimeEvents, RuntimeExtendedEntry, RuntimeHeld, RuntimeResources, RuntimeRssi, RuntimeSetup,
    RuntimeShortEntry, RuntimeToken,
};

pub use oer_espressif_ieee802154_radio::{
    IEEE802154_CSL_CAPABILITY, IEEE802154_ENH_ACK_PROBING_CAPACITY,
    IEEE802154_ENHANCED_ACK_IE_CAPACITY, IEEE802154_RADIO_CAPABILITIES, Ieee802154Random,
};

/// A received frame copied out of the receive ring.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ieee802154OwnedFrame {
    /// MAC bytes without PHR or FCS.
    pub frame: Frame,
    /// Receive metadata.
    pub metadata: RxMetadata,
}

impl Ieee802154OwnedFrame {
    fn copy(received: ReceivedFrame<'_>) -> Self {
        Self {
            frame: Frame::try_from_bytes(received.frame.bytes())
                .expect("a portable frame view fits an owned frame"),
            metadata: received.metadata,
        }
    }

    /// Lend the frame as a portable value.
    pub fn received(&self) -> ReceivedFrame<'_> {
        ReceivedFrame {
            frame: self.frame.view(),
            metadata: self.metadata,
        }
    }
}

/// An owned portable [`RadioEvent`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ieee802154RadioEvent {
    /// A frame was received in receive mode.
    Received(Ieee802154OwnedFrame),
    /// Terminal transmit completion.
    TransmitDone {
        /// The request.
        id: RequestId,
        /// Completion category.
        status: TxStatus,
        /// The received acknowledgement.
        acknowledgement: Option<Ieee802154OwnedFrame>,
        /// The security header fields the radio wrote into the frame.
        security: Option<AppliedSecurity>,
    },
    /// Terminal energy scan completion.
    EnergyScanDone {
        /// The request.
        id: RequestId,
        /// Measured channel energy in dBm.
        energy_dbm: i8,
    },
    /// The energy scan was aborted.
    EnergyScanFailed {
        /// The request.
        id: RequestId,
    },
    /// Terminal standalone CCA completion.
    ClearChannelAssessmentDone {
        /// The request.
        id: RequestId,
        /// The channel was idle.
        idle: bool,
    },
    /// The standalone CCA was aborted.
    ClearChannelAssessmentFailed {
        /// The request.
        id: RequestId,
    },
    /// A scheduled receive window ended; the radio sleeps.
    ScheduledReceiveDone {
        /// The request.
        id: RequestId,
    },
    /// The radio disabled itself after a broken event sequence.
    Fault {
        /// The active request.
        id: Option<RequestId>,
        /// Fault category.
        fault: RadioFault,
    },
    /// The terminal event of a port lifecycle command.
    Lifecycle(LifecycleEvent),
}

impl Ieee802154RadioEvent {
    fn copy(event: RadioEvent<'_>) -> Self {
        match event {
            RadioEvent::Received(frame) => Self::Received(Ieee802154OwnedFrame::copy(frame)),
            RadioEvent::TransmitDone {
                id,
                status,
                acknowledgement,
                security,
            } => Self::TransmitDone {
                id,
                status,
                acknowledgement: acknowledgement.map(Ieee802154OwnedFrame::copy),
                security,
            },
            RadioEvent::EnergyScanDone { id, energy_dbm } => {
                Self::EnergyScanDone { id, energy_dbm }
            }
            RadioEvent::EnergyScanFailed { id } => Self::EnergyScanFailed { id },
            RadioEvent::ClearChannelAssessmentDone { id, idle } => {
                Self::ClearChannelAssessmentDone { id, idle }
            }
            RadioEvent::ClearChannelAssessmentFailed { id } => {
                Self::ClearChannelAssessmentFailed { id }
            }
            RadioEvent::ScheduledReceiveDone { id } => Self::ScheduledReceiveDone { id },
            RadioEvent::Fault { id, fault } => Self::Fault { id, fault },
            RadioEvent::Lifecycle(event) => Self::Lifecycle(event),
        }
    }

    /// Lend the event as a portable value.
    pub fn portable(&self) -> RadioEvent<'_> {
        match self {
            Self::Received(frame) => RadioEvent::Received(frame.received()),
            Self::TransmitDone {
                id,
                status,
                acknowledgement,
                security,
            } => RadioEvent::TransmitDone {
                id: *id,
                status: *status,
                acknowledgement: acknowledgement.as_ref().map(Ieee802154OwnedFrame::received),
                security: *security,
            },
            Self::EnergyScanDone { id, energy_dbm } => RadioEvent::EnergyScanDone {
                id: *id,
                energy_dbm: *energy_dbm,
            },
            Self::EnergyScanFailed { id } => RadioEvent::EnergyScanFailed { id: *id },
            Self::ClearChannelAssessmentDone { id, idle } => {
                RadioEvent::ClearChannelAssessmentDone {
                    id: *id,
                    idle: *idle,
                }
            }
            Self::ClearChannelAssessmentFailed { id } => {
                RadioEvent::ClearChannelAssessmentFailed { id: *id }
            }
            Self::ScheduledReceiveDone { id } => RadioEvent::ScheduledReceiveDone { id: *id },
            Self::Fault { id, fault } => RadioEvent::Fault {
                id: *id,
                fault: *fault,
            },
            Self::Lifecycle(event) => RadioEvent::Lifecycle(*event),
        }
    }
}

/// The engine and the hardware it drives: the HAL `IMacOwners`,
/// or a register model in tests.
pub struct Ieee802154RuntimeParts<'storage, H> {
    /// The MAC engine.
    pub engine: Ieee802154Engine<'storage>,
    /// Register access.
    pub hardware: H,
}

struct Installed<'storage, H, R> {
    radio: Ieee802154Radio<'storage, R>,
    hardware: H,
    /// The enhanced-ACK generator while generation is off: it keeps the
    /// header IEs and probing initiators set meanwhile, which go out once
    /// generation is on again.
    parked_ack: Option<Ieee802154EnhancedAckGenerator>,
}

/// The invariant of every port and control call: the handles exist only
/// while their radio is installed.
const INSTALLED: &str = "a port or control exists only while its radio is installed";

/// What the event queue owes the consumer: the slots it reserved for
/// terminal events and a loss not reported yet.
#[derive(Default)]
struct Owed {
    /// A received frame was dropped: the next event reports the loss first.
    lost: bool,
    /// The admitted operation's terminal event.
    operation: bool,
    /// The fault that may end the enabled period, reserved by `enable`;
    /// `disable` gives its slot back.
    fault: bool,
    /// An event taken after the loss it reported.
    held: Option<Ieee802154RadioEvent>,
}

impl Owed {
    fn reserved(&self) -> usize {
        usize::from(self.operation) + usize::from(self.fault)
    }
}

/// One queued event, with whether a loss precedes it.
struct Entry {
    lost_before: bool,
    event: Ieee802154RadioEvent,
}

/// The bounded event queue: terminal events hold the slots reserved for
/// them, and the loss of received frames is reported in its order, in
/// place of the first dropped frame.
struct EventQueue<M: RawMutex, const EVENTS: usize> {
    entries: Channel<M, Entry, EVENTS>,
    owed: Mutex<M, RefCell<Owed>>,
    /// Raised when a loss is pending without a queued entry.
    changed: Signal<M, ()>,
}

impl<M: RawMutex, const EVENTS: usize> EventQueue<M, EVENTS> {
    const fn new() -> Self {
        Self {
            entries: Channel::new(),
            owed: Mutex::new(RefCell::new(Owed {
                lost: false,
                operation: false,
                fault: false,
                held: None,
            })),
            changed: Signal::new(),
        }
    }

    /// Run `entry` on what the queue owes.
    fn owed<O>(&self, entry: impl FnOnce(&mut Owed) -> O) -> O {
        self.owed.lock(|owed| entry(&mut owed.borrow_mut()))
    }

    /// Whether `slots` more terminal events fit beside the queued and
    /// reserved ones.
    fn has_room(&self, owed: &Owed, slots: usize) -> bool {
        self.entries.len() + owed.reserved() + slots <= EVENTS
    }

    /// Queue `event`: a terminal event in the slot reserved for it, a
    /// received frame when a slot is free; `false` when the frame was
    /// dropped.
    fn push(&self, event: Ieee802154RadioEvent) -> bool {
        self.owed(|owed| {
            let ends_operation = match &event {
                Ieee802154RadioEvent::Received(_) => {
                    // A received frame leaves a slot for the next
                    // operation's terminal, which its permit admits
                    // without asking the queue.
                    if !self.has_room(owed, 1 + usize::from(!owed.operation)) {
                        owed.lost = true;
                        return false;
                    }
                    false
                }
                // The command checked its room.
                Ieee802154RadioEvent::Lifecycle(_) => false,
                // The first fault of an enabled period holds its reserved
                // slot, and a fault of the admitted operation the operation's.
                // A further fault, as a disabled radio may still report, is
                // not promised: it takes a free slot or is lost.
                Ieee802154RadioEvent::Fault { .. } => {
                    if owed.fault {
                        owed.fault = false;
                    } else if !owed.operation && !self.has_room(owed, 1) {
                        owed.lost = true;
                        return false;
                    }
                    true
                }
                _ => true,
            };
            if ends_operation {
                owed.operation = false;
            }
            self.send(owed, event);
            true
        })
    }

    /// Queue `event` in a slot known to be free.
    fn send(&self, owed: &mut Owed, event: Ieee802154RadioEvent) {
        let lost_before = core::mem::take(&mut owed.lost);
        let sent = self.entries.try_send(Entry { lost_before, event });
        assert!(sent.is_ok(), "a terminal event holds a reserved slot");
    }

    /// End the stream of a port that left: its undelivered events, a loss
    /// it owed and every reservation go, so the next port starts empty.
    fn clear(&self) {
        self.owed(|owed| {
            while self.entries.try_receive().is_ok() {}
            *owed = Owed::default();
        });
    }

    fn take(&self) -> Option<Result<Ieee802154RadioEvent, EventsLost>> {
        self.owed(|owed| {
            if let Some(event) = owed.held.take() {
                return Some(Ok(event));
            }
            match self.entries.try_receive() {
                Ok(Entry {
                    lost_before: true,
                    event,
                }) => {
                    owed.held = Some(event);
                    Some(Err(EventsLost))
                }
                Ok(Entry { event, .. }) => Some(Ok(event)),
                Err(_) if core::mem::take(&mut owed.lost) => Some(Err(EventsLost)),
                Err(_) => None,
            }
        })
    }
}

/// The sink of one locked entry: events go to the queue.
struct QueueSink<'a, M: RawMutex, const EVENTS: usize> {
    events: &'a EventQueue<M, EVENTS>,
}

impl<M: RawMutex, const EVENTS: usize> Ieee802154RadioSink for QueueSink<'_, M, EVENTS> {
    fn event(&mut self, event: RadioEvent<'_>) {
        if !self.events.push(Ieee802154RadioEvent::copy(event))
            && let RadioEvent::Received(frame) = event
        {
            trace::emit(|| oer_ieee802154_trace::RxOutcome {
                // The PHR length counts the two FCS bytes the portable
                // frame leaves out.
                length: frame.frame.bytes().len() as u8 + 2,
                result: oer_ieee802154_trace::RxResult::Dropped(
                    oer_ieee802154_trace::RxDrop::QueueFull,
                ),
            });
        }
    }
}

/// The radio role, its hardware and the event queue.
///
/// `EVENTS` bounds the events waiting for the consumer, the reserved
/// terminal slots included. At least three slots let one received frame
/// fit beside the enabled period's fault reserve and the current or next
/// operation's terminal. A full queue drops the newest received frame and
/// reports [`EventsLost`] once in its place. `T` is the image's monotonic
/// time the delays wait on.
// CAPABILITY: i-mac-operation-subset
pub struct Ieee802154Runtime<'storage, M: RawMutex, H, T, R, const EVENTS: usize> {
    timer: T,
    installed: Mutex<M, RefCell<Option<Installed<'storage, H, R>>>>,
    events: EventQueue<M, EVENTS>,
    /// When the running backoff or retry delay ends.
    backoff_until: Mutex<M, Cell<Option<Instant>>>,
    /// Raised when a delay starts, so an awaiting consumer rearms.
    backoff_started: Signal<M, ()>,
    /// The next request identifier the runtime mints.
    next_id: Mutex<M, Cell<u32>>,
    /// The radio fault that poisoned the port.
    fault: Mutex<M, Cell<Option<RadioFault>>>,
}

impl<
    'storage,
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer + Default,
    R: Ieee802154Random,
    const EVENTS: usize,
> Default for Ieee802154Runtime<'storage, M, H, T, R, EVENTS>
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<
    'storage,
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
    const EVENTS: usize,
> Ieee802154Runtime<'storage, M, H, T, R, EVENTS>
{
    /// An empty runtime whose delays wait on `timer`, suitable for a
    /// `static`.
    pub const fn new(timer: T) -> Self {
        assert!(
            EVENTS >= 3,
            "the queue holds a received frame beside an operation's end and a fault"
        );
        Self {
            timer,
            installed: Mutex::new(RefCell::new(None)),
            events: EventQueue::new(),
            backoff_until: Mutex::new(Cell::new(None)),
            backoff_started: Signal::new(),
            next_id: Mutex::new(Cell::new(1)),
            fault: Mutex::new(Cell::new(None)),
        }
    }

    /// The image's monotonic clock the runtime reads: its radio clock
    /// ([`ClockInfo::MONOTONIC_MICROS`](oer_i::ClockInfo::MONOTONIC_MICROS)), for a synchronous reader such as
    /// OpenThread's `otPlatRadioGetNow`.
    pub const fn clock(&self) -> &T {
        &self.timer
    }

    /// Start the timer of a transmission's backoff or retry delay, if one is
    /// due.
    fn start_backoff(&self, backoff_micros: Option<u32>) {
        if let Some(micros) = backoff_micros {
            // A delay past the timer's range never ends.
            let until = self
                .timer
                .deadline_after(Duration::from_micros(u64::from(micros)))
                .unwrap_or(Instant::from_micros(u64::MAX));
            self.backoff_until.lock(|backoff| backoff.set(Some(until)));
            self.backoff_started.signal(());
        }
    }

    /// `esp_i_enable` after clocks, PHY and the interrupt owner are
    /// active: enable the engine, run its MAC initialization and install the
    /// radio role in the `Disabled` state. The platform CPU route may be
    /// enabled afterwards; enabling the port through its owner opens
    /// admission and reserves the enabled period's fault slot.
    ///
    /// Returns the installed radio as the port's backend, with the
    /// capabilities marker `K` its composition names; the composition hands
    /// it to `run_port`, whose end returns the engine and the hardware.
    ///
    /// # Errors
    ///
    /// Returns the parts unchanged if a radio is already installed.
    #[allow(
        clippy::result_large_err,
        clippy::type_complexity,
        reason = "the no-alloc runtime returns the unconsumed owners by value"
    )]
    pub fn install<K>(
        &self,
        mut parts: Ieee802154RuntimeParts<'storage, H>,
        random: R,
        defaults: Ieee802154PibDefaults,
    ) -> Result<
        Ieee802154RuntimeBackend<'_, 'storage, K, M, H, T, R, EVENTS>,
        Ieee802154RuntimeParts<'storage, H>,
    > {
        #[allow(
            clippy::result_large_err,
            reason = "the no-alloc runtime returns the unconsumed owners by value"
        )]
        let install = |installed: &RefCell<Option<Installed<'storage, H, R>>>| {
            let mut installed = installed.borrow_mut();
            if installed.is_some() {
                return Err(parts);
            }
            parts.engine.enable();
            parts.engine.mac_init(&mut parts.hardware, defaults);
            *installed = Some(Installed {
                radio: Ieee802154Radio::new(parts.engine, random),
                hardware: parts.hardware,
                parked_ack: None,
            });
            Ok(())
        };
        self.installed.lock(install)?;
        self.fault.lock(|fault| fault.set(None));
        Ok(Ieee802154RuntimeBackend::new(self))
    }

    /// Enable the installed radio: it enters sleep, and the queue reserves
    /// the slot of the fault that may end the enabled period, so a queue
    /// full of received frames never loses it.
    fn enable_locked(&self) -> Result<(), ()> {
        self.with_installed(|radio, hardware, sink| {
            radio.enable(hardware, &self.timer, sink).map_err(|_| ())
        })?;
        self.events.owed(|owed| owed.fault = true);
        Ok(())
    }

    /// Disable the installed radio, ending the operation in flight with its
    /// terminal; a disabled radio stays as it is.
    fn disable_locked(&self) {
        self.with_installed(|radio, hardware, sink| {
            if radio.state() != RadioState::Disabled {
                let _ = radio.disable(hardware, &self.timer, sink);
            }
        });
        self.events.owed(|owed| {
            owed.operation = false;
            owed.fault = false;
        });
    }

    /// Collect the vendor's TX/RX statistics
    /// (`CONFIG_IEEE802154_TXRX_STATISTIC`), from zero, or stop collecting
    /// them.
    pub fn set_txrx_statistics(&self, collect: bool) {
        self.with_installed(|radio, _, _| radio.engine().set_txrx_statistics(collect));
    }

    /// The TX/RX statistics while collected.
    pub fn txrx_statistics(&self) -> Option<Ieee802154TxRxStatistics> {
        self.with_installed(|radio, _, _| radio.engine().txrx_statistics())
    }

    /// `esp_i_txrx_statistic_clear`.
    pub fn clear_txrx_statistics(&self) {
        self.with_installed(|radio, _, _| radio.engine().clear_txrx_statistics());
    }

    /// Replace the engine's software-coexistence priorities.
    pub fn set_coexistence(
        &self,
        coexistence: oer_espressif_ieee802154_engine::coex::Ieee802154Coexistence,
    ) {
        self.with_installed(|radio, _, _| radio.engine().set_coexistence(coexistence));
    }

    /// Run `entry` on the radio's hardware under the runtime's lock, for a
    /// diagnostic read such as the MAC power sequence; `None` while no radio
    /// is installed.
    pub fn with_hardware<O>(&self, entry: impl FnOnce(&mut H) -> O) -> Option<O> {
        self.with_held(|_, hardware, _| entry(hardware))
    }

    /// `esp_i_disable` after the platform CPU route is disabled:
    /// stop the operation in flight, return the MAC's coexistence PTIs to
    /// the disabled foundation image, and disable and return the engine and
    /// hardware. The port's stream ends here: its undelivered events and
    /// the terminals it owed are discarded, so the next port starts empty.
    fn uninstall(&self) -> Ieee802154RuntimeParts<'storage, H> {
        let parts = self.installed.lock(|installed| {
            let Installed {
                mut radio,
                mut hardware,
                ..
            } = installed.borrow_mut().take().expect(INSTALLED);
            let mut sink = QueueSink {
                events: &self.events,
            };
            // A disabled radio has nothing in flight.
            let _ = radio.disable(&mut hardware, &self.timer, &mut sink);
            let mut engine = radio.into_engine();
            engine.mac_deinit(&mut hardware);
            engine.disable();
            Ieee802154RuntimeParts { engine, hardware }
        });
        self.events.clear();
        self.backoff_until.lock(|backoff| backoff.set(None));
        parts
    }

    /// Apply one portable setting to the installed radio.
    fn apply_setting(&self, setting: RadioSetting<'_>) -> Result<(), SettingError> {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed.as_mut().expect(INSTALLED);
            apply_setting(&mut installed.radio, &mut installed.parked_ack, setting)
        })
    }

    /// Run `entry` on the installed radio and its hardware.
    fn with_installed<O>(
        &self,
        entry: impl FnOnce(
            &mut Ieee802154Radio<'storage, R>,
            &mut H,
            &mut QueueSink<'_, M, EVENTS>,
        ) -> O,
    ) -> O {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed.as_mut().expect(INSTALLED);
            let mut sink = QueueSink {
                events: &self.events,
            };
            entry(&mut installed.radio, &mut installed.hardware, &mut sink)
        })
    }

    /// Run `entry` on the radio and its hardware while a radio is
    /// installed: the interrupt entry and the runner, which outlive the
    /// installed radio.
    fn with_held<O>(
        &self,
        entry: impl FnOnce(
            &mut Ieee802154Radio<'storage, R>,
            &mut H,
            &mut QueueSink<'_, M, EVENTS>,
        ) -> O,
    ) -> Option<O> {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed.as_mut()?;
            let mut sink = QueueSink {
                events: &self.events,
            };
            Some(entry(
                &mut installed.radio,
                &mut installed.hardware,
                &mut sink,
            ))
        })
    }

    /// The platform interrupt handler (`i_isr`). Does nothing while
    /// no radio is installed.
    pub fn on_interrupt(&self) {
        if let Some(backoff) = self.with_held(|radio, port, sink| {
            radio.isr(port, &self.timer, sink);
            radio.take_delay()
        }) {
            self.start_backoff(backoff);
        }
    }

    /// Admit and start one portable command. An operation reserves its
    /// terminal event's slot first and is refused while no slot is free.
    fn submit_locked(&self, command: RadioCommand<'_>) -> Result<AcceptedCommand, CommandError> {
        let operation = matches!(
            command,
            RadioCommand::Transmit(_)
                | RadioCommand::EnergyScan(_)
                | RadioCommand::ClearChannelAssessment { .. }
                | RadioCommand::ScheduledReceive(_)
        );
        let submitted = self.with_installed(|radio, hardware, sink| {
            // A configuration of the PIB or the pending table needs no
            // hardware.
            if !writes_registers(&command) {
                return Ok((radio.submit_software_configuration(command), None));
            }
            // A second operation is the state machine's to refuse.
            let reserved = operation
                && self.events.owed(|owed| {
                    if owed.operation {
                        return Ok(false);
                    }
                    if !self.events.has_room(owed, 1) {
                        return Err(CommandError::EventQueueFull);
                    }
                    owed.operation = true;
                    Ok(true)
                })?;
            let accepted = radio.submit(hardware, &self.timer, command, sink);
            if accepted.is_err() && reserved {
                self.events.owed(|owed| owed.operation = false);
            }
            Ok((accepted, radio.take_delay()))
        });
        let (accepted, backoff) = submitted?;
        self.start_backoff(backoff);
        accepted
    }

    /// End the running operation `target` through its terminal event.
    fn cancel_locked(&self, target: RequestId) -> Result<(), CancelError> {
        let (cancelled, backoff) = self.with_installed(|radio, hardware, sink| {
            let cancelled = radio.cancel(hardware, &self.timer, target, sink);
            (cancelled, radio.take_delay())
        });
        self.start_backoff(backoff);
        cancelled
    }

    /// The radio's runner: a transmission's CSMA-CA backoff or retry delay,
    /// after which the transmission goes on.
    ///
    /// Poll it for as long as the runtime is used, beside the consumer of
    /// the port's events; it never ends. Dropping it keeps the delay for the
    /// next runner.
    pub async fn run(&self) -> Infallible {
        loop {
            let until = self.backoff_until.lock(Cell::get);
            let backoff = async {
                match until {
                    Some(until) => self.timer.wait_until(until).await,
                    None => core::future::pending().await,
                }
            };
            match select(backoff, self.backoff_started.wait()).await {
                Either::First(()) => {
                    self.backoff_until.lock(|backoff| backoff.set(None));
                    if let Some(backoff) = self.with_held(|radio, port, sink| {
                        radio.delay_elapsed(port, &self.timer, sink);
                        radio.take_delay()
                    }) {
                        self.start_backoff(backoff);
                    }
                }
                // A backoff started; wait for its end instead.
                Either::Second(()) => {}
            }
        }
    }
}

/// Apply one portable setting to the radio under the runtime's lock. Every
/// setting changes software state the interrupt context or the next
/// operation reads. While enhanced-ACK generation is off, `parked` keeps
/// the generator with its header IEs and probing initiators.
fn apply_setting<R: Ieee802154Random>(
    radio: &mut Ieee802154Radio<'_, R>,
    parked: &mut Option<Ieee802154EnhancedAckGenerator>,
    setting: RadioSetting<'_>,
) -> Result<(), SettingError> {
    /// The generator a setting changes: the running one, else the parked
    /// one, made on first use.
    fn generator<'g, R: Ieee802154Random>(
        radio: &'g mut Ieee802154Radio<'_, R>,
        parked: &'g mut Option<Ieee802154EnhancedAckGenerator>,
    ) -> &'g mut Ieee802154EnhancedAckGenerator {
        match radio.enhanced_ack() {
            Some(running) => running,
            None => parked.get_or_insert_with(Ieee802154EnhancedAckGenerator::new),
        }
    }
    fn keys<'r, R: Ieee802154Random>(
        radio: &'r mut Ieee802154Radio<'_, R>,
        interface: Interface,
    ) -> Result<&'r mut Option<MacKeys>, SettingError> {
        let interfaces = radio.interfaces();
        radio
            .interface_mac_keys(interface)
            .ok_or(SettingError::UnknownInterface {
                interface,
                interfaces,
            })
    }
    match setting {
        RadioSetting::MacKeys {
            interface,
            key_id,
            previous,
            current,
            next,
        } => keys(radio, interface)?
            .get_or_insert(MacKeys::ZEROED)
            .set_keys(key_id, previous, current, next),
        RadioSetting::FrameCounter { interface, update } => {
            let keys = keys(radio, interface)?.get_or_insert(MacKeys::ZEROED);
            match update {
                FrameCounterUpdate::Set(counter) => keys.set_frame_counter(counter),
                FrameCounterUpdate::SetIfLarger(counter) => {
                    keys.set_frame_counter_if_larger(counter);
                }
            }
        }
        RadioSetting::RemoveMacKeys { interface } => *keys(radio, interface)? = None,
        RadioSetting::Csl(csl) => {
            *radio.csl() = Ieee802154Csl {
                period: csl.period,
                sample_time: csl.sample_time,
            };
        }
        RadioSetting::EnhancedAck(Some(generation)) => {
            let mut running = radio
                .enhanced_ack()
                .take()
                .or_else(|| parked.take())
                .unwrap_or_else(Ieee802154EnhancedAckGenerator::new);
            running
                .probing()
                .set_noise_floor(generation.noise_floor_dbm);
            *radio.enhanced_ack() = Some(running);
        }
        RadioSetting::EnhancedAck(None) => {
            if let Some(running) = radio.enhanced_ack().take() {
                *parked = Some(running);
            }
        }
        RadioSetting::EnhancedAckHeaderIes(ies) => generator(radio, parked)
            .set_header_ies(ies)
            .map_err(|_| SettingError::HeaderIesTooLong {
                capacity: IEEE802154_ENHANCED_ACK_IE_CAPACITY,
            })?,
        RadioSetting::EnhancedAckProbing(initiators) => {
            generator(radio, parked).probing().replace(initiators);
        }
        // The PIB settings the next operation publishes.
        RadioSetting::TransmitPowerDbm(power) => radio
            .set_transmit_power(None, power)
            .map_err(SettingError::TransmitPower)?,
        RadioSetting::ChannelTransmitPowerDbm { channel, power_dbm } => radio
            .set_transmit_power(Some(channel), power_dbm)
            .map_err(SettingError::TransmitPower)?,
        RadioSetting::CcaThresholdDbm(threshold) => radio
            .set_cca_threshold(threshold)
            .map_err(SettingError::CcaThreshold)?,
        RadioSetting::CcaMode(mode) => radio.set_cca_mode(mode),
    }
    Ok(())
}

/// Host models of the hardware for tests of this runtime and the crates
/// that drive it; host targets only, where no MAC writes received frames.
#[cfg(all(any(test, feature = "model"), not(target_arch = "riscv32")))]
impl<M: RawMutex, T: Timer, R: Ieee802154Random, const EVENTS: usize>
    Ieee802154Runtime<
        '_,
        M,
        oer_espressif_ieee802154_engine::ll::model::Ieee802154LlModel,
        T,
        R,
        EVENTS,
    >
{
    /// Model the MAC DMA writing `frame` into the published receive buffer
    /// and raising `events`, then run the interrupt handler.
    ///
    /// # Panics
    ///
    /// No radio is installed, or a frame arrives with no receive buffer
    /// published.
    pub fn model_interrupt(
        &self,
        frame: Option<&[u8]>,
        events: &[oer_espressif_ieee802154_engine::types::Ieee802154Event],
    ) {
        self.installed.lock(|installed| {
            let mut installed = installed.borrow_mut();
            let installed = installed.as_mut().expect("a radio is installed");
            let hardware = &mut installed.hardware;
            if let Some(frame) = frame {
                let address = hardware.rx_address.expect("a receive buffer is published");
                assert!(installed.radio.engine().model_dma_write(address, frame));
            }
            hardware.raise(events);
        });
        self.on_interrupt();
    }

    /// Inspect or set the modelled hardware.
    ///
    /// # Panics
    ///
    /// No radio is installed.
    pub fn with_model<O>(
        &self,
        entry: impl FnOnce(&mut oer_espressif_ieee802154_engine::ll::model::Ieee802154LlModel) -> O,
    ) -> O {
        self.with_held(|_, hardware, _| entry(hardware))
            .expect("a radio is installed")
    }
}

#[cfg(test)]
mod tests;
