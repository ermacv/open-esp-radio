#![no_std]
extern crate alloc;
use oer_ieee80211_contracts_port::{Channel, ChannelWidth, LowerMacPhase, PhyRate};
use oer_ieee80211_wire::{address::IndividualAddress, phy::LegacyRate};
use oer_radio_port::{SlotIdentity, LocalId, with_local_slot, Disabled, Enabled, Ended, Owner, PortBody, run_port};
use oer_ieee80211_testing_model::{ModelBackend, ModelResources};

async fn run_model<P>(channel: Channel, body: P)
where for<'scope> P: PortBody<LocalId<'scope>, ModelBackend<LocalId<'scope>>, Output = ()> {
    with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (backend, _) = ModelBackend::new(channel,
            [IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(), IndividualAddress::new([2, 0, 0, 0, 0, 2]).unwrap()],
            alloc::rc::Rc::new(oer_time_virtual::VirtualClock::<8>::new()));
        drop(oer_radio_port::LocalExecution.scope(oer_radio_port::conformance::registration::guarded((&mut slot).register(backend).ok().unwrap()), async |frontend| run_port(frontend, body).await).await);
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
        let mut target = match owner.enable().await { // ui-error: lifetime may not live long enough
            Ok(owner) => owner,
            Err(_) => panic!("target enable failed"),
        };
        {
            let source = self.source.phase(); // ui-error: lifetime may not live long enough
            let destination = target.phase();
            let raw = PhyRate::Legacy(LegacyRate::Dsss1M);
            assert!(source.rate(raw).is_some());
            assert!(destination.rate(raw).is_none());
            let rate = source.rate(raw).unwrap();
            #[cfg(ui_reject)]
            assert!(destination.no_ack(rate).is_ok());
            #[cfg(not(ui_reject))]
            assert!(source.no_ack(rate).is_ok());
        }
        // Exchanging the rates would force the two independent installation
        // lifetimes to be equal. The returned owner exposes both directions.
        Ended::fail_stop(target.fail_stop(), ())
    }
}
