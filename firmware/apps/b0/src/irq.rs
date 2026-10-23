//! P3 storage and affine source-127 transfers. P2 only changes ownership at
//! the route-off lifecycle boundary; live handlers never share a task borrow.
use super::*;
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};
use esp_hal::interrupt::software::SoftwareInterruptAcknowledger;
use oer_esp32s31_bluetooth_driver::port::Fault;
use oer_esp32s31_bluetooth_driver::{
    interrupt::{ControllerInterruptHalf, SchedulerWakeCell},
    modem_lp_timer_queue::{ModemLpTimerStableInterruptStep, ModemLpTimerWorkerWakeCell},
    modem_timer::{ModemLpTimerRetirementStorage, ModemLpTimerSoftwareOwnerStorage},
};
use oer_esp32s31_hal::bluetooth::{
    ModemLpTimerHandlerRegisterStep, ModemLpTimerInterruptReadyOwner, ModemLpTimerInterruptStep,
    ModemLpTimerSoftwarePendingOwner,
};
use oer_espressif_executor::{Doorbell, SameLevel};
use oer_firmware_apps_b0::routes::{self, Routes};
use oer_interrupt_table::signal::IrqSignal;
use oer_memory::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, CompactSpscCursor, HandlerCell,
};

type Half = ControllerInterruptHalf<X>;
type Q<T> = AffineSpscQueue<T, 1, CompactSpscCursor>;
type Tx<T> = AffineSpscSender<'static, T, 1, CompactSpscCursor>;
type Rx<T> = AffineSpscReceiver<'static, T, 1, CompactSpscCursor>;
struct TimerQueues {
    pending: Q<ModemLpTimerSoftwarePendingOwner>,
    ready: Q<ModemLpTimerInterruptReadyOwner>,
}
impl TimerQueues {
    const fn new() -> Self {
        Self {
            pending: Q::new(),
            ready: Q::new(),
        }
    }
}
enum TimerState {
    Vacant,
    Ready(ModemLpTimerInterruptReadyOwner),
}
struct State {
    half: Option<Half>,
    timer: TimerState,
    pending: Tx<ModemLpTimerSoftwarePendingOwner>,
    ready: Rx<ModemLpTimerInterruptReadyOwner>,
    blocked_pending: Option<ModemLpTimerSoftwarePendingOwner>,
    blocked_ready: Option<ModemLpTimerInterruptReadyOwner>,
}

oer_espressif_staged_runtime::placed_static! {
    static STATE:HandlerCell<State>=HandlerCell::new(),in critical_data "b0";
    static STORAGE:StaticCell<State>=StaticCell::new(),in critical_data "b0";
    static QUEUES:StaticCell<TimerQueues>=StaticCell::new(),in critical_data "b0";
    static ACK:HandlerCell<SoftwareInterruptAcknowledger<'static, 3>>=HandlerCell::new(),in critical_data "b0";
    static ACK_STORE:StaticCell<SoftwareInterruptAcknowledger<'static, 3>>=StaticCell::new(),in critical_data "b0";
    pub static FAULT:AtomicBool=AtomicBool::new(false),in critical_data "b0";
    pub static SCHEDULER_SIGNAL:IrqSignal<X>=IrqSignal::new(),in critical_data "b0";
    pub static REPLY:IrqSignal<X>=IrqSignal::new(),in critical_data "b0";
    pub static TIMER_SIGNAL:IrqSignal<X>=IrqSignal::new(),in critical_data "b0";
    pub static TIMER_WAKE:ModemLpTimerWorkerWakeCell<X>=ModemLpTimerWorkerWakeCell::new(&TIMER_SIGNAL),in critical_data "b0";
    pub static WAKE:SchedulerWakeCell<X>=SchedulerWakeCell::new(&SCHEDULER_SIGNAL),in critical_data "b0";
    pub static CHANNEL:oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerInterruptChannel=oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerInterruptChannel::new(),in critical_data "b0";
}

#[derive(Clone, Copy)]
pub struct Bell(&'static Doorbell<3>);
static BELL: StaticCell<Doorbell<3>> = StaticCell::new();
impl oer_esp32s31_bluetooth_driver::interrupt::RadioDoorbell for Bell {
    fn ring(&self) {
        self.0.ring();
    }
}
impl Bell {
    pub fn ring(&self) {
        self.0.ring();
    }
}
pub type SchedulerBell = oer_esp32s31_bluetooth_driver::interrupt::SchedulerDoorbell<X, Bell>;
/// P2-only endpoint loans. The queues alone cross the context boundary.
pub struct TimerPort {
    pending: RefCell<Rx<ModemLpTimerSoftwarePendingOwner>>,
    ready: RefCell<Tx<ModemLpTimerInterruptReadyOwner>>,
    bell: Bell,
    returned: RefCell<Option<ModemLpTimerInterruptReadyOwner>>,
}
static TIMER_PORT: StaticCell<TimerPort> = StaticCell::new();

pub fn initialize(
    soft: SoftwareInterrupt<'static, 3>,
    token: DoorbellToken,
) -> (
    SchedulerBell,
    &'static mut TimerPort,
    oer_espressif_esp_hal_irq::Route,
) {
    let queues = QUEUES.init(TimerQueues::new());
    let (pending, receive) = queues.pending.split();
    let (restore, ready) = queues.ready.split();
    let _ = STATE
        .install(STORAGE.init(State {
            half: None,
            timer: TimerState::Vacant,
            pending,
            ready,
            blocked_pending: None,
            blocked_ready: None,
        }))
        .ok()
        .expect("one B0 handler storage");
    let (bell, ack) = Doorbell::new(soft, token, SameLevel::<PrimaryToken, DoorbellToken>::new());
    let (ack, route) = ack.into_parts();
    let _ = ACK
        .install(ACK_STORE.init(ack))
        .ok()
        .expect("one radio doorbell acknowledger");
    let bell = BELL.init(bell);
    let port = TIMER_PORT.init(TimerPort {
        pending: RefCell::new(receive),
        ready: RefCell::new(restore),
        bell: Bell(bell),
        returned: RefCell::new(None),
    });
    // Doorbell is a copyable raising endpoint; only ACK owns its reset half.
    (
        oer_esp32s31_bluetooth_driver::interrupt::SchedulerDoorbell::new(Bell(bell), &REPLY),
        port,
        route,
    )
}

pub fn doorbell() {
    let _ = ACK.with(|ack| ack.reset());
    let _ = STATE.with(|s| {
        if let Ok(owner) = s.ready.try_receive() {
            if matches!(s.timer, TimerState::Vacant) {
                s.timer = TimerState::Ready(owner);
            } else {
                s.blocked_ready = Some(owner);
                FAULT.store(true, Ordering::Release);
                TIMER_SIGNAL.raise(1);
            }
        }
        if let Some(half) = s.half.as_mut() {
            half.service_doorbell(&EmbassyClock);
        }
    });
}
pub fn primary() {
    let _ = STATE.with(|s| {
        if let Some(half) = s.half.as_mut() {
            let _ = WAKE.publish_from_interrupt(half.service_primary(&EmbassyClock));
        }
    });
}
pub fn nrt() {
    let _ = STATE.with(|s| {
        if let Some(half) = s.half.as_mut() {
            let _ = half.service_nrt();
        }
    });
}
pub fn timer() {
    let _ = STATE.with(|s| {
        let TimerState::Ready(owner) = core::mem::replace(&mut s.timer, TimerState::Vacant) else {
            return;
        };
        let (ready, step) = match owner.step() {
            ModemLpTimerInterruptStep::Spurious(owner) => {
                (Some(owner), ModemLpTimerStableInterruptStep::Spurious)
            }
            ModemLpTimerInterruptStep::HandlerPending(p) => match p.step_registers() {
                ModemLpTimerHandlerRegisterStep::Rearmed(owner) => {
                    (Some(owner), ModemLpTimerStableInterruptStep::Rearmed)
                }
                ModemLpTimerHandlerRegisterStep::SoftwarePending(owner) => {
                    if let Err(rejected) = s.pending.try_send(owner) {
                        s.blocked_pending = Some(rejected.0);
                        FAULT.store(true, Ordering::Release);
                    }
                    (None, ModemLpTimerStableInterruptStep::SoftwarePending)
                }
            },
        };
        if let Some(owner) = ready {
            s.timer = TimerState::Ready(owner);
        }
        let _ = step.publish(&TIMER_WAKE);
    });
}

/// The fourth route is the doorbell; it is masked before STATE can move.
struct Control {
    primary: PrimaryToken,
    nrt: NrtToken,
    timer: TimerToken,
    doorbell: oer_espressif_esp_hal_irq::Route,
}
impl routes::Control for Control {
    fn enable(&mut self) -> bool {
        oer_espressif_esp_hal_irq::enable(&self.primary).is_ok()
            && oer_espressif_esp_hal_irq::enable(&self.nrt).is_ok()
            && oer_espressif_esp_hal_irq::enable(&self.timer).is_ok()
            && oer_espressif_esp_hal_irq::enable_route(&self.doorbell).is_ok()
    }
    fn disable_radio(&mut self) {
        oer_espressif_esp_hal_irq::disable(&self.primary);
        oer_espressif_esp_hal_irq::disable(&self.nrt);
        oer_espressif_esp_hal_irq::disable(&self.timer);
    }
    fn disable_doorbell(&mut self) {
        oer_espressif_esp_hal_irq::disable_route(&self.doorbell);
    }
}
pub struct Inactive(Routes<Control, routes::Off>);
pub struct Active(Routes<Control, routes::Live>);
pub struct Draining(Routes<Control, routes::Draining>);
impl Inactive {
    pub fn new(
        primary: PrimaryToken,
        nrt: NrtToken,
        timer: TimerToken,
        doorbell: oer_espressif_esp_hal_irq::Route,
    ) -> Self {
        Self(Routes::new(Control {
            primary,
            nrt,
            timer,
            doorbell,
        }))
    }
    pub fn publish(
        &self,
        half: Half,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Half, ModemLpTimerInterruptReadyOwner)> {
        self.0.handover(|| {
            STATE
                .with(|s| {
                    if s.half.is_some() || !matches!(s.timer, TimerState::Vacant) {
                        return Err((half, timer));
                    }
                    s.half = Some(half);
                    s.timer = TimerState::Ready(timer);
                    Ok(())
                })
                .expect("all routes are masked during P2 handover")
        })
    }
    pub fn enable(self) -> Result<Active, Self> {
        self.0.enable().map(Active).map_err(Inactive)
    }
    /// Same-core handlers have returned before P2 resumes with all routes off.
    pub fn take(&self) -> Result<(Half, ModemLpTimerInterruptReadyOwner), Fault> {
        self.0.handover(|| {
            STATE
                .with(|s| {
                    if s.half.is_none() || !matches!(s.timer, TimerState::Ready(_)) {
                        return Err(Fault::Hardware);
                    }
                    let half = s.half.take().expect("checked half");
                    let TimerState::Ready(timer) =
                        core::mem::replace(&mut s.timer, TimerState::Vacant)
                    else {
                        unreachable!("checked ready timer")
                    };
                    Ok((half, timer))
                })
                .ok_or(Fault::Hardware)?
        })
    }
}
impl Active {
    pub fn disable(self) -> Draining {
        Draining(self.0.drain())
    }
}
impl Draining {
    pub fn finish(self) -> Inactive {
        Inactive(self.0.finish())
    }
}
impl ModemLpTimerSoftwareOwnerStorage for TimerPort {
    type TakeError = Fault;
    type RestoreError = Fault;
    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<ModemLpTimerSoftwarePendingOwner, Fault> {
        self.pending
            .borrow_mut()
            .try_receive()
            .map_err(|_| Fault::Hardware)
    }
    fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Fault, ModemLpTimerInterruptReadyOwner)> {
        self.ready
            .borrow_mut()
            .try_send(owner)
            .map_err(|e| (Fault::Hardware, e.0))?;
        self.bell.ring();
        Ok(())
    }
}
impl ModemLpTimerRetirementStorage for TimerPort {
    type RetireError = Fault;
    fn take_modem_lp_timer_ready_after_routes_disabled(
        &self,
    ) -> Result<ModemLpTimerInterruptReadyOwner, Fault> {
        self.returned.borrow_mut().take().ok_or(Fault::Hardware)
    }
}
impl TimerPort {
    pub fn park_after_disable(&self, owner: ModemLpTimerInterruptReadyOwner) {
        assert!(self.returned.borrow().is_none(), "one returned timer owner");
        *self.returned.borrow_mut() = Some(owner);
    }
}
