//! The shared RF owner assembled with the same S31 platform and PHY policy.
use crate::{Binding, InitializationError};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_hal::{
    domain::RadioDomain,
    power::ChipClocks,
    root::{ProtocolPartitions, RadioHardware},
};
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig, concurrent::ConcurrentPhy,
    phy_get_rf_cal_version, sar_pilot,
};
use oer_esp32s31_radio_client::{self as client, LpClock, RcSlow};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
type Inner<P> = RadioSystem<
    client::wifi::Wifi,
    RcSlow,
    Esp32s31Radio<
        ChipClocks,
        oer_espressif_executor::Cooperative<Esp32s31, EmbassyClock>,
        NoopPhyTargetObserver,
        P,
    >,
    EmbassyClock,
>;
pub struct RadioActor<P: sar_pilot::Service> {
    inner: Inner<P>,
}
impl<P: sar_pilot::Service> RadioActor<P> {
    pub async fn run(&mut self) -> core::convert::Infallible {
        self.inner.run().await
    }
}
pub struct WifiHardware {
    pub cell: client::MembershipCell<client::wifi::Wifi>,
    pub wifi: oer_esp32s31_hal::root::WifiPartition,
}
pub struct RadioResources {
    domain: RadioDomain<ChipClocks, ConcurrentPhy>,
    partitions: ProtocolPartitions,
    clocks: ChipClocks,
    identity: PhyCalibrationIdentity,
    end: client::ActorEnd,
    cells: client::ClientCells,
    lp: LpClock<RcSlow>,
}
impl RadioResources {
    pub fn new<W: Binding>(
        hardware: RadioHardware,
        clocks: ChipClocks,
        factory: &oer_esp32s31_hal::peripherals::efuse::FactoryIdentity,
    ) -> Self {
        let identity = PhyCalibrationIdentity {
            rf_cal_version: phy_get_rf_cal_version(),
            base_mac_address: factory.base_mac(),
            mac_extension: factory.mac_extension(),
        };
        let (domain, partitions) = hardware
            .split::<ChipClocks, _>(
                ConcurrentPhy::new(),
                oer_esp32s31_compose::phy_fault::identity(),
            )
            .unwrap_or_else(|failure| {
                let fault = failure.fault();
                let _owners = failure;
                W::fail(InitializationError::Phy(fault))
            });
        let (Some((end, cells)), Some(lp)) = (client::claim(), LpClock::<RcSlow>::claim()) else {
            W::fail(InitializationError::MembershipUnavailable)
        };
        Self {
            domain,
            partitions,
            clocks,
            identity,
            end,
            cells,
            lp,
        }
    }
    pub fn start<P: sar_pilot::Service>(self, pilot: P) -> (RadioActor<P>, WifiHardware) {
        let backend = Esp32s31Radio::new(
            self.domain,
            self.partitions.wifi_coex,
            self.clocks,
            oer_espressif_executor::Cooperative::<Esp32s31, _>::new(EmbassyClock),
            NoopPhyTargetObserver,
            PhyRegisterConfig::new(self.identity),
            pilot,
        );
        (
            RadioActor {
                inner: RadioSystem::new(self.end, backend, EmbassyClock, self.lp),
            },
            WifiHardware {
                cell: self.cells.wifi,
                wifi: self.partitions.wifi,
            },
        )
    }
}
