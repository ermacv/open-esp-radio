#![no_std]
use oer_ieee80211_lower_mac::{
    Channel, ChannelWidth, LowerMacPhase, LowerMacSetup, PhyRate, SupportedChannel,
};
use oer_ieee80211_mac::phy::LegacyRate;
use oer_radio_port::{Disabled, Ended, Owner, PortBody};
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelBackend, ModelResources};

pub struct Retune;
impl PortBody<ModelBackend> for Retune {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend, Disabled>,
        _resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
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
            SupportedChannel::new(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap()).unwrap(),
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
        Ended::fail_stopped(owner.fail_stop(), ())
    }
}
