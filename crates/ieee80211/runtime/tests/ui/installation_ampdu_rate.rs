#![no_std]
use oer_ieee80211_lower_mac::{
    AmpduPhase, Channel, ChannelWidth, LowerMacPhase, PhaseAggregate, PhyRate, Tid, TxProtection,
};
use oer_ieee80211_mac::{
    address::IndividualAddress,
    phy::{HtMcs, HtRate, PpduBandwidth},
};
use oer_radio_port::{Disabled, Enabled, Ended, Owner, PortBody, run_port};
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelBackend, ModelResources};

fn backend(channel: Channel) -> oer_radio_port::Installed<ModelBackend> {
    ModelBackend::new(
        channel,
        [
            IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
            IndividualAddress::new([2, 0, 0, 0, 0, 2]).unwrap(),
        ],
    )
    .0
}

pub struct Source;

impl PortBody<ModelBackend> for Source {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend, Disabled>,
        _resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        let mut source = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("source enable failed"),
        };
        let target = backend(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap());
        assert!(matches!(
            run_port(
                target,
                Target {
                    source: &mut source
                }
            )
            .await,
            oer_radio_port::PortEnd::FailStopped { .. }
        ));
        Ended::fail_stopped(source.fail_stop(), ())
    }
}

struct Target<'s, 'p, 'outer> {
    source: &'s mut Owner<'p, 'outer, ModelBackend, Enabled>,
}

impl PortBody<ModelBackend> for Target<'_, '_, '_> {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend, Disabled>,
        _resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
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
            let peer = IndividualAddress::new([2, 0, 0, 0, 0, 9]).unwrap();
            #[cfg(ui_reject)]
            let parts = destination
                .station()
                .aggregate(peer, Tid::new(0).unwrap(), rate, TxProtection::Open)
                .into_parts();
            #[cfg(not(ui_reject))]
            let parts = source
                .station()
                .aggregate(peer, Tid::new(0).unwrap(), rate, TxProtection::Open)
                .into_parts();
            drop(parts);
        }
        // Exchanging the rates would force the two independent installation
        // lifetimes to be equal. The returned owner exposes both directions.
        Ended::fail_stopped(target.fail_stop(), ()) // ui-error: null lifetime may not live long enough // ui-error: null lifetime may not live long enough
    }
}
