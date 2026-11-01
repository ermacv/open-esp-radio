use core::sync::atomic::{AtomicU32, Ordering};

use oer_crypto::block::{Aes128Blocks, Block, BlockCipher, BlockKeyUnwrap};
use oer_crypto::{Aes128Key, KeyData, KeyUnwrap as _, KeyUnwrapRequest};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_hal::peripherals::aes::{
    AesInterrupt, BlockWords, IDLE, InterruptingAes, KeyWords, Registers,
};
use oer_esp32s31_hal::peripherals::software_interrupt::FromCpu0Irq;
use oer_esp32s31_hal::peripherals::system_timer::SystimerAlarm0;
use oer_esp32s31_hal::peripherals::usb_serial_jtag;
use oer_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_esp32s31_hal::platform::Port;
use oer_esp32s31_hal::root::Root;
use oer_espressif_executor::Executor;
use oer_ieee80211_core_security_cipher::aes::{SoftwareAes128, software_aes128_key_wrap};
use oer_time::Timer as _;

use crate::ops::{Line, NONCE, Stats, ccm_seal, cmac};

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; aes_done: 2; }
    /// The boot hart's executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The boot hart's executor's timer alarm; it only acknowledges and pends.
    alarm: AlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<Tasks, 0, SystimerAlarm0>, line 17, context app_wake, hart 0, Hardware;
    /// The AES interrupt: its handler stamps its entry and acknowledges.
    aes: AesToken = AesInterrupt => on_aes, line 18, context aes_done, hart 0, Hardware;
}

oer_espressif_executor::tasks! {
    /// The boot hart's tasks.
    Tasks: Executor<Esp32s31, 0> {
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
        measure(aes: InterruptingAes) = measure;
    }
}

/// The cycle counter at the AES handler's entry, and its count of entries.
static HANDLED_AT: AtomicU32 = AtomicU32::new(0);
static HANDLED: AtomicU32 = AtomicU32::new(0);

/// The AES interrupt's handler: stamp the entry, acknowledge, count.
fn on_aes(part: &mut AesInterrupt) {
    HANDLED_AT.store(oer_riscv::cycle32(), Ordering::Release);
    part.clear();
    HANDLED.fetch_add(1, Ordering::AcqRel);
}

/// The bound of a wait on the accelerator, in polls: far above a block's
/// time; a wait that reaches it counts as a timeout.
const SPIN_LIMIT: u32 = 100_000;

/// The key-encryption and the cipher key every case uses.
const KEY: [u8; 16] = *b"oer-aes-timing-k";
/// CCMP's nonce and a 22-byte AAD (a data frame without QoS or A4).
const CCM_NONCE: [u8; NONCE] = [0x5a; NONCE];
const AAD: [u8; 22] = [0xa5; 22];

/// RFC 3394 inputs: the PTK's group key data (32 bytes, wrapped 40) and the
/// largest RSN key data (504, wrapped 512).
const UNWRAP_PLAIN: [usize; 2] = [32, 504];
/// CCMP bodies and BIP management frames.
const CCM_BODIES: [usize; 3] = [64, 512, 1500];
const CMAC_LENGTHS: [usize; 2] = [64, 512];

oer_espressif_staged_runtime::entry!(main, Port);

fn main(handoff: oer_espressif_staged_runtime::Handoff<Port>) -> ! {
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32));
    };
    let ((timer, software, watchdogs, aes), _psram, mut adopted) =
        match oer_espressif_staged_runtime::adoption::run::<oer_esp32s31_compose::Esp32s31, _, _>(
            handoff,
            root.into_runtime(),
            INTERRUPT_TABLE,
            board::BootstrapPsram,
            oer_esp32s31_compose::phy_fault::report_previous,
            |hardware| {
                (
                    hardware.system_timer,
                    hardware.software_interrupts,
                    hardware.watchdogs,
                    hardware.aes,
                )
            },
        ) {
            Ok(adopted) => adopted.into_parts(),
            Err(error) => fail(error.name()),
        };
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_diagnostics::name!("interrupt-table"; 32));
    };
    let _timers = oer_espressif_executor::init::<Esp32s31, 0, _, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    let (aes, aes_interrupt) = aes.into_interrupting();
    let _bound = adopted
        .router()
        .bind(interrupts.aes, aes_interrupt)
        .unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<Port, _>(error)
        });
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let watchdog = watchdogs.rtc;
    text(b"OER-PROBE aes-timing ready\r\n");
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    match executor
        .run(&hart, &mut router, interrupts.wake, software.irq0, (), (), move |spawner| {
            if Tasks::rtc_watchdog(spawner, watchdog).is_err()
                || Tasks::measure(spawner, aes).is_err()
            {
                fail(oer_diagnostics::name!("task-start"; 32));
            }
        })
        .unwrap_or_else(|failure| failure.reset())
    {}
}

/// Every case, in order, then the verdict: every hardware result equals the
/// software one.
async fn measure(mut aes: InterruptingAes) {
    let mut agree = true;
    calibrate().await;
    software_cases().await;
    if !prepare(&mut aes) {
        text(b"OER-PROBE aes-timing accelerator-not-idle\r\n");
        text(b"OER-RESULT: fail aes-timing\r\n");
        return;
    }
    flag_latency(&mut aes);
    pause().await;
    agree &= hardware_cases(&mut aes, Wait::Flag, "flag").await;
    aes.enable_interrupt();
    if interrupt_latency(&mut aes) {
        pause().await;
        agree &= hardware_cases(&mut aes, Wait::Interrupt, "interrupt").await;
    } else {
        text(b"OER-PROBE aes-timing interrupt none-in-typical-mode\r\n");
    }
    agree &= wifi_mode(aes).await;
    text(if agree {
        b"OER-RESULT: pass aes-timing\r\n"
    } else {
        b"OER-RESULT: fail aes-timing\r\n"
    });
}

/// The CPU cycles of 10 ms of the system timer.
async fn calibrate() {
    let start = oer_riscv::cycle64();
    if EmbassyClock
        .wait_for(oer_time::Duration::from_millis(10))
        .await
        .is_err()
    {
        fail(oer_diagnostics::name!("calibrate-time"; 32));
    }
    let cycles = oer_riscv::cycle64().wrapping_sub(start);
    let mut line = Line::new();
    line.text("OER-PROBE aes-timing clock cycles-per-10ms=")
        .number(cycles)
        .text("\r\n");
    text(line.as_bytes());
}

/// Yield a millisecond between cases: the watchdog's feed runs.
async fn pause() {
    if EmbassyClock
        .wait_for(oer_time::Duration::from_millis(1))
        .await
        .is_err()
    {
        fail(oer_diagnostics::name!("pause-time"; 32));
    }
}

/// The product's software block (`SoftwareAes128`) through the same
/// operations, and CCMP's and BIP's RustCrypto calls as the product makes
/// them.
async fn software_cases() {
    report("software", "block", 16, &block_case(&mut SoftwareAes128));
    pause().await;
    for plain in UNWRAP_PLAIN {
        let (stats, _) = unwrap_case(SoftwareAes128, plain);
        report("software", "unwrap", plain.saturating_add(8), &stats);
        pause().await;
    }
    for body in CCM_BODIES {
        let (stats, _) = ccm_case(&mut SoftwareAes128, body);
        report("software", "ccm-seal", body, &stats);
        report("software-crate", "ccm-seal", body, &ccm_crate_case(body));
        pause().await;
    }
    for length in CMAC_LENGTHS {
        let (stats, _) = cmac_case(&mut SoftwareAes128, length);
        report("software", "cmac", length, &stats);
        report("software-crate", "cmac", length, &cmac_crate_case(length));
        pause().await;
    }
}

/// The hardware block through every operation, each result checked against
/// the software one.
async fn hardware_cases(aes: &mut InterruptingAes, wait: Wait, mode: &str) -> bool {
    let mut agree = true;
    let mut blocks = Hardware::new(aes, wait);
    report(mode, "block", 16, &block_case(&mut blocks));
    pause().await;
    for plain in UNWRAP_PLAIN {
        let (stats, plaintext) = unwrap_case(Hardware::new(&mut *blocks.aes, wait), plain);
        agree &= plaintext;
        report(mode, "unwrap", plain.saturating_add(8), &stats);
        pause().await;
    }
    for body in CCM_BODIES {
        let (stats, mic) = ccm_case(&mut blocks, body);
        agree &= Some(mic) == ccm_case(&mut SoftwareAes128, body).1.into();
        report(mode, "ccm-seal", body, &stats);
        pause().await;
    }
    for length in CMAC_LENGTHS {
        let (stats, tag) = cmac_case(&mut blocks, length);
        agree &= tag == cmac_case(&mut SoftwareAes128, length).1;
        report(mode, "cmac", length, &stats);
        pause().await;
    }
    if blocks.timeouts != 0 {
        let mut line = Line::new();
        line.text("OER-PROBE aes-timing ")
            .text(mode)
            .text(" timeouts=")
            .number(u64::from(blocks.timeouts))
            .text("\r\n");
        text(line.as_bytes());
        agree = false;
    }
    let mut line = Line::new();
    line.text("OER-PROBE aes-timing ")
        .text(mode)
        .text(if agree { " agrees" } else { " disagrees" })
        .text(" with software\r\n");
    text(line.as_bytes());
    agree
}

/// The Wi-Fi station's engine (`oer-esp32s31-crypto`, timer-woken samples
/// every 1 µs through the system timer), unwrapping as the station does.
async fn wifi_mode(aes: InterruptingAes) -> bool {
    let mut engine = oer_esp32s31_crypto::AesKeyUnwrap::new(
        aes,
        EmbassyClock,
        oer_esp32s31_compose::crypto::AesWait,
    );
    let mut agree = true;
    for plain in UNWRAP_PLAIN {
        let (plaintext, wrapped) = wrapped(plain);
        let mut stats = Stats::default();
        for _ in 0..runs(plain) {
            let Ok(data) = KeyData::<512>::try_copy(wrapped.as_bytes()) else {
                fail(oer_diagnostics::name!("wrapped-copy"; 32));
            };
            let request = KeyUnwrapRequest::new(Aes128Key::copy_from(&KEY), data);
            let start = oer_riscv::cycle32();
            let done = engine.unwrap_key(request).await;
            stats.add(oer_riscv::cycle32().wrapping_sub(start));
            agree &= done
                .response
                .plaintext()
                .is_ok_and(|bytes| bytes == plaintext.get(..plain).unwrap_or(&[]));
            engine = done.engine;
        }
        report("wifi", "unwrap", plain.saturating_add(8), &stats);
        pause().await;
    }
    text(if agree {
        b"OER-PROBE aes-timing wifi agrees with software\r\n"
    } else {
        b"OER-PROBE aes-timing wifi disagrees with software\r\n"
    });
    agree
}

/// How many runs a case of `bytes` takes: fewer for the long inputs.
fn runs(bytes: usize) -> u32 {
    match bytes {
        0..=64 => 32,
        65..=512 => 8,
        _ => 4,
    }
}

/// One encryption of a block, 256 times under one loaded key.
fn block_case<P: Aes128Blocks>(blocks: &mut P) -> Stats {
    let key = Aes128Key::copy_from(&KEY);
    let mut cipher = blocks.load_key(&key);
    let mut block: Block = [0x3c; 16];
    let mut stats = Stats::default();
    for _ in 0..256 {
        let start = oer_riscv::cycle32();
        cipher.encrypt_block(&mut block);
        stats.add(oer_riscv::cycle32().wrapping_sub(start));
    }
    stats
}

/// `plain` bytes of key data and their RFC 3394 wrap.
fn wrapped(plain: usize) -> ([u8; 504], KeyData<512>) {
    let mut plaintext = [0_u8; 504];
    for (index, byte) in plaintext.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(13);
    }
    let Ok(wrapped) = software_aes128_key_wrap(&KEY, plaintext.get(..plain).unwrap_or(&[])) else {
        fail(oer_diagnostics::name!("key-wrap"; 32));
    };
    (plaintext, wrapped)
}

/// RFC 3394 unwraps of `plain` bytes of key data, the key loaded each time
/// as the station does; whether every plaintext came back.
fn unwrap_case<P: Aes128Blocks>(blocks: P, plain: usize) -> (Stats, bool) {
    let (plaintext, wrapped) = wrapped(plain);
    let mut engine = BlockKeyUnwrap::new(blocks);
    let mut stats = Stats::default();
    let mut agree = true;
    for _ in 0..runs(plain) {
        let Ok(data) = KeyData::<512>::try_copy(wrapped.as_bytes()) else {
            fail(oer_diagnostics::name!("wrapped-copy"; 32));
        };
        let request = KeyUnwrapRequest::new(Aes128Key::copy_from(&KEY), data);
        let start = oer_riscv::cycle32();
        let response = engine.unwrap_now(request);
        stats.add(oer_riscv::cycle32().wrapping_sub(start));
        agree &= response
            .plaintext()
            .is_ok_and(|bytes| bytes == plaintext.get(..plain).unwrap_or(&[]));
    }
    (stats, agree)
}

/// CCMP-128 seals of a `body`-byte payload, the key loaded each time; the
/// last MIC.
fn ccm_case<P: Aes128Blocks>(blocks: &mut P, body: usize) -> (Stats, [u8; 8]) {
    let mut payload = [0_u8; 1500];
    let mut stats = Stats::default();
    let mut mic = [0; 8];
    for _ in 0..runs(body) {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let start = oer_riscv::cycle32();
        let key = Aes128Key::copy_from(&KEY);
        let mut cipher = blocks.load_key(&key);
        mic = ccm_seal(
            &mut cipher,
            &CCM_NONCE,
            &AAD,
            payload.get_mut(..body).unwrap_or(&mut []),
        );
        drop(cipher);
        stats.add(oer_riscv::cycle32().wrapping_sub(start));
    }
    (stats, mic)
}

/// CCMP-128 seals through RustCrypto's `ccm`, as the product's software
/// CCMP calls it (key schedule included).
fn ccm_crate_case(body: usize) -> Stats {
    use ccm::aead::{AeadInPlace as _, KeyInit as _};
    type CcmpCcm = ccm::Ccm<aes::Aes128, ccm::consts::U8, ccm::consts::U13>;
    let mut payload = [0_u8; 1500];
    let mut stats = Stats::default();
    for _ in 0..runs(body) {
        let start = oer_riscv::cycle32();
        let cipher = CcmpCcm::new(&KEY.into());
        let _tag = cipher.encrypt_in_place_detached(
            &CCM_NONCE.into(),
            &AAD,
            payload.get_mut(..body).unwrap_or(&mut []),
        );
        stats.add(oer_riscv::cycle32().wrapping_sub(start));
    }
    stats
}

/// BIP AES-CMACs over a `length`-byte frame, the key loaded each time; the
/// last tag.
fn cmac_case<P: Aes128Blocks>(blocks: &mut P, length: usize) -> (Stats, Block) {
    let mut frame = [0_u8; 512];
    for (index, byte) in frame.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(5);
    }
    let mut stats = Stats::default();
    let mut tag = [0; 16];
    for _ in 0..runs(length) {
        let start = oer_riscv::cycle32();
        let key = Aes128Key::copy_from(&KEY);
        let mut cipher = blocks.load_key(&key);
        tag = cmac(&mut cipher, frame.get(..length).unwrap_or(&[]));
        drop(cipher);
        stats.add(oer_riscv::cycle32().wrapping_sub(start));
    }
    (stats, tag)
}

/// BIP AES-CMACs through RustCrypto's `cmac`, as the product's BIP calls it.
fn cmac_crate_case(length: usize) -> Stats {
    use cmac::Mac as _;
    let frame = [0x5a_u8; 512];
    let mut stats = Stats::default();
    for _ in 0..runs(length) {
        let start = oer_riscv::cycle32();
        let mut mac = <cmac::Cmac<aes::Aes128> as cmac::digest::KeyInit>::new(&KEY.into());
        mac.update(frame.get(..length).unwrap_or(&[]));
        let _tag = mac.finalize();
        stats.add(oer_riscv::cycle32().wrapping_sub(start));
    }
    stats
}

/// How the hardware block waits for its end.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Wait {
    /// Poll `STATE` until it reads idle.
    Flag,
    /// Spin until the AES handler counts an entry.
    Interrupt,
}

/// The accelerator as a block primitive of one wait.
struct Hardware<'a> {
    aes: &'a mut InterruptingAes,
    wait: Wait,
    timeouts: u32,
}

impl<'a> Hardware<'a> {
    fn new(aes: &'a mut InterruptingAes, wait: Wait) -> Self {
        Self {
            aes,
            wait,
            timeouts: 0,
        }
    }
}

/// The accelerator with a key loaded, in the mode of its last block.
struct Keyed<'k> {
    aes: &'k mut InterruptingAes,
    wait: Wait,
    timeouts: &'k mut u32,
    encrypting: Option<bool>,
}

impl Aes128Blocks for Hardware<'_> {
    type Keyed<'k>
        = Keyed<'k>
    where
        Self: 'k;

    fn load_key(&mut self, key: &Aes128Key) -> Keyed<'_> {
        let mut words = KeyWords::default();
        for (word, chunk) in words.iter_mut().zip(key.as_bytes().chunks_exact(4)) {
            let mut bytes = [0; 4];
            for (out, byte) in bytes.iter_mut().zip(chunk) {
                *out = *byte;
            }
            *word = u32::from_le_bytes(bytes);
        }
        self.aes.write_key(&words);
        Keyed {
            aes: &mut *self.aes,
            wait: self.wait,
            timeouts: &mut self.timeouts,
            encrypting: None,
        }
    }
}

impl Keyed<'_> {
    fn run(&mut self, block: &mut Block) {
        let mut words = BlockWords::default();
        for (word, chunk) in words.iter_mut().zip(block.chunks_exact(4)) {
            let mut bytes = [0; 4];
            for (out, byte) in bytes.iter_mut().zip(chunk) {
                *out = *byte;
            }
            *word = u32::from_le_bytes(bytes);
        }
        self.aes.write_block(&words);
        let finished = match self.wait {
            Wait::Flag => {
                self.aes.start();
                (0..SPIN_LIMIT).any(|_| self.aes.state() == IDLE)
            }
            Wait::Interrupt => {
                let before = HANDLED.load(Ordering::Acquire);
                self.aes.start();
                (0..SPIN_LIMIT).any(|_| HANDLED.load(Ordering::Acquire) != before)
            }
        };
        if !finished {
            *self.timeouts = self.timeouts.saturating_add(1);
        }
        let out = self.aes.read_block();
        for (chunk, word) in block.chunks_exact_mut(4).zip(out) {
            for (byte, value) in chunk.iter_mut().zip(word.to_le_bytes()) {
                *byte = value;
            }
        }
    }
}

impl BlockCipher for Keyed<'_> {
    fn encrypt_block(&mut self, block: &mut Block) {
        if self.encrypting != Some(true) {
            self.aes.encrypt128();
            self.encrypting = Some(true);
        }
        self.run(block);
    }

    fn decrypt_block(&mut self, block: &mut Block) {
        if self.encrypting != Some(false) {
            self.aes.decrypt128();
            self.encrypting = Some(false);
        }
        self.run(block);
    }
}

/// The accelerator's setup, as the Wi-Fi engine's: clock on, out of reset,
/// no DMA, no pseudo rounds, not preemptible; whether it reads idle.
fn prepare(aes: &mut InterruptingAes) -> bool {
    aes.clock(true);
    // The module reset, through the trait: no terminal reset.
    Registers::reset(aes, true);
    Registers::reset(aes, false);
    aes.release_parent_resets();
    aes.disable_interrupt();
    aes.disable_dma();
    aes.disable_pseudo_rounds();
    aes.hold();
    aes.state() == IDLE
}

/// From the start of a block to `STATE` reading idle, polled back to back.
fn flag_latency(aes: &mut InterruptingAes) {
    aes.encrypt128();
    aes.write_key(&KeyWords::default());
    aes.write_block(&BlockWords::default());
    let mut stats = Stats::default();
    for _ in 0..256 {
        let start = oer_riscv::cycle32();
        aes.start();
        let idle = (0..SPIN_LIMIT).any(|_| aes.state() == IDLE);
        let end = oer_riscv::cycle32();
        if idle {
            stats.add(end.wrapping_sub(start));
        }
    }
    report("flag", "start-to-idle", 16, &stats);
}

/// From the start of a block to the AES handler's entry; whether the
/// interrupt came in Typical mode.
fn interrupt_latency(aes: &mut InterruptingAes) -> bool {
    let mut stats = Stats::default();
    let mut missed = 0_u32;
    for _ in 0..256 {
        let before = HANDLED.load(Ordering::Acquire);
        let start = oer_riscv::cycle32();
        aes.start();
        if (0..SPIN_LIMIT).any(|_| HANDLED.load(Ordering::Acquire) != before) {
            stats.add(HANDLED_AT.load(Ordering::Acquire).wrapping_sub(start));
        } else {
            missed = missed.saturating_add(1);
        }
    }
    report("interrupt", "start-to-handler", 16, &stats);
    if missed != 0 {
        let mut line = Line::new();
        line.text("OER-PROBE aes-timing interrupt missed=")
            .number(u64::from(missed))
            .text("\r\n");
        text(line.as_bytes());
    }
    stats.runs != 0 && missed == 0
}

fn report(mode: &str, operation: &str, bytes: usize, stats: &Stats) {
    text(Line::case(mode, operation, bytes, stats).as_bytes());
}

/// Feed the RTC watchdog for good.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Write `bytes` through the early writer, within its bounded budget.
fn text(bytes: &[u8]) {
    let _written = usb_serial_jtag::early_text(bytes);
}

/// Retain and report a failed boot step before explicitly resetting the image.
fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    oer_espressif_staged_runtime::fatal::boot_failure::<Port>(reason.as_str(), || {
        let _note = usb_serial_jtag::fatal_note(&oer_diagnostics::Note::runtime_fail(reason));
    })
}

oer_espressif_executor::time_driver!(Esp32s31);
