//! P2 owns the S31 device, its DMA and the port registration.
use crate::chip::{
    self as irq, Binding, Config, Driver, InitializationError, Interrupts, RadioExecutor, Rng,
};
use crate::chip::{B, N, Q, S, TX};
use crate::station::ImageComposition;
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
use oer_esp32s31_ieee80211_driver::mac::tx::{TxSlot, runtime::WifiTxRuntimePolicy};
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
use oer_esp32s31_phy::{PhyConfig, PhyTxTargetPowerProfile};
use oer_esp32s31_radio_client::{MembershipCell, wifi::Wifi};
use oer_ieee80211_contracts_port::*;
use oer_ieee80211_wire::{
    address::IndividualAddress,
    channel::{WifiChannel, WifiChannelWidth},
};
use oer_radio_port::{self as radio, SlotIdentity};
use oer_radio_regulatory::{Country, RegulatoryDomain};
use oer_time::Instant;
use static_cell::StaticCell;

type Inner<W> = RadioDevice<
    ImageComposition<W>,
    EmbassyClock,
    Platform,
    oer_espressif_interrupt_table::Bound<<W as Binding>::Wifi>,
    irq::Bell<<W as Binding>::Wifi, <W as Binding>::Radio>,
    N,
    B,
    S,
    Q,
    RadioExecutor,
>;
pub struct Device<W: Binding>(Inner<W>);
pub type Backend<Id, W> = Esp32s31Backend<Id, Device<W>, TX, ()>;
pub type Caps = Esp32s31Caps<TX>;
pub type Family<Id, W> = Types<Id, Device<W>, TX, ()>;
pub(crate) struct DriverStorage<W: Binding> {
    shared: StaticCell<PortSharedStorage<TX>>,
    storage: StaticCell<PortStorage<Device<W>, TX>>,
    device: StaticCell<Device<W>>,
    slots: [StaticCell<TxSlot<TX>>; 9],
}
impl<W: Binding> DriverStorage<W> {
    pub(crate) const fn new() -> Self {
        Self {
            shared: StaticCell::new(),
            storage: StaticCell::new(),
            device: StaticCell::new(),
            slots: [const { StaticCell::new() }; 9],
        }
    }
}
pub(crate) struct Boot<W: Binding> {
    pub cell: MembershipCell<Wifi>,
    pub wifi: WifiPartition,
    pub rng: Rng,
    pub station: [u8; 6],
    pub ap: [u8; 6],
    pub bound: oer_espressif_interrupt_table::Bound<W::Wifi>,
    pub bell: irq::Bell<W::Wifi, W::Radio>,
    pub config: Config,
    pub entropy: u32,
    pub interrupts: &'static Interrupts,
    pub mac: &'static oer_espressif_interrupt_table::signal::IrqSignal<RadioExecutor>,
    pub power: &'static oer_espressif_interrupt_table::signal::IrqSignal<RadioExecutor>,
    pub rx: oer_esp32s31_hal::memory::dma_static::Placed<
        oer_esp32s31_ieee80211_driver::rx::arena::WifiRxStatic<N, B, S, Q, RadioExecutor>,
    >,
    pub tx: [oer_esp32s31_hal::memory::dma_static::Placed<TxDmaStorage<TX>>; 9],
    pub storage: &'static mut DriverStorage<W>,
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
        const {
            oer_esp32s31_hal::types::MacPti::from_coex(
                oer_espressif_coex::CoexPtiTable::VENDOR
                    .pti(oer_espressif_coex::wifi_events::BEACON),
            )
        }
    }
    fn connection_frame_priorities(&self, _: ConnectionFrame) -> Option<ConnectionFramePriorities> {
        None
    }
}
pub(crate) type Front<W> =
    radio::SlotFrontend<'static, <W as Binding>::Id, Backend<<W as Binding>::Id, W>>;
pub(crate) type ReadySlot<W> = oer_memory::RequestSlot<(), Result<Front<W>, InitializationError>>;
type Stopped<W> =
    oer_esp32s31_ieee80211_driver::port::StoppedPort<<W as Binding>::Id, Device<W>, TX, ()>;
/// The ordinary join of a station that ended for uninstall.
pub(crate) type Retire<W> = radio::Joined<<W as Binding>::Id>;
pub(crate) type RetireSlot<W> =
    oer_memory::RequestSlot<Retire<W>, Result<(), crate::failure::ReleaseCause>>;
pub(crate) async fn drive<W: Binding>(driver: Driver<W>) {
    let Driver {
        boot,
        ready,
        retire,
        bell,
    } = driver;
    let slot = W::take_slot().unwrap_or_else(|| W::fail(InitializationError::Storage));
    let mut slot = oer_espressif_executor::port_slot::StaticPortSlot::new(slot);
    poll_fn(|cx| bell.poll_rung(cx)).await;
    let (_, response) = ready
        .take_request()
        .unwrap_or_else(|| W::fail(InitializationError::Frontend));
    let config = boot.config;
    let parts = match install(boot, config).await {
        Ok(p) => p,
        Err(e) => {
            response.answer(Err(e));
            return;
        }
    };
    let backend = Backend::new(parts)
        .ok()
        .unwrap_or_else(|| W::fail(InitializationError::Backend));
    let (frontend, registration) =
        match oer_esp32s31_ieee80211_driver::port::Registration::guard_result(
            slot.register(backend),
        ) {
            Ok(r) => r,
            Err(e) => {
                response.answer(Err(InitializationError::Registration));
                drop(e);
                return;
            }
        };
    response.answer(Ok(frontend));
    let Ok(completion) = registration.run().await else {
        // A fail-stop issues no Joined; the station reports it.
        return;
    };
    completion.unregistered();
    // A ring that lands between the check and the wait stays pending.
    let (joined, response) = loop {
        if let Some(request) = retire.take_request() {
            break request;
        }
        poll_fn(|cx| bell.poll_rung(cx)).await;
    };
    response.answer(release::<W>(slot.retire(joined)).await);
}

/// Release the retired installation's radio membership. Every owner a
/// failed step does not return stays quarantined in its static backing.
async fn release<W: Binding>(
    vacant: Result<Stopped<W>, radio::SlotRetireRefused<W::Id>>,
) -> Result<(), crate::failure::ReleaseCause> {
    use crate::failure::{ReleaseCause, convert};
    let mut vacant = vacant.map_err(|refused| {
        let cause = convert::retirement_cause(refused.cause);
        let _quarantined = core::mem::ManuallyDrop::new(refused.joined);
        ReleaseCause::Retire(cause)
    })?;
    // A device another borrow holds is as unconfirmed as one still running.
    let Ok(Some((stopped, cell))) = vacant.with_device(|device| device.0.take_stopped()) else {
        let _quarantined = core::mem::ManuallyDrop::new(vacant);
        return Err(ReleaseCause::NotStopped);
    };
    match stopped.release(cell).await {
        Ok((_cell, Ok(released))) => convert::released(released.domain),
        Ok((_cell, Err(failure))) => {
            let cause = convert::still_active(failure.error);
            let _quarantined = core::mem::ManuallyDrop::new(failure.stopped);
            Err(cause)
        }
        Err(quarantine) => Err(ReleaseCause::LeaveUnanswered(
            quarantine.reason.observations(),
        )),
    }
}
async fn install<W: Binding>(
    boot: Boot<W>,
    config: Config,
) -> Result<PortParts<Device<W>, TX, ()>, InitializationError> {
    let domain = RegulatoryDomain::of(
        Country::new(config.country).map_err(|_| InitializationError::Country)?,
    )
    .ok_or(InitializationError::Domain)?;
    let channel = WifiChannel::new_2_4_ghz(config.channel, WifiChannelWidth::Mhz20)
        .map_err(|_| InitializationError::Channel)?;
    let initial = domain
        .permit(oer_ieee80211_contracts_port::RegulatedChannel::from(
            channel,
        ))
        .map_err(|_| InitializationError::Channel)?;
    let limits = board::wifi_power_limits(
        i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
        Some(i32::from(config.cap_dbm) * 100),
    );
    let ceiling = initial.tx_ceiling(limits);
    let start = RadioStartConfig::new(
        WifiColdStartConfig::new(initial, ceiling),
        WifiMacStartConfig::new(
            MAC_COLD_HANDSHAKE_DEADLINE,
            IndividualAddress::for_interface(boot.station)
                .map_err(|_| InitializationError::Address)?,
            IndividualAddress::for_interface(boot.ap).map_err(|_| InitializationError::Address)?,
        )
        .map_err(|_| InitializationError::Radio)?,
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
    .await
    .unwrap_or_else(|error| W::fail(InitializationError::Request(error.into())));
    let stopped = stopped.map_err(|_| InitializationError::Radio)?;
    let power = stopped.start_report().track.tx_power;
    let arena = boot.rx;
    let (rx, _, _) =
        oer_esp32s31_ieee80211_driver::rx::arena::WifiRxArena::<N, B, S, Q, RadioExecutor>::bind(
            arena,
            WifiIrqSignals {
                mac: boot.mac,
                power: boot.power,
            },
        )
        .map_err(|_| InitializationError::Rx)?;
    let storage = boot.storage;
    let device = storage
        .device
        .try_init_with(|| {
            Device(RadioDevice::new(RadioParts {
                wifi: stopped,
                rx,
                interrupt_cell: &boot.interrupts.line,
                bound: boot.bound,
                actor: cell,
                timer: EmbassyClock,
                doorbell: boot.bell,
                domain,
                power_limits: limits,
            }))
        })
        .ok_or(InitializationError::Storage)?;
    let [t0, t1, t2, t3, t4, t5, t6, t7, t8] = boot.tx;
    let [s0, s1, s2, s3, s4, s5, s6, s7, s8] = &mut storage.slots;
    macro_rules! slot {
        ($dma:ident, $slot:ident) => {{
            let dma = TxDmaStorage::pin::<ChipDma>($dma).map_err(|_| InitializationError::Tx)?;
            Pin::new(
                $slot
                    .try_init(TxSlot::from_dma(dma))
                    .ok_or(InitializationError::Storage)?,
            )
        }};
    }
    let transmitter = OrdinaryTxOwner::new(WifiTxResources {
        slot: slot!(t0, s0),
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: CappedTxPowerProfile::new(power, ceiling),
        entropy: Entropy(boot.entropy.max(1)),
        timer: EmbassyClock,
    })
    .into_idle()
    .map_err(|_| InitializationError::Tx)?;
    let parts = PortParts {
        storage: storage
            .storage
            .try_init_with(|| {
                PortStorage::new(
                    device,
                    storage
                        .shared
                        .try_init_with(PortSharedStorage::new)
                        .unwrap_or_else(|| W::fail(InitializationError::Storage)),
                )
            })
            .ok_or(InitializationError::Storage)?,
        transmitter,
        buffers: [
            slot!(t1, s1),
            slot!(t2, s2),
            slot!(t3, s3),
            slot!(t4, s4),
            slot!(t5, s5),
            slot!(t6, s6),
            slot!(t7, s7),
            slot!(t8, s8),
        ],
        config: LowerMacConfig {
            station_address: boot.station,
            channel,
            publication_timeout: PUBLICATION_BOUND,
            tsf_epoch: 1,
        },
        access_point_address: IndividualAddress::for_interface(boot.ap)
            .map_err(|_| InitializationError::Address)?,
        coex: &Coex,
        observations: (),
    };
    Ok(parts)
}

impl<W: Binding> port::Device<TX> for Device<W> {
    type Power = <Inner<W> as port::Device<TX>>::Power;
    type Coex = <Inner<W> as port::Device<TX>>::Coex;
    type Entropy = Entropy;
    type Timer = EmbassyClock;
    type Hardware = <Inner<W> as port::Device<TX>>::Hardware;
    type Receive = <Inner<W> as port::Device<TX>>::Receive;
    type Body = core::convert::Infallible;
    const LENT_RX: usize = <Inner<W> as port::Device<TX>>::LENT_RX;
    type OwnedContext<Id: SlotIdentity> = <Inner<W> as port::Device<TX>>::OwnedContext<Id>;
    type Context<'p, Id: SlotIdentity + 'p> = <Inner<W> as port::Device<TX>>::Context<'p, Id>;
    type OwnedConfigureContext<Id: SlotIdentity> =
        <Inner<W> as port::Device<TX>>::OwnedConfigureContext<Id>;
    type ConfigureContext<'p, Id: SlotIdentity + 'p> =
        <Inner<W> as port::Device<TX>>::ConfigureContext<'p, Id>;
    fn hardware(&mut self) -> Result<&mut Self::Hardware, Fault> {
        <Inner<W> as port::Device<TX>>::hardware(&mut self.0)
    }
    fn timer(&self) -> &EmbassyClock {
        <Inner<W> as port::Device<TX>>::timer(&self.0)
    }
    fn next_deadline(&self) -> Option<Instant> {
        <Inner<W> as port::Device<TX>>::next_deadline(&self.0)
    }
    fn take_power_profile(&mut self) -> Option<Self::Power> {
        <Inner<W> as port::Device<TX>>::take_power_profile(&mut self.0)
    }
    fn radio_now(&mut self) -> Result<Ieee80211Instant, radio::ClockError> {
        <Inner<W> as port::Device<TX>>::radio_now(&mut self.0)
    }
    fn poll_start(
        &mut self,
        cx: &mut Context<'_>,
        channel: WifiChannel,
    ) -> Poll<Result<(), Fault>> {
        <Inner<W> as port::Device<TX>>::poll_start(&mut self.0, cx, channel)
    }
    fn poll_stop(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Fault>> {
        <Inner<W> as port::Device<TX>>::poll_stop(&mut self.0, cx)
    }
    fn poll_receive(
        &mut self,
        room: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<DeviceEvent<Self::Receive>, Fault>> {
        let result = <Inner<W> as port::Device<TX>>::poll_receive(&mut self.0, room, cx);
        #[cfg(oer_trace)]
        if let Poll::Ready(Ok(DeviceEvent::Tx(
            oer_esp32s31_ieee80211_driver::tx::WifiTxWake::Interrupt { .. },
        ))) = &result
        {
            for queue in oer_esp32s31_hal::types::MacTxQueueIndex::ALL {
                // A trace observation only: registers held elsewhere show nothing.
                if <Inner<W> as port::Device<TX>>::hardware(&mut self.0).is_ok_and(|hardware| {
                    hardware
                        .wifi_mac_hal()
                        .ordinary_tx_queue_snapshot(queue)
                        .collision_pending
                }) {
                    {
                        let observer = W::DriverObserver::default();
                        oer_trace::observe!(observer, crate::observation::TxQueueCollision);
                    }
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
        <Inner<W> as port::Device<TX>>::poll_rx_cut(&mut self.0, room, cx)
    }
    fn finish_rx_cut(&mut self) -> Result<(), Fault> {
        <Inner<W> as port::Device<TX>>::finish_rx_cut(&mut self.0)
    }
    fn recycle(&mut self, frame: Self::Receive) {
        <Inner<W> as port::Device<TX>>::recycle(&mut self.0, frame)
    }
    fn returned(&mut self) {
        <Inner<W> as port::Device<TX>>::returned(&mut self.0)
    }
    fn abandon(&mut self) -> bool {
        <Inner<W> as port::Device<TX>>::abandon(&mut self.0)
    }
    fn detach(&mut self) {
        <Inner<W> as port::Device<TX>>::detach(&mut self.0)
    }
}

/// The image declaration names only this facade-composed backing.
pub type Slot<W> = oer_radio_port::PortSlot<<W as Binding>::Id, Backend<<W as Binding>::Id, W>>;
