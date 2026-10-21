#![no_std]
extern crate alloc;
use oer_ieee80211_port::{
    Channel, ChannelWidth, ConfigurePermission, LowerMacPhase, Station, StationPhase,
    StationReceive,
};
use oer_ieee80211_wire::address::IndividualAddress;
use oer_radio_port::{SlotIdentity, LocalId, with_local_slot, Disabled, Ended, Owner, PortBody, run_port};
use oer_ieee80211_model::{ModelBackend, ModelPool, ModelResources};
use oer_radio_port::Child;

async fn run_model<P>(channel: Channel, body: P)
where for<'scope> P: PortBody<LocalId<'scope>, ModelBackend<LocalId<'scope>>, Output = ()> {
    with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (backend, _) = ModelBackend::new(channel,
            [IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(), IndividualAddress::new([2, 0, 0, 0, 0, 2]).unwrap()],
            alloc::rc::Rc::new(oer_time_virtual::VirtualClock::<8>::new()));
        drop(oer_radio_port::LocalExecution.scope((&mut slot).register(backend).ok().unwrap(), async |frontend| run_port(frontend, body).await).await);
        slot.end_model_world();
    }).await
}

pub struct Outer;
impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Outer {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()> where ModelBackend<Id>: 'id, Id: 'id, 'id: 'p {
        run_model(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(), Inner(resources.station_commands.configure)).await;
        Ended::uninstall(owner, (), ())
    }
}
struct Inner<'outer, OuterId: SlotIdentity + 'outer>(ConfigurePermission<'outer, Station, Child<ModelPool<OuterId, 9>, 1>>);
impl<Id: SlotIdentity, OuterId: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Inner<'_, OuterId> {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()> where ModelBackend<Id>: 'id, Id: 'id, 'id: 'p {
        let mut owner = owner.enable().await.ok().unwrap();
        #[cfg(ui_reject)]
        owner
            .phase()
            .station()
            .configure(self.0, StationReceive::Off); // ui-error: E0308
        #[cfg(not(ui_reject))]
        owner
            .phase()
            .station()
            .configure(resources.station_commands.configure, StationReceive::Off);
        Ended::fail_stop(owner.fail_stop(), ())
    }
}
