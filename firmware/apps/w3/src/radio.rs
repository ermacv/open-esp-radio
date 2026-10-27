//! P2 owns the S31 device, its DMA and the port registration.
use crate::Rng;
use crate::irq::{B, N, Q, S, TX};
use crate::station::ImageComposition;
use crate::{W3Config, irq};
use core::{
    future::poll_fn,
    pin::Pin,
    task::{Context, Poll},
};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::{
    ieee80211::{
        dma::{space::ChipDma, tx_storage::TxDmaStorage},
        init::{
            MAC_COLD_HANDSHAKE_DEADLINE, MacDelayEntropy, MacSlowClockCalibration,
            MacSlowClockCalibrationSource,
        },
    },
    root::WifiPartition,
};
use oer_esp32s31_ieee80211_driver::{
    lower_mac::{ConnectionFramePriorities, LowerMacConfig, RadioCoexPriorities},
    ordinary_tx::{OrdinaryTxOwner, WifiTxEntropy},
    port::{
        self,
        device::{RadioDevice, RadioParts},
        *,
    },
    rx::arena::WifiIrqSignals,
    startup::{
        RadioStartConfig, WifiColdStartConfig, WifiMacPlatform, WifiMacStartConfig,
        start_esp32s31_radio,
    },
    tx::{CappedTxPowerProfile, WifiTxResources},
};
use oer_esp32s31_ieee80211_mac::tx::{TxSlot, runtime::WifiTxRuntimePolicy};
use oer_esp32s31_phy::{PhyConfig, PhyTxTargetPowerProfile};
use oer_esp32s31_radio_client::{MembershipCell, wifi::Wifi};
use oer_ieee80211_port::*;
use oer_ieee80211_wire::{
    address::IndividualAddress,
    channel::{WifiChannel, WifiChannelWidth},
};
use oer_radio_port::{self as radio, SlotIdentity};
use oer_radio_regulatory::{Country, RegulatoryDomain};
use oer_time::Instant;
use static_cell::StaticCell;

type Inner = RadioDevice<
    ImageComposition,
    EmbassyClock,
    Platform,
    irq::WifiBound,
    irq::ImageDoorbell,
    N,
    B,
    S,
    Q,
    irq::RadioExecutor,
>;
pub struct Device(Inner);
pub type Backend<Id> = Esp32s31Backend<Id, Device, TX>;
pub type Caps = Esp32s31Caps<TX>;
pub type Family<Id> = Types<Id, Device, TX>;
oer_espressif_executor::declare_port_slot!(pub slot: super::Backend<Id>);
static SHARED: StaticCell<PortSharedStorage<TX>> = StaticCell::new();
static STORAGE: StaticCell<PortStorage<Device, TX>> = StaticCell::new();
static DEVICE: StaticCell<Device> = StaticCell::new();
pub static TIMEOUTS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static COLLISIONS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub struct Boot {
    pub cell: MembershipCell<Wifi>,
    pub wifi: WifiPartition,
    pub rng: Rng,
    pub station: [u8; 6],
    pub ap: [u8; 6],
    pub bound: irq::WifiBound,
    pub bell: irq::ImageDoorbell,
    pub config: W3Config,
    pub entropy: u32,
}
pub struct Platform {
    rng: Rng,
    power: Option<PhyTxTargetPowerProfile>,
}
impl MacDelayEntropy for Platform {
    fn mac_delay_random(&mut self) -> u32 {
        self.rng.random()
    }
}
impl MacSlowClockCalibrationSource for Platform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        MacSlowClockCalibration::Unavailable
    }
}
impl WifiMacPlatform for Platform {
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile) {
        self.power = Some(profile)
    }
}
pub struct Entropy(u32);
impl WifiTxEntropy for Entropy {
    fn next_u32(&mut self) -> u32 {
        // Marsaglia xorshift32, seeded from the hardware RNG at every boot.
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.0 = value;
        value
    }
}
pub struct Coex;
impl RadioCoexPriorities for Coex {
    fn beacon_window_pti(&self) -> oer_esp32s31_hal::types::MacPti {
        let event = const { oer_esp32s31_radio_coex::CoexEventId::new(0).expect("beacon event") };
        oer_esp32s31_hal::types::MacPti::new(
            oer_esp32s31_radio_coex::CoexPtiTable::VENDOR
                .pti(event)
                .value()
                .into(),
        )
        .expect("four bit PTI")
    }
    fn connection_frame_priorities(&self, _: ConnectionFrame) -> Option<ConnectionFramePriorities> {
        None
    }
}
type Front = radio::SlotFrontend<'static, slot::Id, Backend<slot::Id>>;
static READY_BELL: oer_memory::Doorbell = oer_memory::Doorbell::new();
type ReadySlot = oer_memory::RequestSlot<(), Result<Front, &'static str>>;
pub struct DriverChannel(&'static ReadySlot);
pub struct FrontendChannel(oer_memory::Poster<(), Result<Front, &'static str>>);
pub fn channels() -> (DriverChannel, FrontendChannel) {
    static SLOT: StaticCell<ReadySlot> = StaticCell::new();
    let slot = SLOT.init_with(|| ReadySlot::new(&READY_BELL, 1));
    (
        DriverChannel(slot),
        FrontendChannel(slot.claim().expect("one station frontend")),
    )
}
pub async fn drive(boot: Boot, channel: DriverChannel) {
    let slot = slot::take_static().expect("one W3 port slot");
    let mut slot = oer_espressif_executor::port_slot::StaticPortSlot::new(slot);
    poll_fn(|cx| READY_BELL.poll_rung(cx)).await;
    let (_, response) = channel
        .0
        .take_request()
        .expect("frontend requested its port");
    let config = boot.config;
    let parts = match install(boot, config).await {
        Ok(parts) => parts,
        Err(error) => {
            response.answer(Err(error));
            return;
        }
    };
    let backend = Backend::new(parts).ok().expect("validated W3 port parts");
    let (frontend, registration) = slot.register(backend).ok().expect("one W3 registration");
    response.answer(Ok(frontend));
    if let Some(completion) = registration.run().await {
        completion.unregistered();
    }
}
pub async fn serve(channel: FrontendChannel, config: W3Config, address: [u8; 6], seed: u64) {
    let mut pending = channel.0.post(());
    let (_, front) = poll_fn(|cx| pending.poll_answer(cx)).await;
    let mut report = match front {
        Ok(front) => {
            let mut running = core::pin::pin!(radio::run_port(
                front,
                crate::station::Body {
                    config,
                    address,
                    seed,
                },
            ));
            match crate::future::Boundary(running.as_mut()).await {
                radio::PortEnd::Uninstalled { output, .. } => output,
                _ => {
                    oer_firmware_apps_w3::report::Report::new(config.channel, address, config.stand)
                }
            }
        }
        Err(_) => oer_firmware_apps_w3::report::Report::new(config.channel, address, config.stand),
    };
    report.tx_timeout = TIMEOUTS.load(core::sync::atomic::Ordering::Relaxed);
    report.queue_collision = COLLISIONS.load(core::sync::atomic::Ordering::Relaxed);
    report.irq_free =
        oer_espressif_staged_runtime::stacks::current_hart_interrupt_stack_free_bytes();
    crate::print_report(report);
}
async fn install(boot: Boot, config: W3Config) -> Result<PortParts<Device, TX>, &'static str> {
    let domain = RegulatoryDomain::of(Country::new(config.country).map_err(|_| "country")?)
        .ok_or("domain")?;
    let channel =
        WifiChannel::new_2_4_ghz(config.channel, WifiChannelWidth::Mhz20).map_err(|_| "channel")?;
    let initial = domain.permit(channel).map_err(|_| "channel refused")?;
    let limits = board::wifi_power_limits(
        i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
        Some(i32::from(config.cap_dbm) * 100),
    );
    let ceiling = initial.tx_ceiling(limits);
    let start = RadioStartConfig::new(
        WifiColdStartConfig::new(initial, ceiling),
        WifiMacStartConfig::new(
            MAC_COLD_HANDSHAKE_DEADLINE,
            IndividualAddress::for_interface(boot.station).map_err(|_| "station")?,
            IndividualAddress::for_interface(boot.ap).map_err(|_| "AP")?,
        )
        .map_err(|_| "MAC")?,
    );
    let (cell, stopped) = start_esp32s31_radio(
        boot.cell,
        boot.wifi,
        Platform {
            rng: boot.rng,
            power: None,
        },
        start,
    )
    .await;
    let stopped = stopped.map_err(|_| "radio start")?;
    let power = stopped.start_report().track.tx_power;
    let arena = irq::RX.take().ok_or("RX claimed")?;
    let (rx, _, _) = irq::Arena::bind(
        arena,
        WifiIrqSignals {
            mac: &irq::MAC_SIGNAL,
            power: &irq::POWER_SIGNAL,
        },
    )
    .map_err(|_| "RX bind")?;
    let device = DEVICE.init_with(|| {
        Device(RadioDevice::new(RadioParts {
            wifi: stopped,
            rx,
            interrupt_cell: &irq::LINE,
            bound: boot.bound,
            actor: cell,
            timer: EmbassyClock,
            doorbell: boot.bell,
            domain,
            power_limits: limits,
        }))
    });
    macro_rules! slot {
        ($dma:ident, $slot:ident) => {{
            let dma = TxDmaStorage::pin::<ChipDma>(irq::$dma.take().ok_or("TX claimed")?)
                .map_err(|_| "TX bind")?;
            Pin::new(irq::$slot.init(TxSlot::from_dma(dma)))
        }};
    }
    let transmitter = OrdinaryTxOwner::new(WifiTxResources {
        slot: slot!(TX0, SLOT0),
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: CappedTxPowerProfile::new(power, ceiling),
        entropy: Entropy(boot.entropy.max(1)),
        timer: EmbassyClock,
    })
    .into_idle()
    .map_err(|_| "TX idle")?;
    let parts = PortParts {
        storage: STORAGE
            .init_with(|| PortStorage::new(device, SHARED.init_with(PortSharedStorage::new))),
        transmitter,
        buffers: [
            slot!(TX1, SLOT1),
            slot!(TX2, SLOT2),
            slot!(TX3, SLOT3),
            slot!(TX4, SLOT4),
            slot!(TX5, SLOT5),
            slot!(TX6, SLOT6),
            slot!(TX7, SLOT7),
            slot!(TX8, SLOT8),
        ],
        config: LowerMacConfig {
            station_address: boot.station,
            channel,
            publication_timeout: PUBLICATION_BOUND,
            tsf_epoch: 1,
        },
        access_point_address: IndividualAddress::for_interface(boot.ap).map_err(|_| "AP")?,
        coex: &Coex,
    };
    Ok(parts)
}

impl port::Device<TX> for Device {
    type Power = <Inner as port::Device<TX>>::Power;
    type Coex = <Inner as port::Device<TX>>::Coex;
    type Entropy = Entropy;
    type Timer = EmbassyClock;
    type Hardware = <Inner as port::Device<TX>>::Hardware;
    type Receive = <Inner as port::Device<TX>>::Receive;
    type Body = core::convert::Infallible;
    type OwnedContext<Id: SlotIdentity> = <Inner as port::Device<TX>>::OwnedContext<Id>;
    type Context<'p, Id: SlotIdentity + 'p> = <Inner as port::Device<TX>>::Context<'p, Id>;
    type OwnedConfigureContext<Id: SlotIdentity> =
        <Inner as port::Device<TX>>::OwnedConfigureContext<Id>;
    type ConfigureContext<'p, Id: SlotIdentity + 'p> =
        <Inner as port::Device<TX>>::ConfigureContext<'p, Id>;
    fn hardware(&mut self) -> &mut Self::Hardware {
        <Inner as port::Device<TX>>::hardware(&mut self.0)
    }
    fn timer(&self) -> &EmbassyClock {
        <Inner as port::Device<TX>>::timer(&self.0)
    }
    fn next_deadline(&self) -> Option<Instant> {
        <Inner as port::Device<TX>>::next_deadline(&self.0)
    }
    fn take_power_profile(&mut self) -> Option<Self::Power> {
        <Inner as port::Device<TX>>::take_power_profile(&mut self.0)
    }
    fn radio_now(&mut self) -> Result<Ieee80211Instant, radio::ClockError> {
        <Inner as port::Device<TX>>::radio_now(&mut self.0)
    }
    fn poll_start(
        &mut self,
        cx: &mut Context<'_>,
        channel: WifiChannel,
    ) -> Poll<Result<(), Fault>> {
        <Inner as port::Device<TX>>::poll_start(&mut self.0, cx, channel)
    }
    fn poll_stop(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Fault>> {
        <Inner as port::Device<TX>>::poll_stop(&mut self.0, cx)
    }
    fn poll_receive(
        &mut self,
        room: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<DeviceEvent<Self::Receive>, Fault>> {
        let result = <Inner as port::Device<TX>>::poll_receive(&mut self.0, room, cx);
        if let Poll::Ready(Ok(DeviceEvent::Tx(
            oer_esp32s31_ieee80211_driver::tx::WifiTxWake::Interrupt { .. },
        ))) = &result
        {
            use core::sync::atomic::Ordering;

            for queue in 0..4 {
                if <Inner as port::Device<TX>>::hardware(&mut self.0)
                    .wifi_mac_hal()
                    .ordinary_tx_queue_snapshot(queue)
                    .collision_pending
                {
                    COLLISIONS.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        result
    }
    fn poll_rx_cut(
        &mut self,
        room: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Self::Receive>, Fault>> {
        <Inner as port::Device<TX>>::poll_rx_cut(&mut self.0, room, cx)
    }
    fn finish_rx_cut(&mut self) -> Result<(), Fault> {
        <Inner as port::Device<TX>>::finish_rx_cut(&mut self.0)
    }
    fn recycle(&mut self, frame: Self::Receive) {
        <Inner as port::Device<TX>>::recycle(&mut self.0, frame)
    }
    fn abandon(&mut self) -> bool {
        <Inner as port::Device<TX>>::abandon(&mut self.0)
    }
    fn detach(&mut self) {
        <Inner as port::Device<TX>>::detach(&mut self.0)
    }
}
