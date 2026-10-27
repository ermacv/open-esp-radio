//! Affine SAR/PWDET custody between the radio actor and its P2 TIMG0 entry.
use crate::tx::sar::{Engine, Event};
pub use crate::tx::sar::{Error, Readiness, Sample};
use core::{
    future::Future,
    task::{Context, Poll},
};
use oer_esp32s31_hal::{
    domain::transaction::{FaultHandle, PhyHardware, PhyTransaction, SarTransferFault},
    platform::timg0_alarm::{Timg0AlarmInterrupt, Timg0AlarmProgram},
};
use oer_interrupt_table::{
    Bound, Entry,
    signal::{IrqSignal, SignalExecutor},
};
use oer_memory::{AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, CompactSpscCursor};
use oer_time::{Clock, Duration};

type Queue<I> = AffineSpscQueue<Bundle<I>, 1, CompactSpscCursor>;
type Sender<I> = AffineSpscSender<'static, Bundle<I>, 1, CompactSpscCursor>;
type Receiver<I> = AffineSpscReceiver<'static, Bundle<I>, 1, CompactSpscCursor>;
struct Bundle<I: Entry + 'static> {
    phy: PhyTransaction,
    alarm: Timg0AlarmProgram,
    bound: Bound<I>,
    engine: Engine,
    result: Option<Result<Sample, Error>>,
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static> Bundle<I> {
    fn arm(&mut self, delay: Duration) {
        if self.alarm.arm_after(delay, &self.bound).is_err() {
            self.phy
                .sar_transfer_reporter()
                .quarantine(SarTransferFault::InvalidDelay { requested: delay });
        }
    }
}
/// Permanent one-slot request and return cells; endpoints are split once.
pub struct Cells<I: Entry + 'static> {
    request: Queue<I>,
    reply: Queue<I>,
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static> Cells<I> {
    pub const fn new() -> Self {
        Self {
            request: Queue::new(),
            reply: Queue::new(),
        }
    }
    /// Startup stops/masks the program and explicitly CLR-acknowledges stale
    /// RAW before a request may unmask. Rearm itself does not clear RAW.
    pub fn split<E: SignalExecutor>(
        &'static mut self,
        mut alarm: Timg0AlarmProgram,
        interrupt: &mut Timg0AlarmInterrupt,
        signal: &'static IrqSignal<E>,
    ) -> (UnboundClient<I, E>, Interrupt<I, E>) {
        alarm.disarm();
        let _stale = interrupt.acknowledge();
        let (send, receive) = self.request.split();
        let (return_send, return_receive) = self.reply.split();
        (
            UnboundClient {
                alarm,
                send,
                receive: return_receive,
                signal,
            },
            Interrupt {
                receive,
                send: return_send,
                signal,
                active: None,
            },
        )
    }
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static> Default for Cells<I> {
    fn default() -> Self {
        Self::new()
    }
}
/// No alarm may be armed until the image has bound its permanent IRQ owner.
pub struct UnboundClient<I: Entry + 'static, E: SignalExecutor> {
    alarm: Timg0AlarmProgram,
    send: Sender<I>,
    receive: Receiver<I>,
    signal: &'static IrqSignal<E>,
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static, E: SignalExecutor> UnboundClient<I, E> {
    pub fn bind(self, bound: Bound<I>) -> Client<I, E> {
        const {
            assert!(matches!(
                I::CLASS,
                oer_interrupt_table::Class::ExemptExecutorLevel
            ));
        }
        Client {
            alarm: Some((self.alarm, bound)),
            send: self.send,
            receive: self.receive,
            signal: self.signal,
        }
    }
}
/// The actor's program owner between requests. A waiting lifecycle future
/// retains the exact DetachedRadioDomain, so cancellation resets even while
/// the PHY half is in one of the cells or the handler.
pub struct Client<I: Entry + 'static, E: SignalExecutor> {
    alarm: Option<(Timg0AlarmProgram, Bound<I>)>,
    send: Sender<I>,
    receive: Receiver<I>,
    signal: &'static IrqSignal<E>,
}
/// The one public leaf submission contract; the production client moves
/// custody, and returns it only after the machine has reached a terminal edge.
pub trait Service {
    fn sample(
        &mut self,
        phy: PhyTransaction,
        clear_tone: bool,
    ) -> impl Future<Output = (PhyTransaction, Result<Sample, Error>)>;
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static, E: SignalExecutor> Service for Client<I, E> {
    async fn sample(
        &mut self,
        phy: PhyTransaction,
        clear_tone: bool,
    ) -> (PhyTransaction, Result<Sample, Error>) {
        let reporter = phy.sar_transfer_reporter();
        // Validate/register the receiving executor context before touching
        // the alarm. In particular a P1 caller cannot arm across P2 entry.
        core::future::poll_fn(|cx| {
            if let Poll::Ready(Err(_)) = self.signal.poll_take(cx) {
                reporter.quarantine(SarTransferFault::ForeignSignal);
            }
            Poll::Ready(())
        })
        .await;
        let Some((alarm, bound)) = self.alarm.take() else {
            phy.quarantine_sar_transfer(SarTransferFault::ProgramUnavailable);
        };
        let mut bundle = Bundle {
            phy,
            alarm,
            bound,
            engine: Engine::new(clear_tone),
            result: None,
        };
        // Both contexts are on P2 of the same hart. Arm before publication;
        // the handler cannot enter until this actor poll has returned.
        bundle.arm(Duration::from_micros(1));
        if let Err(refused) = self.send.try_send(bundle) {
            refused
                .0
                .phy
                .quarantine_sar_transfer(SarTransferFault::RequestOccupied {
                    queued: self.send.len(),
                });
        }
        let bundle = core::future::poll_fn(|cx: &mut Context<'_>| {
            if let Ok(bundle) = self.receive.try_receive() {
                return Poll::Ready(bundle);
            }
            match self.signal.poll_take(cx) {
                Poll::Ready(Err(_)) => reporter.quarantine(SarTransferFault::ForeignSignal),
                Poll::Ready(Ok(_)) => match self.receive.try_receive() {
                    Ok(bundle) => Poll::Ready(bundle),
                    Err(_) => Poll::Pending,
                },
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
        self.alarm = Some((bundle.alarm, bundle.bound));
        let Some(result) = bundle.result else {
            bundle
                .phy
                .quarantine_sar_transfer(SarTransferFault::ResultUnavailable);
        };
        (bundle.phy, result)
    }
}
/// State lent only to the P2 TIMG0 handler; the table retains the interrupt
/// part independently for the entire image. No task poll or waker is called.
pub struct Interrupt<I: Entry + 'static, E: SignalExecutor> {
    active: Option<Bundle<I>>,
    receive: Receiver<I>,
    send: Sender<I>,
    signal: &'static IrqSignal<E>,
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static, E: SignalExecutor> Interrupt<I, E> {
    #[cfg(target_arch = "riscv32")]
    pub fn service(&mut self, interrupt: &mut Timg0AlarmInterrupt, clock: &impl Clock) {
        if !interrupt.acknowledge().pending {
            return;
        }
        let Some(mut bundle) = self
            .active
            .take()
            .or_else(|| self.receive.try_receive().ok())
        else {
            return;
        };
        match bundle.engine.advance(clock, bundle.phy.hardware_mut()) {
            Event::WaitAt(at) => {
                bundle.arm(
                    at.saturating_duration_since(clock.now())
                        .max(Duration::from_micros(1)),
                );
                self.active = Some(bundle);
            }
            Event::Complete(result) => {
                bundle.alarm.disarm();
                bundle.result = Some(result);
                if let Err(refused) = self.send.try_send(bundle) {
                    refused
                        .0
                        .phy
                        .quarantine_sar_transfer(SarTransferFault::ReplyOccupied {
                            queued: self.send.len(),
                        });
                }
                let _new = self.signal.raise(1);
            }
        }
    }
}
/// Short loans into the owned PHY between submissions. It never implements
/// SharedPhyAccess: a suspended leaf cannot keep a reference into its bundle.
pub(crate) struct Physical<'a, S> {
    phy: Option<PhyTransaction>,
    service: &'a mut S,
    fault: FaultHandle,
}
impl<'a, S: Service> Physical<'a, S> {
    pub(crate) fn new(phy: PhyTransaction, service: &'a mut S, fault: FaultHandle) -> Self {
        Self {
            phy: Some(phy),
            service,
            fault,
        }
    }
    pub(crate) fn hardware(&mut self) -> &mut PhyHardware {
        self.fault.require_owner(self.phy.as_mut()).hardware_mut()
    }
    pub(crate) fn transaction(&mut self) -> &mut PhyTransaction {
        self.fault.require_owner(self.phy.as_mut())
    }
    pub(crate) async fn sample(&mut self, clear_tone: bool) -> Result<Sample, Error> {
        let phy = self.fault.require_owner(self.phy.take());
        let (phy, result) = self.service.sample(phy, clear_tone).await;
        self.phy = Some(phy);
        result
    }
    pub(crate) fn finish(mut self) -> PhyTransaction {
        self.fault.require_owner(self.phy.take())
    }
}

#[cfg(test)]
mod tests;
