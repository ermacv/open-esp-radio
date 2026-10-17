#![no_std]
use oer_ieee80211_lower_mac::{
    Attempt, Channel, ChannelWidth, ClassFrame, CoexPriority, GroupAddress, HeaderSpec,
    InterfacePhase, LowerMacPhase, MpduLength, PhyRate, Protection, SizedFrame, Station,
    SupportedCoex, SupportedPower, TxBufferPool, TxFrame, TxPower, TxProtection, TxSource,
};
use oer_ieee80211_mac::{
    address::IndividualAddress, frame::FrameControl, phy::LegacyRate, qos::WmmAccessCategory,
};
use oer_radio_port::{Disabled, Enabled, Ended, Owner, PortBody, run_port};
use oer_ieee80211_runtime::lower_mac_owner::model::{
    ModelBackend, ModelCaps, ModelResources, ModelSmall, ModelTypes,
};

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
        resources: ModelResources<'id>,
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
                    source: &mut source,
                    resources
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
    resources: ModelResources<'outer>,
}

impl PortBody<ModelBackend> for Target<'_, '_, '_> {
    type Output = ();
    async fn run<'p, 'id>(
        mut self,
        owner: Owner<'p, 'id, ModelBackend, Disabled>,
        mut resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        let mut target = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("target enable failed"),
        };
        {
            let mut source = self.source.phase();
            let mut destination = target.phase();
            let rate = source.rate(PhyRate::Legacy(LegacyRate::Dsss1M)).unwrap();
            let header = HeaderSpec::new(
                FrameControl::new(0x0108).unwrap(),
                0,
                GroupAddress::new([255; 6]).unwrap(),
                [2, 0, 0, 0, 0, 1],
                [2, 0, 0, 0, 0, 9],
                0,
                None,
                None,
                None,
            )
            .unwrap();
            let frame = ClassFrame::new(TxFrame::Group {
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
            let prepared = <ModelTypes as TxBufferPool<ModelSmall>>::prepare::<Station, ModelCaps>(
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
                coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
            };
            #[cfg(ui_reject)]
            destination.station().submit((attempt).admit()).unwrap();
            #[cfg(not(ui_reject))]
            source.station().submit((attempt).admit()).unwrap();
        }
        // Exchanging the rates would force the two independent installation
        // lifetimes to be equal. The returned owner exposes both directions.
        Ended::fail_stopped(target.fail_stop(), ()) // ui-error: null lifetime may not live long enough // ui-error: null lifetime may not live long enough
    }
}
