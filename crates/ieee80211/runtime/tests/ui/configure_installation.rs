#![no_std]
use oer_ieee80211_lower_mac::{
    Channel, ChannelWidth, ConfigurePermission, LowerMacPhase, Station, StationPhase,
    StationReceive,
};
use oer_ieee80211_mac::address::IndividualAddress;
use oer_radio_port::{Disabled, Ended, Owner, PortBody, run_port};
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelBackend, ModelPool, ModelResources};
use oer_radio_port::Child;

fn backend() -> oer_radio_port::Installed<ModelBackend> {
    ModelBackend::new(
        Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
        [
            IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
            IndividualAddress::new([2, 0, 0, 0, 0, 2]).unwrap(),
        ],
    )
    .0
}
pub struct Outer;
impl PortBody<ModelBackend> for Outer {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        let _ended = run_port(backend(), Inner(resources.station_commands.configure)).await;
        Ended::uninstall(owner, (), ())
    }
}
struct Inner<'outer>(ConfigurePermission<'outer, Station, Child<ModelPool<9>, 1>>);
impl PortBody<ModelBackend> for Inner<'_> {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        let mut owner = owner.enable().await.ok().unwrap();
        #[cfg(ui_reject)]
        owner
            .phase()
            .station()
            .configure(self.0, StationReceive::Off);
        #[cfg(not(ui_reject))]
        owner
            .phase()
            .station()
            .configure(resources.station_commands.configure, StationReceive::Off);
        Ended::fail_stopped(owner.fail_stop(), ()) // ui-error: null lifetime may not live long enough // ui-error: null lifetime may not live long enough
    }
}
