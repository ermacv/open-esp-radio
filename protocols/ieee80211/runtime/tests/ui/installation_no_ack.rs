#![no_std]
extern crate alloc;
use oer_ieee80211_port::{
    Attempt, Channel, ChannelWidth, ClassFrame, TxOperation, HeaderSpec, InterfacePhase,
    LowerMacPhase, MpduLength, NoAckHeader, PhyRate, Protection, SizedFrame, Station,
    SupportedPower, TxBufferPool, TxFrame, TxPower, TxProtection, TxSource,
};
use oer_ieee80211_wire::{
    address::IndividualAddress, frame::FrameControl, phy::LegacyRate, qos::WmmAccessCategory,
};
use oer_radio_port::{SlotIdentity, LocalId, with_local_slot, Disabled, Enabled, Ended, Owner, PortBody, run_port};
use oer_ieee80211_model::{
    ModelBackend, ModelCaps, ModelResources, ModelSmall, ModelTypes,
};

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

pub struct Source;

impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Source {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()> where ModelBackend<Id>: 'id, Id: 'id, 'id: 'p {
        let mut source = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("source enable failed"),
        };
        run_model(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap(), Target {
                    source: &mut source,
                    resources
                }).await;
        Ended::fail_stop(source.fail_stop(), ())
    }
}

struct Target<'s, 'p, 'outer: 'p, OuterId: SlotIdentity + 'outer> {
    source: &'s mut Owner<'p, 'outer, OuterId, ModelBackend<OuterId>, Enabled>,
    resources: ModelResources<'outer, ModelBackend<OuterId>>,
}

impl<Id: SlotIdentity, OuterId: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Target<'_, '_, '_, OuterId> {
    type Output = ();
    async fn run<'p, 'id>(
        mut self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        mut resources: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()> where ModelBackend<Id>: 'id, Id: 'id, 'id: 'p {
        let mut target = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("target enable failed"),
        };
        {
            let mut source = self.source.phase();
            let mut destination = target.phase();
            let rate = source
                .no_ack(source.rate(PhyRate::Legacy(LegacyRate::Dsss1M)).unwrap())
                .ok()
                .unwrap();
            let header = HeaderSpec::new(
                FrameControl::new(0x0188).unwrap(),
                0,
                IndividualAddress::new([2, 0, 0, 0, 0, 9]).unwrap(),
                [2, 0, 0, 0, 0, 1],
                [2, 0, 0, 0, 0, 9],
                0,
                None,
                Some(oer_ieee80211_port::QosControl {
                    tid: oer_ieee80211_port::Tid::new(0).unwrap(),
                    end_of_service_period: false,
                    amsdu_present: false,
                    high: 0,
                }),
                None,
            )
            .and_then(NoAckHeader::new)
            .unwrap();
            let frame = ClassFrame::new(TxFrame::IndividualNoAck {
                frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
                rate,
            })
            .ok()
            .unwrap();
            let input = TxSource::Fresh {
                buffer: self.resources.small.remove(0),
                frame,
                body: None,
                protection: TxProtection::Open,
            };
            let prepared = <ModelTypes<ModelBackend<OuterId>> as TxBufferPool<ModelSmall>>::prepare::<Station, ModelCaps>(
                input.check().ok().unwrap(),
            )
            .ok()
            .unwrap();
            #[cfg(ui_reject)]
            let ticket = resources.tickets.remove(0);
            #[cfg(not(ui_reject))]
            let ticket = self.resources.tickets.remove(0);
            let attempt = Attempt {
                context: (),
                ticket,
                source: prepared,
                access_category: WmmAccessCategory::BestEffort,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                coex_operation: TxOperation::Data,
            };
            #[cfg(ui_reject)]
            destination.station().submit((attempt).admit()).unwrap(); // ui-error: E0599
            #[cfg(not(ui_reject))]
            source.station().submit((attempt).admit()).unwrap();
        }
        // Exchanging the rates would force the two independent installation
        // lifetimes to be equal. The returned owner exposes both directions.
        Ended::fail_stop(target.fail_stop(), ())
    }
}
