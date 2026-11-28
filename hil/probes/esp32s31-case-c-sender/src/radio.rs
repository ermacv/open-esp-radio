//! The radio held by the probe: Wi-Fi brought up by the public operations of
//! the PHY crate and the HAL in the radio system's order (README, "The
//! order"), and the receive ring's start and stop by the path table of
//! `tasks/277` (the cold preparation, the channel, the core, the publication,
//! the walker; the walker's edge, the stop request, the reset after a stop,
//! the release). Shared by the case C sender and the receive ring's probe
//! (`esp32s31-rx-ring`), which includes this file.

use core::mem::MaybeUninit;

use oer_chips_esp32s31_hal::{
    layouts::DmaWindow,
    power::ChipClocks,
    radio::domain::RadioDomain,
    radio::ieee80211::{
        client::{MacFirstReset, WifiClocked, WifiClocksOn, WifiCold, WifiInitialization},
        dma::receive::{RxRing, RxRingParts, WalkerEnabled},
        init::{
            MAC_COLD_HANDSHAKE_DEADLINE, MacBlockReset, MacCoexEvent, MacCoexPti, MacCoexPtiSource,
            MacColdStartConfig, MacDelayEntropy, MacPrepared, MacSlowClockCalibration,
            MacSlowClockCalibrationSource, MacTxPowerSource, initialize_wifi_mac,
        },
        registers::WifiRxDmaRegisters,
    },
    system::owner::{MacInterruptRegisters, MacPowerInterruptRegisters, RadioAccess},
    types::MacTxPowerPair,
};
use oer_chips_esp32s31_platform_board as board;
use oer_chips_esp32s31_platform_runtime::Esp32s31;
use oer_chips_esp32s31_radio_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig, PhyState,
    PhyTargetObserver, PhyTxTargetPowerProfile, RomShortDelay, enable_wifi_rx,
    executor::wait::{Polled, WaitTally, Waited},
    owner::{ChipPhy, PhyHandle, PhyPlace, RfCold, Tracking, tracking_due},
};
use oer_hil_services_radio_pair as service;
use oer_portable_clock::Clock as _;
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_memory::Occupied;
use oer_portable_memory::arena::Brand;
use oer_protocols_ieee80211_wire::channel::{ChannelWidth, Ghz2_4Channel};
use oer_protocols_radio_regulatory::{Country, RegulatoryDomain};

/// The stand's busy channel, 20 MHz.
pub(crate) const CHANNEL: u8 = 13;
/// The channel's lowered width: the PHY's channel-or-frequency argument by
/// number and CBW 0 (`radio-system` `channel::lower_ghz2_4_channel`).
pub(crate) const CBW: u8 = 0;

pub(crate) type Radio = RadioDomain<ChipClocks>;
/// The handle of the PHY's place, held by the session.
pub(crate) type Phy<'p> = PhyHandle<'p, ChipPhy>;
/// The proof that RF is open.
pub(crate) type RfOpen = oer_chips_esp32s31_radio_phy::owner::RfOpen<ChipPhy>;
pub(crate) type Cooperative =
    oer_chips_family_platform_executor::Cooperative<Esp32s31, EmbassyClock>;
/// The executor's cooperative timer beside the settle counter at the adopted
/// CPU rate, counting its waits in the session's book: what the PHY's
/// operations take.
pub(crate) type CooperativeTimer<'t> = oer_chips_esp32s31_radio_phy::executor::wait::PhyTimer<
    't,
    Cooperative,
    oer_chips_esp32s31_hal::system::cycles::CycleCounter,
    &'t WaitBook,
>;

/// The session's waits and polls of readiness since it was last read, by
/// kind.
#[derive(Default)]
pub(crate) struct WaitBook(core::cell::Cell<service::Waits>);

impl WaitBook {
    /// The waits since the last read, and an empty book.
    pub(crate) fn take(&self) -> service::Waits {
        self.0.take()
    }
}

impl WaitTally for WaitBook {
    const COUNTS: bool = true;
    fn waited(&self, kind: Waited, asked_micros: u64, taken_micros: u64) {
        let mut waits = self.0.get();
        let wait = match kind {
            Waited::SettleSpun => &mut waits.settle_spun,
            Waited::SettleTimed => &mut waits.settle_timed,
            Waited::Completion => &mut waits.completion,
            Waited::BusBusy => &mut waits.bus_busy,
            Waited::EngineRetry => &mut waits.engine_retry,
            Waited::Yield => &mut waits.yields,
        };
        wait.count = wait.count.saturating_add(1);
        wait.asked_micros = wait.asked_micros.saturating_add(saturated(asked_micros));
        wait.taken_micros = wait.taken_micros.saturating_add(saturated(taken_micros));
        self.0.set(waits);
    }
    fn polled(&self, kind: Polled, reads: u32, ready: bool, taken_micros: u64) {
        let mut waits = self.0.get();
        let poll = match kind {
            Polled::DcIqReadiness => &mut waits.polls.dc_iq_readiness,
            Polled::ToneSarReady => &mut waits.polls.tone_sar_ready,
            Polled::PwdetReady => &mut waits.polls.pwdet_ready,
        };
        poll.count = poll.count.saturating_add(1);
        poll.reads = poll.reads.saturating_add(reads);
        poll.ready = poll.ready.saturating_add(u32::from(ready));
        poll.taken_micros = poll.taken_micros.saturating_add(saturated(taken_micros));
        self.0.set(waits);
    }
}

/// `micros` as a counter's `u32`, saturating.
pub(crate) fn saturated(micros: u64) -> u32 {
    u32::try_from(micros).unwrap_or(u32::MAX)
}

/// A failure: its phase and its index.
pub(crate) type Failure = (u32, u32);

/// The phases a failure names.
pub(crate) mod phase {
    pub const REGISTER: u32 = 1;
    pub const POWER: u32 = 2;
    pub const CLOCKS: u32 = 3;
    pub const INITIALIZED: u32 = 4;
    pub const MAINTAIN: u32 = 6;
    pub const CHANNEL: u32 = 8;
    pub const MAC_COLD: u32 = 10;
    pub const RING: u32 = 11;
    #[allow(dead_code, reason = "the receive ring's probe's phase")]
    pub const RECEIVE: u32 = 12;
    pub const CEILING: u32 = 19;
    pub const SPLIT: u32 = 20;
}

/// What the session is given at boot.
pub(crate) struct SessionParts {
    /// The radio's root, which the session splits in its own frame.
    pub(crate) radio: oer_chips_esp32s31_hal::system::root::RadioHardware,
    pub(crate) clocks: ChipClocks,
    pub(crate) identity: PhyCalibrationIdentity,
    pub(crate) mac_irq: MacInterruptRegisters,
    pub(crate) power_irq: MacPowerInterruptRegisters,
    pub(crate) seed: u32,
    pub(crate) station: [u8; 6],
    pub(crate) ap: [u8; 6],
    /// The settle counter at the adopted CPU rate.
    pub(crate) cycles: oer_chips_esp32s31_hal::system::cycles::CycleCounter,
}

/// The platform values the MAC cold start reads.
///
/// SOURCE: `oer-chips-esp32s31-radio-client` `wifi::MacColdSources` and
/// `MAC_COEX_EVENTS` (the four events in `MacColdCoexPti::query` order).
pub(crate) struct Sources {
    pti: [MacCoexPti; 4],
    pub(crate) tx_power: PhyTxTargetPowerProfile,
    delay_random: u32,
}

impl MacCoexPtiSource for Sources {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti {
        let [one, three, ten, fifteen] = self.pti;
        match event {
            MacCoexEvent::Event1 => one,
            MacCoexEvent::Event3 => three,
            MacCoexEvent::Event10 => ten,
            MacCoexEvent::Event15 => fifteen,
        }
    }
}

impl MacDelayEntropy for Sources {
    fn mac_delay_random(&mut self) -> u32 {
        self.delay_random
    }
}

impl MacSlowClockCalibrationSource for Sources {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        MacSlowClockCalibration::Unavailable
    }
}

impl MacTxPowerSource for Sources {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair {
        let pair = self.tx_power.pair(rate);
        MacTxPowerPair {
            primary: pair.primary,
            alternate: pair.alternate,
        }
    }
}

/// Await `$future` pinned in place, polled by [`poll_apart`] in a frame of
/// its own: inlined into one poll, its temporaries would lie beside the
/// PHY's deepest calls on the radio executor's interrupt stack.
macro_rules! apart {
    ($future:expr) => {{
        let mut future = core::pin::pin!($future);
        core::future::poll_fn(|context| $crate::radio::poll_apart(future.as_mut(), context)).await
    }};
}
pub(crate) use apart;

/// Poll `future` in this function's frame, never inlined into its caller's.
#[inline(never)]
pub(crate) fn poll_apart<F: core::future::Future>(
    future: core::pin::Pin<&mut F>,
    context: &mut core::task::Context<'_>,
) -> core::task::Poll<F::Output> {
    future.poll(context)
}

/// The PHY's registration over `radio`, in the place of `phy`: the proof
/// that RF is open.
async fn register(
    radio: &mut Radio,
    phy: &mut Phy<'_>,
    cold: RfCold<ChipPhy>,
    timer: &CooperativeTimer<'_>,
    platform: &mut (),
    clocks: &ChipClocks,
    identity: PhyCalibrationIdentity,
) -> Result<RfOpen, Failure> {
    let registered = apart!(oer_chips_esp32s31_radio_phy::register::<
        _,
        (),
        RomShortDelay,
        _,
    >(
        timer,
        radio,
        phy,
        cold,
        platform,
        clocks,
        PhyRegisterConfig::new(identity),
        NoopPhyTargetObserver,
    ))
    .map_err(|_| (phase::REGISTER, 0))?;
    Ok(registered.open)
}

/// The PHY's tracking pass over `radio`, when one is due.
pub(crate) async fn track_if_due(
    radio: &mut Radio,
    phy: &mut Phy<'_>,
    open: RfOpen,
    timer: &CooperativeTimer<'_>,
    platform: &mut (),
) -> Result<RfOpen, Failure> {
    if let Tracking::NotBefore(_) = tracking_due(phy.state(&open), timer.now()) {
        return Ok(open);
    }
    track(radio, phy, open, timer, platform, NoopPhyTargetObserver).await
}

/// The PHY's tracking pass over `radio`, observed by `observer`.
pub(crate) async fn track(
    radio: &mut Radio,
    phy: &mut Phy<'_>,
    open: RfOpen,
    timer: &CooperativeTimer<'_>,
    platform: &mut (),
    observer: impl PhyTargetObserver,
) -> Result<RfOpen, Failure> {
    let (open, _outcome) = apart!(
        oer_chips_esp32s31_radio_phy::track::<_, (), RomShortDelay, _>(
            timer, radio, phy, open, platform, observer
        )
    )
    .map_err(|_| (phase::MAINTAIN, 0))?;
    Ok(open)
}

/// The channel's selection over `radio`, by its primary number (20 MHz).
async fn tune(
    radio: &mut Radio,
    clocked: &mut WifiClocked,
    phy: &mut Phy<'_>,
    open: RfOpen,
    timer: &CooperativeTimer<'_>,
    platform: &mut (),
    channel: Ghz2_4Channel,
) -> Result<RfOpen, Failure> {
    let mut observer = NoopPhyTargetObserver;
    let mut hal = clocked.channel_hal(platform, radio);
    apart!(oer_chips_esp32s31_radio_phy::select_channel::<
        RomShortDelay,
        _,
        _,
    >(
        timer,
        u16::from(channel.primary()),
        CBW,
        &mut hal,
        phy,
        open,
        &mut observer,
    ))
    .map_err(|_| (phase::CHANNEL, 1))
}

/// What stays held for the run beside the radio: coexistence, power and the
/// platform's clocks.
pub(crate) struct Held {
    _coex: Option<
        oer_chips_esp32s31_hal::radio::domain::Coexisting<
            oer_chips_esp32s31_hal::radio::domain::WifiClient,
        >,
    >,
    _power: oer_chips_esp32s31_hal::radio::domain::CommonPower<
        oer_chips_esp32s31_hal::radio::domain::WifiClient,
    >,
    clocks: ChipClocks,
}

impl Held {
    /// The platform's clocks, for their words' snapshot.
    pub(crate) const fn clocks(&self) -> &ChipClocks {
        &self.clocks
    }
}

/// Wi-Fi brought up to its first reset (S0 to S3): the clocked client with
/// the first reset's proof, the walker's registers by value, the cold
/// start's sources, the channel tuned, the registered PHY with RF open.
pub(crate) struct Brought<'p> {
    /// The radio domain, in the session's place.
    pub(crate) radio: Occupied<'p, Radio>,
    pub(crate) phy: Phy<'p>,
    pub(crate) open: RfOpen,
    /// The registration's time, in microseconds.
    pub(crate) register_micros: u32,
    /// The registration's waits, by kind.
    pub(crate) waits: service::Waits,
    pub(crate) clocked: WifiClocked,
    pub(crate) first: MacFirstReset,
    pub(crate) walker: WifiRxDmaRegisters,
    pub(crate) sources: Sources,
    pub(crate) mac: MacColdStartConfig,
    pub(crate) channel: Ghz2_4Channel,
    pub(crate) mac_irq: MacInterruptRegisters,
    pub(crate) power_irq: MacPowerInterruptRegisters,
    pub(crate) station: [u8; 6],
    pub(crate) held: Held,
}

/// Bring Wi-Fi up in the radio system's order: the PHY's registration into
/// the place made of `memory` and the split's right, coexistence, power, the clocks (and the first reset's proof),
/// the tracking due, RX enable and the channel.
pub(crate) async fn bring_up<'p>(
    parts: SessionParts,
    radio_memory: &'p mut MaybeUninit<Radio>,
    memory: &'p mut MaybeUninit<PhyState>,
    timer: &CooperativeTimer<'_>,
    platform: &mut (),
) -> Result<Brought<'p>, Failure> {
    let SessionParts {
        radio,
        clocks,
        identity,
        mac_irq,
        power_irq,
        seed,
        station,
        ap,
        cycles: _,
    } = parts;
    // A refused split is the bring-up's failure; its root is let go with
    // the session, which then serves only refusals.
    let (mut radio, partitions) = radio
        .split::<ChipClocks>(radio_memory)
        .map_err(|_| (phase::SPLIT, 0))?;
    let oer_chips_esp32s31_hal::system::root::ProtocolPartitions {
        wifi,
        wifi_coex: coex,
        phy: right,
    } = partitions;
    let (mut phy, cold) = PhyPlace::new(right, memory).fill();
    let _before = timer.tally().take();
    let began = timer.now();
    let open = register(
        &mut radio, &mut phy, cold, timer, platform, &clocks, identity,
    )
    .await?;
    let register_micros = micros_between(began, timer.now());
    let waits = timer.tally().take();
    let coexisting = radio.enter_coex(coex);
    let (cold, walker) = WifiCold::from_partition(wifi);
    let (powered, power) = cold
        .power_up(&mut radio, &clocks)
        .map_err(|_| (phase::POWER, 0))?;
    let (mut clocked, first) = powered
        .enable_clocks(&mut radio, &clocks)
        .map_err(|_| (phase::CLOCKS, 0))?;
    clocked
        .set_initialized(&mut radio, WifiInitialization::Initialized)
        .map_err(|_| (phase::INITIALIZED, 0))?;
    let open = track_if_due(&mut radio, &mut phy, open, timer, platform).await?;
    enable_wifi_rx(&mut radio, &open);
    let channel =
        Ghz2_4Channel::new(CHANNEL, ChannelWidth::Mhz20).map_err(|_| (phase::CHANNEL, 0))?;
    let open = apart!(tune(
        &mut radio,
        &mut clocked,
        &mut phy,
        open,
        timer,
        platform,
        channel
    ))?;
    let ceiling = Country::new(*b"DE")
        .ok()
        .and_then(RegulatoryDomain::of)
        .and_then(|domain| {
            domain
                .permit(oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(channel))
                .ok()
        })
        .map(|permitted| {
            permitted.tx_ceiling(board::wifi_power_limits(
                i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
                Some(1000),
            ))
        })
        .ok_or((phase::CEILING, 0))?;
    let tx_power = phy
        .state(&open)
        .tx_target_power_profile()
        .with_maximum_quarter_dbm(ceiling.quarter_dbm());
    let events = [
        MacCoexEvent::Event1,
        MacCoexEvent::Event3,
        MacCoexEvent::Event10,
        MacCoexEvent::Event15,
    ];
    let sources = Sources {
        pti: events.map(|event| MacCoexPti::from(radio.coex_pti(event.coex_event()))),
        tx_power,
        delay_random: seed,
    };
    let mac = MacColdStartConfig {
        handshake_deadline: MAC_COLD_HANDSHAKE_DEADLINE,
        station_address: station,
        access_point_address: ap,
    };
    Ok(Brought {
        radio,
        phy,
        open,
        register_micros,
        waits,
        clocked,
        first,
        walker,
        sources,
        mac,
        channel,
        mac_irq,
        power_irq,
        station,
        held: Held {
            _coex: Some(coexisting),
            _power: power,
            clocks,
        },
    })
}

/// The cold preparation (S5) with the walker's registers lent, then the
/// channel (S6): the preparation's proof, which the core's opening takes.
/// A failed preparation gives none, and RF's proof does not come back.
#[allow(
    clippy::too_many_arguments,
    reason = "each is one owner the preparation borrows"
)]
pub(crate) async fn prepare(
    radio: &mut Radio,
    clocked: &mut WifiClocked,
    phy: &mut Phy<'_>,
    open: RfOpen,
    walker: &mut WifiRxDmaRegisters,
    sources: &mut Sources,
    reset: MacBlockReset,
    mac: MacColdStartConfig,
    channel: Ghz2_4Channel,
    timer: &CooperativeTimer<'_>,
    platform: &mut (),
) -> Result<(MacPrepared, RfOpen), Failure> {
    let (_outcome, prepared) = {
        let mut hal = clocked.cold_mac_hal(walker);
        initialize_wifi_mac(reset, sources, &mut hal, timer, mac)
            .await
            .map_err(|_| (phase::MAC_COLD, 0))?
    };
    let open = apart!(tune(radio, clocked, phy, open, timer, platform, channel))?;
    Ok((prepared, open))
}

/// The microseconds from `began` to `ended`, saturated.
pub(crate) fn micros_between(
    began: oer_portable_time::Instant,
    ended: oer_portable_time::Instant,
) -> u32 {
    u32::try_from(ended.saturating_duration_since(began).as_micros()).unwrap_or(u32::MAX)
}

/// The station's receive policy under which a frame addressed to the
/// station is taken and acknowledged whatever its BSSID: the HAL's scan
/// transition with the broadcast BSSID.
fn station_policy(access: &mut RadioAccess) {
    let broadcast = [0xff; 6];
    access
        .wifi_mac_hal()
        .configure_station_receive_policy(broadcast);
    access
        .wifi_mac_hal()
        .configure_station_scan_policy(broadcast);
}

/// The interrupt epoch as the port's start opens it: both banks masked and
/// cleared, the station's mask published, the station TBTT enabled. No line
/// of the image routes the Wi-Fi sources.
fn open_epoch(mac_irq: &mut MacInterruptRegisters, power_irq: &mut MacPowerInterruptRegisters) {
    mac_irq.mask_all_mac_interrupts();
    power_irq.mask_all_power_interrupts();
    let stale = mac_irq.mac_interrupt_raw();
    mac_irq.acknowledge_mac_interrupt_raw(stale);
    power_irq.clear_all_power_interrupts();
    mac_irq.publish_mac_interrupt_mask(
        oer_chips_esp32s31_hal::radio::ieee80211::init::MAC_STATION_INTERRUPT_MASK,
    );
    power_irq.enable_station_tbtt_interrupt();
}

/// The receive ring's start over a prepared MAC (S7 to S10): the core
/// opened by the preparation's proof, the cold publication, the open
/// receive and the epoch, the walker, the MAC's receive.
#[allow(
    clippy::too_many_arguments,
    reason = "each is one owner the start takes or borrows"
)]
pub(crate) fn begin<'p, B, const BYTES: usize, const N: usize>(
    access: &mut RadioAccess,
    mac_irq: &mut MacInterruptRegisters,
    power_irq: &mut MacPowerInterruptRegisters,
    prepared: MacPrepared,
    place: &'p mut MaybeUninit<RxRing<B, BYTES, N, WifiRxDmaRegisters>>,
    parts: RxRingParts<B, BYTES, N, WifiRxDmaRegisters>,
) -> (
    Occupied<'p, RxRing<B, BYTES, N, WifiRxDmaRegisters>>,
    WalkerEnabled,
)
where
    B: Brand<Window = DmaWindow>,
{
    let core = access.wifi_mac_hal().open_core(prepared);
    let (mut ring, start) = RxRing::publish_cold(place, parts, core);
    access.wifi_mac_hal().configure_open_promiscuous_receive();
    station_policy(access);
    open_epoch(mac_irq, power_irq);
    let enabled = ring.enable_walker(start);
    access.wifi_mac_hal().start_receive(&enabled);
    (ring, enabled)
}

#[allow(
    dead_code,
    reason = "the receive ring's probe stops; the sender holds its ring"
)]
/// The stop in its order (T1 to T4): the walker's edge, the stop request
/// and its hold, the reset after a stop, the release. The reset's proof the
/// next cold preparation takes, every handle back, and the empty place.
pub(crate) fn stop<'p, B, const BYTES: usize, const N: usize>(
    radio: &mut Radio,
    access: &mut RadioAccess,
    clocks_on: &WifiClocksOn,
    mut ring: Occupied<'p, RxRing<B, BYTES, N, WifiRxDmaRegisters>>,
    enabled: WalkerEnabled,
    cycles: oer_chips_esp32s31_hal::system::cycles::CycleCounter,
) -> (
    MacBlockReset,
    RxRingParts<B, BYTES, N, WifiRxDmaRegisters>,
    &'p mut MaybeUninit<RxRing<B, BYTES, N, WifiRxDmaRegisters>>,
)
where
    B: Brand<Window = DmaWindow>,
{
    let edge = ring.stop_walker(enabled);
    let requested = access.wifi_mac_hal().request_stop(edge, &cycles);
    let (reset, stopped) = radio.reset_stopped_wifi_mac_block(clocks_on, requested, CBW);
    let (parts, place) = RxRing::release(ring, stopped);
    (reset, parts, place)
}

/// The radio's private hand-over between its owner, a task of the
/// application's level, and the radio level's task: `portable/memory`'s
/// exchange, one command in work at a time and its one result. The radio's
/// end is woken from the level below it, so it names `PriorityWaker`s; the
/// owner's end ordinary wakers. The composition splits it once: the owner
/// holds [`Commands`], the radio level's task [`Work`], and nothing else
/// names them.
pub(crate) type Handover<Q, A> = oer_portable_memory::Exchange<
    Q,
    A,
    oer_portable_sync::PriorityWaker,
    oer_portable_memory::AtomicWaker,
>;
/// The owner's end of the hand-over.
pub(crate) type Commands<Q, A> = oer_portable_memory::Asking<
    'static,
    Q,
    A,
    oer_portable_sync::PriorityWaker,
    oer_portable_memory::AtomicWaker,
>;
/// The radio level's end of the hand-over.
pub(crate) type Work<Q, A> = oer_portable_memory::Answering<
    'static,
    Q,
    A,
    oer_portable_sync::PriorityWaker,
    oer_portable_memory::AtomicWaker,
>;
/// The permit of one command's result.
pub(crate) type Done<'a, A> = oer_portable_memory::Reply<'a, A, oer_portable_memory::AtomicWaker>;

/// The hand-over, for a static of the composition.
pub(crate) const fn handover<Q, A>() -> Handover<Q, A> {
    oer_portable_memory::Exchange::with_notifies(
        oer_portable_sync::PriorityWaker::new(),
        oer_portable_sync::PriorityWaker::new(),
        oer_portable_memory::AtomicWaker::new(),
        oer_portable_memory::AtomicWaker::new(),
    )
}
