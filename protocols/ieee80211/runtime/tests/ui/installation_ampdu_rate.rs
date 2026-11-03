#![no_std]
extern crate alloc;
use oer_protocols_ieee80211_contracts_port::arena::{ArenaFormat, ArenaPublication, TxArena};
use oer_protocols_ieee80211_contracts_port::{
    AmpduPhase, Channel, ChannelWidth, LowerMacPhase, MpduProtection, MpduStartSpacing, PhyRate,
    Protection, StationPhase, SupportedPower, TxPower,
};
use oer_protocols_ieee80211_wire::{
    address::IndividualAddress,
    phy::{HtMcs, HtRate, PpduBandwidth},
};
use oer_protocols_radio_port::{SlotIdentity, LocalId, with_local_slot, Disabled, Enabled, Ended, Owner, PortBody, run_port};
use oer_protocols_ieee80211_testing_model::{ModelBackend, ModelResources};

async fn run_model<P>(channel: Channel, body: P)
where for<'scope> P: PortBody<LocalId<'scope>, ModelBackend<LocalId<'scope>>, Output = ()> {
    with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (backend, _) = ModelBackend::new(channel,
            [IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(), IndividualAddress::new([2, 0, 0, 0, 0, 2]).unwrap()],
            alloc::rc::Rc::new(oer_portable_time_virtual::VirtualClock::<8>::new()));
        drop(oer_protocols_radio_port::LocalExecution.scope(oer_protocols_radio_port::conformance::registration::guarded((&mut slot).register(backend).ok().unwrap()), async |frontend| run_port(frontend, body).await).await);
        slot.end_model_world();
    }).await
}

pub struct Source;

impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Source {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        _resources: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()> where ModelBackend<Id>: 'id, Id: 'id, 'id: 'p {
        let mut source = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("source enable failed"),
        };
        run_model(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap(), Target {
                    source: &mut source
                }).await;
        Ended::fail_stop(source.fail_stop(), ())
    }
}

struct Target<'s, 'p, 'outer: 'p, OuterId: SlotIdentity + 'outer> {
    source: &'s mut Owner<'p, 'outer, OuterId, ModelBackend<OuterId>, Enabled>,
}

impl<Id: SlotIdentity, OuterId: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Target<'_, '_, '_, OuterId> {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        _resources: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()> where ModelBackend<Id>: 'id, Id: 'id, 'id: 'p {
        let mut target = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("target enable failed"),
        };
        {
            let mut source = self.source.phase();
            let mut destination = target.phase();
            let raw = PhyRate::Ht(
                HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
            );
            let rate = source.rate(raw).unwrap();
            let rate = source.station().ampdu_rate(rate).ok().unwrap();
            let publication = |arena| ArenaPublication { // ui-error: lifetime may not live long enough
                arena,
                format: ArenaFormat::Aggregate {
                    rate,
                    spacing: MpduStartSpacing::new(0).unwrap(),
                },
                access_category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
                protection: Protection::None,
                encryption: MpduProtection::Open,
                power: SupportedPower::new(TxPower::Calibrated).unwrap(),
            };
            let now = oer_portable_time::Instant::EPOCH;
            #[cfg(ui_reject)]
            {
                let mut station = destination.station();
                let mut arena = station.data_arena();
                let [first, _] = arena.arenas().unwrap();
                drop(arena.publish(publication(first), now));
            }
            #[cfg(not(ui_reject))]
            {
                let _ = &mut destination;
                let mut station = source.station();
                let mut arena = station.data_arena();
                let [first, _] = arena.arenas().unwrap();
                drop(arena.publish(publication(first), now));
            }
        }
        // Exchanging the rates would force the two independent installation
        // lifetimes to be equal. The returned owner exposes both directions.
        Ended::fail_stop(target.fail_stop(), ())
    }
}
