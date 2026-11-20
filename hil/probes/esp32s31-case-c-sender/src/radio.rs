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
        client::{MacFirstReset, WifiClocked, WifiClocksOn, WifiCold},
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
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig,
    PhyTxTargetPowerProfile, RomShortDelay,
    concurrent::{ConcurrentAcquire, ConcurrentPhy, PhyClientAccess, phy_ready},
    maintain_concurrent_phy, register_concurrent_phy, select_concurrent_wifi_channel,
    wifi_client::{WifiPhyMembership, join_wifi, set_wifi_rx},
};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_memory::Occupied;
use oer_portable_memory::arena::Brand;
use oer_protocols_ieee80211_wire::channel::{ChannelWidth, Ghz2_4Channel};
use oer_protocols_radio_regulatory::{Country, RegulatoryDomain};

use crate::phy_pilot;

/// The stand's busy channel, 20 MHz.
pub(crate) const CHANNEL: u8 = 13;
/// The channel's lowered width: the PHY's channel-or-frequency argument by
/// number and CBW 0 (`radio-system` `channel::lower_ghz2_4_channel`).
pub(crate) const CBW: u8 = 0;

pub(crate) type Radio = RadioDomain<ChipClocks, ConcurrentPhy>;
pub(crate) type CooperativeTimer =
    oer_chips_family_platform_executor::Cooperative<Esp32s31, EmbassyClock>;
/// A failure: its phase and its index.
pub(crate) type Failure = (u32, u32);

/// The phases a failure names.
pub(crate) mod phase {
    pub const REGISTER: u32 = 1;
    pub const POWER: u32 = 2;
    pub const CLOCKS: u32 = 3;
    pub const INITIALIZED: u32 = 4;
    pub const JOIN: u32 = 5;
    pub const MAINTAIN: u32 = 6;
    pub const RX_ENABLE: u32 = 7;
    pub const CHANNEL: u32 = 8;
    pub const POWER_PROFILE: u32 = 9;
    pub const MAC_COLD: u32 = 10;
    pub const RING: u32 = 11;
    #[allow(dead_code, reason = "the receive ring's probe's phase")]
    pub const RECEIVE: u32 = 12;
    pub const CEILING: u32 = 19;
}

/// What the session is given at boot.
pub(crate) struct SessionParts {
    pub(crate) radio: Radio,
    pub(crate) clocks: ChipClocks,
    pub(crate) wifi: oer_chips_esp32s31_hal::system::root::WifiPartition,
    pub(crate) coex: oer_chips_esp32s31_hal::radio::domain::CoexAbsent<
        oer_chips_esp32s31_hal::radio::domain::WifiClient,
    >,
    pub(crate) pilot: phy_pilot::Client,
    pub(crate) identity: PhyCalibrationIdentity,
    pub(crate) mac_irq: MacInterruptRegisters,
    pub(crate) power_irq: MacPowerInterruptRegisters,
    pub(crate) seed: u32,
    pub(crate) station: [u8; 6],
    pub(crate) ap: [u8; 6],
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

/// The session's radio, where it stays: taken by a PHY operation that
/// owns it and put back by its answer.
pub(crate) fn held(slot: &mut Option<Radio>) -> Result<&mut Radio, Failure> {
    slot.as_mut().ok_or((phase::REGISTER, 9))
}

/// The PHY's registration over the radio in `slot`.
async fn register(
    slot: &mut Option<Radio>,
    timer: &CooperativeTimer,
    platform: &mut (),
    clocks: &ChipClocks,
    identity: PhyCalibrationIdentity,
) -> Result<(), Failure> {
    let radio = slot.take().ok_or((phase::REGISTER, 1))?;
    let mut operation = core::pin::pin!(register_concurrent_phy::<_, (), RomShortDelay, _>(
        timer,
        radio,
        platform,
        clocks,
        PhyRegisterConfig::new(identity),
        NoopPhyTargetObserver,
    ));
    let mut completion = MaybeUninit::uninit();
    let (returned, result) = oer_portable_memory::poll_placed(operation.as_mut(), &mut completion)
        .await
        .split();
    *slot = Some(returned.read());
    let _outcome = result.read().map_err(|_| (phase::REGISTER, 0))?.outcome();
    Ok(())
}

/// The PHY's tracking over the radio in `slot`.
async fn maintain(
    slot: &mut Option<Radio>,
    timer: &CooperativeTimer,
    platform: &mut (),
    pilot: &mut phy_pilot::Client,
) -> Result<(), Failure> {
    let radio = slot.take().ok_or((phase::MAINTAIN, 1))?;
    let mut operation = core::pin::pin!(maintain_concurrent_phy::<_, (), RomShortDelay, _>(
        timer,
        radio,
        platform,
        NoopPhyTargetObserver,
        pilot,
    ));
    let mut completion = MaybeUninit::uninit();
    let (returned, result) = oer_portable_memory::poll_placed(operation.as_mut(), &mut completion)
        .await
        .split();
    *slot = Some(returned.read());
    result.read().map_err(|_| (phase::MAINTAIN, 0))?;
    Ok(())
}

/// The channel's selection over the radio in `slot`, by its primary number
/// (20 MHz).
async fn tune(
    slot: &mut Option<Radio>,
    clocked: &mut WifiClocked,
    timer: &CooperativeTimer,
    platform: &mut (),
    channel: Ghz2_4Channel,
) -> Result<(), Failure> {
    let radio = slot.take().ok_or((phase::CHANNEL, 2))?;
    let transaction =
        radio.detach(oer_chips_esp32s31_hal::radio::domain::transaction::Operation::Channel);
    let mut observer = NoopPhyTargetObserver;
    let selection = clocked.channel_transaction(platform, transaction);
    let mut operation = core::pin::pin!(select_concurrent_wifi_channel::<RomShortDelay, _, _, _>(
        timer,
        u16::from(channel.primary()),
        CBW,
        selection,
        &mut observer,
    ));
    let mut completion = MaybeUninit::uninit();
    let (returned, result) = oer_portable_memory::poll_placed(operation.as_mut(), &mut completion)
        .await
        .split();
    *slot = Some(returned.read());
    result.read().map_err(|_| (phase::CHANNEL, 1))?;
    Ok(())
}

/// What stays held for the run beside the radio: coexistence, power, the
/// membership and the pilot, and the platform's clocks.
pub(crate) struct Held {
    _coex: Option<
        oer_chips_esp32s31_hal::radio::domain::Coexisting<
            oer_chips_esp32s31_hal::radio::domain::WifiClient,
        >,
    >,
    _power: oer_chips_esp32s31_hal::radio::domain::CommonPower<
        oer_chips_esp32s31_hal::radio::domain::WifiClient,
    >,
    _membership: WifiPhyMembership,
    _pilot: phy_pilot::Client,
    _clocks: ChipClocks,
}

/// Wi-Fi brought up to its first reset (S0 to S3): the clocked client with
/// the first reset's proof, the walker's registers by value, the cold
/// start's sources, the channel tuned.
pub(crate) struct Brought {
    pub(crate) slot: Option<Radio>,
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

/// Bring Wi-Fi up in the radio system's order: the PHY's registration,
/// coexistence, power, the clocks (and the first reset's proof), the PHY
/// client, the tracking due, RX enable and the channel.
pub(crate) async fn bring_up(
    parts: SessionParts,
    timer: &CooperativeTimer,
    platform: &mut (),
) -> Result<Brought, Failure> {
    let SessionParts {
        radio,
        clocks,
        wifi,
        coex,
        mut pilot,
        identity,
        mac_irq,
        power_irq,
        seed,
        station,
        ap,
    } = parts;
    let mut slot = Some(radio);
    apart!(register(&mut slot, timer, platform, &clocks, identity))?;
    let radio = held(&mut slot)?;
    let (coexisting, _participants) = radio.enter_coex(coex);
    let (cold, walker) = WifiCold::from_partition(wifi);
    let (powered, power) = cold
        .power_up(radio, &clocks)
        .map_err(|_| (phase::POWER, 0))?;
    let (mut clocked, first) = powered
        .enable_clocks(radio, &clocks)
        .map_err(|_| (phase::CLOCKS, 0))?;
    clocked
        .set_initialized(radio, true)
        .map_err(|_| (phase::INITIALIZED, 0))?;
    let (membership, acquired) = {
        let ready = phy_ready(radio).map_err(|_| (phase::JOIN, 0))?;
        let PhyClientAccess::Available(available) = ready.client::<WifiPhyMembership>() else {
            return Err((phase::JOIN, 1));
        };
        let joined = join_wifi(available, &clocked, timer).map_err(|_| (phase::JOIN, 2))?;
        (joined.membership, joined.acquired)
    };
    if acquired == ConcurrentAcquire::TrackingDue {
        apart!(maintain(&mut slot, timer, platform, &mut pilot))?;
    }
    set_wifi_rx(held(&mut slot)?, &membership, true).map_err(|_| (phase::RX_ENABLE, 0))?;
    let channel =
        Ghz2_4Channel::new(CHANNEL, ChannelWidth::Mhz20).map_err(|_| (phase::CHANNEL, 0))?;
    apart!(tune(&mut slot, &mut clocked, timer, platform, channel))?;
    let radio = held(&mut slot)?;
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
    let tx_power = radio
        .attachment()
        .phy_state()
        .map_err(|_| (phase::POWER_PROFILE, 0))?
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
        slot,
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
            _membership: membership,
            _pilot: pilot,
            _clocks: clocks,
        },
    })
}

/// The cold preparation (S5) with the walker's registers lent, then the
/// channel (S6): the preparation's proof, which the core's opening takes.
/// A failed preparation gives none.
#[allow(
    clippy::too_many_arguments,
    reason = "each is one owner the preparation borrows"
)]
pub(crate) async fn prepare(
    slot: &mut Option<Radio>,
    clocked: &mut WifiClocked,
    walker: &mut WifiRxDmaRegisters,
    sources: &mut Sources,
    reset: MacBlockReset,
    mac: MacColdStartConfig,
    channel: Ghz2_4Channel,
    timer: &CooperativeTimer,
    platform: &mut (),
) -> Result<MacPrepared, Failure> {
    let (_outcome, prepared) = {
        let mut hal = clocked.cold_mac_hal(walker);
        initialize_wifi_mac(reset, sources, &mut hal, timer, mac)
            .await
            .map_err(|_| (phase::MAC_COLD, 0))?
    };
    apart!(tune(slot, clocked, timer, platform, channel))?;
    Ok(prepared)
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
) -> (
    MacBlockReset,
    RxRingParts<B, BYTES, N, WifiRxDmaRegisters>,
    &'p mut MaybeUninit<RxRing<B, BYTES, N, WifiRxDmaRegisters>>,
)
where
    B: Brand<Window = DmaWindow>,
{
    let edge = ring.stop_walker(enabled);
    let requested = access.wifi_mac_hal().request_stop(edge, &EmbassyClock);
    let (reset, stopped) = radio.reset_stopped_wifi_mac_block(clocks_on, requested, CBW);
    let (parts, place) = RxRing::release(ring, stopped);
    (reset, parts, place)
}
