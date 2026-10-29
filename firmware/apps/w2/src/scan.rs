//! One radio-executor task consumes every RX/TX interrupt batch and keeps
//! both DMA owners until their stopped boundaries, before posting a retune.
use crate::Rng;
use crate::{
    W2Config, irq,
    irq::{B, N, Q, S, TX},
    report::Report,
};
use core::{future::poll_fn, pin::Pin, sync::atomic::Ordering};
use embassy_futures::select::{Either3, select3};
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
use oer_espressif_interrupt_table::Bound;
use oer_ieee80211_port::Band;
use oer_ieee80211_upper_mac::protection::BasicRates;
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
    oer_esp32s31_ieee80211_driver::mac::{
        irq::{EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT, IrqWork},
        tx::{TxPhyRate, TxSlot, runtime::WifiTxRuntimePolicy},
    },
};
use {
    oer_esp32s31_ieee80211_driver::ordinary_tx::MacTxPlan,
    oer_ieee80211_wire::address::IndividualAddress,
};

type Rx = ReceiveSession<N, B, S, Q, crate::irq::RadioExecutor, Bound<crate::WifiToken>>;
type Frames = FrameReceiver<'static, Q, B, crate::irq::RadioExecutor>;
type Publisher = QueueSink<'static, EmbassyClock, Q, B>;
type Tx = OrdinaryTxOwner<
    'static,
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
    pub rng: Rng,
    pub station: [u8; 6],
    pub ap: [u8; 6],
    pub config: W2Config,
    pub bound: Bound<crate::WifiToken>,
    pub bell: irq::ImageDoorbell,
    pub sender: AffineSpscSender<'static, Report, 1>,
}
struct Platform {
    rng: Option<Rng>,
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
struct Entropy(Rng);
impl WifiTxEntropy for Entropy {
    fn next_u32(&mut self) -> u32 {
        self.0.random()
    }
}

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
        boot.bound,
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

// Keep ownership-transfer temporaries out of the scanner's poll frame.
#[inline(never)]
fn retuned_tx(
    tx: &mut Option<Tx>,
    profile: PhyTxTargetPowerProfile,
    limit: TxCeiling,
    busy: oer_diagnostics::Name<32>,
) -> Result<(), oer_diagnostics::Name<32>> {
    let previous = tx
        .take()
        .expect("the scanner retains its transmitter between retunes");
    let mut resources = previous.try_into_resources().map_err(|_| busy)?;
    resources.power = CappedTxPowerProfile::new(profile, limit);
    *tx = Some(OrdinaryTxOwner::new_legacy(resources));
    Ok(())
}

fn ceiling(channel: oer_radio_regulatory::Permitted<WifiChannel>, config: W2Config) -> TxCeiling {
    channel.tx_ceiling(board::wifi_power_limits(
        i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
        Some(i32::from(config.cap_dbm) * 100),
    ))
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
async fn run(
    mut cell: MembershipCell<Wifi>,
    wifi: WifiPartition,
    rng: Rng,
    station: [u8; 6],
    ap: [u8; 6],
    config: W2Config,
    bound: Bound<crate::WifiToken>,
    bell: irq::ImageDoorbell,
    report: &mut Report,
) -> Result<(), oer_diagnostics::Name<32>> {
    let domain = RegulatoryDomain::of(
        Country::new(config.country).map_err(|_| oer_diagnostics::name!("country"; 32))?,
    )
    .ok_or(oer_diagnostics::name!("domain"; 32))?;
    let initial = domain
        .permit(
            WifiChannel::new_2_4_ghz(config.channel, WifiChannelWidth::Mhz20)
                .map_err(|_| oer_diagnostics::name!("channel"; 32))?,
        )
        .map_err(|_| oer_diagnostics::name!("lab-channel-refused"; 32))?;
    let start = RadioStartConfig::new(
        WifiColdStartConfig::new(initial, ceiling(initial, config)),
        WifiMacStartConfig::new(
            MAC_COLD_HANDSHAKE_DEADLINE,
            IndividualAddress::for_interface(station)
                .map_err(|_| oer_diagnostics::name!("station-mac"; 32))?,
            IndividualAddress::for_interface(ap)
                .map_err(|_| oer_diagnostics::name!("ap-mac"; 32))?,
        )
        .map_err(|_| oer_diagnostics::name!("mac-address"; 32))?,
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
    .await
    .unwrap_or_else(
        oer_espressif_staged_runtime::panic::reset_request::<oer_esp32s31_hal::platform::Port, _>,
    );
    cell = returned;
    let stopped = started.map_err(|_| oer_diagnostics::name!("bringup"; 32))?;
    let profile = stopped.start_report().track.tx_power;

    let WifiRoleMaterialized {
        owner,
        mut registers,
        ..
    } = materialize_esp32s31_wifi_role(stopped, ());
    let (mut platform, context) = owner.into_runtime_parts();
    let rng = platform
        .rng
        .take()
        .ok_or(oer_diagnostics::name!("entropy"; 32))?;
    let mut owner = WifiRoleOwner::from_runtime_parts(platform, context);
    let arena = irq::RX
        .take()
        .ok_or(oer_diagnostics::name!("rx-claimed"; 32))?;
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
    .map_err(|_| oer_diagnostics::name!("rx-bind"; 32))?;
    let mut session = Rx::new(parts, bound, &mut registers, &irq::LINE)
        .map_err(|_| oer_diagnostics::name!("rx-session"; 32))?;
    let publisher = QueueSink::new(sender, &CLOCK);
    let dma = TxDmaStorage::pin::<ChipDma>(
        irq::TX_DMA
            .take()
            .ok_or(oer_diagnostics::name!("tx-claimed"; 32))?,
    )
    .map_err(|_| oer_diagnostics::name!("tx-bind"; 32))?;
    let slot = irq::TX_SLOT.init(TxSlot::from_dma(dma));
    let mut tx = Some(OrdinaryTxOwner::new_legacy(WifiTxResources {
        slot: Pin::new(slot),
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: CappedTxPowerProfile::new(profile, ceiling(initial, config)),
        entropy: Entropy(rng),
        timer: EmbassyClock,
    }));
    let mut sequence = SequenceNumber::ZERO;
    let mut basic = None;
    session
        .start(&mut registers, &CLOCK, &bell)
        .await
        .map_err(|_| oer_diagnostics::name!("rx-start"; 32))?;
    for number in 1..=13 {
        let channel = WifiChannel::new_2_4_ghz(number, WifiChannelWidth::Mhz20)
            .map_err(|_| oer_diagnostics::name!("channel"; 32))?;
        let Ok(channel) = domain.permit(channel) else {
            continue;
        };
        let limit = ceiling(channel, config);
        session
            .pause(&mut registers, &bell, &CLOCK, &publisher)
            .await
            .map_err(|_| oer_diagnostics::name!("rx-pause"; 32))?;
        let stopped = owner.into_stopped(registers, ()).wifi;
        let (returned, tuned) =
            stopped.retune(cell, channel, limit).await.unwrap_or_else(
                oer_espressif_staged_runtime::panic::reset_request::<
                    oer_esp32s31_hal::platform::Port,
                    _,
                >,
            );
        cell = returned;
        let (stopped, tuned) = tuned.map_err(|_| oer_diagnostics::name!("retune"; 32))?;
        let parts = materialize_esp32s31_wifi_role(stopped, ());
        owner = parts.owner;
        registers = parts.registers;
        session.reattach();
        retuned_tx(
            &mut tx,
            tuned.tx_power,
            limit,
            oer_diagnostics::name!("tx-active-on-retune"; 32),
        )?;
        let priority = tuned.probe_priority;
        session
            .start(&mut registers, &CLOCK, &bell)
            .await
            .map_err(|_| oer_diagnostics::name!("rx-restart"; 32))?;
        let index = report.count;
        report.count += 1;
        let rate =
            management_rate(Band::Ghz2_4, None).ok_or(oer_diagnostics::name!("group-rate"; 32))?;
        report.visits[index].channel = number;
        report.visits[index].sequence = sequence.get();
        report.visits[index].allowed = channel.initiate() == Initiate::Allowed;
        report.visits[index].rate_kbps = rate.kbps();
        report.visits[index].retune_us = tuned.micros;
        report.visits[index].ceiling = limit.dbm();
        if channel.initiate() == Initiate::Allowed {
            begin_probe(
                tx.as_mut()
                    .expect("the scanner retains its transmitter between retunes"),
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
            tx.as_mut()
                .expect("the scanner retains its transmitter between retunes"),
            &mut receiver,
            &publisher,
            &bell,
            config,
            station,
            &mut basic,
            report,
            Some(index),
            after(DWELL),
        )
        .await?;
        if let Some(outcome) = tx
            .as_mut()
            .expect("the scanner retains its transmitter between retunes")
            .take_last_outcome()
        {
            report.visits[index].success = outcome.is_success();
        }
    }
    // Return to the lab channel with both DMA owners idle, then solicit an ACK.
    session
        .pause(&mut registers, &bell, &CLOCK, &publisher)
        .await
        .map_err(|_| oer_diagnostics::name!("rx-pause-unicast"; 32))?;
    let stopped = owner.into_stopped(registers, ()).wifi;
    let limit = ceiling(initial, config);
    let (returned, tuned) = stopped.retune(cell, initial, limit).await.unwrap_or_else(
        oer_espressif_staged_runtime::panic::reset_request::<oer_esp32s31_hal::platform::Port, _>,
    );
    cell = returned;
    let (stopped, tuned) = tuned.map_err(|_| oer_diagnostics::name!("retune-unicast"; 32))?;
    let parts = materialize_esp32s31_wifi_role(stopped, ());
    owner = parts.owner;
    registers = parts.registers;
    session.reattach();
    retuned_tx(
        &mut tx,
        tuned.tx_power,
        limit,
        oer_diagnostics::name!("tx-active-before-unicast"; 32),
    )?;
    session
        .start(&mut registers, &CLOCK, &bell)
        .await
        .map_err(|_| oer_diagnostics::name!("rx-start-unicast"; 32))?;
    let rate = management_rate(
        Band::Ghz2_4,
        Some(&basic.ok_or(oer_diagnostics::name!("lab-basic-rates"; 32))?),
    )
    .ok_or(oer_diagnostics::name!("unicast-rate"; 32))?;
    report.unicast_rate = rate.kbps();
    report.unicast_sequence = sequence.get();
    report.unicast_ceiling = limit.dbm();
    begin_probe(
        tx.as_mut()
            .expect("the scanner retains its transmitter between retunes"),
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
        tx.as_mut()
            .expect("the scanner retains its transmitter between retunes"),
        &mut receiver,
        &publisher,
        &bell,
        config,
        station,
        &mut basic,
        report,
        None,
        after(DWELL),
    )
    .await?;
    if let Some(outcome) = tx
        .as_mut()
        .expect("the scanner retains its transmitter between retunes")
        .take_last_outcome()
    {
        report.unicast_success = outcome.is_success();
        report.unicast_attempts = 1 + outcome.report().retries.total();
        report.unicast_raw = outcome.report().completion.map_or(255, |c| c.status());
    }
    report.errors += session.counters().report().service_errors;
    session
        .pause(&mut registers, &bell, &CLOCK, &publisher)
        .await
        .map_err(|_| oer_diagnostics::name!("rx-final-stop"; 32))?;
    let stopped = owner.into_stopped(registers, ()).wifi;
    let (_cell, released) = stopped.release(cell).await.unwrap_or_else(
        oer_espressif_staged_runtime::panic::reset_request::<oer_esp32s31_hal::platform::Port, _>,
    );
    released.map_err(|_| oer_diagnostics::name!("release"; 32))?;
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
) -> Result<(), oer_diagnostics::Name<32>> {
    if channel.initiate() != Initiate::Allowed {
        return Err(oer_diagnostics::name!("probe-no-ir"; 32));
    }
    let management = if destination == BROADCAST_ADDRESS {
        oer_ieee80211_upper_mac::management::broadcast(rate)
    } else {
        oer_ieee80211_upper_mac::management::unicast(rate)
    };
    let OrdinaryTxAccess::Idle(mut idle) = tx.access() else {
        return Err(oer_diagnostics::name!("tx-not-idle"; 32));
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
            .ok_or(oer_diagnostics::name!("tx-buffer"; 32))?,
    )
    .map_err(|_| oer_diagnostics::name!("probe-encode"; 32))?;
    let plan = OrdinaryTxPlan {
        frame_length: length,
        descriptor_capacity: None,
        exchange: MacTxPlan {
            access_category: WmmAccessCategory::Voice,
            initial_rate: TxPhyRate::Legacy(oer_esp32s31_ieee80211_driver::mac::portable::legacy_from_phy_rate(rate)),
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
        .map_err(|_| oer_diagnostics::name!("tx-start"; 32))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    session: &mut Rx,
    registers: &mut RadioAccess,
    tx: &mut Tx,
    frames: &mut Frames,
    publisher: &Publisher,
    bell: &irq::ImageDoorbell,
    config: W2Config,
    station: [u8; 6],
    basic: &mut Option<BasicRates>,
    report: &mut Report,
    visit: Option<usize>,
    deadline: Instant,
) -> Result<(), oer_diagnostics::Name<32>> {
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
                let batch = batch.map_err(|_| oer_diagnostics::name!("irq-context"; 32))?;
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
                            .map_err(|_| oer_diagnostics::name!("tx-irq"; 32))?;
                    }
                }
                if rx {
                    session
                        .drain(registers, bell, &CLOCK, publisher)
                        .await
                        .map_err(|_| oer_diagnostics::name!("rx-drain"; 32))?;
                }
            }
            Either3::Third(()) => {
                if tx.next_deadline().is_some_and(|d| CLOCK.now() >= d)
                    && let OrdinaryTxAccess::Active(active) = tx.access()
                {
                    active
                        .service(&mut registers.wifi_mac_hal(), WifiTxWake::Deadline)
                        .map_err(|_| oer_diagnostics::name!("tx-deadline"; 32))?;
                }
            }
        }
    }
}

fn observe(
    frames: &Frames,
    held: &oer_esp32s31_ieee80211_driver::rx::returns::HeldRxFrame<
        'static,
        'static,
        Q,
        B,
        crate::irq::RadioExecutor,
    >,
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
