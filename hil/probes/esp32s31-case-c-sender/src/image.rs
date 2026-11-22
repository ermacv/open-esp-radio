//! The sender's boot and its radio session: Wi-Fi brought up over the radio
//! domain the probe holds (`radio`), the MAC started on the receive ring's
//! atoms over a small ring of its own, then each step the host's program
//! (`hil/scenarios/radio-pair`) asks for over the HIL protocol: the
//! receiver's address the host names (`target`), then case C's rounds
//! (`amsdu`) or cell E's (`ccmp`).
use crate::pair;
use crate::radio::{
    Brought, Cooperative, CooperativeTimer, Failure, SessionParts, apart, begin, bring_up,
    held as held_radio, phase, prepare, stop,
};
use crate::usb;
use core::mem::MaybeUninit;
use oer_chips_esp32s31_hal::{
    layouts::DmaWindow,
    peripherals::usb_serial_jtag::ConsoleInterrupt,
    peripherals::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
    peripherals::{
        efuse::{FactoryIdentity, Interface},
        software_interrupt::{FromCpu0Irq, FromCpu2Irq},
        system_timer::{SystimerAlarm0, SystimerAlarm1},
    },
    power::ChipClocks,
    radio::ieee80211::{
        dma::{
            receive::{RxDescriptors, RxRing, RxRingParts, RxTable},
            space::ChipDma,
        },
        registers::WifiRxDmaRegisters,
    },
    system::owner::RadioAccess,
    system::root::Root,
};
use oer_chips_esp32s31_platform_board as board;
use oer_chips_esp32s31_platform_runtime::Esp32s31;
use oer_chips_esp32s31_radio_phy::{
    PhyCalibrationIdentity, concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_chips_family_platform_executor::{Executor, InterruptExecutor};
use oer_hil_agent::answer;
use oer_hil_services_radio_pair::{self as service, Refusal, SenderRequest};
use oer_portable_diagnostics::{Name, name};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_memory::arena::{Arena, Handle};
use oer_portable_time::{Clock, Duration, Timer};
use oer_protocols_hil_protocol::RequestIdentity;

/// The sender's receive ring: eight descriptors and R, each buffer of 256
/// octets with the 64-octet header below its floor. The MAC's start takes
/// it; the sender serves no frame of it, and the counters report its state.
const RING: usize = 8;
const SIZE: usize = 256;
const BUFFERS: usize = RING + 1;
const BYTES: usize = BUFFERS * SIZE;

oer_portable_memory::arena! {
    /// The sender's receive buffers.
    pub(crate) SenderRx: window DmaWindow, count 9, size 256, floor 64,
        placed by oer_chips_esp32s31_hal::dma_static
        as oer_chips_esp32s31_hal::system::memory::dma_static::DmaStatic
        in ".dma.bss.case_c_sender_rx";
}
oer_chips_esp32s31_hal::dma_static! {
    static RX_TABLE: RxDescriptors<RING> = zeroed in ".dma.bss.case_c_sender_rx_table";
}
type Ring = RxRing<SenderRx, BYTES, RING, WifiRxDmaRegisters>;
static RING_PLACE: static_cell::StaticCell<MaybeUninit<Ring>> = static_cell::StaticCell::new();

use crate::phy_pilot;

oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: oer_chips_family_platform_runtime::interrupts::LINES,
    handler_attributes: [];
    contexts { app: 1; radio_exec: 2; radio_irq: 3; }
    wake: WakeToken = FromCpu0Irq => oer_chips_family_platform_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 40, context app, hart 0, Hardware;
    // SOURCE: family.toml [interrupts]; HW23_S31_P2_ARBITRATION_20261027:
    // only P2 line17 before16 is measured on S31.
    phy: PhyToken = oer_chips_esp32s31_hal::peripherals::timg0_alarm::Timg0AlarmInterrupt => phy_pilot::interrupt, line 16, context radio_exec, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    radio: RadioToken = FromCpu2Irq => oer_chips_family_platform_executor::Round<RadioTasks>, line 17, context radio_exec, hart 0, Executor;
    radio_alarm: RadioAlarm = SystimerAlarm0 => oer_chips_family_platform_executor::alarm_interrupt::<RadioTasks, 0, SystimerAlarm0>, line 18, context radio_irq, hart 0, Hardware;
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 41, context app, hart 0, Hardware;
    app_alarm: AppAlarm = SystimerAlarm1 => oer_chips_family_platform_executor::alarm_interrupt::<AppTasks, 1, SystimerAlarm1>, line 42, context app, hart 0, Hardware;
}

oer_chips_family_platform_executor::tasks! {
    pub RadioTasks: InterruptExecutor<Esp32s31, 2> {
        session(parts: SessionParts, inbox: oer_hil_agent::Inbox<'static, SenderRequest>, reset: u32) = session_task;
    }
}
oer_chips_family_platform_executor::tasks! {
    AppTasks: Executor<Esp32s31, 0> {
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_watchdog;
        host(host: Host) = host_task;
    }
}
oer_chips_family_platform_executor::time_driver!(Esp32s31);
oer_chips_esp32s31_hal::phy_fault_reporter!(oer_chips_esp32s31_platform_runtime::phy_fault::reset);
oer_chips_family_platform_runtime::entry!(
    main,
    oer_chips_esp32s31_hal::system::platform::Port,
    hook = crate::diagnostic::panic,
    exception = crate::diagnostic::exception
);
fn main(
    handoff: oer_chips_family_platform_runtime::Handoff<
        oer_chips_esp32s31_hal::system::platform::Port,
    >,
) -> ! {
    let Some(root) = Root::take() else {
        fail(name!("root"; 32));
    };
    let (
        (
            reset_cause,
            runtime_efuse,
            runtime_radio,
            runtime_timg0_alarm,
            runtime_clocks,
            runtime_system_timer,
            runtime_wifi_interrupts,
            runtime_software_interrupts,
            runtime_entropy,
            runtime_watchdogs,
            runtime_cycles,
        ),
        _psram,
        mut adopted,
    ) = match oer_chips_family_platform_runtime::adoption::run::<
        oer_chips_esp32s31_platform_runtime::Esp32s31,
        _,
        _,
        _,
    >(
        handoff,
        root.into_runtime(),
        InterruptTable,
        board::BootstrapPsram,
        oer_chips_esp32s31_platform_runtime::phy_fault::report_previous,
        |hardware| {
            let reset_causes =
                oer_chips_esp32s31_platform_runtime::reset_causes::read(hardware.reset_cause);
            crate::diagnostic::report_previous();
            (
                reset_causes.of(oer_chips_esp32s31_hal::system::platform::BOOT_HART),
                hardware.efuse,
                hardware.radio,
                hardware.timg0_alarm,
                hardware.clocks,
                hardware.system_timer,
                hardware.wifi_interrupts,
                hardware.software_interrupts,
                hardware.entropy,
                hardware.watchdogs,
                hardware.cycles,
            )
        },
    ) {
        Ok(value) => value.into_parts(),
        Err(error) => fail(error.name()),
    };
    let Some(interrupts) = Interrupts::take() else {
        fail(name!("interrupts"; 32));
    };
    crate::diagnostic::phase(1);
    let factory = FactoryIdentity::read(runtime_efuse);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(Interface::Station);
    let ap = factory.address(Interface::AccessPoint);
    let (radio, partitions) = runtime_radio
        .split::<ChipClocks, _>(
            ConcurrentPhy::new(),
            oer_chips_esp32s31_platform_runtime::phy_fault::identity(),
        )
        .unwrap_or_else(|failure| {
            let fault = failure.fault();
            let _owners = failure;
            oer_chips_esp32s31_platform_runtime::phy_fault::reset(fault)
        });
    let pilot = crate::phy_pilot::init(runtime_timg0_alarm, adopted.router(), interrupts.phy);
    let timer = runtime_system_timer;
    let mut timer_ready = oer_chips_family_platform_executor::init::<Esp32s31, 2, _, _, _, _>(
        oer_chips_family_platform_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    oer_chips_family_platform_executor::bind::<Esp32s31, 0, _, _, _, _>(
        &mut timer_ready,
        adopted.router(),
        interrupts.app_alarm,
        timer.alarm1,
    )
    .unwrap_or_else(|failure| failure.reset());
    // No Wi-Fi interrupt consumer is installed: the session polls.
    let mut wifi_irqs = runtime_wifi_interrupts;
    wifi_irqs.mac.mask_all_mac_interrupts();
    wifi_irqs.power.mask_all_power_interrupts();
    let software = runtime_software_interrupts;
    let (hart, router) = adopted.parts();
    let radio_executor = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, ())
        .unwrap_or_else(|failure| failure.reset());
    let mut rng =
        oer_chips_esp32s31_hal::peripherals::entropy::Entropy::new(runtime_entropy, runtime_cycles);
    let (seed, boot) = match (rng.random(), rng.random(), rng.random()) {
        (Ok(seed), Ok(high), Ok(low)) => (seed, (u64::from(high) << 32) | u64::from(low)),
        _ => fail(name!("entropy"; 32)),
    };
    let parts = SessionParts {
        radio,
        clocks: runtime_clocks,
        wifi: partitions.wifi,
        coex: partitions.wifi_coex,
        pilot,
        identity,
        mac_irq: wifi_irqs.mac,
        power_irq: wifi_irqs.power,
        seed,
        station,
        ap,
        cycles: runtime_cycles,
    };
    let (console, pads) = match usb::open(adopted.router(), interrupts.console_usb) {
        Ok(value) => value,
        Err(error) => fail(error.reason()),
    };
    // The pads stay held by `main`, which never returns, so that nothing
    // switches them off.
    let _pads = pads;
    let Some(mailbox) = MAILBOX.try_take() else {
        fail(name!("mailbox"; 32));
    };
    let (outbox, inbox) = mailbox.split();
    // The probe reads no previous boot's panic or exception record: its
    // diagnostic hook keeps its own (`diagnostic::report_previous`).
    let host = Host {
        console,
        boot,
        records: oer_hil_agent::Records {
            reset_cause,
            previous_panic: None,
            previous_exception: None,
        },
        outbox,
    };
    if RadioTasks::session(&radio_executor, parts, inbox, u32::from(reset_cause.code())).is_err() {
        fail(name!("session-task"; 32));
    }
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(value) => value,
        Err(error) => fail(error.name()),
    };
    match Executor::<Esp32s31, 0>::new(software.line0)
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            (),
            move |spawner| {
                if AppTasks::watchdog(spawner, runtime_watchdogs.rtc).is_err()
                    || AppTasks::host(spawner, host).is_err()
                {
                    fail(name!("task"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

fn fail<const B: usize>(reason: Name<B>) -> ! {
    oer_chips_family_platform_runtime::fatal::boot_failure::<
        oer_chips_esp32s31_hal::system::platform::Port,
    >(reason.as_str(), || {
        let _ = oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
            &oer_portable_diagnostics::Note::runtime_fail(reason),
        );
    })
}

async fn feed_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    match watchdog
        .feed_every(
            &EmbassyClock,
            Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS),
        )
        .await {}
}

// ---------------------------------------------------------------- host

oer_protocols_hil_agent::image_keys! {
    /// The sender: the base module and the sender's requests.
    KEYS = [SenderRequest]
}

/// What the sender announces in its Hello.
const IMAGE: oer_hil_agent::Image = oer_hil_agent::Image {
    keys: KEYS,
    maximum_payload_bytes: service::MAXIMUM_PAYLOAD_BYTES,
};

static MAILBOX: static_cell::ConstStaticCell<oer_hil_agent::Mailbox<SenderRequest>> =
    static_cell::ConstStaticCell::new(oer_hil_agent::Mailbox::new());

/// What the host's link takes at boot: the console, this boot's identity,
/// what the adoption read and the mailbox's console end.
pub(crate) struct Host {
    pub(crate) console: usb::Console,
    pub(crate) boot: u64,
    pub(crate) records: oer_hil_agent::Records,
    pub(crate) outbox: oer_hil_agent::Outbox<'static, SenderRequest>,
}

/// How the sender's serving ends: the console's transport broke.
enum Ended {
    Console,
}

/// The application level's task: the HIL console, which hands each request
/// to the radio level's worker through the mailbox and writes the worker's
/// answers. Its own worker never ends: the sender's work is the radio
/// level's.
async fn host_task(host: Host) {
    let Host {
        console,
        boot,
        records,
        outbox,
    } = host;
    match oer_hil_agent::serve(
        console,
        boot,
        records,
        IMAGE,
        outbox,
        Ended::Console,
        core::future::pending::<Result<core::convert::Infallible, Ended>>,
    )
    .await
    {
        Err(Ended::Console) => fail(name!("console"; 32)),
        Ok(never) => match never {},
    }
}

mod amsdu;
#[cfg(feature = "ccmp")]
mod ccmp;
mod transmit;

type SessionRun = impl core::future::Future<Output = ()>;
static SESSION: static_cell::ConstStaticCell<oer_portable_memory::StaticPlace<SessionRun>> =
    static_cell::ConstStaticCell::new(oer_portable_memory::StaticPlace::new());

/// The radio level's one task: the worker that drives the radio, each step
/// as the host asks for it.
#[define_opaque(SessionRun)]
fn session_future(
    parts: SessionParts,
    inbox: oer_hil_agent::Inbox<'static, SenderRequest>,
    reset: u32,
) -> SessionRun {
    async move {
        match worker(parts, inbox, reset).await {
            Ok(never) => match never {},
            Err(Ended::Console) => fail(name!("console"; 32)),
        }
    }
}

async fn session_task(
    parts: SessionParts,
    inbox: oer_hil_agent::Inbox<'static, SenderRequest>,
    reset: u32,
) {
    let Some(place) = SESSION.try_take() else {
        fail(name!("session-place"; 32));
    };
    place.future(|| session_future(parts, inbox, reset)).await;
}

/// Wait `micros` on the executor's clock.
pub(crate) async fn wait_micros(micros: u64) {
    if let Some(at) = EmbassyClock
        .now()
        .checked_add(Duration::from_micros(micros))
    {
        EmbassyClock.wait_until(at).await;
    }
}

/// The first `N` items of `items`, or `None` when it has fewer.
fn first_of<T, const N: usize>(items: impl Iterator<Item = T>) -> Option<[T; N]> {
    let mut collected = heapless::Vec::<T, N>::new();
    for item in items.take(N) {
        collected.push(item).ok()?;
    }
    collected.into_array().ok()
}

/// Answer `request` with its endpoint's refusal.
fn refuse(identity: RequestIdentity, request: SenderRequest, refusal: Refusal) {
    match request {
        SenderRequest::SnapshotCold(_) | SenderRequest::SnapshotUp(_) => {
            answer(identity, &service::SnapshotPage::Refused(refusal));
        }
        SenderRequest::BringUp(_) => answer(identity, &service::BroughtUp::Refused(refusal)),
        SenderRequest::ReceiveCounters(_) => {
            answer(identity, &service::Counters::Refused(refusal));
        }
        SenderRequest::Stop(_) => answer(identity, &service::Stopped::Refused(refusal)),
        SenderRequest::Target(_) => answer(identity, &service::Targeted::Refused(refusal)),
        SenderRequest::Round(_) => answer(identity, &service::Sent::Refused(refusal)),
        SenderRequest::Single(_) => answer(identity, &service::SentOne::Refused(refusal)),
    }
}

/// After the bring-up's failure or the stop: the cold snapshot still
/// answers, every other step is refused, until the next boot.
async fn down(
    mut inbox: oer_hil_agent::Inbox<'static, SenderRequest>,
    cold: &pair::Readings,
) -> Result<core::convert::Infallible, Ended> {
    loop {
        let (identity, request) = inbox.next().await;
        match request {
            SenderRequest::SnapshotCold(request) => {
                answer(identity, &pair::page(cold, request.page));
            }
            other => refuse(identity, other, Refusal::Down),
        }
    }
}

/// Whether a step of the rounds failed.
#[derive(Clone, Copy)]
enum Steps {
    Open,
    /// A round failed: rounds and targets are refused until the next boot.
    Failed,
}

/// The sender's worker: each step the host asks for, in its order.
async fn worker(
    parts: SessionParts,
    mut inbox: oer_hil_agent::Inbox<'static, SenderRequest>,
    reset: u32,
) -> Result<core::convert::Infallible, Ended> {
    let cold = pair::cold(reset, &parts.radio, &parts.clocks);
    let identity = parts.identity;
    let cycles = parts.cycles;
    let ap = parts.ap;
    let bring = loop {
        let (id, request) = inbox.next().await;
        match request {
            SenderRequest::SnapshotCold(request) => answer(id, &pair::page(&cold, request.page)),
            SenderRequest::BringUp(_) => break id,
            other => refuse(id, other, Refusal::NotUp),
        }
    };
    let cooperative = Cooperative::new(EmbassyClock);
    let timer = CooperativeTimer::new(&cooperative, cycles);
    let mut platform = ();
    let up = async {
        let brought = apart!(bring_up(parts, &timer, &mut platform))?;
        // The ring's parts: the table, the walker's registers, the arena
        // and its buffers (the ring's and R's).
        let table = RX_TABLE.take().ok_or((phase::RING, 0))?;
        let table = RxTable::place::<ChipDma>(table).map_err(|_| (phase::RING, 1))?;
        let (arena, mut fresh) = Arena::<SenderRx, BYTES>::claim().ok_or((phase::RING, 2))?;
        let ring: [Handle<SenderRx>; RING] = first_of(&mut fresh).ok_or((phase::RING, 3))?;
        let reserved = fresh.next().ok_or((phase::RING, 4))?;
        let place = RING_PLACE
            .try_init(MaybeUninit::uninit())
            .ok_or((phase::RING, 5))?;
        let Brought {
            mut slot,
            mut clocked,
            first,
            mut walker,
            mut sources,
            mac,
            channel,
            mut mac_irq,
            mut power_irq,
            station,
            held,
        } = brought;
        // S4 to S10: the first reset, the cold preparation and the channel,
        // the core, the publication, the walker, the MAC's receive.
        let reset = held_radio(&mut slot)?.reset_wifi_mac_block(
            clocked.clocks_on(),
            first,
            crate::radio::CBW,
        );
        let prepared = apart!(prepare(
            &mut slot,
            &mut clocked,
            &mut walker,
            &mut sources,
            reset,
            mac,
            channel,
            &timer,
            &mut platform,
        ))?;
        let (mut access, clocks_on) = clocked.into_running();
        let parts = RxRingParts {
            table,
            walker,
            arena,
            ring,
            reserved,
        };
        let (ring, enabled) = begin(
            &mut access,
            &mut mac_irq,
            &mut power_irq,
            prepared,
            place,
            parts,
        );
        let (frames, ordinary) = transmit::prepare(&sources)?;
        Ok((
            slot, access, clocks_on, ring, enabled, mac_irq, power_irq, station, held, frames,
            ordinary,
        ))
    };
    let (
        mut slot,
        mut access,
        clocks_on,
        mut ring,
        enabled,
        _mac_irq,
        _power_irq,
        station,
        held,
        frames,
        mut ordinary,
    ) = match up.await {
        Ok(up) => up,
        Err((phase, index)) => {
            answer(
                bring,
                &service::BroughtUp::Failed(service::Failure { phase, index }),
            );
            return down(inbox, &cold).await;
        }
    };
    answer(bring, &service::BroughtUp::Up { station, ap });
    let up = match pair::up(&mut slot, held.clocks(), reset, identity) {
        Ok(readings) => readings,
        Err(_) => cold.clone(),
    };
    // The receiver the host named, absent until its `target`.
    let mut receiver = None;
    let mut steps = Steps::Open;
    loop {
        let (id, request) = inbox.next().await;
        match request {
            SenderRequest::SnapshotCold(request) => answer(id, &pair::page(&cold, request.page)),
            SenderRequest::SnapshotUp(request) => answer(id, &pair::page(&up, request.page)),
            SenderRequest::BringUp(_) => {
                answer(id, &service::BroughtUp::Refused(Refusal::AlreadyUp));
            }
            SenderRequest::ReceiveCounters(_) => answer(
                id,
                &pair::counters(
                    &access,
                    Some(pair::ring_state(
                        ring.standing(),
                        ring.descriptor_controls(),
                    )),
                ),
            ),
            SenderRequest::Target(_) if matches!(steps, Steps::Failed) => {
                answer(id, &service::Targeted::Refused(Refusal::StepFailed));
            }
            SenderRequest::Round(_) if matches!(steps, Steps::Failed) => {
                answer(id, &service::Sent::Refused(Refusal::StepFailed));
            }
            SenderRequest::Single(_) if matches!(steps, Steps::Failed) => {
                answer(id, &service::SentOne::Refused(Refusal::StepFailed));
            }
            SenderRequest::Single(request) => {
                let Some(peer) = receiver else {
                    answer(id, &service::SentOne::Refused(Refusal::NoReceiver));
                    continue;
                };
                if !service::SINGLE_LENGTHS.contains(&request.length) {
                    answer(id, &service::SentOne::Refused(Refusal::LengthOutOfRange));
                    continue;
                }
                match transmit::single(
                    &frames,
                    &mut ordinary,
                    &mut access,
                    station,
                    peer,
                    usize::from(request.length),
                )
                .await
                {
                    Ok(ended) => answer(
                        id,
                        &service::SentOne::Ended {
                            status: ended.status,
                        },
                    ),
                    Err((phase, index)) => {
                        answer(
                            id,
                            &service::SentOne::Failed(service::Failure { phase, index }),
                        );
                        // As after a failed round: the frame may still be
                        // the hardware's.
                        steps = Steps::Failed;
                    }
                }
            }
            SenderRequest::Target(request) => {
                receiver = Some(request.receiver);
                answer(id, &service::Targeted::Targeted);
            }
            SenderRequest::Round(request) => {
                let Some(peer) = receiver else {
                    answer(id, &service::Sent::Refused(Refusal::NoReceiver));
                    continue;
                };
                let began = EmbassyClock.now();
                let sent = amsdu::series(
                    amsdu::AmsduParts {
                        frames: &frames,
                        ordinary: &mut ordinary,
                        station,
                        peer,
                        rounds: request.rounds,
                    },
                    &mut access,
                )
                .await;
                let micros = u32::try_from(
                    EmbassyClock
                        .now()
                        .saturating_duration_since(began)
                        .as_micros(),
                )
                .unwrap_or(u32::MAX);
                match sent {
                    Ok(cases) => answer(id, &service::Sent::Sent { cases, micros }),
                    Err(stop) => {
                        let (phase, index) = stop.failure;
                        answer(
                            id,
                            &service::Sent::Failed(service::RoundFailure {
                                failure: service::Failure { phase, index },
                                round: stop.round,
                                tag: stop.tag,
                                micros,
                                bound_millis: transmit::COMPLETION_MS,
                                queue: transmit::queue_state(&mut access),
                            }),
                        );
                        // The frame may still be the hardware's: no round
                        // runs again; the counters, the snapshots and the
                        // stop still answer.
                        steps = Steps::Failed;
                    }
                }
            }
            SenderRequest::Stop(_) => {
                match held_radio(&mut slot) {
                    Ok(radio) => {
                        let began = EmbassyClock.now();
                        let _stopped = stop(radio, &mut access, &clocks_on, ring, enabled, cycles);
                        let micros = u32::try_from(
                            EmbassyClock
                                .now()
                                .saturating_duration_since(began)
                                .as_micros(),
                        )
                        .unwrap_or(u32::MAX);
                        answer(id, &service::Stopped::Stopped { micros });
                    }
                    Err((phase, index)) => {
                        answer(
                            id,
                            &service::Stopped::Failed(service::Failure { phase, index }),
                        );
                    }
                }
                return down(inbox, &cold).await;
            }
        }
    }
}
