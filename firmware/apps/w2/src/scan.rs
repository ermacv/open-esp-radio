//! One radio-executor task consumes every RX/TX interrupt batch and keeps
//! both DMA owners until their stopped boundaries, before posting a retune.
use crate::{
    W2Config, irq,
    irq::{B, N, Q, S, TX},
    report::Report,
};
use core::{future::poll_fn, pin::Pin, sync::atomic::Ordering};
use embassy_futures::select::{Either3, select3};
use esp_hal::rng::RngOwner;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::{
    ieee80211::dma::{space::ChipDma, tx_storage::TxDmaStorage},
    owner::RadioAccess,
    root::WifiPartition,
};
use oer_esp32s31_ieee80211_driver::{
    ordinary_tx::{
        OrdinaryTxAccess, OrdinaryTxInterface, OrdinaryTxOwner, OrdinaryTxPlan, TX_METADATA_SIZE,
    },
    runtime::{WifiRoleMaterialized, WifiRoleOwner, materialize_esp32s31_wifi_role},
    rx::{
        arena::WifiIrqSignals,
        session::ReceiveSession,
        upper::{FrameReceiver, QueueSink, RxFrameEnds},
    },
    startup::{
        RadioStartConfig, WifiColdStartConfig, WifiMacPlatform, WifiMacStartConfig,
        start_esp32s31_radio,
    },
    tx::{CappedTxPowerProfile, WifiTxEntropy, WifiTxResources, WifiTxWake},
};
use oer_esp32s31_phy::{PhyConfig, PhyTxTargetPowerProfile};
use oer_esp32s31_radio_client::{
    MembershipCell,
    wifi::{ProbePriority, Wifi},
};
use oer_ieee80211_port::backend_protection::BasicRates;
use oer_ieee80211_port::{Band, ManagementTxPlan};
use oer_ieee80211_upper_mac::rate_selection::management_rate;
use oer_ieee80211_wire::{
    channel::{WifiChannel, WifiChannelWidth},
    management::{BROADCAST_ADDRESS, ProbeRequest},
    qos::WmmAccessCategory,
    scan::parse_management,
    sequence::SequenceNumber,
};
use oer_memory::AffineSpscSender;
use oer_radio_regulatory::{Country, Initiate, RegulatoryDomain, TxCeiling};
use oer_time::{Clock, Duration, Instant, Timer};
use {
    oer_esp32s31_hal::ieee80211::init::{
        MAC_COLD_HANDSHAKE_DEADLINE, MacDelayEntropy, MacSlowClockCalibration,
        MacSlowClockCalibrationSource,
    },
    oer_esp32s31_ieee80211_mac::{
        irq::{EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT, IrqWork},
        tx::{LegacyRate, TxPhyRate, TxSlot, runtime::WifiTxRuntimePolicy},
    },
};
use {oer_ieee80211_port::MacTxPlan, oer_ieee80211_wire::address::IndividualAddress};

type Rx = ReceiveSession<N, B, S, Q>;
type Frames = FrameReceiver<'static, Q, B>;
type Publisher = QueueSink<'static, EmbassyClock, Q, B>;
type Tx = OrdinaryTxOwner<
    'static,
    oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile,
    CappedTxPowerProfile<PhyTxTargetPowerProfile>,
    Entropy,
    EmbassyClock,
    TX,
    true,
>;
static CLOCK: EmbassyClock = EmbassyClock;
const DWELL: Duration = Duration::from_millis(150);
const TX_TIMEOUT: Duration = Duration::from_millis(250);
const RATES: [u8; 12] = [2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108];

pub struct Boot {
    pub cell: MembershipCell<Wifi>,
    pub wifi: WifiPartition,
    pub rng: RngOwner<'static>,
    pub station: [u8; 6],
    pub ap: [u8; 6],
    pub config: W2Config,
    pub routes: irq::ImageRoutes,
    pub bell: irq::ImageDoorbell,
    pub sender: AffineSpscSender<'static, Report, 1>,
}
struct Platform {
    rng: Option<RngOwner<'static>>,
    tx_power: Option<PhyTxTargetPowerProfile>,
}
impl MacDelayEntropy for Platform {
    fn mac_delay_random(&mut self) -> u32 {
        self.rng
            .as_mut()
            .expect("cold init retains entropy")
            .random()
    }
}
impl MacSlowClockCalibrationSource for Platform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        MacSlowClockCalibration::Unavailable
    }
}
impl WifiMacPlatform for Platform {
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile) {
        self.tx_power = Some(profile);
    }
}
struct Entropy(RngOwner<'static>);
impl WifiTxEntropy for Entropy {
    fn next_u32(&mut self) -> u32 {
        self.0.random()
    }
}

#[embassy_executor::task]
pub async fn scanner(boot: Boot) {
    let mut report = Report::new(boot.config, boot.station);
    let sender = boot.sender;
    if let Err(error) = run(
        boot.cell,
        boot.wifi,
        boot.rng,
        boot.station,
        boot.ap,
        boot.config,
        boot.routes,
        boot.bell,
        &mut report,
    )
    .await
    {
        report.error = Some(error);
    }
    report.errors += irq::ERRORS.load(Ordering::Relaxed);
    let _ = sender.try_send(report);
    core::future::pending::<()>().await;
}

fn ceiling(channel: oer_radio_regulatory::Permitted<WifiChannel>, config: W2Config) -> TxCeiling {
    channel.tx_ceiling(board::wifi_power_limits(
        i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
        Some(i32::from(config.cap_dbm) * 100),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn run(
    mut cell: MembershipCell<Wifi>,
    wifi: WifiPartition,
    rng: RngOwner<'static>,
    station: [u8; 6],
    ap: [u8; 6],
    config: W2Config,
    mut routes: irq::ImageRoutes,
    bell: irq::ImageDoorbell,
    report: &mut Report,
) -> Result<(), &'static str> {
    let domain = RegulatoryDomain::of(Country::new(config.country).map_err(|_| "country")?)
        .ok_or("domain")?;
    let initial = domain
        .permit(
            WifiChannel::new_2_4_ghz(config.channel, WifiChannelWidth::Mhz20)
                .map_err(|_| "channel")?,
        )
        .map_err(|_| "lab-channel-refused")?;
    let start = RadioStartConfig::new(
        WifiColdStartConfig::new(initial, ceiling(initial, config)),
        WifiMacStartConfig::new(
            MAC_COLD_HANDSHAKE_DEADLINE,
            IndividualAddress::for_interface(station).map_err(|_| "station-mac")?,
            IndividualAddress::for_interface(ap).map_err(|_| "ap-mac")?,
        )
        .map_err(|_| "mac-address")?,
    );
    let (returned, started) = start_esp32s31_radio(
        cell,
        wifi,
        Platform {
            rng: Some(rng),
            tx_power: None,
        },
        start,
    )
    .await;
    cell = returned;
    let stopped = started.map_err(|_| "bringup")?;
    let profile = stopped.start_report().track.tx_power;

    let WifiRoleMaterialized {
        owner,
        mut registers,
        interrupt_setup,
        ..
    } = materialize_esp32s31_wifi_role(stopped, ());
    let (mut platform, context) = owner.into_runtime_parts();
    let rng = platform.rng.take().ok_or("entropy")?;
    let mut owner = WifiRoleOwner::from_runtime_parts(platform, context);
    let arena = irq::RX.take().ok_or("rx-claimed")?;
    let (
        parts,
        _stop,
        RxFrameEnds {
            sender,
            mut receiver,
        },
    ) = irq::Arena::bind(
        arena,
        WifiIrqSignals {
            mac: &irq::MAC_SIGNAL,
            power: &irq::POWER_SIGNAL,
        },
    )
    .map_err(|_| "rx-bind")?;
    let mut session =
        Rx::new(parts, interrupt_setup, &mut registers, &irq::LINE).map_err(|_| "rx-session")?;
    let publisher = QueueSink::new(sender, &CLOCK);
    let dma = TxDmaStorage::pin::<ChipDma>(irq::TX_DMA.take().ok_or("tx-claimed")?)
        .map_err(|_| "tx-bind")?;
    let slot = irq::TX_SLOT.init(TxSlot::from_dma(dma));
    let mut tx = OrdinaryTxOwner::new_legacy(WifiTxResources {
        slot: Pin::new(slot),
        policy: WifiTxRuntimePolicy::<
            oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile,
        >::vendor_defaults(),
        power: CappedTxPowerProfile::new(profile, ceiling(initial, config)),
        entropy: Entropy(rng),
        timer: EmbassyClock,
    });
    let mut sequence = SequenceNumber::ZERO;
    let mut basic = None;
    session
        .start(&mut registers, &CLOCK, &mut routes)
        .await
        .map_err(|_| "rx-start")?;
    for number in 1..=13 {
        let channel =
            WifiChannel::new_2_4_ghz(number, WifiChannelWidth::Mhz20).map_err(|_| "channel")?;
        let Ok(channel) = domain.permit(channel) else {
            continue;
        };
        let limit = ceiling(channel, config);
        let setup = session
            .pause(&mut registers, &mut routes, &bell, &CLOCK, &publisher)
            .await
            .map_err(|_| "rx-pause")?;
        let stopped = owner.into_stopped(registers, setup, ()).wifi;
        let (returned, tuned) = stopped.retune(cell, channel, limit).await;
        cell = returned;
        let (stopped, tuned) = tuned.map_err(|_| "retune")?;
        let parts = materialize_esp32s31_wifi_role(stopped, ());
        owner = parts.owner;
        registers = parts.registers;
        session
            .reattach(parts.interrupt_setup)
            .map_err(|_| "irq-reattach")?;
        let mut resources = tx.try_into_resources().map_err(|_| "tx-active-on-retune")?;
        resources.power = CappedTxPowerProfile::new(tuned.tx_power, limit);
        tx = OrdinaryTxOwner::new_legacy(resources);
        let priority = tuned.probe_priority;
        session
            .start(&mut registers, &CLOCK, &mut routes)
            .await
            .map_err(|_| "rx-restart")?;
        let index = report.count;
        report.count += 1;
        let rate = management_rate(Band::Ghz2_4, None).ok_or("group-rate")?;
        report.visits[index].channel = number;
        report.visits[index].sequence = sequence.get();
        report.visits[index].allowed = channel.initiate() == Initiate::Allowed;
        report.visits[index].rate_kbps = rate.kbps();
        report.visits[index].retune_us = tuned.micros;
        report.visits[index].ceiling = limit.dbm();
        if channel.initiate() == Initiate::Allowed {
            begin_probe(
                &mut tx,
                &mut registers,
                channel,
                config,
                station,
                BROADCAST_ADDRESS,
                rate,
                sequence,
                priority,
            )?;
            sequence = sequence.next();
            report.visits[index].sent = 1;
            report.visits[index].power = registers
                .wifi_mac_hal()
                .ordinary_tx_queue_snapshot(0)
                .power_codes;
        }
        drive(
            &mut session,
            &mut registers,
            &mut tx,
            &mut receiver,
            &publisher,
            &mut routes,
            &bell,
            config,
            station,
            &mut basic,
            report,
            Some(index),
            after(DWELL),
        )
        .await?;
        if let Some(outcome) = tx.take_last_outcome() {
            report.visits[index].success = outcome.is_success();
        }
    }
    // Return to the lab channel with both DMA owners idle, then solicit an ACK.
    let setup = session
        .pause(&mut registers, &mut routes, &bell, &CLOCK, &publisher)
        .await
        .map_err(|_| "rx-pause-unicast")?;
    let stopped = owner.into_stopped(registers, setup, ()).wifi;
    let limit = ceiling(initial, config);
    let (returned, tuned) = stopped.retune(cell, initial, limit).await;
    cell = returned;
    let (stopped, tuned) = tuned.map_err(|_| "retune-unicast")?;
    let parts = materialize_esp32s31_wifi_role(stopped, ());
    owner = parts.owner;
    registers = parts.registers;
    session
        .reattach(parts.interrupt_setup)
        .map_err(|_| "irq-reattach-unicast")?;
    let mut resources = tx
        .try_into_resources()
        .map_err(|_| "tx-active-before-unicast")?;
    resources.power = CappedTxPowerProfile::new(tuned.tx_power, limit);
    tx = OrdinaryTxOwner::new_legacy(resources);
    session
        .start(&mut registers, &CLOCK, &mut routes)
        .await
        .map_err(|_| "rx-start-unicast")?;
    let rate = management_rate(Band::Ghz2_4, Some(&basic.ok_or("lab-basic-rates")?))
        .ok_or("unicast-rate")?;
    report.unicast_rate = rate.kbps();
    report.unicast_sequence = sequence.get();
    report.unicast_ceiling = limit.dbm();
    begin_probe(
        &mut tx,
        &mut registers,
        initial,
        config,
        station,
        config.stand,
        rate,
        sequence,
        tuned.probe_priority,
    )?;
    report.unicast_power = registers
        .wifi_mac_hal()
        .ordinary_tx_queue_snapshot(0)
        .power_codes;
    drive(
        &mut session,
        &mut registers,
        &mut tx,
        &mut receiver,
        &publisher,
        &mut routes,
        &bell,
        config,
        station,
        &mut basic,
        report,
        None,
        after(DWELL),
    )
    .await?;
    if let Some(outcome) = tx.take_last_outcome() {
        report.unicast_success = outcome.is_success();
        report.unicast_attempts = 1 + outcome.report().retries.total();
        report.unicast_raw = outcome.report().completion.map_or(255, |c| c.status());
    }
    report.errors += session.counters().report().service_errors;
    let setup = session
        .pause(&mut registers, &mut routes, &bell, &CLOCK, &publisher)
        .await
        .map_err(|_| "rx-final-stop")?;
    let stopped = owner.into_stopped(registers, setup, ()).wifi;
    let (_cell, released) = stopped.release(cell).await;
    released.map_err(|_| "release")?;
    Ok(())
}

fn after(duration: Duration) -> Instant {
    CLOCK.now().saturating_add(duration)
}
#[allow(clippy::too_many_arguments)]
fn begin_probe(
    tx: &mut Tx,
    registers: &mut RadioAccess,
    channel: oer_radio_regulatory::Permitted<WifiChannel>,
    config: W2Config,
    station: [u8; 6],
    destination: [u8; 6],
    rate: oer_ieee80211_wire::phy::LegacyRate,
    sequence: SequenceNumber,
    priority: ProbePriority,
) -> Result<(), &'static str> {
    if channel.initiate() != Initiate::Allowed {
        return Err("probe-no-ir");
    }
    let management = if destination == BROADCAST_ADDRESS {
        ManagementTxPlan::broadcast(rate)
    } else {
        ManagementTxPlan::unicast(rate)
    };
    let OrdinaryTxAccess::Idle(mut idle) = tx.access() else {
        return Err("tx-not-idle");
    };
    let length = ProbeRequest {
        destination,
        source: station,
        bssid: destination,
        sequence_number: sequence,
        ssid: config.ssid,
        supported_rates: &RATES,
    }
    .encode(
        idle.buffer_mut()
            .get_mut(TX_METADATA_SIZE..)
            .ok_or("tx-buffer")?,
    )
    .map_err(|_| "probe-encode")?;
    let plan = OrdinaryTxPlan {
        frame_length: length,
        descriptor_capacity: None,
        exchange: MacTxPlan {
            access_category: WmmAccessCategory::Voice,
            initial_rate: TxPhyRate::Legacy(LegacyRate::from_phy_rate(rate)),
            publication_limit: management.attempts(),
            publication_timeout: TX_TIMEOUT,
        },
        hardware_mic_length: 0,
        hardware_key_selector: 0,
        interface: OrdinaryTxInterface::Station,
        scheduler_priority: priority.scheduler.value(),
        packet_priority: priority.packet.value(),
        priority_count: 1,
    };
    idle.start_management(&mut registers.wifi_mac_hal(), plan, management)
        .map_err(|_| "tx-start")?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    session: &mut Rx,
    registers: &mut RadioAccess,
    tx: &mut Tx,
    frames: &mut Frames,
    publisher: &Publisher,
    routes: &mut irq::ImageRoutes,
    bell: &irq::ImageDoorbell,
    config: W2Config,
    station: [u8; 6],
    basic: &mut Option<BasicRates>,
    report: &mut Report,
    visit: Option<usize>,
    deadline: Instant,
) -> Result<(), &'static str> {
    loop {
        if CLOCK.now() >= deadline && !tx.active() {
            while let Some(held) = frames.try_next() {
                observe(frames, &held, config, station, basic, report, visit);
                drop(held);
            }
            return Ok(());
        }
        let wake = tx.next_deadline().map_or(deadline, |next| {
            if CLOCK.now() >= deadline {
                next
            } else {
                next.min(deadline)
            }
        });
        match select3(
            frames.next(),
            poll_fn(|cx| session.poll(cx)),
            CLOCK.wait_until(wake),
        )
        .await
        {
            Either3::First(held) => {
                observe(frames, &held, config, station, basic, report, visit);
                drop(held);
            }
            Either3::Second(batch) => {
                let batch = batch.map_err(|_| "irq-context")?;
                let mut rx = batch.rx_capacity_returned();
                let mut events = 0;
                for work in batch {
                    match work {
                        IrqWork::RxSuccess => rx = true,
                        IrqWork::TxComplete => events |= EVENT_TX_COMPLETE,
                        IrqWork::TxTimeout => {
                            report.timeouts += 1;
                            events |= EVENT_TX_TIMEOUT;
                        }
                        IrqWork::BssColorCollision => report.colors += 1,
                    }
                }
                if tx.active() && events != 0 {
                    if registers
                        .wifi_mac_hal()
                        .ordinary_tx_queue_snapshot(0)
                        .collision_pending
                    {
                        report.collisions += 1;
                    }
                    if let OrdinaryTxAccess::Active(active) = tx.access() {
                        active
                            .service(
                                &mut registers.wifi_mac_hal(),
                                WifiTxWake::Interrupt { events },
                            )
                            .map_err(|_| "tx-irq")?;
                    }
                }
                if rx {
                    session
                        .drain(registers, routes, bell, &CLOCK, publisher)
                        .await
                        .map_err(|_| "rx-drain")?;
                }
            }
            Either3::Third(()) => {
                if tx.next_deadline().is_some_and(|d| CLOCK.now() >= d)
                    && let OrdinaryTxAccess::Active(active) = tx.access()
                {
                    active
                        .service(&mut registers.wifi_mac_hal(), WifiTxWake::Deadline)
                        .map_err(|_| "tx-deadline")?;
                }
            }
        }
    }
}

fn observe(
    frames: &Frames,
    held: &oer_esp32s31_ieee80211_driver::rx::returns::HeldRxFrame<'static, 'static, Q, B>,
    config: W2Config,
    station: [u8; 6],
    basic: &mut Option<BasicRates>,
    report: &mut Report,
    visit: Option<usize>,
) {
    if let Ok(frame) = frames.normalize(held) {
        let channel = visit.map_or(config.channel, |i| report.visits[i].channel);
        if let Some(record) = parse_management(frame.mpdu, channel, frame.rssi_dbm)
            && record.bssid.get() == config.stand
            && record.ssid_bytes() == config.ssid
            && record.channel == config.channel
        {
            report.supported_len = record.supported_rates_bytes().len();
            report.supported[..report.supported_len]
                .copy_from_slice(record.supported_rates_bytes());
            report.extended_len = record.extended_supported_rates_bytes().len();
            report.extended[..report.extended_len]
                .copy_from_slice(record.extended_supported_rates_bytes());
            *basic = Some(BasicRates::from_rate_elements(
                record.supported_rates_bytes(),
                record.extended_supported_rates_bytes(),
            ));
            if frame.mpdu.first().is_some_and(|v| v >> 4 == 5)
                && frame.mpdu.get(4..10) == Some(station.as_slice())
            {
                if let Some(i) = visit {
                    report.visits[i].responses += 1;
                } else {
                    report.unicast_responses += 1;
                }
            }
        }
    }
}
