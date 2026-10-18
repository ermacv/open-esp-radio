//! Ordered paired STA+AP RX handoff with its fact-only VIF route.
//!
//! The physical RX producer classifies each staged unit by its public
//! IEEE 802.11 header fields and publishes it, with that route, into one
//! ordered queue. This module owns only that transport. The STA+AP protocol
//! dispatcher in `roles::concurrent` decides which role consumes a frame.

use core::marker::PhantomData;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_memory::{AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, AffineSpscTrySendError};

use oer_esp32s31_ieee80211_mac::rx::{
    RxError, RxIngressConfig, RxSegment,
    pool::{VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
    view_normalized_rx_frame,
};

use oer_ieee80211_mac::vif::{StaApRxAddresses, StaApRxRoute, classify_sta_ap_rx};

use super::staging::StagedRxFrame;

/// Normalize one hardware-completed S31 receive unit and route only its
/// public IEEE 802.11 header fields to a logical interface.
///
/// Hardware status validation remains in the MAC RX backend. Association,
/// authorization, key and BlockAck policy remain in the selected role
/// consumer; this boundary cannot turn a header match into protocol trust.
pub fn classify_sta_ap_segment(
    segment: &RxSegment<'_>,
    ingress: RxIngressConfig,
    addresses: StaApRxAddresses,
) -> Result<StaApRxRoute, RxError> {
    let normalized = view_normalized_rx_frame(segment, ingress)?;
    Ok(classify_sta_ap_rx(normalized.mpdu, addresses))
}

/// One ordered staged owner together with the fact-only VIF classification
/// made at the common physical RX boundary.
///
/// Every outcome retains the unique staging lease. In particular, malformed,
/// foreign, ambiguous and invalid units cannot disappear merely because no
/// role consumer accepted them. The common protocol dispatcher must consume
/// the result and account for its final disposition.
pub struct StaApStagedRxFrame<
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    route: Result<StaApRxRoute, RxError>,
    frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
}

impl<'pool, const CAPACITY: usize, const SLOTS: usize> StaApStagedRxFrame<'pool, CAPACITY, SLOTS> {
    pub fn classify(
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
        ingress: RxIngressConfig,
        addresses: StaApRxAddresses,
    ) -> Self {
        let route = classify_sta_ap_segment(&frame.segment(), ingress, addresses);
        Self { route, frame }
    }

    pub const fn route(&self) -> Result<StaApRxRoute, RxError> {
        self.route
    }

    pub const fn frame(&self) -> &StagedRxFrame<'pool, CAPACITY, SLOTS> {
        &self.frame
    }

    pub fn into_parts(
        self,
    ) -> (
        Result<StaApRxRoute, RxError>,
        StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) {
        (self.route, self.frame)
    }

    pub fn into_frame(self) -> StagedRxFrame<'pool, CAPACITY, SLOTS> {
        self.frame
    }
}

/// Single ordered handoff from the physical RX producer to the STA+AP
/// protocol dispatcher.
///
/// This is deliberately not split into one queue per VIF: doing that in the
/// DMA producer would make inter-interface ordering and ownership loss
/// dependent on queue capacity. One consumer owns the ordered stream and
/// delegates each retained lease to the selected role protocol.
pub struct StaApStagedRxQueue<
    'pool,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    frames: AffineSpscQueue<StaApStagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    mutex: PhantomData<M>,
}

/// Sole physical-DMA producer for one ordered paired RX epoch.
pub struct StaApStagedRxSender<
    'pool,
    'queue,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    frames: AffineSpscSender<'queue, StaApStagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    mutex: PhantomData<M>,
}

impl<'pool, 'queue, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    StaApStagedRxSender<'pool, 'queue, M, DEPTH, CAPACITY, SLOTS>
{
    #[inline]
    pub fn try_send(
        &self,
        frame: StaApStagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Result<(), AffineSpscTrySendError<StaApStagedRxFrame<'pool, CAPACITY, SLOTS>>> {
        #[cfg(feature = "task-poll-telemetry")]
        let started = crate::diagnostics::core0_rx_cycles::cycle_count();
        let result = self.frames.try_send(frame);
        #[cfg(feature = "task-poll-telemetry")]
        crate::diagnostics::core0_rx_service_histogram::CORE0_RX_SERVICE_HISTOGRAM
            .record_spsc_push(
                crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(started),
                result.is_err(),
            );
        result
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn free_capacity(&self) -> usize {
        self.frames.free_capacity()
    }
}

/// Sole receiving endpoint of one ordered paired RX epoch.
///
/// It only yields frames in producer order. Choosing the role that consumes a
/// frame belongs to the STA+AP protocol dispatcher that owns this endpoint.
pub struct StaApStagedRxReceiver<
    'pool,
    'queue,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    frames: AffineSpscReceiver<'queue, StaApStagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
    mutex: PhantomData<M>,
}

impl<'pool, 'queue, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    StaApStagedRxReceiver<'pool, 'queue, M, DEPTH, CAPACITY, SLOTS>
{
    #[inline]
    pub fn try_receive(&mut self) -> Option<StaApStagedRxFrame<'pool, CAPACITY, SLOTS>> {
        #[cfg(feature = "task-poll-telemetry")]
        let started = crate::diagnostics::core0_rx_cycles::cycle_count();
        let received = self.frames.try_receive();
        #[cfg(feature = "task-poll-telemetry")]
        crate::diagnostics::core0_rx_service_histogram::CORE0_RX_SERVICE_HISTOGRAM.record_spsc_pop(
            crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(started),
            received.is_err(),
        );
        received.ok()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

impl<'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    StaApStagedRxQueue<'pool, M, DEPTH, CAPACITY, SLOTS>
{
    pub const fn new() -> Self {
        assert!(DEPTH != 0, "STA+AP staged RX queue must not be empty");
        assert!(
            DEPTH <= SLOTS,
            "STA+AP staged RX queue cannot outgrow its ownership pool"
        );
        Self {
            frames: AffineSpscQueue::new(),
            mutex: PhantomData,
        }
    }

    pub fn split(
        &self,
    ) -> (
        StaApStagedRxSender<'pool, '_, M, DEPTH, CAPACITY, SLOTS>,
        StaApStagedRxReceiver<'pool, '_, M, DEPTH, CAPACITY, SLOTS>,
    ) {
        let (sender, receiver) = self.frames.split();
        (
            StaApStagedRxSender {
                frames: sender,
                mutex: PhantomData,
            },
            StaApStagedRxReceiver {
                frames: receiver,
                mutex: PhantomData,
            },
        )
    }
}

impl<'pool, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize> Default
    for StaApStagedRxQueue<'pool, M, DEPTH, CAPACITY, SLOTS>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
