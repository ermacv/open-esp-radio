//! The datapath actor's task: it installs the S31 data port, runs the
//! actor over it, reinstalls it for each restart of the station and releases
//! the radio when the station stopped.
use crate::chip::{
    self as irq, Binding, Config, Datapath, DriverExecutor, InitializationError, Interrupts,
};
use crate::chip::{B, N, Q, S, TX};
use crate::station::ImageComposition;
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use oer_chips_esp32s31_hal::{
    radio::ieee80211::{
        dma::{space::ChipDma, tx_storage::TxDmaStorage},
        init::{
            MAC_COLD_HANDSHAKE_DEADLINE, MacSlowClockCalibration, MacSlowClockCalibrationSource,
        },
    },
    system::root::WifiPartition,
};
use oer_chips_esp32s31_platform_board as board;
use oer_chips_esp32s31_radio_client::{MembershipCell, wifi::Wifi};
use oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::{
    TxBacking, TxSlot,
    ampdu::{HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage, RetainedDmaAmpduTx},
    runtime::WifiTxRuntimePolicy,
};
use oer_chips_esp32s31_radio_ieee80211_backend::{
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
use oer_chips_esp32s31_radio_phy::{PhyConfig, PhyTxTargetPowerProfile};
use oer_chips_family_radio_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_events::EventSink;
use oer_portable_memory::{WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use oer_portable_time::Duration;
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::*;
use oer_protocols_ieee80211_core_roles_sta::events::StaEvent;
use oer_protocols_ieee80211_core_roles_sta::link::LinkReturn;
use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent;
use oer_protocols_ieee80211_runtime::datapath::{
    Actor, ActorEnd, ActorEpochPath, ActorEvent, ActorLink, ActorSpan, DatapathPort, FromActor,
    GroupPending, LIFECYCLE_FRAMES, LifecycleEnds, NetworkEnds, PARCELS, Parcel, Returned, ToActor,
    from_actor_depth, returns_depth,
};
use oer_protocols_ieee80211_runtime::lifecycle::{ActorEnds, LifecyclePort, LifecycleTxBuffer};
use oer_protocols_ieee80211_runtime::station::{RxBlockAckStorage, ServiceEvent};
use oer_protocols_ieee80211_wire::{
    address::IndividualAddress,
    channel::{WifiChannel, WifiChannelWidth},
};
use oer_protocols_radio_port::{self as radio, SlotIdentity};
use oer_protocols_radio_regulatory::{Country, RegulatoryDomain};
use static_cell::StaticCell;

type Inner<W> = RadioDevice<
    ImageComposition<W>,
    EmbassyClock,
    oer_chips_esp32s31_radio_ieee80211_backend::runtime::WifiStopped<Platform>,
    oer_chips_family_platform_interrupt_table::Bound<<W as Binding>::Wifi>,
    irq::Bell<<W as Binding>::Wifi, <W as Binding>::DriverEntry>,
    N,
    B,
    S,
    Q,
    DriverExecutor,
>;
pub struct Device<W: Binding>(Inner<W>);
pub type Caps = Esp32s31Caps<TX>;
pub type Family<Id, W> = Types<Id, Device<W>, TX, ()>;
/// How `W`'s lent receive units tell their returns.
pub(crate) type Lent<W> = <<W as Binding>::Notifies as crate::Notifies>::Lent;
pub(crate) struct DriverStorage<W: Binding> {
    /// The lent units' notifies, until the installation places them.
    lent: Option<Lent<W>>,
    shared: StaticCell<PortSharedStorage<TX, Lent<W>>>,
    device: StaticCell<Device<W>>,
    /// The transmitter's descriptor and one per EDCA queue.
    slots: [StaticCell<TxSlot<TX>>; 5],
    /// Each aggregate chain's metadata and the leases it retains.
    chains: [StaticCell<HtAmpduTxStorage<TX_AMPDU_SUBFRAMES>>; AMPDU_CHAINS],
    retention:
        [StaticCell<RetainedAmpduDmaStorage<TxBacking<TX>, TX_AMPDU_SUBFRAMES>>; AMPDU_CHAINS],
    /// The data port's driver state, which every installation reuses.
    driver: PortDriverOf<W>,
}
impl<W: Binding> DriverStorage<W> {
    pub(crate) const fn new(lent: Lent<W>) -> Self {
        Self {
            lent: Some(lent),
            shared: StaticCell::new(),
            device: StaticCell::new(),
            slots: [const { StaticCell::new() }; 5],
            chains: [const { StaticCell::new() }; AMPDU_CHAINS],
            retention: [const { StaticCell::new() }; AMPDU_CHAINS],
            driver: PortDriver::vacant(),
        }
    }
}
pub(crate) struct Boot<W: Binding> {
    pub cell: MembershipCell<Wifi>,
    pub wifi: WifiPartition,
    pub delay_random: u32,
    pub station: [u8; 6],
    pub ap: [u8; 6],
    pub bound: oer_chips_family_platform_interrupt_table::Bound<W::Wifi>,
    pub bell: irq::Bell<W::Wifi, W::DriverEntry>,
    pub config: Config,
    pub entropy: u32,
    pub interrupts: &'static Interrupts,
    pub mac: &'static oer_chips_family_platform_interrupt_table::signal::IrqSignal<DriverExecutor>,
    /// The MAC's receive facts, which wake the station's task.
    pub receive:
        &'static oer_chips_family_platform_interrupt_table::signal::IrqSignal<DriverExecutor>,
    pub power:
        &'static oer_chips_family_platform_interrupt_table::signal::IrqSignal<DriverExecutor>,
    pub rx: oer_chips_esp32s31_hal::system::memory::dma_static::Placed<
        oer_chips_esp32s31_radio_ieee80211_backend::rx::arena::WifiRxStatic<
            N,
            B,
            S,
            Q,
            DriverExecutor,
        >,
    >,
    /// The transmit pool, pinned: the port holds its first buffers.
    pub pool: &'static TxPool<TX>,
    pub descriptors:
        [oer_chips_esp32s31_hal::system::memory::dma_static::Placed<crate::dma::TxDescriptor>; 5],
    pub chains:
        [oer_chips_esp32s31_hal::system::memory::dma_static::Placed<crate::dma::TxChain>; 2],
    pub storage: &'static mut DriverStorage<W>,
}
pub struct Platform {
    power: Option<PhyTxTargetPowerProfile>,
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
    fn beacon_window_pti(&self) -> oer_chips_esp32s31_hal::types::MacPti {
        const {
            oer_chips_esp32s31_hal::types::MacPti::from_coex(
                oer_chips_family_radio_coex::CoexPtiTable::VENDOR
                    .pti(oer_chips_family_radio_coex::wifi_events::BEACON),
            )
        }
    }
    fn connection_frame_priorities(&self, _: ConnectionFrame) -> Option<ConnectionFramePriorities> {
        None
    }
}
/// The S31 data port of `W`'s installations.
pub type Port<W> = Esp32s31Port<'static, <W as Binding>::Id, Device<W>, TX, ()>;
/// The installation commands' issuer of `W`'s port.
pub type Commands<W> = Esp32s31Commands<'static, <W as Binding>::Id, Device<W>, TX, ()>;
/// The minting authority of `W`'s installations.
pub type Installations<W> =
    oer_protocols_radio_port::Installations<<W as Binding>::Id, InstallationPools>;
/// The data port's driver state of `W`.
pub(crate) type PortDriverOf<W> = PortDriver<<W as Binding>::Id, Device<W>, TX, ()>;
/// The receive units the actor holds at most: the station's share of the
/// staged units, less the frames its lifecycle holds.
const ACTOR_RX_HELD: u8 = {
    let held = port::RX_HELD.saturating_sub(LIFECYCLE_FRAMES);
    if held > u8::MAX as usize || held <= LIFECYCLE_FRAMES {
        panic!("the actor's receive share holds the lifecycle's lane and fits a u8");
    }
    held as u8
};
/// The lifecycle's messages to the actor at most.
pub(crate) const TO: usize = 24;
/// The owners the actor returns at most: every terminal the port owes.
pub(crate) const RETURNS: usize = returns_depth(port::TERMINAL_CAPACITY);
/// The actor's messages to the lifecycle at most.
pub(crate) const FROM: usize = from_actor_depth(RETURNS);
/// The receive Block Ack agreements and their reorder frames.
const RX_BA_BANKS: usize = 8;
const RX_BA_FRAMES: usize = port::RX_HELD;
type RxBlockAcks<W> =
    RxBlockAckStorage<'static, Family<<W as Binding>::Id, W>, RX_BA_BANKS, RX_BA_FRAMES>;

/// The composition of `W`'s data port for the datapath actor and the
/// station's lifecycle.
pub struct ActorPort<W>(core::marker::PhantomData<fn() -> W>);
impl<W: Binding> DatapathPort<'static> for ActorPort<W> {
    type Port = Port<W>;
    type Caps = Caps;
    type Types = Family<W::Id, W>;
    type Small = port::BufferClass<TX>;
    type TxMemory = crate::TxMemory;
    type LentRx = crate::LentRx<W::Notifies>;
    const RX_HELD: u8 = ACTOR_RX_HELD;
    fn arena_ack(response: port::BlockAckAnswer) -> Option<port::Ack> {
        response.snr_db.map(|snr_db| port::Ack {
            snr: SnrSample::Measured(snr_db),
        })
    }
    #[cfg_attr(
        not(oer_trace),
        allow(unused_variables, reason = "only a trace build observes the span")
    )]
    fn span(span: ActorSpan, end: bool) {
        #[cfg(oer_trace)]
        {
            let observer = W::DriverObserver::default();
            oer_portable_trace::observe!(observer, crate::observation::ActorSpan { span, end });
        }
    }
    const OBSERVES_RETURNS: bool = cfg!(oer_trace);
    #[cfg_attr(
        not(oer_trace),
        allow(unused_variables, reason = "only a trace build observes the burst")
    )]
    fn returned(burst: oer_protocols_ieee80211_runtime::datapath::ReturnBurst) {
        #[cfg(oer_trace)]
        {
            let observer = W::DriverObserver::default();
            oer_portable_trace::observe!(observer, crate::observation::ActorReturned::of(burst));
        }
    }
    #[cfg_attr(
        not(oer_trace),
        allow(unused_variables, reason = "only a trace build observes the unit")
    )]
    fn received(frame: &[u8], meta: &<Family<W::Id, W> as PortTypes<'static>>::RxMeta) {
        #[cfg(oer_trace)]
        crate::station::observe_received::<W>(frame, meta.common());
    }
    #[cfg_attr(
        not(oer_trace),
        allow(unused_variables, reason = "only a trace build observes the terminal")
    )]
    fn terminal(terminal: &LowerMacTerminal<'static, Family<W::Id, W>>) {
        #[cfg(oer_trace)]
        crate::station::observe_terminal::<W, W::Id>(terminal);
    }
}
impl<W: Binding> LifecyclePort<'static> for ActorPort<W> {
    fn tx_buffer(
        buffer: port::AnyBuffer<'static, W::Id, Device<W>, TX, ()>,
    ) -> Result<LifecycleTxBuffer<'static, Self>, port::AnyBuffer<'static, W::Id, Device<W>, TX, ()>>
    {
        // The station takes the pool's first places; no buffer beyond
        // them is ever submitted.
        match buffer {
            port::AnyBuffer::Fresh(buffer) if buffer.index() < crate::station::MANAGEMENT => {
                Ok(ReturnedMpdu::Fresh(buffer))
            }
            port::AnyBuffer::Sent(buffer) if buffer.index() < crate::station::MANAGEMENT => {
                Ok(ReturnedMpdu::Sent(buffer))
            }
            buffer => Err(buffer),
        }
    }
}

/// The actor's sink: the station service's events of the image's `S`; the
/// actor's own decisions go unobserved.
pub struct ActorSink<S>(S);
impl<S: EventSink<ServiceEvent>> EventSink<ServiceEvent> for ActorSink<S> {
    const ENABLED: bool = S::ENABLED;
    fn event(&mut self, event: ServiceEvent) {
        self.0.event(event)
    }
}
impl<S: EventSink<StaEvent>> EventSink<StaEvent> for ActorSink<S> {
    const ENABLED: bool = S::ENABLED;
    fn event(&mut self, event: StaEvent) {
        self.0.event(event)
    }
}
impl<S: EventSink<RxBlockAckEvent>> EventSink<RxBlockAckEvent> for ActorSink<S> {
    const ENABLED: bool = S::ENABLED;
    fn event(&mut self, event: RxBlockAckEvent) {
        self.0.event(event)
    }
}
impl<S> EventSink<ActorEvent> for ActorSink<S> {
    const ENABLED: bool = false;
    fn event(&mut self, _: ActorEvent) {}
}

/// The datapath actor of `W`.
pub(crate) type ActorOf<W> =
    Actor<'static, ActorPort<W>, crate::station::Rates<W>, ActorSink<<W as Binding>::Sink>>;

/// The tokens of one installation the station's lifecycle takes.
pub(crate) type Tokens<W> = port::StationTokens<'static, <W as Binding>::Id, Device<W>, TX, ()>;

/// What the actor's task tells the station's task, in order, through the
/// one-slot installation mailbox.
pub(crate) enum News<W: Binding> {
    /// An installation, on first install and after every reinstall: its
    /// commands' issuer and the lifecycle's tokens.
    Installed(Commands<W>, Tokens<W>),
    /// The first installation did not come up.
    Unavailable(InitializationError),
    /// The port did not reinstall after the station's restart.
    Reinstall(crate::failure::ReinstallCause),
    /// The radio's release after the lifecycle retired the port, and
    /// whether the last stop's network participant was lost.
    Released {
        result: Result<(), crate::failure::ReleaseCause>,
        network_lost: bool,
    },
}

/// The datapath actor's static places and the queues between it and the
/// station's lifecycle, all on hart 0's executor.
pub(crate) struct ActorStorage<W: Binding> {
    actor: oer_portable_memory::StaticPlace<ActorOf<W>>,
    receive: oer_portable_memory::StaticPlace<RxBlockAcks<W>>,
    to_actor: WakingSpscQueue<ToActor<'static, ActorPort<W>>, TO>,
    parcels: WakingSpscQueue<Parcel<'static, ActorPort<W>>, PARCELS>,
    from_actor: WakingSpscQueue<FromActor<'static, ActorPort<W>>, FROM>,
    returns: WakingSpscQueue<Returned<'static, ActorPort<W>>, RETURNS>,
    groups: WakingSpscQueue<GroupPending<'static, ActorEpochPath<'static, ActorPort<W>>>, 1>,
    link: WakingSpscQueue<ActorLink<'static, ActorPort<W>>, 1>,
    link_return: WakingSpscQueue<LinkReturn, 1>,
    mailbox: WakingSpscQueue<News<W>, 1>,
}
impl<W: Binding> ActorStorage<W> {
    pub(crate) const fn new() -> Self {
        Self {
            actor: oer_portable_memory::StaticPlace::new(),
            receive: oer_portable_memory::StaticPlace::new(),
            to_actor: WakingSpscQueue::new(),
            parcels: WakingSpscQueue::new(),
            from_actor: WakingSpscQueue::new(),
            returns: WakingSpscQueue::new(),
            groups: WakingSpscQueue::new(),
            link: WakingSpscQueue::new(),
            link_return: WakingSpscQueue::new(),
            mailbox: WakingSpscQueue::new(),
        }
    }
    /// The actor's side and the lifecycle's side.
    pub(crate) fn split(
        &'static mut self,
    ) -> (
        ActorPlaces<W>,
        ActorEnds<'static, 'static, ActorPort<W>, TO, FROM, RETURNS>,
        WakingSpscReceiver<'static, News<W>, 1>,
    ) {
        let (to_actor, to_actor_end) = self.to_actor.split();
        let (parcels, parcels_end) = self.parcels.split();
        let (from_actor_end, from_actor) = self.from_actor.split();
        let (returns_end, returns) = self.returns.split();
        let (groups_end, groups) = self.groups.split();
        let (link, link_end) = self.link.split();
        let (link_return_end, link_return) = self.link_return.split();
        let (mailbox, news) = self.mailbox.split();
        (
            ActorPlaces {
                actor: &mut self.actor,
                receive: &mut self.receive,
                lifecycle: LifecycleEnds {
                    to_actor: to_actor_end,
                    parcels: parcels_end,
                    from_actor: from_actor_end,
                    returns: returns_end,
                    groups: groups_end,
                    link: link_end,
                    link_return: link_return_end,
                },
                mailbox,
            },
            ActorEnds {
                to_actor,
                parcels,
                from_actor,
                returns,
                groups,
                link,
                link_return,
            },
            news,
        )
    }
}

/// The actor task's places and its ends to the lifecycle.
pub(crate) struct ActorPlaces<W: Binding> {
    actor: &'static mut oer_portable_memory::StaticPlace<ActorOf<W>>,
    receive: &'static mut oer_portable_memory::StaticPlace<RxBlockAcks<W>>,
    lifecycle: LifecycleEnds<'static, 'static, ActorPort<W>, TO, FROM, RETURNS>,
    mailbox: WakingSpscSender<'static, News<W>, 1>,
}

/// The actor task's ends to the network stack.
pub(crate) type NetworkSide<W> = NetworkEnds<
    'static,
    crate::TxMemory,
    crate::LentRx<<W as Binding>::Notifies>,
    crate::NetworkStopState<<W as Binding>::Notifies>,
    { crate::TX_DEPTH },
    { crate::RX_DEPTH },
    <<W as Binding>::Notifies as crate::Notifies>::Channels,
>;

/// Post `news` to the station's task once the mailbox has room.
async fn post<W: Binding>(mailbox: &mut WakingSpscSender<'static, News<W>, 1>, news: News<W>) {
    mailbox.ready().await.send(news);
}

/// The datapath actor's task: install the port, run the actor, reinstall
/// the port for each restart and release the radio after the stop. The
/// actor's rates report to `observer`; its station events go to `sink`.
pub(crate) async fn drive<W: Binding>(datapath: Datapath<W>, observer: W::Observer, sink: W::Sink) {
    let Datapath {
        boot,
        places,
        mut network,
    } = datapath;
    let ActorPlaces {
        actor: place,
        receive,
        mut lifecycle,
        mut mailbox,
    } = places;
    let Some(mut installations) = W::take_installations() else {
        post(
            &mut mailbox,
            News::Unavailable(InitializationError::Storage),
        )
        .await;
        return;
    };
    let config = boot.config;
    let (parts, driver) = match install(boot, config).await {
        Ok(installed) => installed,
        Err(error) => {
            post(&mut mailbox, News::Unavailable(error)).await;
            return;
        }
    };
    let Some(minted) = installations.first() else {
        post(
            &mut mailbox,
            News::Unavailable(InitializationError::Registration),
        )
        .await;
        return;
    };
    let Ok(PortInstallation {
        port,
        commands,
        resources,
    }) = Port::<W>::install(parts, minted, driver)
    else {
        post(
            &mut mailbox,
            News::Unavailable(InitializationError::Backend),
        )
        .await;
        return;
    };
    let (block_acks, tokens) = resources.into_station();
    const {
        if core::mem::size_of::<RxBlockAcks<W>>() > 8192 {
            // A 32-MPDU window's reorder slots: placed static storage,
            // never a stack value.
            panic!("RX block-ack owner exceeds 8 KiB");
        }
    }
    let receive =
        RxBlockAcks::<W>::init_in_place(receive.uninit(), block_acks, Duration::from_millis(100))
            .view();
    let mut actor = ActorOf::<W>::init_in_place(
        place.uninit(),
        port,
        crate::station::Rates(EspressifRateSelection::new(), observer),
        receive,
        ActorSink(sink),
    );
    post(&mut mailbox, News::Installed(commands, tokens)).await;
    let mut epoch = FIRST_TSF_EPOCH;
    loop {
        match actor.run(&mut lifecycle, &mut network, &EmbassyClock).await {
            ActorEnd::Reinstall => {
                let Some(next) = epoch.checked_add(1) else {
                    post(
                        &mut mailbox,
                        News::Reinstall(crate::failure::ReinstallCause::Epochs),
                    )
                    .await;
                    return;
                };
                match actor.reinstall_in_place(|port| port.reinstall(&mut installations, next)) {
                    Ok((commands, resources)) => {
                        epoch = next;
                        // The actor keeps its receive agreements' storage;
                        // the new installation's slots stay unused.
                        let (_kept, tokens) = resources.into_station();
                        post(&mut mailbox, News::Installed(commands, tokens)).await;
                    }
                    Err(refused) => {
                        post(
                            &mut mailbox,
                            News::Reinstall(crate::failure::convert::reinstall(refused)),
                        )
                        .await;
                        return;
                    }
                }
            }
            ActorEnd::Retired => {
                // Read before the release: the loss of the last stop's
                // network participant, which the runner's exit records.
                let network_lost = actor.network_lost();
                let result = release::<W>(actor.port_mut()).await;
                post(
                    &mut mailbox,
                    News::Released {
                        result,
                        network_lost,
                    },
                )
                .await;
                return;
            }
            // The lifecycle hears the fault and ends with it: every owner
            // stays with the actor until the image resets.
            ActorEnd::Faulted(_) => return,
        }
    }
}

/// The TSF epoch of the first installation ([`LowerMacConfig::tsf_epoch`]).
const FIRST_TSF_EPOCH: u32 = 1;

/// Release the retired port's radio membership. Every owner a failed step
/// does not return stays quarantined in its static backing.
async fn release<W: Binding>(port: &mut Port<W>) -> Result<(), crate::failure::ReleaseCause> {
    use crate::failure::{ReleaseCause, convert};
    let Some(device) = port.release() else {
        return Err(ReleaseCause::NotStopped);
    };
    let Some((stopped, cell)) = device.0.take_stopped() else {
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
/// What bounds the station's transmit power besides its regulatory domain:
/// the board, the PHY's highest target and the owner's cap of `cap_dbm`.
fn power_limits(cap_dbm: i8) -> oer_protocols_radio_regulatory::PowerLimits {
    board::wifi_power_limits(
        i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
        Some(i32::from(cap_dbm) * 100),
    )
}

/// The power elements of an HE20 Association Request on `channel` of
/// `domain` at `cap_dbm` ([`crate::he::he_power`]): each HE-MCS's PHY
/// target power ([`PhyTxTargetPowerProfile::pair`], the MAC's power table)
/// bounded by the transmit ceiling the port's transmissions take
/// ([`CappedTxPowerProfile`]), and the MAC's minimum.
pub(crate) fn he_power(
    domain: &RegulatoryDomain,
    channel: u8,
    cap_dbm: i8,
) -> Result<oer_protocols_ieee80211_core_roles_sta::association::StaHePower, InitializationError> {
    let channel = WifiChannel::new_2_4_ghz(channel, WifiChannelWidth::Mhz20)
        .map_err(|_| InitializationError::Channel)?;
    let ceiling = domain
        .permit(oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(channel))
        .map_err(|_| InitializationError::Channel)?
        .tx_ceiling(power_limits(cap_dbm))
        .dbm();
    let targets = PhyConfig::production().tx_target_power_profile();
    crate::he::he_power(
        oer_chips_esp32s31_hal::types::MAC_MINIMUM_TX_POWER_DBM,
        |rate| targets.pair(rate).primary.min(ceiling),
    )
    .map_err(|_| InitializationError::HePower)
}

async fn install<W: Binding>(
    boot: Boot<W>,
    config: Config,
) -> Result<(PortParts<Device<W>, TX, ()>, &'static mut PortDriverOf<W>), InitializationError> {
    let domain = RegulatoryDomain::of(
        Country::new(config.country).map_err(|_| InitializationError::Country)?,
    )
    .ok_or(InitializationError::Domain)?;
    let channel = WifiChannel::new_2_4_ghz(config.channel, WifiChannelWidth::Mhz20)
        .map_err(|_| InitializationError::Channel)?;
    let initial = domain
        .permit(oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(channel))
        .map_err(|_| InitializationError::Channel)?;
    let limits = power_limits(config.cap_dbm);
    let ceiling = initial.tx_ceiling(limits);
    let start = RadioStartConfig::new(
        WifiColdStartConfig::new(initial, ceiling),
        WifiMacStartConfig::new(
            MAC_COLD_HANDSHAKE_DEADLINE,
            IndividualAddress::for_interface(boot.station)
                .map_err(|_| InitializationError::Address)?,
            IndividualAddress::for_interface(boot.ap).map_err(|_| InitializationError::Address)?,
            boot.delay_random,
        )
        .map_err(|_| InitializationError::Radio)?,
    );
    let (cell, stopped) =
        start_esp32s31_radio(boot.cell, boot.wifi, Platform { power: None }, start)
            .await
            .unwrap_or_else(|error| W::fail(InitializationError::Request(error.into())));
    let stopped = stopped.map_err(|_| InitializationError::Radio)?;
    let power = stopped.start_report().track.tx_power;
    let arena = boot.rx;
    let (rx, _, _) = oer_chips_esp32s31_radio_ieee80211_backend::rx::arena::WifiRxArena::<
        N,
        B,
        S,
        Q,
        DriverExecutor,
    >::bind(
        arena,
        WifiIrqSignals {
            mac: boot.mac,
            receive: boot.receive,
            power: boot.power,
        },
    )
    .map_err(|_| InitializationError::Rx)?;
    // Every bind lends the same static arena's counters: the first stays.
    #[cfg(oer_trace)]
    let _bound = crate::RX_COUNTERS.set(rx.counters());
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
                #[cfg(oer_trace)]
                stall: &crate::RX_STALL,
            }))
        })
        .ok_or(InitializationError::Storage)?;
    let pool = boot.pool;
    let [t0, t1, t2, t3, t4] = boot.descriptors;
    let [c0, c1] = boot.chains;
    let [s0, s1, s2, s3, s4] = &mut storage.slots;
    let [m0, m1] = &mut storage.chains;
    let [r0, r1] = &mut storage.retention;
    macro_rules! slot {
        ($dma:ident, $slot:ident) => {{
            let dma = TxDmaStorage::pin::<ChipDma, _>($dma).map_err(|_| InitializationError::Tx)?;
            Pin::new(
                $slot
                    .try_init(TxSlot::from_dma(dma))
                    .ok_or(InitializationError::Storage)?,
            )
        }};
    }
    macro_rules! chain {
        ($dma:ident, $metadata:ident, $retention:ident) => {{
            let metadata = $metadata
                .try_init(HtAmpduTxStorage::new())
                .ok_or(InitializationError::Storage)?;
            let retention = $retention
                .try_init(RetainedAmpduDmaStorage::new())
                .ok_or(InitializationError::Storage)?;
            let resources = HtAmpduTxResources::pin_static(metadata, $dma)
                .map_err(|_| InitializationError::Tx)?;
            RetainedDmaAmpduTx::new(resources, retention)
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
    let lent = storage.lent.take().ok_or(InitializationError::Storage)?;
    let shared = storage
        .shared
        .try_init_with(|| PortSharedStorage::with_network(lent))
        .ok_or(InitializationError::Storage)?;
    let parts = PortParts {
        device,
        shared,
        transmitter,
        pool,
        bank: crate::supply::PORT_BUFFERS,
        descriptors: TxDescriptors {
            single: [slot!(t1, s1), slot!(t2, s2), slot!(t3, s3), slot!(t4, s4)],
            chains: [chain!(c0, m0, r0), chain!(c1, m1, r1)],
        },
        config: LowerMacConfig {
            station_address: boot.station,
            channel,
            publication_timeout: PUBLICATION_BOUND,
            tsf_epoch: FIRST_TSF_EPOCH,
        },
        access_point_address: IndividualAddress::for_interface(boot.ap)
            .map_err(|_| InitializationError::Address)?,
        coex: &Coex,
        observations: (),
    };
    Ok((parts, &mut storage.driver))
}

impl<W: Binding> port::Device<TX> for Device<W> {
    type Power = <Inner<W> as port::Device<TX>>::Power;
    type Coex = <Inner<W> as port::Device<TX>>::Coex;
    type Entropy = Entropy;
    type Timer = EmbassyClock;
    type Hardware = <Inner<W> as port::Device<TX>>::Hardware;
    type Receive = <Inner<W> as port::Device<TX>>::Receive;
    type Lent = Lent<W>;
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
    fn next_deadline(&self) -> Option<Instant> {
        <Inner<W> as port::Device<TX>>::next_deadline(&self.0)
    }
    fn event_waiting(&self) -> bool {
        <Inner<W> as port::Device<TX>>::event_waiting(&self.0)
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
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Result<DeviceEvent, Fault>> {
        let result = <Inner<W> as port::Device<TX>>::poll_event(&mut self.0, cx);
        #[cfg(oer_trace)]
        if let Poll::Ready(Ok(DeviceEvent::Tx(
            oer_chips_esp32s31_radio_ieee80211_backend::tx::WifiTxWake::Interrupt { .. },
        ))) = &result
        {
            for queue in oer_chips_esp32s31_hal::types::MacTxQueueIndex::ALL {
                // A trace observation only: registers held elsewhere show nothing.
                if <Inner<W> as port::Device<TX>>::hardware(&mut self.0).is_ok_and(|hardware| {
                    hardware
                        .wifi_mac_hal()
                        .ordinary_tx_queue_snapshot(queue)
                        .collision_pending
                }) {
                    {
                        let observer = W::DriverObserver::default();
                        oer_portable_trace::observe!(
                            observer,
                            crate::observation::TxQueueCollision
                        );
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
    fn cut_held_by_units(&self) -> bool {
        <Inner<W> as port::Device<TX>>::cut_held_by_units(&self.0)
    }
    fn finish_rx_cut(&mut self) -> Result<(), Fault> {
        <Inner<W> as port::Device<TX>>::finish_rx_cut(&mut self.0)
    }
    fn returns_quiet(&mut self, held: usize, returned: bool) -> bool {
        <Inner<W> as port::Device<TX>>::returns_quiet(&mut self.0, held, returned)
    }
    fn poll_rx_work(&mut self, cx: &mut Context<'_>) -> Result<bool, Fault> {
        <Inner<W> as port::Device<TX>>::poll_rx_work(&mut self.0, cx)
    }
    fn pull_unit(
        &mut self,
        room: usize,
        now: oer_portable_time::Instant,
    ) -> Result<port::DevicePull<Self::Receive>, Fault> {
        <Inner<W> as port::Device<TX>>::pull_unit(&mut self.0, room, now)
    }
    fn recycle(&mut self, frame: Self::Receive) {
        <Inner<W> as port::Device<TX>>::recycle(&mut self.0, frame)
    }
    fn returned(&mut self) {
        <Inner<W> as port::Device<TX>>::returned(&mut self.0)
    }
    fn detach(&mut self) {
        <Inner<W> as port::Device<TX>>::detach(&mut self.0)
    }
}
