use super::*;
use alloc::collections::BTreeSet;
use oer_protocols_ieee80211_contracts_port::association::RxBlockAckPool;

/// Every Block Ack slot the pool mints carries a running agreement: the
/// pool's length is the capacity the backend applies, with no smaller table
/// behind it.
struct EverySlotApplied<'a>(&'a ModelAir);

impl Work for EverySlotApplied<'_> {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let agreement = |tid| {
            SupportedBlockAck::new(
                IndividualAddress::new(PEER).unwrap(),
                Tid::new(tid).unwrap(),
                SequenceNumber::new(0).unwrap(),
                32,
            )
            .unwrap()
        };
        let mut running = Vec::new();
        for (tid, slot) in (0..).zip(resources.block_acks) {
            running.push(phase.station().add_block_ack(slot, agreement(tid)));
        }
        yield_now().await;
        let applied = self.0.receive_agreements();
        assert_eq!(
            applied.len(),
            <<ModelTypes<ModelBackend<Id>> as PhaseTypes<'id>>::BlockAckPool as RxBlockAckPool>::LEN
        );
        let tids: BTreeSet<_> = applied
            .iter()
            .map(|(_, agreement)| agreement.tid.get())
            .collect();
        assert_eq!(
            tids.len(),
            applied.len(),
            "each slot keeps its own agreement"
        );
        for agreement in running {
            let _slot = phase.station().remove_block_ack(agreement);
        }
    }
}

#[test]
fn every_minted_block_ack_slot_carries_an_applied_agreement() {
    let (backend, air) = backend();
    let _port_end = block_on(run_model(backend, Body(EverySlotApplied(&air))));
}
