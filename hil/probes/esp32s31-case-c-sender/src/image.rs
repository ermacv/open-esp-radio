//! The sender's boot and its radio session: Wi-Fi brought up over the radio
//! domain the probe holds (`radio`), the MAC started on the receive ring's
//! atoms over a small ring of its own, the peer learned from the first frame
//! addressed to the station, then case C's rounds (`amsdu`) or cell E's
//! (`ccmp`).
use crate::radio::{
    Brought, CooperativeTimer, Failure, SessionParts, apart, begin, bring_up, held, phase, prepare,
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
            receive::{RxDescriptors, RxRing, RxRingParts, RxTable, Unit},
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
use oer_portable_diagnostics::{Name, name};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_memory::arena::{Arena, Handle};
use oer_portable_time::{Clock, Duration, Timer};

/// The sender's receive ring: eight descriptors and R, each buffer of 256
/// octets with the 64-octet header below its floor. It serves the learning
/// of the peer only, every unit copied in place.
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

// SAFETY: the production runtime script maps .hot.text.* to executable SRAM;
// each generated wrapper retains the interrupt-table ABI and owned part.
oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: oer_chips_family_platform_runtime::interrupts::LINES,
    handler_attributes: [#[unsafe(link_section = ".hot.text.case_c_sender_vectors")]];
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
        session(parts: SessionParts) = session_task;
    }
}
oer_chips_family_platform_executor::tasks! {
    AppTasks: Executor<Esp32s31, 0> {
        report(console: usb::Console, pads: oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads) = report;
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_watchdog;
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
            runtime_efuse,
            runtime_radio,
            runtime_timg0_alarm,
            runtime_clocks,
            runtime_system_timer,
            runtime_wifi_interrupts,
            runtime_software_interrupts,
            runtime_entropy,
            runtime_watchdogs,
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
            let _reset_causes =
                oer_chips_esp32s31_platform_runtime::reset_causes::read(hardware.reset_cause);
            crate::diagnostic::report_previous();
            (
                hardware.efuse,
                hardware.radio,
                hardware.timg0_alarm,
                hardware.clocks,
                hardware.system_timer,
                hardware.wifi_interrupts,
                hardware.software_interrupts,
                hardware.entropy,
                hardware.watchdogs,
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
        oer_chips_esp32s31_hal::peripherals::entropy::Entropy::new(runtime_entropy, EmbassyClock);
    let seed = match rng.random() {
        Ok(word) => word,
        Err(_) => fail(name!("entropy"; 32)),
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
    };
    if RadioTasks::session(&radio_executor, parts).is_err() {
        fail(name!("session-task"; 32));
    }
    let (console, pads) = match usb::open(adopted.router(), interrupts.console_usb) {
        Ok(value) => value,
        Err(error) => fail(error.reason()),
    };
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
                if AppTasks::report(spawner, console, pads).is_err()
                    || AppTasks::watchdog(spawner, runtime_watchdogs.rtc).is_err()
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

// ---------------------------------------------------------------- journal

/// What one record says.
#[derive(Clone, Copy)]
#[repr(u32)]
enum Kind {
    /// The bring-up's end: the MAC started on the receive ring.
    Up = 1,
    /// A failure: its phase and its index.
    Failed = 5,
    /// The rounds' end: their microseconds.
    End = 4,
    /// The peer's address, learned from its frames.
    Peer = 13,
    /// Case C's sender: one case's frames and how they ended.
    #[cfg(not(feature = "ccmp"))]
    AmsduSent = 15,
    /// Cell E's sender: one case's protected frames and how they ended.
    #[cfg(feature = "ccmp")]
    CcmpSent = 16,
}

const WIDTH: usize = 22;
const ROWS: usize = 16;

/// The session's records, filled on hart 0's radio executor and printed by
/// the console's task once the session has ended.
#[derive(bytemuck::Zeroable)]
struct Journal {
    rows: [[u32; WIDTH]; ROWS],
    used: usize,
}

impl Journal {
    fn record(&mut self, kind: Kind, values: &[u32]) {
        if let Some(row) = self.rows.get_mut(self.used) {
            *row = [0; WIDTH];
            if let Some(first) = row.first_mut() {
                *first = kind as u32;
            }
            for (cell, value) in row.iter_mut().skip(1).zip(values) {
                *cell = *value;
            }
            self.used = self.used.saturating_add(1);
        }
    }
}

static JOURNAL: oer_portable_memory::zeroed::ZeroedStatic<Journal> =
    oer_portable_memory::zeroed::ZeroedStatic::new();
static DONE: oer_portable_memory::ClaimCell<&'static mut Journal> =
    oer_portable_memory::ClaimCell::new();
/// The phase the session is in, printed as it changes so that the host
/// can place the receiver's lines beside the sender's (`MARK <n>`).
static MARK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn mark(phase: u32) {
    MARK.store(phase, core::sync::atomic::Ordering::Relaxed);
}

async fn report(
    mut console: usb::Console,
    _pads: oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
) {
    use core::fmt::Write as _;
    usb::text(&mut console, b"CASECSENDER boot v=1\n").await;
    let mut shown = 0_u32;
    let journal = loop {
        let now = MARK.load(core::sync::atomic::Ordering::Relaxed);
        if now != shown {
            shown = now;
            let mut line = heapless::String::<32>::new();
            let _ = writeln!(line, "MARK {now}");
            usb::text(&mut console, line.as_bytes()).await;
        }
        if let Ok(journal) = DONE.take() {
            break journal;
        }
        match EmbassyClock.now().checked_add(Duration::from_millis(10)) {
            Some(at) => EmbassyClock.wait_until(at).await,
            None => return,
        }
    };
    let mut line = heapless::String::<256>::new();
    for row in journal.rows.iter().take(journal.used) {
        line.clear();
        let _ = line.push_str("R");
        for value in row {
            let _ = write!(line, " {value}");
        }
        let _ = line.push('\n');
        usb::text(&mut console, line.as_bytes()).await;
    }
    let failed = journal
        .rows
        .iter()
        .take(journal.used)
        .any(|row| row.first() == Some(&(Kind::Failed as u32)));
    if failed {
        usb::text(&mut console, b"OER-RESULT: fail case-c-sender\n").await;
    } else {
        usb::text(&mut console, b"OER-RESULT: pass case-c-sender\n").await;
    }
}

// ---------------------------------------------------------------- session

mod amsdu;
#[cfg(feature = "ccmp")]
mod ccmp;
mod transmit;

type SessionRun = impl core::future::Future<Output = ()>;
static SESSION: static_cell::ConstStaticCell<oer_portable_memory::StaticPlace<SessionRun>> =
    static_cell::ConstStaticCell::new(oer_portable_memory::StaticPlace::new());
#[define_opaque(SessionRun)]
fn session_future(parts: SessionParts) -> SessionRun {
    async move {
        let Some(journal) = JOURNAL.try_take() else {
            return;
        };
        journal.used = 0;
        if let Err((phase, index)) = apart!(session(parts, journal)) {
            journal.record(Kind::Failed, &[phase, index]);
        }
        let _ = DONE.install(journal);
    }
}

async fn session_task(parts: SessionParts) {
    let Some(place) = SESSION.try_take() else {
        return;
    };
    place.future(|| session_future(parts)).await;
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

/// Bring Wi-Fi up, start the MAC on the receive ring, learn the peer, then
/// the rounds. Every owner stays held until the leased host reset.
async fn session(parts: SessionParts, journal: &mut Journal) -> Result<(), Failure> {
    let timer = CooperativeTimer::new(EmbassyClock);
    let mut platform = ();
    let brought = apart!(bring_up(parts, &timer, &mut platform))?;
    // The ring's parts: the table, the walker's registers, the arena and
    // its buffers (the ring's and R's).
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
        held: _held,
    } = brought;
    // S4 to S10: the first reset, the cold preparation and the channel, the
    // core, the publication, the walker, the MAC's receive.
    let reset =
        held(&mut slot)?.reset_wifi_mac_block(clocked.clocks_on(), first, crate::radio::CBW);
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
    let (mut access, _clocks_on) = clocked.into_running();
    let parts = RxRingParts {
        table,
        walker,
        arena,
        ring,
        reserved,
    };
    let (mut ring, _enabled) = begin(
        &mut access,
        &mut mac_irq,
        &mut power_irq,
        prepared,
        place,
        parts,
    );
    journal.record(Kind::Up, &[]);
    mark(70);
    let peer = learn_peer(&mut ring, station).await?;
    journal.record(
        Kind::Peer,
        &[
            u32::from_be_bytes([0, 0, peer[0], peer[1]]),
            u32::from_be_bytes([peer[2], peer[3], peer[4], peer[5]]),
        ],
    );
    let began = EmbassyClock.now();
    let result = apart!(transmit::series(
        &mut access,
        &sources,
        station,
        peer,
        journal
    ));
    let micros = EmbassyClock
        .now()
        .saturating_duration_since(began)
        .as_micros();
    journal.record(Kind::End, &[u32::try_from(micros).unwrap_or(u32::MAX)]);
    let _held = (slot, ring, mac_irq, power_irq, _held);
    result
}

/// The peer: the sender of the first frame addressed to the station, served
/// from the ring within two seconds, every unit copied in place.
async fn learn_peer(ring: &mut Ring, station: [u8; 6]) -> Result<[u8; 6], Failure> {
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_secs(2))
        .ok_or((transmit::tx_phase::PEER, 0))?;
    while EmbassyClock.now() < end {
        let served = ring.serve(&EmbassyClock, |view| {
            let mut found = None;
            while let Some(unit) = view.next_unit() {
                let mut head = [0_u8; 16];
                let copied = match unit {
                    Unit::Single(unit) => unit.copy_in_place(|frame| frame.copy_to(&mut head)),
                    Unit::Chain(unit) => unit.copy_in_place(|frame| frame.copy_to(&mut head)),
                };
                if found.is_none()
                    && copied == head.len()
                    && let Some((to, from)) = addresses(&head)
                    && to == station
                {
                    found = Some(from);
                }
            }
            found
        });
        match served {
            Ok(Some(peer)) => return Ok(peer),
            Ok(None) => wait_micros(100).await,
            Err((anomaly, _)) => {
                return Err((transmit::tx_phase::PEER, anomaly_code(anomaly)));
            }
        }
    }
    Err((transmit::tx_phase::PEER, 1))
}

/// A ring anomaly's code in a failure's index.
fn anomaly_code(
    anomaly: oer_chips_esp32s31_hal::radio::ieee80211::dma::receive::RingAnomaly,
) -> u32 {
    use oer_chips_esp32s31_hal::radio::ieee80211::dma::receive::RingAnomaly;
    match anomaly {
        RingAnomaly::Length { word } => 0x1000_0000 | (word.length() as u32),
        RingAnomaly::DoorbellPending => 0x2000_0000,
    }
}

/// A frame's Address 1 and Address 2, from its first 16 octets.
pub(crate) fn addresses(frame: &[u8; 16]) -> Option<([u8; 6], [u8; 6])> {
    Some((
        frame.get(4..10)?.try_into().ok()?,
        frame.get(10..16)?.try_into().ok()?,
    ))
}
