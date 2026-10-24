//! What the executor takes from the chip through esp-hal, in one place: the
//! software interrupts that pend and wake executors, the wait for an
//! interrupt, the current core and the system timer's counter. The SYSTIMER
//! comparators are ours ([`crate::systimer`]), and so is the read of the
//! current interrupt level ([`interrupt_level`]).

pub(crate) use esp_hal::{
    interrupt::{
        InterruptStatus, Priority,
        software::{SoftwareInterrupt, SoftwareInterruptAcknowledger, SoftwareInterruptSender},
        wait_for_interrupt,
    },
    peripherals::{Interrupt, SYSTIMER},
    system::Cpu,
    time::Instant,
    timer::systimer::SystemTimer,
};

/// The software interrupts an executor can own.
pub(crate) const SOFTWARE_INTERRUPT_COUNT: usize = crate::task::EXECUTORS as usize;

/// The core running the caller.
#[inline(always)]
pub(crate) fn current_core() -> usize {
    Cpu::current() as usize
}

/// The interrupt level the caller runs at: the CLIC's `mintstatus.mil`,
/// which the hart raises on a trap's entry to the taken interrupt's level
/// and restores on its return; 0 in thread mode. Both chips' cores take
/// interrupts through a CLIC.
#[inline(always)]
pub(crate) fn interrupt_level() -> u8 {
    let status: usize;
    #[allow(unsafe_code, reason = "a CSR read is an instruction")]
    // SAFETY: `csrr` of `mintstatus`, a read-only CSR, writes only the output
    // register; it touches no memory and no other state.
    unsafe {
        core::arch::asm!(
            "csrr {status}, {mintstatus}",
            status = out(reg) status,
            mintstatus = const oer_riscv::clic::MINTSTATUS,
            options(nomem, nostack, preserves_flags),
        );
    }
    // `mil` is the top byte: the truncation keeps exactly the field.
    (status >> oer_riscv::clic::MINTSTATUS_MIL_SHIFT) as u8
}

/// The system timer's counter, in microseconds.
// Share the same clock read with cooperative admission and poll stamping.
#[inline(never)]
pub(crate) fn now() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

/// The system timer's ticks per second.
pub(crate) fn ticks_per_second() -> u64 {
    SystemTimer::ticks_per_second()
}

/// Whether `source` is pending at the interrupt matrix.
#[inline(always)]
pub(crate) fn is_pending(source: Interrupt) -> bool {
    InterruptStatus::is_pending(source)
}

/// The peripheral interrupt source of software interrupt `SWI`.
pub(crate) const fn software_interrupt_source(swi: u8) -> Interrupt {
    match swi {
        0 => Interrupt::FROM_CPU_INTR0,
        1 => Interrupt::FROM_CPU_INTR1,
        2 => Interrupt::FROM_CPU_INTR2,
        _ => Interrupt::FROM_CPU_INTR3,
    }
}

// The sender is published once for wakers; only the line's ISR borrows
// its acknowledger. Boot resets the line before either half is published.
pub(crate) enum Sender {
    Zero(SoftwareInterruptSender<'static, 0>),
    One(SoftwareInterruptSender<'static, 1>),
    Two(SoftwareInterruptSender<'static, 2>),
    Three(SoftwareInterruptSender<'static, 3>),
}
pub(crate) enum Acknowledger {
    Zero(SoftwareInterruptAcknowledger<'static, 0>),
    One(SoftwareInterruptAcknowledger<'static, 1>),
    Two(SoftwareInterruptAcknowledger<'static, 2>),
    Three(SoftwareInterruptAcknowledger<'static, 3>),
}
static SENDERS: [oer_memory::zeroed::ZeroedOnce<Sender>; SOFTWARE_INTERRUPT_COUNT] =
    [const { oer_memory::zeroed::ZeroedOnce::new() }; SOFTWARE_INTERRUPT_COUNT];
static ACK_STORAGE: [static_cell::StaticCell<Acknowledger>; SOFTWARE_INTERRUPT_COUNT] =
    [const { static_cell::StaticCell::new() }; SOFTWARE_INTERRUPT_COUNT];
static ACKNOWLEDGERS: [oer_memory::HandlerCell<Acknowledger>; SOFTWARE_INTERRUPT_COUNT] =
    [const { oer_memory::HandlerCell::new() }; SOFTWARE_INTERRUPT_COUNT];

impl Acknowledger {
    fn reset(&mut self) {
        match self {
            Self::Zero(ack) => ack.reset(),
            Self::One(ack) => ack.reset(),
            Self::Two(ack) => ack.reset(),
            Self::Three(ack) => ack.reset(),
        }
    }
}

/// Install software interrupt `swi`'s two halves: its sender for wakers,
/// its acknowledger for its handler.
///
/// # Panics
/// When the line is installed already.
pub(crate) fn install_line(swi: usize, sender: Sender, mut ack: Acknowledger) {
    ack.reset();
    let (Some(senders), Some(acknowledger), Some(storage)) = (
        SENDERS.get(swi),
        ACKNOWLEDGERS.get(swi),
        ACK_STORAGE.get(swi),
    ) else {
        panic!("no such software interrupt");
    };
    assert!(
        senders.get().is_none(),
        "software interrupt owner is already installed"
    );
    let _sender = senders.get_or_init(|| sender);
    assert!(
        acknowledger.install(storage.init(ack)).is_ok(),
        "software interrupt acknowledger is already installed"
    );
}

/// Raise software interrupt `swi`.
#[inline(always)]
pub(crate) fn raise_interrupt(swi: u8) {
    let raised = SENDERS
        .get(usize::from(swi))
        .and_then(oer_memory::zeroed::ZeroedOnce::get)
        .map(|sender| match sender {
            Sender::Zero(sender) => sender.raise(),
            Sender::One(sender) => sender.raise(),
            Sender::Two(sender) => sender.raise(),
            Sender::Three(sender) => sender.raise(),
        });
    assert!(
        raised.is_some(),
        "software interrupt sender is installed before use"
    );
}

/// Reset software interrupt `swi`, from its handler.
#[inline(always)]
pub(crate) fn acknowledge_interrupt(swi: u8) {
    assert!(
        ACKNOWLEDGERS
            .get(usize::from(swi))
            .and_then(|ack| ack.with(Acknowledger::reset))
            .is_some(),
        "only the installed software interrupt handler acknowledges its line"
    );
}
