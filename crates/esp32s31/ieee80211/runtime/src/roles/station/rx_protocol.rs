//! Ownership handoff between the PAC/DMA RX actor and protocol processing.
//!
//! The producer queue stores unique staging-pool leases, never DMA pointers.
//! The standalone STA DATAPATH owner services the queue and physical DMA as
//! one bounded producer/consumer turn before returning to TX/control
//! arbitration.

#[cfg(feature = "task-poll-telemetry")]
use oer_esp32s31_ieee80211_sta::connected_rx::ConnectedRxDataCycleProfile;
use oer_ieee80211_mac::sequence::SequenceNumber;

use core::future::{Future, pending, ready};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_time::{Duration, Instant, Timer};

use oer_esp32s31_ieee80211_mac::rx::{
    RxPhyInfo,
    ampdu::{
        RX_BLOCK_ACK_BANK_COUNT, RxAmpduError, RxAmpduMpdu, RxAmpduRelease, RxBlockAckReorderBanks,
    },
    pool::{VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
};

use oer_esp32s31_ieee80211_sta::connected_rx::{
    ConnectedRxConfig, ConnectedRxDispatch, ConnectedRxDispatcher, ConnectedRxEvent,
    ConnectedRxSink, StaCcmpRxReplayError,
};

use oer_ieee80211_mac::data::EthernetFrameParts;

use oer_ieee80211_softmac::MacRxMetadata;

#[cfg(test)]
use crate::datapath::rx::staging::StagedRxQueue;
#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::rx_pipeline::{
    RxPipelineObservation, RxPipelineObserver, RxReorderAgreementObservation,
    RxReorderAgreementObserver,
};

use crate::datapath::{
    irq::EmbassyMacIrqRuntime,
    rx::{
        ethernet::PackedEthernetWriter,
        reorder::{
            RX_REORDER_BACKING_SLOT_COUNT, RX_REORDER_CURRENT_SLOT, RX_REORDER_SLOT_DOMAIN,
            RxReorderCommand, RxReorderCommandReceiver, RxReorderFrame, RxReorderFrameStorage,
            try_receive_rx_reorder_command,
        },
        staging::{
            StagedEthernetPublication, StagedRxDisposition, StagedRxFrame, StagedRxReceiver,
        },
    },
};

/// Ownership released when a connected staged-RX epoch is stopped.
///
/// The counts are diagnostic evidence for an outer station lifecycle. A
/// nonzero value is legal: disconnect may race with already staged input, but
/// every counted frame or command has been discarded before this value is
/// returned.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedRxProtocolShutdown {
    pub queued_frames: usize,
    pub retained_frames: usize,
    pub reorder_commands: usize,
    pub active_reorders: usize,
    /// ESP-NOW peer fingerprints explicitly cleared before owner return.
    pub esp_now_duplicate_entries: usize,
    /// Incomplete Open MSDUs revoked before the dispatcher leaves its
    /// connected association epoch.
    pub incomplete_fragment_contexts: usize,
}

/// Result of one bounded, non-waiting protocol service turn.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedRxProtocolTurn {
    /// Staging leases removed from the physical-to-protocol queue.
    pub consumed_frames: usize,
    /// Commands, deadlines or frames remain ready for the next outer turn.
    pub work_remaining: bool,
    /// Frames handled by the synchronous in-order fast path (diagnostic).
    pub direct_frames: crate::diagnostics::core0_rx_performance::Core0PathCount,
    /// Frames which retained the general asynchronous dispatch path
    /// (diagnostic).
    pub asynchronous_frames: crate::diagnostics::core0_rx_performance::Core0PathCount,
}

/// Admission contract for an ordinary frame which retains its staging slot.
///
/// `Immediate` is a capability, not a transient queue observation: the sink
/// guarantees that publishing this staging owner cannot require another
/// capacity slot. This lets the protocol owner omit an otherwise immediately
/// ready async edge without turning a racy `is_full` check into correctness.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StagedRxAdmission {
    /// Publication may need an independently owned output slot.
    #[default]
    AwaitCapacity,
    /// The staging slot itself is the complete output ownership credit.
    Immediate,
}

/// Scratch and runtime-arena ownership returned only after a staged RX
/// protocol epoch stops.
///
/// The runtime arena is part of the stopped value deliberately. Losing it
/// would make a second connected epoch reinitialize one-shot static storage
/// instead of reusing the quiesced protocol state.
pub struct ConnectedRxProtocolStopped<'scratch, R> {
    shutdown: ConnectedRxProtocolShutdown,
    mpdu: &'scratch mut [u8],
    ethernet: &'scratch mut [u8],
    runtime: R,
}

impl<'scratch, R> ConnectedRxProtocolStopped<'scratch, R> {
    pub const fn shutdown(&self) -> ConnectedRxProtocolShutdown {
        self.shutdown
    }

    pub fn into_parts(self) -> (&'scratch mut [u8], &'scratch mut [u8], R) {
        (self.mpdu, self.ethernet, self.runtime)
    }
}

/// Concrete stopped owner returned by [`ConnectedReceiveProtocol`].
///
/// Composition roots should name this alias rather than restating the
/// internal runtime-arena reference type in executor control resources.
pub type ConnectedProtocolStopped<
    'scratch,
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> = ConnectedRxProtocolStopped<
    'scratch,
    &'pool mut ConnectedReceiveStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
>;

/// Async admission edge required by the staged protocol consumer.
///
/// The synchronous [`ConnectedRxSink`] callback remains useful for finite
/// parsing and control observers. This companion edge lets a network adapter
/// retain the staged frame until its bounded output queue has ownership.
pub trait ConnectedRxProtocolSink<
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
>: ConnectedRxSink
{
    /// Capacity edge for a frame that must be copied into adapter-owned
    /// storage, including A-MSDU subframes and reorder slow paths.
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_;

    /// Return the stable ordinary-frame admission capability of this sink.
    fn staged_rx_admission(&self) -> StagedRxAdmission {
        StagedRxAdmission::AwaitCapacity
    }

    /// Capacity edge for an ordinary frame still owning its staging slot.
    /// In-place sinks need no second slot and therefore return immediately.
    fn wait_staged_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.wait_ready()
    }

    /// Publish an ordinary Ethernet frame from its unique staging owner.
    /// The default preserves the copying behavior for test/control sinks.
    fn publish_staged(
        &mut self,
        frame: StagedRxFrame<'_, CAPACITY, SLOTS>,
        ethernet: StagedEthernetPublication,
    ) -> StagedRxDisposition {
        {
            let raw = frame.segment().buffer;
            let payload_end = ethernet
                .payload_offset
                .checked_add(ethernet.payload_length)
                .expect("captured Ethernet payload range cannot overflow");
            let payload = raw
                .get(ethernet.payload_offset..payload_end)
                .expect("captured Ethernet payload belongs to its staged frame");
            self.publish(ConnectedRxEvent::Ethernet {
                frame: EthernetFrameParts {
                    destination: ethernet.destination,
                    source: ethernet.source,
                    ether_type: ethernet.ether_type,
                    payload,
                },
                raw,
                amsdu: false,
                metadata: ethernet.metadata,
            });
        }
        drop(frame);
        StagedRxDisposition::Released
    }
}

/// Adapter for sinks whose `publish` operation cannot experience ownership
/// backpressure, such as fixed test observers.
pub struct AlwaysReadyConnectedRxSink<S>(pub S);

impl<S: ConnectedRxSink> ConnectedRxSink for AlwaysReadyConnectedRxSink<S> {
    fn publish(&mut self, event: oer_esp32s31_ieee80211_sta::connected_rx::ConnectedRxEvent<'_>) {
        self.0.publish(event);
    }

    fn wants_power_save_data(&self) -> bool {
        self.0.wants_power_save_data()
    }

    fn supports_esp_now_v2(&self) -> bool {
        self.0.supports_esp_now_v2()
    }

    fn publish_esp_now_v2(
        &mut self,
        received: oer_ieee80211_softmac::EspNowReceivedV2<'_>,
        metadata: oer_ieee80211_softmac::MacRxMetadata<oer_esp32s31_ieee80211_mac::rx::RxPhyInfo>,
    ) {
        self.0.publish_esp_now_v2(received, metadata);
    }
}

impl<S: ConnectedRxSink, const CAPACITY: usize, const SLOTS: usize>
    ConnectedRxProtocolSink<CAPACITY, SLOTS> for AlwaysReadyConnectedRxSink<S>
{
    fn staged_rx_admission(&self) -> StagedRxAdmission {
        StagedRxAdmission::Immediate
    }

    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        ready(())
    }
}

/// Scratch-backed A-MSDU publication plan.
///
/// Each record is a big-endian `u16` Ethernet length followed by the complete
/// frame. Decapsulation removes the eight-byte LLC/SNAP header from every
/// subframe, so the two-byte record prefix still leaves the packed output
/// strictly smaller than its valid A-MSDU input.
struct DeferredEthernetFrames<'storage> {
    frames: PackedEthernetWriter<'storage>,
    metadata: Option<MacRxMetadata<RxPhyInfo>>,
    power_save_data: Option<oer_esp32s31_ieee80211_sta::connected_rx::PowerSaveData>,
    wants_power_save_data: bool,
}

impl<'storage> DeferredEthernetFrames<'storage> {
    fn new(storage: &'storage mut [u8], wants_power_save_data: bool) -> Self {
        Self {
            frames: PackedEthernetWriter::new(storage),
            metadata: None,
            power_save_data: None,
            wants_power_save_data,
        }
    }

    const fn used(&self) -> usize {
        self.frames.used()
    }
}

impl ConnectedRxSink for DeferredEthernetFrames<'_> {
    fn wants_power_save_data(&self) -> bool {
        self.wants_power_save_data
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::PowerSaveData(data) = event {
            self.power_save_data = Some(data);
            return;
        }
        let ConnectedRxEvent::Ethernet {
            frame, metadata, ..
        } = event
        else {
            return;
        };
        if let Some(previous) = self.metadata {
            debug_assert_eq!(previous, metadata);
        } else {
            self.metadata = Some(metadata);
        }
        self.frames
            .push(frame)
            .expect("A-MSDU output fits the constructor-qualified scratch buffer");
    }
}

enum RetainedRxFrame<'pool, const CAPACITY: usize, const SLOTS: usize, const REORDER_SLOTS: usize> {
    Hot(StagedRxFrame<'pool, CAPACITY, SLOTS>),
    Cold(RxReorderFrame<'pool, CAPACITY, REORDER_SLOTS>),
}

/// Caller-owned state arena for one connected RX protocol instance.
///
/// This state is deliberately separate from [`ConnectedReceiveProtocol`]:
/// the dispatcher, retained lease table and eight BlockAck reorder machines
/// are large, long-lived data, while the protocol value is moved through
/// composition and async task boundaries. Embedded composition roots should
/// place this arena in static storage and pass a unique mutable borrow to each
/// connected epoch.
pub struct ConnectedReceiveStorage<
    'pool,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    dispatcher: ConnectedRxDispatcher,
    dispatcher_configured: bool,
    reorder_banks: RxBlockAckReorderBanks<RX_REORDER_SLOT_DOMAIN>,
    reorder_first_starts: [Option<SequenceNumber>; RX_BLOCK_ACK_BANK_COUNT],
    gap_deadlines: [Option<Instant>; RX_BLOCK_ACK_BANK_COUNT],
    retained: [Option<RetainedRxFrame<'pool, CAPACITY, SLOTS, REORDER_SLOTS>>; REORDER_SLOTS],
}

impl<'pool, const CAPACITY: usize, const SLOTS: usize, const REORDER_SLOTS: usize>
    ConnectedReceiveStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>
{
    pub const fn new() -> Self {
        Self {
            dispatcher: ConnectedRxDispatcher::unconfigured(),
            dispatcher_configured: false,
            reorder_banks: RxBlockAckReorderBanks::new(),
            reorder_first_starts: [None; RX_BLOCK_ACK_BANK_COUNT],
            gap_deadlines: [None; RX_BLOCK_ACK_BANK_COUNT],
            retained: [const { None }; REORDER_SLOTS],
        }
    }

    /// Revoke the parked association before installing a new connected epoch.
    ///
    /// The dispatcher stays inside this arena, so successful reconfiguration
    /// changes protocol identity without moving its multi-kilobyte state
    /// through a task frame. A live replay publication rejects the edge and
    /// leaves the previous identity fail-closed.
    pub fn try_reconfigure_dispatcher(
        &mut self,
        config: ConnectedRxConfig,
    ) -> Result<(), StaCcmpRxReplayError> {
        assert!(
            (0..RX_BLOCK_ACK_BANK_COUNT).all(|bank| self.reorder_banks.identity(bank).is_none()),
            "connected RX dispatcher reconfiguration requires stopped reorder banks"
        );
        assert!(
            self.reorder_first_starts.iter().all(Option::is_none),
            "connected RX dispatcher reconfiguration requires no pending reorder start"
        );
        assert!(
            self.gap_deadlines.iter().all(Option::is_none),
            "connected RX dispatcher reconfiguration requires no reorder deadline"
        );
        assert!(
            self.retained.iter().all(Option::is_none),
            "connected RX dispatcher reconfiguration requires no retained frame"
        );
        self.dispatcher_configured = false;
        self.dispatcher.try_reconfigure(config)?;
        self.dispatcher_configured = true;
        Ok(())
    }

    pub const fn dispatcher(&self) -> &ConnectedRxDispatcher {
        assert!(
            self.dispatcher_configured,
            "connected RX dispatcher must be configured for this epoch"
        );
        &self.dispatcher
    }

    pub(crate) fn dispatcher_mut(&mut self) -> &mut ConnectedRxDispatcher {
        assert!(
            self.dispatcher_configured,
            "connected RX dispatcher must be configured for this epoch"
        );
        &mut self.dispatcher
    }

    pub const fn dispatcher_configured(&self) -> bool {
        self.dispatcher_configured
    }

    fn mark_dispatcher_stopped(&mut self) {
        self.dispatcher_configured = false;
    }
}

impl<'pool, const CAPACITY: usize, const SLOTS: usize, const REORDER_SLOTS: usize> Default
    for ConnectedReceiveStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Protocol-side consumer of staged RX ownership.
///
/// `dispatch_next` contains no PAC access. Dropping the frame after dispatch
/// returns its staging credit and wakes a radio actor that previously stopped
/// at natural ownership backpressure.
pub struct ConnectedReceiveProtocol<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    K,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    frames: StagedRxReceiver<'queue, 'pool, M, DEPTH, CAPACITY, SLOTS>,
    processor: ConnectedRxProcessor<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        K,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >,
}

/// Queue-independent connected-station RX protocol processor.
///
/// It owns parsing, reorder state, output scratch and the station sink, but no
/// source queue. Standalone STA scheduling wraps it in
/// [`ConnectedReceiveProtocol`]; same-channel STA+AP scheduling can feed an
/// already routed staging lease directly without manufacturing a second DMA
/// producer or compatibility queue.
pub struct ConnectedRxProcessor<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    K,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const REORDER_SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    irq: &'irq EmbassyMacIrqRuntime<M>,
    /// Monotonic clock of the reorder gap deadlines.
    clock: K,
    sink: S,
    mpdu: &'scratch mut [u8],
    ethernet: &'scratch mut [u8],
    #[cfg(any(feature = "diagnostics", test))]
    pipeline_observer: Option<&'queue dyn RxPipelineObserver>,
    #[cfg(any(feature = "diagnostics", test))]
    reorder_observer: Option<&'queue dyn RxReorderAgreementObserver>,
    reorder_commands: Option<RxReorderCommandReceiver<'queue, M>>,
    reorder_storage: Option<&'pool RxReorderFrameStorage<CAPACITY, REORDER_SLOTS>>,
    reorder_scratch: Option<&'scratch mut [u8]>,
    runtime: &'pool mut ConnectedReceiveStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
}

mod dispatch;
mod owner;
mod reorder;
mod scheduler;

#[cfg(test)]
mod tests;
