//! The Bluetooth interrupts' one handler (P3) and the statics it reaches:
//! the gate's queues, the doorbell's lowering half, the workers' signals and
//! the scheduler channel. The part is bound once at boot and serves every
//! epoch; the task opens and closes each epoch through the gate.
use super::*;
use core::sync::atomic::AtomicBool;
use oer_esp32s31_bluetooth_driver::interrupt::{
    BluetoothInterruptGate, BluetoothInterruptSignals, BluetoothInterruptStorage,
    BluetoothInterrupts, ChipGate, DoorbellLine, ModemLpTimerPort, RadioDoorbell,
};
use oer_esp32s31_bluetooth_driver::runtime_resources::ControllerEventCells;
use oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerInterruptChannel;
use oer_esp32s31_hal::platform::{FromCpu3, FromCpu3Irq};
use oer_espressif_executor::{Doorbell, DoorbellAcknowledger};
use oer_interrupt_table::{
    Controller, Router,
    signal::{FromHandler, IrqSignal},
};

/// The doorbell's lowering half, which the handler's part holds.
pub struct Ack(DoorbellAcknowledger<3>);

impl DoorbellLine for Ack {
    fn acknowledge(&mut self) {
        self.0.acknowledge();
    }
}

/// The handler's part: the bank's primary and NRT sources, the modem
/// low-power timer and the radio doorbell, on one line.
pub type Interrupts = BluetoothInterrupts<X, Ack>;

/// The doorbell's ringing half, which the task's ends share.
pub struct Bell<
    E: oer_interrupt_table::Entry = crate::BtToken,
    R: oer_interrupt_table::Entry = crate::RadioToken,
>(Doorbell<3, E, R>);

impl<E: oer_interrupt_table::Entry, R: oer_interrupt_table::Entry> RadioDoorbell for &Bell<E, R> {
    const HANDLER: oer_interrupt_table::Context = Doorbell::<3, E, R>::HANDLER;
    const EXECUTOR: oer_interrupt_table::Context = Doorbell::<3, E, R>::EXECUTOR;
    fn ring(&self) {
        self.0.ring();
    }
}

/// The task's end of the gate.
pub type Gate = BluetoothInterruptGate<ChipGate, X, &'static Bell>;
/// The task's end of the timer's software work.
pub type TimerPort = ModemLpTimerPort<ChipGate, &'static Bell>;
/// The scheduler channel's doorbell the task rings.
pub type SchedulerBell =
    oer_esp32s31_bluetooth_driver::interrupt::SchedulerDoorbell<X, &'static Bell>;

oer_espressif_staged_runtime::placed_static! {
    static STORAGE: StaticCell<BluetoothInterruptStorage<ChipGate>> = StaticCell::new(), in critical_data "b0";
    pub static FAULT: AtomicBool = AtomicBool::new(false), in critical_data "b0";
    pub static GATE_FAULT: oer_memory::OnceValue<oer_esp32s31_bluetooth_driver::interrupt::gate::GateFault> = oer_memory::OnceValue::new(), in critical_data "b0";
    pub static SCHEDULER_SIGNAL: IrqSignal<X> = IrqSignal::new::<FromHandler<crate::BtToken>, crate::RadioToken>(), in critical_data "b0";
    pub static REPLY: IrqSignal<X> = IrqSignal::new::<FromHandler<crate::BtToken>, crate::RadioToken>(), in critical_data "b0";
    pub static TIMER_SIGNAL: IrqSignal<X> = IrqSignal::new::<FromHandler<crate::BtToken>, crate::RadioToken>(), in critical_data "b0";
    pub static CHANNEL: SchedulerInterruptChannel = SchedulerInterruptChannel::new(), in critical_data "b0";
    pub static CELLS: ControllerEventCells<X> = ControllerEventCells::new(&SCHEDULER_SIGNAL, &TIMER_SIGNAL), in critical_data "b0";
}
static BELL: StaticCell<Bell> = StaticCell::new();
static PORT: StaticCell<TimerPort> = StaticCell::new();

/// The line's handler: it serves the part its entry holds.
pub fn bt(part: &mut Interrupts) {
    if let Err(fault) = part.service(&EmbassyClock) {
        crate::gate_fault::reset(fault);
    }
}

/// What the boot gets from binding the handler's part.
pub struct Bound {
    pub gate: Gate,
    pub port: &'static mut TimerPort,
    pub bell: SchedulerBell,
}

/// Why the handler's part could not be bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindError {
    /// The static storage was taken before.
    Storage,
    /// The router refused the entry.
    Route,
}

/// Bind the handler's part once, over software interrupt line 3, then make
/// the doorbell's ringing half the gate and the timer's port share.
pub fn bind<C: Controller>(
    router: &mut Router<C>,
    token: BtToken,
    line: FromCpu3,
    irq: FromCpu3Irq,
    status: oer_esp32s31_hal::bluetooth::ModemLpTimerStatus,
) -> Result<Bound, BindError> {
    let (line, ack) = oer_espressif_executor::DoorbellLine::<3>::split(line);
    let storage = STORAGE
        .try_init(BluetoothInterruptStorage::new())
        .ok_or(BindError::Storage)?;
    let (ends, part) = storage.split(
        irq,
        status,
        Ack(ack),
        BluetoothInterruptSignals {
            reply: &REPLY,
            cells: &CELLS,
            fault: &GATE_FAULT,
        },
    );
    let bound = router.bind(token, part).map_err(|_| BindError::Route)?;
    let doorbell = BELL
        .try_init(Bell(Doorbell::new::<FromCpu3Irq>(line, &bound)))
        .ok_or(BindError::Storage)?;
    let ring: &Bell = doorbell;
    let (gate, port) = ends.attach(&bound, ring);
    let port = PORT.try_init(port).ok_or(BindError::Storage)?;
    Ok(Bound {
        gate,
        port,
        bell: SchedulerBell::new(ring, &REPLY),
    })
}
