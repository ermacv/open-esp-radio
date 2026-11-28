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
use oer_chips_esp32s31_radio_phy::{PhyCalibrationIdentity, phy_get_rf_cal_version};
use oer_chips_family_platform_executor::{Executor, InterruptExecutor};
use oer_hil_services_radio_pair::{self as service, Refusal, SenderCommand, SenderRequest};
use oer_portable_clock::{Clock, Timer};
use oer_portable_diagnostics::{Name, name};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_memory::arena::{Arena, Handle};
use oer_portable_time::Duration;

/// The sender's receive ring: eight descriptors, each buffer of 256
/// octets with the 64-octet header below its floor. The MAC's start takes
/// it; the sender serves no frame of it, and the counters report its state.
const RING: usize = 8;
const SIZE: usize = 256;
const BUFFERS: usize = RING;
const BYTES: usize = BUFFERS * SIZE;

oer_portable_memory::arena! {
    /// The sender's receive buffers.
    pub(crate) SenderRx: window DmaWindow, count 8, size 256, floor 64,
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
        session(parts: SessionParts, work: crate::radio::Work<SenderCommand, Answer>, reset: u32) = session_task;
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
        .split::<ChipClocks>(oer_chips_esp32s31_platform_runtime::phy_fault::identity())
        .unwrap_or_else(|failure| {
            let fault = failure.fault();
            let _owners = failure;
            oer_chips_esp32s31_platform_runtime::phy_fault::reset(fault)
        });
    let power_detector =
        crate::phy_pilot::init(runtime_timg0_alarm, adopted.router(), interrupts.phy);
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
        power_detector,
        phy: partitions.phy,
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
    // The owner's cold snapshot, read before the radio's parts go to the
    // radio level's task.
    let observing = Observing {
        cold: pair::cold(u32::from(reset_cause.code()), &parts.radio, &parts.clocks),
    };
    let Some(handover) = RADIO.try_take() else {
        fail(name!("radio-handover"; 32));
    };
    let (commands, work) = handover.split();
    let Some(writer) = WRITER.try_take() else {
        fail(name!("writer"; 32));
    };
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
        writer,
        observing,
        owner: Owner { commands },
    };
    if RadioTasks::session(&radio_executor, parts, work, u32::from(reset_cause.code())).is_err() {
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

/// The console's state: only the console's task touches it.
static WRITER: static_cell::ConstStaticCell<oer_hil_agent::Writer> =
    static_cell::ConstStaticCell::new(oer_hil_agent::Writer::new());

/// What the radio level's task gives for a command: one variant an
/// endpoint's answer, a refusal included. Its copy into the hand-over's
/// place and out of it is a payload's copy.
pub(crate) enum Answer {
    SnapshotPage(service::SnapshotPage),
    BroughtUp(service::BroughtUp),
    Counters(service::Counters),
    Stopped(service::Stopped),
    Targeted(service::Targeted),
    Sent(service::Sent),
    SentOne(service::SentOne),
    Bursted(service::Bursted),
}

impl From<service::SnapshotPage> for Answer {
    fn from(answer: service::SnapshotPage) -> Self {
        Self::SnapshotPage(answer)
    }
}

impl From<service::BroughtUp> for Answer {
    fn from(answer: service::BroughtUp) -> Self {
        Self::BroughtUp(answer)
    }
}

impl From<service::Counters> for Answer {
    fn from(answer: service::Counters) -> Self {
        Self::Counters(answer)
    }
}

impl From<service::Stopped> for Answer {
    fn from(answer: service::Stopped) -> Self {
        Self::Stopped(answer)
    }
}

impl From<service::Targeted> for Answer {
    fn from(answer: service::Targeted) -> Self {
        Self::Targeted(answer)
    }
}

impl From<service::Sent> for Answer {
    fn from(answer: service::Sent) -> Self {
        Self::Sent(answer)
    }
}

impl From<service::SentOne> for Answer {
    fn from(answer: service::SentOne) -> Self {
        Self::SentOne(answer)
    }
}

impl From<service::Bursted> for Answer {
    fn from(answer: service::Bursted) -> Self {
        Self::Bursted(answer)
    }
}

impl oer_hil_agent::serve::Answer for Answer {
    fn write<'f>(
        &self,
        frame: oer_hil_agent::serve::Frame<'f>,
    ) -> oer_hil_agent::serve::Written<'f> {
        match self {
            Self::SnapshotPage(answer) => frame.write(answer),
            Self::BroughtUp(answer) => frame.write(answer),
            Self::Counters(answer) => frame.write(answer),
            Self::Stopped(answer) => frame.write(answer),
            Self::Targeted(answer) => frame.write(answer),
            Self::Sent(answer) => frame.write(answer),
            Self::SentOne(answer) => frame.write(answer),
            Self::Bursted(answer) => frame.write(answer),
        }
    }
}

/// The radio's owner, running on the application's level: the console calls
/// it for each command, and it commands the radio level's task through the
/// radio's hand-over, whose asking end it holds alone.
pub(crate) struct Owner {
    commands: crate::radio::Commands<SenderCommand, Answer>,
}

impl oer_hil_agent::serve::Run<Owner> for SenderCommand {
    type Answer = Answer;
    type Trap = core::convert::Infallible;
    async fn run(self, owner: &mut Owner) -> oer_hil_agent::serve::Ran<Answer, Self::Trap> {
        use oer_hil_agent::serve::{After, Unanswered};
        // One command is in work at a time by the console's borrow of the
        // owner, so the command's place is free.
        owner.commands.hand(self).map_err(|_| Unanswered)?;
        match owner.commands.answered().await {
            oer_portable_memory::Answered::Given(answer) => Ok((answer, After::Nothing)),
            // The radio level's task dropped its permit.
            oer_portable_memory::Answered::Unanswered => Err(Unanswered),
        }
    }
}

/// What the radio's owner gives at once: its cold snapshot, read at the
/// composition before the radio's parts went to the radio level's task.
pub(crate) struct Observing {
    cold: pair::Readings,
}

impl service::SenderObserver for Observing {
    fn snapshot_cold(&mut self, request: service::SnapshotCold) -> service::SnapshotPage {
        pair::page(&self.cold, request.page)
    }
}

/// The radio's hand-over between its owner and the radio level's task.
static RADIO: static_cell::ConstStaticCell<crate::radio::Handover<SenderCommand, Answer>> =
    static_cell::ConstStaticCell::new(crate::radio::handover());

/// What the host's link takes at boot: the console, this boot's identity,
/// what the adoption read and the mailbox's console end.
pub(crate) struct Host {
    pub(crate) console: usb::Console,
    pub(crate) boot: u64,
    pub(crate) records: oer_hil_agent::Records,
    pub(crate) writer: &'static mut oer_hil_agent::Writer,
    pub(crate) observing: Observing,
    pub(crate) owner: Owner,
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
        writer,
        mut observing,
        mut owner,
    } = host;
    match oer_hil_agent::serve::<_, SenderRequest, _, _, _>(
        console,
        boot,
        records,
        IMAGE,
        oer_hil_agent::Mount {
            writer,
            observer: &mut observing,
            commander: &mut owner,
        },
        Ended::Console,
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
    inbox: crate::radio::Work<SenderCommand, Answer>,
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
    inbox: crate::radio::Work<SenderCommand, Answer>,
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

/// The permit of one command's result: the radio level's task names no
/// part of the HIL agent, only the service's request and answer types as
/// data.
type Reply<'a> = crate::radio::Done<'a, Answer>;

/// Gives a command's result through its permit. A function of its own, so
/// that the waker call of the hand-over keeps its inline scope and the image's
/// stack analysis resolves it to the executor's vtable (`Fact::WakerVtables`).
#[inline(never)]
fn answer(done: Reply<'_>, body: impl Into<Answer>) {
    done.answer(body.into());
}

/// Answer `request` with its endpoint's refusal.
fn refuse(identity: Reply<'_>, request: SenderCommand, refusal: Refusal) {
    match request {
        SenderCommand::SnapshotUp(_) => {
            answer(identity, service::SnapshotPage::Refused(refusal));
        }
        SenderCommand::BringUp(_) => answer(identity, service::BroughtUp::Refused(refusal)),
        SenderCommand::ReceiveCounters(_) => {
            answer(identity, service::Counters::Refused(refusal));
        }
        SenderCommand::Stop(_) => answer(identity, service::Stopped::Refused(refusal)),
        SenderCommand::Target(_) => answer(identity, service::Targeted::Refused(refusal)),
        SenderCommand::Round(_) => answer(identity, service::Sent::Refused(refusal)),
        SenderCommand::Single(_) => answer(identity, service::SentOne::Refused(refusal)),
        SenderCommand::Burst(_) => answer(identity, service::Bursted::Refused(refusal)),
    }
}

/// After the bring-up's failure or the stop: the cold snapshot still
/// answers, every other step is refused, until the next boot.
async fn down(
    mut inbox: crate::radio::Work<SenderCommand, Answer>,
) -> Result<core::convert::Infallible, Ended> {
    loop {
        let (request, identity) = inbox.next().await;
        refuse(identity, request, Refusal::Down);
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
    mut inbox: crate::radio::Work<SenderCommand, Answer>,
    reset: u32,
) -> Result<core::convert::Infallible, Ended> {
    let cold = pair::cold(reset, &parts.radio, &parts.clocks);
    let identity = parts.identity;
    let cycles = parts.cycles;
    let ap = parts.ap;
    let bring = loop {
        let (request, id) = inbox.next().await;
        match request {
            SenderCommand::BringUp(_) => break id,
            other => refuse(id, other, Refusal::NotUp),
        }
    };
    let cooperative = Cooperative::new(EmbassyClock);
    let timer = CooperativeTimer::new(&cooperative, cycles);
    let mut platform = ();
    let mut phy_memory = MaybeUninit::uninit();
    let up = async {
        let brought = apart!(bring_up(parts, &mut phy_memory, &timer, &mut platform))?;
        // The ring's parts: the table, the walker's registers, the arena
        // and its buffers.
        let table = RX_TABLE.take().ok_or((phase::RING, 0))?;
        let table = RxTable::place::<ChipDma>(table).map_err(|_| (phase::RING, 1))?;
        let (arena, mut fresh) = Arena::<SenderRx, BYTES>::claim().ok_or((phase::RING, 2))?;
        let ring: [Handle<SenderRx>; RING] = first_of(&mut fresh).ok_or((phase::RING, 3))?;
        let place = RING_PLACE
            .try_init(MaybeUninit::uninit())
            .ok_or((phase::RING, 5))?;
        let Brought {
            mut slot,
            mut phy,
            open,
            register_micros,
            // The sender runs no pass after its bring-up.
            power_detector: _power_detector,
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
        let (prepared, open) = apart!(prepare(
            &mut slot,
            &mut clocked,
            &mut phy,
            open,
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
            slot,
            (phy, open, register_micros),
            access,
            clocks_on,
            ring,
            enabled,
            mac_irq,
            power_irq,
            station,
            held,
            frames,
            ordinary,
        ))
    };
    let (
        mut slot,
        // The registered PHY with RF open stays held for the run.
        (phy, _open, register_micros),
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
                service::BroughtUp::Failed(service::Failure { phase, index }),
            );
            return down(inbox).await;
        }
    };
    answer(
        bring,
        service::BroughtUp::Up {
            station,
            ap,
            register_micros,
        },
    );
    let up = match pair::up(&mut slot, held.clocks(), reset, &phy, identity) {
        Ok(readings) => readings,
        Err(_) => cold.clone(),
    };
    // The receiver the host named, absent until its `target`.
    let mut receiver = None;
    let mut steps = Steps::Open;
    loop {
        let (request, id) = inbox.next().await;
        match request {
            SenderCommand::SnapshotUp(request) => answer(id, pair::page(&up, request.page)),
            SenderCommand::BringUp(_) => {
                answer(id, service::BroughtUp::Refused(Refusal::AlreadyUp));
            }
            SenderCommand::ReceiveCounters(_) => answer(
                id,
                pair::counters(
                    &access,
                    Some(pair::ring_state(
                        ring.standing(),
                        ring.descriptor_controls(),
                    )),
                ),
            ),
            SenderCommand::Target(_) if matches!(steps, Steps::Failed) => {
                answer(id, service::Targeted::Refused(Refusal::StepFailed));
            }
            SenderCommand::Round(_) if matches!(steps, Steps::Failed) => {
                answer(id, service::Sent::Refused(Refusal::StepFailed));
            }
            SenderCommand::Single(_) if matches!(steps, Steps::Failed) => {
                answer(id, service::SentOne::Refused(Refusal::StepFailed));
            }
            SenderCommand::Burst(_) if matches!(steps, Steps::Failed) => {
                answer(id, service::Bursted::Refused(Refusal::StepFailed));
            }
            SenderCommand::Burst(request) => {
                let Some(peer) = receiver else {
                    answer(id, service::Bursted::Refused(Refusal::NoReceiver));
                    continue;
                };
                if let service::To::Receiver { length } = request.to
                    && !service::SINGLE_LENGTHS.contains(&length)
                {
                    answer(id, service::Bursted::Refused(Refusal::LengthOutOfRange));
                    continue;
                }
                let (mut ended, mut acknowledged) = (0_u16, 0_u16);
                let mut failure = None;
                for frame in 0..request.frames {
                    if frame != 0 && request.gap_micros != 0 {
                        wait_micros(u64::from(request.gap_micros)).await;
                    }
                    let sent = match request.to {
                        service::To::Receiver { length } => {
                            transmit::single(
                                &frames,
                                &mut ordinary,
                                &mut access,
                                transmit::Addresses {
                                    receiver: peer,
                                    transmitter: station,
                                    bssid: peer,
                                },
                                usize::from(length),
                            )
                            .await
                        }
                        service::To::ProbeRequest => {
                            transmit::probe_request(&frames, &mut ordinary, &mut access, station)
                                .await
                        }
                    };
                    match sent {
                        Ok(end) => {
                            ended = ended.saturating_add(1);
                            acknowledged = acknowledged.saturating_add(u16::from(end.status == 0));
                        }
                        Err(failed) => {
                            failure = Some(failed);
                            break;
                        }
                    }
                }
                match failure {
                    None => answer(
                        id,
                        service::Bursted::Sent {
                            ended,
                            acknowledged,
                        },
                    ),
                    Some((phase, index)) => {
                        answer(
                            id,
                            service::Bursted::Failed(service::Failure { phase, index }),
                        );
                        // As after a failed single: the frame may still be
                        // the hardware's.
                        steps = Steps::Failed;
                    }
                }
            }
            SenderCommand::Single(request) => {
                let Some(peer) = receiver else {
                    answer(id, service::SentOne::Refused(Refusal::NoReceiver));
                    continue;
                };
                if !service::SINGLE_LENGTHS.contains(&request.length) {
                    answer(id, service::SentOne::Refused(Refusal::LengthOutOfRange));
                    continue;
                }
                match transmit::single(
                    &frames,
                    &mut ordinary,
                    &mut access,
                    transmit::Addresses {
                        receiver: peer,
                        transmitter: station,
                        bssid: peer,
                    },
                    usize::from(request.length),
                )
                .await
                {
                    Ok(ended) => answer(
                        id,
                        service::SentOne::Ended {
                            status: ended.status,
                        },
                    ),
                    Err((phase, index)) => {
                        answer(
                            id,
                            service::SentOne::Failed(service::Failure { phase, index }),
                        );
                        // As after a failed round: the frame may still be
                        // the hardware's.
                        steps = Steps::Failed;
                    }
                }
            }
            SenderCommand::Target(request) => {
                receiver = Some(request.receiver);
                answer(id, service::Targeted::Targeted);
            }
            SenderCommand::Round(request) => {
                let Some(peer) = receiver else {
                    answer(id, service::Sent::Refused(Refusal::NoReceiver));
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
                    Ok(cases) => answer(id, service::Sent::Sent { cases, micros }),
                    Err(stop) => {
                        let (phase, index) = stop.failure;
                        answer(
                            id,
                            service::Sent::Failed(service::RoundFailure {
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
            SenderCommand::Stop(_) => {
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
                        answer(id, service::Stopped::Stopped { micros });
                    }
                    Err((phase, index)) => {
                        answer(
                            id,
                            service::Stopped::Failed(service::Failure { phase, index }),
                        );
                    }
                }
                return down(inbox).await;
            }
        }
    }
}
