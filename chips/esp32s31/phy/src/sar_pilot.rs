//! Affine SAR/PWDET custody between the radio actor and its P2 TIMG0 entry.
use crate::tx::sar::{Engine, Event};
pub use crate::tx::sar::{Error, Readiness, Sample};
use core::{
    future::Future,
    task::{Context, Poll},
};
use oer_esp32s31_hal::{
    domain::transaction::{
        FaultHandle, PhyHardware, PhyTransaction, SarAlarmWithoutBundle, SarTransferFault,
    },
    platform::timg0_alarm::{InvalidDelay, Observation, Timg0AlarmInterrupt, Timg0AlarmProgram},
};
use oer_espressif_interrupt_table::{
    Bound, Entry,
    signal::{IrqSignal, SignalExecutor},
};
use oer_memory::{AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, CompactSpscCursor};
use oer_time::{Clock, Duration};

type Queue<B, A> = AffineSpscQueue<Bundle<B, A>, 1, CompactSpscCursor>;
type Sender<B, A> = AffineSpscSender<'static, Bundle<B, A>, 1, CompactSpscCursor>;
type Receiver<B, A> = AffineSpscReceiver<'static, Bundle<B, A>, 1, CompactSpscCursor>;
struct Bundle<B, A> {
    phy: PhyTransaction,
    alarm: A,
    bound: B,
    engine: Engine,
    result: Option<Result<Sample, Error>>,
}
impl<B, A: Program<B>> Bundle<B, A> {
    fn arm(&mut self, delay: Duration) {
        if self.alarm.arm_after(delay, &self.bound).is_err() {
            self.phy
                .sar_transfer_reporter()
                .quarantine(SarTransferFault::InvalidDelay { requested: delay });
        }
    }
}
/// Permanent one-slot request and return cells; endpoints are split once.
pub struct Cells<B: 'static, A: 'static = Timg0AlarmProgram> {
    request: Queue<B, A>,
    reply: Queue<B, A>,
}
impl<B: 'static, A: 'static> Cells<B, A> {
    pub const fn new() -> Self {
        Self {
            request: Queue::new(),
            reply: Queue::new(),
        }
    }
    fn split_with<E: SignalExecutor>(
        &'static mut self,
        mut alarm: A,
        interrupt: &mut impl Acknowledge,
        signal: &'static IrqSignal<E>,
    ) -> (UnboundClient<B, E, A>, Interrupt<B, E, A>)
    where
        A: Program<B>,
    {
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
impl<B: 'static, A: 'static> Default for Cells<B, A> {
    fn default() -> Self {
        Self::new()
    }
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static> Cells<Bound<I>> {
    /// Startup stops/masks the program and CLR-acknowledges stale RAW before
    /// unmask. IRQ service returns an orphan event's data to the image.
    pub fn split<E: SignalExecutor>(
        &'static mut self,
        alarm: Timg0AlarmProgram,
        interrupt: &mut Timg0AlarmInterrupt,
        signal: &'static IrqSignal<E>,
    ) -> (UnboundClient<Bound<I>, E>, Interrupt<Bound<I>, E>) {
        self.split_with(alarm, interrupt, signal)
    }
}

// Hardware effects are separate from the custody protocol. Host models run
// the same Client, Bundle and Interrupt paths with virtual alarm/SAR edges.
trait Program<B> {
    fn arm_after(&mut self, delay: Duration, bound: &B) -> Result<(), InvalidDelay>;
    fn disarm(&mut self);
}
impl<I: Entry<Part = Timg0AlarmInterrupt>> Program<Bound<I>> for Timg0AlarmProgram {
    fn arm_after(&mut self, delay: Duration, bound: &Bound<I>) -> Result<(), InvalidDelay> {
        self.arm_after(delay, bound)
    }
    fn disarm(&mut self) {
        self.disarm();
    }
}
trait Acknowledge {
    fn acknowledge(&mut self) -> Observation;
}
impl Acknowledge for Timg0AlarmInterrupt {
    fn acknowledge(&mut self) -> Observation {
        self.acknowledge()
    }
}

/// No alarm may be armed until the image has bound its permanent IRQ owner.
pub struct UnboundClient<B: 'static, E: SignalExecutor, A: 'static = Timg0AlarmProgram> {
    alarm: A,
    send: Sender<B, A>,
    receive: Receiver<B, A>,
    signal: &'static IrqSignal<E>,
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static, E: SignalExecutor> UnboundClient<Bound<I>, E> {
    pub fn bind(self, bound: Bound<I>) -> Client<Bound<I>, E> {
        const {
            assert!(matches!(
                I::CLASS,
                oer_espressif_interrupt_table::Class::ExemptExecutorLevel
            ));
        }
        self.into_client(bound)
    }
}
impl<B: 'static, E: SignalExecutor, A: 'static> UnboundClient<B, E, A> {
    fn into_client(self, bound: B) -> Client<B, E, A> {
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
pub struct Client<B: 'static, E: SignalExecutor, A: 'static = Timg0AlarmProgram> {
    alarm: Option<(A, B)>,
    send: Sender<B, A>,
    receive: Receiver<B, A>,
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
impl<B: 'static, E: SignalExecutor, A: Program<B> + 'static> Service for Client<B, E, A> {
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
pub struct Interrupt<B: 'static, E: SignalExecutor, A: 'static = Timg0AlarmProgram> {
    active: Option<Bundle<B, A>>,
    receive: Receiver<B, A>,
    send: Sender<B, A>,
    signal: &'static IrqSignal<E>,
}
impl<I: Entry<Part = Timg0AlarmInterrupt> + 'static, E: SignalExecutor> Interrupt<Bound<I>, E> {
    #[cfg(target_arch = "riscv32")]
    /// An orphan fault leaves all owners in place; the image must record it
    /// and reset without servicing the handler or actor again.
    pub fn service(
        &mut self,
        interrupt: &mut Timg0AlarmInterrupt,
        clock: &impl Clock,
    ) -> Result<(), SarAlarmWithoutBundle> {
        self.service_with(interrupt, |engine, phy| {
            engine.advance(clock, phy.hardware_mut())
        })
    }
}
impl<B: 'static, E: SignalExecutor, A: 'static> Interrupt<B, E, A> {
    fn service_with(
        &mut self,
        interrupt: &mut impl Acknowledge,
        advance: impl FnOnce(&mut Engine, &mut PhyTransaction) -> Event,
    ) -> Result<(), SarAlarmWithoutBundle>
    where
        A: Program<B>,
    {
        let observation = interrupt.acknowledge();
        if !observation.pending {
            return Ok(());
        }
        let Some(mut bundle) = self
            .active
            .take()
            .or_else(|| self.receive.try_receive().ok())
        else {
            // Startup and completion stop/mask the program. An acknowledged
            // event without custody is impossible; retain every owner in
            // place while the image records these observations and resets.
            return Err(SarAlarmWithoutBundle {
                observation,
                requests: self.receive.len(),
                replies: self.send.len(),
            });
        };
        match advance(&mut bundle.engine, &mut bundle.phy) {
            Event::Wait { delay, .. } => {
                // A fresh relative interval after the edge: no second clock
                // read can eat a tick from the vendor's 1/2-us minimum.
                bundle.arm(delay);
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
        Ok(())
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
