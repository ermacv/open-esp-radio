//! One finite RX publication. No take, recycle, reload, or live buffer write.
use crate::usb;
use core::fmt::Write as _;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::{
    ieee80211::{
        dma::{descriptor::RxDescriptor, rx_storage::RxDmaStorage},
        init::{
            MAC_COLD_HANDSHAKE_DEADLINE, MacDelayEntropy, MacSlowClockCalibration,
            MacSlowClockCalibrationSource,
        },
    },
    platform::{
        FactoryIdentity, FromCpu0Irq, FromCpu2Irq, Interface, SystimerAlarm0, SystimerAlarm1,
    },
    power::ChipClocks,
    root::Root,
    usb_serial_jtag::ConsoleInterrupt,
    watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
};
use oer_esp32s31_ieee80211_driver::startup::{
    RadioStartConfig, WifiColdStartConfig, WifiMacPlatform, WifiMacStartConfig,
    start_esp32s31_radio,
};
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig,
    PhyTxTargetPowerProfile, concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_executor::{Executor, InterruptExecutor};
use oer_ieee80211_wire::{
    address::IndividualAddress,
    channel::{WifiChannel, WifiChannelWidth},
};
use oer_memory::zeroed::ZeroedStatic;
use oer_radio_regulatory::{Country, RegulatoryDomain};
use oer_time::{Clock, Duration, Timer};
use static_cell::StaticCell;

use oer_esp32s31_hal::ieee80211::dma::rx_ring::RX_BUFFER_SENTINEL;

const COUNT: usize = 96;
const BUFFER: usize = 256;
const NODES: usize = COUNT + 1;
const RECORDS: usize = 2048;
const FRAME_LIMIT: usize = 32;
const CAPTURE_MS: u64 = 4500;
const CHANNEL: u8 = 13;
const _: () = assert!(BUFFER >= 4 && BUFFER.is_multiple_of(4));
type Storage = RxDmaStorage<COUNT, BUFFER, BUFFER>;
type Rng = oer_esp32s31_hal::platform::Entropy<EmbassyClock>;
type Actor = RadioSystem<
    oer_esp32s31_radio_client::wifi::Wifi,
    RcSlow,
    Esp32s31Radio<
        ChipClocks,
        oer_espressif_executor::Cooperative<EmbassyClock>,
        NoopPhyTargetObserver,
    >,
    EmbassyClock,
>;
static ACTOR: StaticCell<Actor> = StaticCell::new();
oer_esp32s31_hal::dma_static! { static RX: Storage = zeroed in ".dma.bss.rx_chain_probe"; }
#[derive(bytemuck::Zeroable)]
struct Trace {
    previous: [[u32; 5]; NODES],
    cursor: [u32; 4],
    expected: [u32; NODES],
    rows: [[u32; 16]; RECORDS],
    used: usize,
    scans: u32,
    max_scan_cycles: u32,
    max_gap_cycles: u32,
    previous_end: u32,
}
static TRACE: ZeroedStatic<Trace> = ZeroedStatic::new();

// SAFETY: the production runtime script maps .hot.text.* to executable SRAM;
// each generated wrapper retains the interrupt-table ABI and owned part.
oer_interrupt_table::interrupt_table! {
    lines: oer_espressif_staged_runtime::interrupts::LINES,
    handler_attributes: [#[unsafe(link_section = ".hot.text.rx_chain_vectors")]];
    contexts { app: 1; radio_exec: 2; radio_irq: 3; }
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<0, FromCpu0Irq>, line 16, context app, hart 0, Hardware;
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<RadioTasks, FromCpu2Irq>, line 17, context radio_exec, hart 0, Executor;
    radio_alarm: RadioAlarm = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<0, SystimerAlarm0>, line 18, context radio_irq, hart 0, Hardware;
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 19, context app, hart 0, Hardware;
    app_alarm: AppAlarm = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<1, SystimerAlarm1>, line 20, context app, hart 0, Hardware;
}
oer_espressif_executor::tasks! {
    RadioTasks: InterruptExecutor<2> { actor(actor: &'static mut Actor) = radio_actor; }
}
oer_espressif_executor::tasks! {
    AppTasks: Executor<0> {
        capture(boot: Boot) = capture;
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_watchdog;
    }
}
struct Boot {
    console: usb::Console,
    _pads: oer_esp32s31_hal::usb_serial_jtag::UsbPads,
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: Rng,
    station: [u8; 6],
    ap: [u8; 6],
}
oer_espressif_staged_runtime::entry!(main);
fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    let Some(root) = Root::take() else {
        fail("root");
    };
    let runtime = root
        .into_runtime()
        .unwrap_or_else(|error| fail(error.name()));
    let (_psram, mut adopted) = match handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        board::BootstrapPsram,
    ) {
        Ok(value) => value,
        Err(error) => fail(error.name()),
    };
    let Some(interrupts) = Interrupts::take() else {
        fail("interrupts");
    };
    crate::diagnostic::report_previous();
    crate::diagnostic::phase(1);
    let factory = FactoryIdentity::read(runtime.efuse);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(Interface::Station);
    let ap = factory.address(Interface::AccessPoint);
    let (domain, partitions) = runtime.radio.split::<ChipClocks, _>(ConcurrentPhy::new());
    let (Some((end, cells)), Some(lp)) = (claim(), LpClock::<RcSlow>::claim()) else {
        fail("claim");
    };
    let backend = Esp32s31Radio::new(
        domain,
        partitions.wifi_coex,
        runtime.clocks,
        oer_espressif_executor::Cooperative::new(EmbassyClock),
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
    );
    let actor = ACTOR.init_with(|| RadioSystem::new(end, backend, EmbassyClock, lp));
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    );
    oer_espressif_executor::bind::<0, _, _>(adopted.router(), interrupts.app_alarm, timer.alarm1);
    // No Wi-Fi interrupt consumer is installed: this experiment polls only.
    // Keep both owned interrupt banks masked for the entire publication.
    let mut wifi_irqs = runtime.wifi_interrupts;
    wifi_irqs.mac.mask_all_mac_interrupts();
    wifi_irqs.power.mask_all_power_interrupts();
    let software = runtime.software_interrupts;
    let radio = InterruptExecutor::<2>::new(software.line2).start(
        adopted.router(),
        interrupts.radio,
        software.irq2,
    );
    if RadioTasks::actor(&radio, actor).is_err() {
        fail("actor-task");
    }
    let (console, pads) = match usb::open(adopted.router(), interrupts.console_usb) {
        Ok(value) => value,
        Err(error) => fail(error.reason()),
    };
    let mut router = match adopted.enable_interrupts() {
        Ok(value) => value,
        Err(error) => fail(error.name()),
    };
    let boot = Boot {
        console,
        _pads: pads,
        cell: cells.wifi,
        wifi: partitions.wifi,
        rng: Rng::new(runtime.entropy, EmbassyClock),
        station,
        ap,
    };
    Executor::<0>::new(software.line0).run(
        &mut router,
        interrupts.wake,
        software.irq0,
        move |spawner| {
            if AppTasks::capture(spawner, boot).is_err()
                || AppTasks::watchdog(spawner, runtime.watchdogs.rtc).is_err()
            {
                fail("task");
            }
        },
    )
}
fn fail(reason: &'static str) -> ! {
    oer_espressif_staged_runtime::fatal_halt(|_| {
        let _ = oer_esp32s31_hal::usb_serial_jtag::fatal_note(
            &oer_diagnostics::Note::runtime_fail(reason),
        );
    })
}
async fn radio_actor(actor: &'static mut Actor) {
    match actor.run().await {}
}
async fn feed_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    match watchdog
        .feed_every(
            &EmbassyClock,
            Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS),
        )
        .await {}
}
struct Platform {
    rng: Rng,
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
    fn install_phy_tx_power_profile(&mut self, _: PhyTxTargetPowerProfile) {}
}

fn node(storage: &Storage, index: usize) -> Option<&RxDescriptor> {
    if index == COUNT {
        Some(storage.reserved_descriptor())
    } else {
        storage.descriptors().get(index)
    }
}
// This exception is limited to this probe and explicitly authorized by the
// experiment: an asynchronous DMA observation, never a Rust buffer reference.
#[allow(
    unsafe_code,
    reason = "probe-only volatile observation of retained DMA bytes"
)]
fn read_word(
    storage: &Storage,
    descriptor: &RxDescriptor,
    expected: u32,
    offset: usize,
) -> Result<u32, &'static str> {
    let address = descriptor.buffer_address();
    let base = core::ptr::from_ref(storage).addr();
    let limit = base
        .checked_add(core::mem::size_of::<Storage>())
        .ok_or("arena-end")?;
    let end = (address as usize).checked_add(BUFFER).ok_or("buffer-end")?;
    if !offset.is_multiple_of(4)
        || offset.checked_add(4).is_none_or(|end| end > BUFFER)
        || address != expected
        || !(address as usize).is_multiple_of(4)
        || (address as usize) < base
        || end > limit
        || usize::from(descriptor.word0().size()) != BUFFER
    {
        return Err("buffer-binding");
    }
    // SAFETY: production placement/preparation published expected from this
    // permanent arena before RX started. Address identity, u32 alignment and
    // full advertised capacity containment are checked above. Only DMA writes
    // this storage; this load creates no reference and grants no ownership.
    Ok(unsafe { (address as *const u32).byte_add(offset).read_volatile() })
}
impl Trace {
    fn push(&mut self, row: [u32; 16]) -> Result<(), &'static str> {
        let out = self.rows.get_mut(self.used).ok_or("trace-full")?;
        *out = row;
        self.used = self.used.saturating_add(1);
        Ok(())
    }
    fn scan(
        &mut self,
        storage: &Storage,
        access: &mut oer_esp32s31_hal::owner::RadioAccess,
        force: bool,
    ) -> Result<usize, &'static str> {
        let begin = oer_riscv::cycle32();
        if self.scans != 0 {
            self.max_gap_cycles = self
                .max_gap_cycles
                .max(begin.wrapping_sub(self.previous_end));
        }
        let mut terminals = 0_usize;
        for index in 0..NODES {
            let descriptor = node(storage, index).ok_or("node")?;
            let before = access.receive_dma_snapshot();
            let t0 = oer_riscv::cycle32();
            let w0 = descriptor.word0();
            let address = descriptor.buffer_address();
            let next = descriptor.next_address();
            let expected = *self.expected.get(index).ok_or("expected")?;
            let lead = read_word(storage, descriptor, expected, 0)?;
            let w1 = descriptor.word0();
            let t1 = oer_riscv::cycle32();
            let after = access.receive_dma_snapshot();
            terminals += usize::from(w1.rx_done());
            let state = [w0.bits(), w1.bits(), lead, address, next];
            let previous = self.previous.get_mut(index).ok_or("previous")?;
            let changed = *previous != state;
            *previous = state;
            let cursor = [
                after.descriptor_base,
                after.last_descriptor_low,
                after.next_descriptor_low,
                u32::from(after.next_descriptor_upper_unknown),
            ];
            let cursor_changed = self.cursor != cursor;
            self.cursor = cursor;
            if changed
                || force
                || cursor_changed
                || before.last_descriptor_low != after.last_descriptor_low
                || before.next_descriptor_low != after.next_descriptor_low
            {
                self.push([
                    index as u32,
                    t0,
                    t1,
                    w0.bits(),
                    w1.bits(),
                    lead,
                    address,
                    next,
                    before.descriptor_base,
                    before.last_descriptor_low,
                    before.next_descriptor_low,
                    after.last_descriptor_low,
                    after.next_descriptor_low,
                    u32::from(before.next_descriptor_upper_unknown),
                    u32::from(after.next_descriptor_upper_unknown),
                    self.scans,
                ])?;
            }
        }
        self.previous_end = oer_riscv::cycle32();
        self.max_scan_cycles = self
            .max_scan_cycles
            .max(self.previous_end.wrapping_sub(begin));
        self.scans = self.scans.saturating_add(1);
        Ok(terminals)
    }
}
async fn capture(boot: Boot) {
    let Boot {
        mut console,
        _pads,
        cell,
        wifi,
        rng,
        station,
        ap,
    } = boot;
    usb::text(&mut console, b"RXCHAIN boot\n").await;
    let result = run(&mut console, cell, wifi, rng, station, ap).await;
    if let Err(reason) = result {
        usb::text(&mut console, reason.as_bytes()).await;
        usb::text(&mut console, b"\nOER-RESULT: fail rx-chain\n").await;
    } else {
        usb::text(&mut console, b"OER-RESULT: pass rx-chain\n").await;
    }
}
async fn run(
    console: &mut usb::Console,
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: Rng,
    station: [u8; 6],
    ap: [u8; 6],
) -> Result<(), &'static str> {
    let station = IndividualAddress::for_interface(station).map_err(|_| "station")?;
    let ap = IndividualAddress::for_interface(ap).map_err(|_| "ap")?;
    let channel =
        WifiChannel::new_2_4_ghz(CHANNEL, WifiChannelWidth::Mhz20).map_err(|_| "channel")?;
    let domain = Country::new(*b"DE")
        .ok()
        .and_then(RegulatoryDomain::of)
        .ok_or("domain")?;
    let channel = domain.permit(channel).map_err(|_| "permit")?;
    let ceiling = channel.tx_ceiling(board::wifi_power_limits(
        i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
        Some(1000),
    ));
    let config = RadioStartConfig::new(
        WifiColdStartConfig::new(channel, ceiling),
        WifiMacStartConfig::new(MAC_COLD_HANDSHAKE_DEADLINE, station, ap)
            .map_err(|_| "mac-config")?,
    );
    usb::text(console, b"RXCHAIN radio-start\n").await;
    crate::diagnostic::phase(2);
    let (cell, started) = start_esp32s31_radio(cell, wifi, Platform { rng }, config).await;
    crate::diagnostic::phase(3);
    let stopped = started.map_err(|_| "radio-start")?;
    let parts = stopped.into_runtime_parts();
    let mut access = parts.registers;
    let placed = RxDmaStorage::place(RX.take().ok_or("rx-claimed")?).map_err(|_| "rx-place")?;
    let storage = placed.storage();
    let prepared = placed
        .prepare_ring(access.rx_dma())
        .map_err(|_| "rx-prepare")?;
    let ready = access.wifi_mac_hal().prepare_mac_runtime(prepared);
    let stopped_ring = ready.publish(access.rx_dma()).map_err(|_| "rx-publish")?;
    let ring = stopped_ring
        .try_start(access.rx_dma())
        .map_err(|_| "rx-start")?;
    let trace = TRACE.try_take().ok_or("trace-claimed")?;
    for index in 0..NODES {
        *trace.expected.get_mut(index).ok_or("expected")? =
            node(storage, index).ok_or("node")?.buffer_address();
    }
    trace.scan(storage, &mut access, true)?;
    access.wifi_mac_hal().configure_open_promiscuous_receive();
    usb::text(
        console,
        b"RXCHAIN begin v=1 channel=13 buffer=256 count=96 frames=32 cap_ms=4500\n",
    )
    .await;
    let deadline = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(CAPTURE_MS as u32))
        .ok_or("deadline")?;
    access.wifi_mac_hal().start_mac_runtime(&ring);
    let mut terminals = 0;
    let mut observation = Ok(());
    while EmbassyClock.now() < deadline && terminals < FRAME_LIMIT {
        match trace.scan(storage, &mut access, false) {
            Ok(count) => terminals = count,
            Err(reason) => {
                observation = Err(reason);
                break;
            }
        }
    }
    // Stop outer RX before withdrawing walker credit. No descriptor is ever
    // rearmed or taken: all observed words retain their original generation.
    access.wifi_mac_hal().request_channel_stop();
    let stop_deadline = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(50))
        .ok_or("stop-deadline")?;
    while access.wifi_mac_hal().channel_active_state() != 0 {
        if EmbassyClock.now() >= stop_deadline {
            return Err("mac-stop");
        }
        EmbassyClock
            .wait_until(
                EmbassyClock
                    .now()
                    .checked_add(Duration::from_millis(1))
                    .ok_or("wait")?,
            )
            .await;
    }
    let halted = ring.try_stop(access.rx_dma()).map_err(|_| "walker-stop")?;
    trace.scan(storage, &mut access, true)?;
    let snapshot = access.receive_dma_snapshot();
    let mut line = heapless::String::<768>::new();
    writeln!(&mut line, "RXCHAIN end terminals={} records={} scans={} max_scan={} max_gap={} base={:08x} last={:x} next={:x}",
        terminals, trace.used, trace.scans, trace.max_scan_cycles, trace.max_gap_cycles,
        snapshot.descriptor_base, snapshot.last_descriptor_low, snapshot.next_descriptor_low).map_err(|_| "format")?;
    usb::text(console, line.as_bytes()).await;
    for row in trace.rows.iter().take(trace.used) {
        line.clear();
        line.push_str("E").map_err(|_| "format")?;
        for value in row {
            write!(&mut line, " {value:x}").map_err(|_| "format")?;
        }
        line.push('\n').map_err(|_| "format")?;
        usb::text(console, line.as_bytes()).await;
    }
    for index in 0..NODES {
        let descriptor = node(storage, index).ok_or("node")?;
        let expected = *trace.expected.get(index).ok_or("expected")?;
        if read_word(storage, descriptor, expected, 0)? == RX_BUFFER_SENTINEL {
            continue;
        }
        line.clear();
        write!(&mut line, "D {index:x}").map_err(|_| "format")?;
        for offset in (0..BUFFER).step_by(4) {
            write!(
                &mut line,
                " {:08x}",
                read_word(storage, descriptor, expected, offset)?
            )
            .map_err(|_| "format")?;
        }
        line.push('\n').map_err(|_| "format")?;
        usb::text(console, line.as_bytes()).await;
    }
    // The stopped ring keeps its arena retained; only the radio owner closes RF.
    let _halted = halted;
    let stopped = parts.context.into_stopped(parts.platform, access);
    let (_cell, release) = stopped.release(cell).await;
    release.map_err(|_| "radio-release")?;
    observation?;
    if terminals == 0 {
        return Err("no-frames");
    }
    Ok(())
}
