//! Control destination inspection while retaining the production source owners.
use oer_ieee80211_datapath::{DestinationTxHead, DestinationTxQueues, SelectedBurstMaterializer};
use oer_network_interface::NetworkInterfaceId;

pub(super) struct Fifo<'a, S, const EMPTY_SNAPSHOT: bool = false>(pub(super) &'a S);

impl<S: SelectedBurstMaterializer, const EMPTY_SNAPSHOT: bool> SelectedBurstMaterializer
    for Fifo<'_, S, EMPTY_SNAPSHOT>
{
    type SoftwareFrame = S::SoftwareFrame;
    type PhysicalFrame = S::PhysicalFrame;

    fn interface(&self) -> NetworkInterfaceId {
        self.0.interface()
    }

    fn queue_len(&self) -> usize {
        self.0.queue_len()
    }

    fn try_take(&self) -> Option<Self::SoftwareFrame> {
        self.0.try_take()
    }

    fn destination_queues(&self) -> Option<&dyn DestinationTxQueues<Frame = Self::SoftwareFrame>> {
        EMPTY_SNAPSHOT.then_some(self)
    }

    fn try_materialize(
        &self,
        frame: Self::SoftwareFrame,
    ) -> Result<Self::PhysicalFrame, Self::SoftwareFrame> {
        self.0.try_materialize(frame)
    }

    fn try_materialize_next(&self) -> Option<Self::PhysicalFrame> {
        self.0.try_materialize_next()
    }

    fn materialization_capacity(&self) -> usize {
        self.0.materialization_capacity()
    }

    fn ownership_snapshot(&self) -> oer_ieee80211_datapath::MaterializationOwnershipSnapshot {
        self.0.ownership_snapshot()
    }

    fn try_materialize_batch<const BATCH: usize>(
        &self,
        sources: &mut [Option<Self::SoftwareFrame>; BATCH],
        destinations: &mut [Option<Self::PhysicalFrame>; BATCH],
    ) -> bool {
        self.0.try_materialize_batch(sources, destinations)
    }
}

// Models a source publishing immediately after its empty demand snapshot.
impl<S: SelectedBurstMaterializer, const EMPTY_SNAPSHOT: bool> DestinationTxQueues
    for Fifo<'_, S, EMPTY_SNAPSHOT>
{
    type Frame = S::SoftwareFrame;

    fn next_head_after(&self, _: Option<[u8; 6]>) -> Option<([u8; 6], DestinationTxHead)> {
        None
    }
    fn head_for(&self, _: [u8; 6]) -> Option<DestinationTxHead> {
        None
    }
    fn try_take_for(&self, _: [u8; 6]) -> Option<Self::Frame> {
        panic!("an empty snapshot cannot select a destination");
    }
    fn poll_ready_for(
        &self,
        _: [u8; 6],
        _: usize,
        _: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        panic!("synchronous selection does not poll readiness");
    }

    fn poll_ready_any(&self, _: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        panic!("synchronous selection does not poll readiness");
    }
}
