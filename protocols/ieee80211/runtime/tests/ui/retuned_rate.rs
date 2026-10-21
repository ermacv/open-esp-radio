#![no_std]
use oer_ieee80211_port::{
    Channel, ChannelWidth, LowerMacPhase, LowerMacSetup, PhyRate, SupportedChannel,
};
use oer_ieee80211_wire::phy::LegacyRate;
use oer_radio_port::{SlotIdentity, LocalId, with_local_slot, Disabled, Ended, Owner, PortBody};
use oer_ieee80211_model::{ModelBackend, ModelResources};

pub struct Retune;
impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Retune {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        _resources: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()> where ModelBackend<Id>: 'id, Id: 'id, 'id: 'p {
        let mut owner = owner.enable().await.ok().unwrap();
        #[cfg(ui_reject)]
        let old = owner
            .phase()
            .rate(PhyRate::Legacy(LegacyRate::Dsss1M))
            .unwrap();
        #[cfg(not(ui_reject))]
        {
            let phase = owner.phase();
            let old = phase.rate(PhyRate::Legacy(LegacyRate::Dsss1M)).unwrap();
            assert!(phase.no_ack(old).is_ok());
        }
        let mut owner = owner.disable(&mut |_| core::ops::ControlFlow::Continue(())).await.ok().unwrap(); // ui-error: E0505
        owner.setup().set_channel(
            SupportedChannel::new(oer_radio_regulatory::TEST_DOMAIN.permit(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap()).unwrap()).unwrap(),
        );
        let mut owner = owner.enable().await.ok().unwrap();
        {
            let phase = owner.phase();
            #[cfg(ui_reject)]
            assert!(phase.no_ack(old).is_ok());
            #[cfg(not(ui_reject))]
            {
                assert!(phase.rate(PhyRate::Legacy(LegacyRate::Dsss1M)).is_none());
                let rate = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm6M)).unwrap();
                assert!(phase.no_ack(rate).is_ok());
            }
        }
        Ended::fail_stop(owner.fail_stop(), ())
    }
}
