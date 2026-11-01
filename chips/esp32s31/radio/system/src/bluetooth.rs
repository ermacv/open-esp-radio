//! A borrowed Bluetooth attachment; the shared actor keeps its concrete type.
use super::*;
use oer_esp32s31_radio_client::bluetooth::{
    Bluetooth, BluetoothJoin, BluetoothJoined, BluetoothLeft, Installation,
};

/// Bluetooth-specific ownership attached to an existing common backend.
/// No method implements or replaces the common PHY domain. Implementations
/// keep every entered failure's owners until Controller reset or quarantine.
#[allow(async_fn_in_trait)]
pub trait BluetoothBackend<B, S: LpSource> {
    /// Composition hooks alter no hardware; detach runs only after stopped.
    fn attached(&mut self, shared: &mut B);
    fn detached(&mut self, shared: &mut B);
    fn is_stopped(&self, shared: &B) -> bool;
    fn admits(&self, shared: &B, join: &BluetoothJoin) -> bool;
    async fn enter(
        &mut self,
        shared: &mut B,
        join: BluetoothJoin,
        phy: PhyPreparation,
        lp: &LpClock<S>,
    ) -> Result<BluetoothJoined, Failed<BluetoothJoin>>;
    async fn track(
        &mut self,
        shared: &mut B,
        installation: Installation,
    ) -> Result<Installation, Failed<Installation>>;
    async fn exit(&mut self, shared: &mut B, installation: Installation) -> (BluetoothLeft, Exit);
}

/// Exclusive attachment loan. Both permanent owners remain at their original
/// addresses; dropping the loan does not clear its claimed attachment seat.
pub struct BluetoothActor<'a, S: LpSource, B, T, A> {
    actor: &'a mut RadioSystem<Wifi, S, B, T>,
    inbox: ClientInbox<Bluetooth>,
    attachment: &'a mut A,
}
impl<S: LpSource, B: PhyDomain, T: oer_time::Timer> RadioSystem<Wifi, S, B, T> {
    /// Lend this actor to one Bluetooth attachment. Refusal returns the actual
    /// inbox; a dropped loan cannot grant a second attachment/reset epoch.
    /// The common actor cannot be borrowed while its attachment is in use:
    /// ```compile_fail,E0499
    /// use oer_esp32s31_radio_system::{RadioSystem,BluetoothBackend,ClientBackend};
    /// use oer_esp32s31_radio_client::{LpSource,ClientInbox,wifi::Wifi,bluetooth::Bluetooth};
    /// fn conflict<S,B,T,A>(actor:&mut RadioSystem<Wifi,S,B,T>,inbox:ClientInbox<Bluetooth>,attachment:&mut A)
    /// where S:LpSource,B:ClientBackend<Wifi,S>,T:oer_time::Timer,A:BluetoothBackend<B,S> {
    ///     let mut loan=actor.attach_bluetooth(inbox,attachment).ok().expect("one attachment");
    ///     let _=actor.backend_mut();
    ///     let _=loan.run();
    /// }
    /// ```
    pub fn attach_bluetooth<'a, A: BluetoothBackend<B, S>>(
        &'a mut self,
        inbox: ClientInbox<Bluetooth>,
        attachment: &'a mut A,
    ) -> Result<BluetoothActor<'a, S, B, T, A>, ClientInbox<Bluetooth>> {
        if self.core.attached {
            return Err(inbox);
        }
        self.core.attached = true;
        attachment.attached(&mut self.backend);
        Ok(BluetoothActor {
            actor: self,
            inbox,
            attachment,
        })
    }
}
impl<S: LpSource, B: PhyDomain, T: oer_time::Timer, A: BluetoothBackend<B, S>>
    BluetoothActor<'_, S, B, T, A>
{
    /// Access the retained controller only between separate run borrows.
    pub fn attachment_mut(&mut self) -> &mut A {
        self.attachment
    }
    /// Consume a normally stopped attachment loan without moving the PHY.
    pub fn detach(self) -> Result<ClientInbox<Bluetooth>, Self> {
        if !self.attachment.is_stopped(&self.actor.backend) {
            return Err(self);
        }
        self.attachment.detached(&mut self.actor.backend);
        self.actor.core.attached = false;
        Ok(self.inbox)
    }
}
impl<S, B, T, A> BluetoothActor<'_, S, B, T, A>
where
    S: LpSource,
    B: ClientBackend<Wifi, S>,
    T: oer_time::Timer,
    A: BluetoothBackend<B, S>,
{
    /// Serve both clients until the periodic tracking fails.
    pub async fn run(&mut self) -> TrackingFault {
        match self
            .run_until(core::future::pending::<core::convert::Infallible>())
            .await
        {
            Ok(never) => match never {},
            Err(fault) => fault,
        }
    }
    /// Observe the composition only between completed entered transactions.
    ///
    /// # Errors
    ///
    /// The periodic tracking failed before `next` completed.
    pub async fn run_until<F: core::future::Future>(
        &mut self,
        next: F,
    ) -> Result<F::Output, TrackingFault> {
        let mut next = core::pin::pin!(next);
        loop {
            if let core::task::Poll::Ready(out) =
                poll_fn(|cx| core::task::Poll::Ready(next.as_mut().poll(cx))).await
            {
                return Ok(out);
            }
            let mut served = false;
            if let Some(request) = self.actor.inbox.take() {
                // Same receiver/backend type as a Wi-Fi-only composition.
                Boundary::new(self.actor.serve(request)).await;
                served = true;
            }
            if let Some(request) = self.inbox.take() {
                Boundary::new(self.serve(request)).await;
                served = true;
            }
            if served {
                continue;
            }
            let deadline = self.actor.tracking_deadline()?;
            let wait = Boundary::new(self.actor.wait(deadline));
            match select(next.as_mut(), wait).await {
                Either::First(out) => return Ok(out),
                Either::Second(true) => Boundary::new(self.actor.tick()).await?,
                Either::Second(false) => {}
            }
        }
    }
    async fn serve(&mut self, request: Request<Bluetooth>) {
        match request {
            Request::Join(join, responder) => {
                if let Some(poison) = self.actor.core.poisoned() {
                    return responder.answer(Err(Failed {
                        returned: join,
                        cause: Failure::Poisoned(poison),
                    }));
                }
                if !self.attachment.admits(&self.actor.backend, &join) {
                    return responder.answer(Err(Failed {
                        returned: join,
                        cause: Failure::Refused,
                    }));
                }
                let phy = match Boundary::new(self.actor.backend.prepare()).await {
                    Ok(phy) => phy,
                    Err(Fault(cause)) => {
                        let poison = self.actor.core.poison(Poison {
                            origin: PoisonOrigin::Prepare,
                            cause,
                        });
                        return responder.answer(Err(Failed {
                            returned: join,
                            cause: Failure::Poisoned(poison),
                        }));
                    }
                };
                let epoch = self
                    .actor
                    .core
                    .prepared(matches!(phy, PhyPreparation::AlreadyOpen));
                let entered = {
                    let entering = core::pin::pin!(self.attachment.enter(
                        &mut self.actor.backend,
                        join,
                        phy,
                        &self.actor.lp,
                    ));
                    entering.await
                };
                responder.answer(match entered {
                    Ok(payload) => {
                        self.actor.core.joined();
                        Ok(Joined {
                            member: self.actor.core.mint.untracked(),
                            epoch,
                            payload,
                        })
                    }
                    Err(Failed { returned, cause }) => {
                        if matches!(self.actor.core.failed_enter(cause), Action::CloseRf) {
                            Boundary::new(self.actor.close_rf()).await;
                        }
                        Err(Failed {
                            returned,
                            cause: self.actor.core.failure(cause),
                        })
                    }
                })
            }
            Request::Track(member, installation, responder) => {
                if let Some(poison) = self.actor.core.poisoned() {
                    return responder.answer(Err(Failed {
                        returned: (member, installation),
                        cause: Failure::Poisoned(poison),
                    }));
                }
                let tracked = {
                    let tracking = core::pin::pin!(
                        self.attachment.track(&mut self.actor.backend, installation)
                    );
                    tracking.await
                };
                responder.answer(match tracked {
                    Ok(payload) => {
                        self.actor.core.tracked();
                        Ok(TrackedMember {
                            member: self.actor.core.mint.track(member),
                            payload,
                        })
                    }
                    Err(Failed { returned, cause }) => {
                        self.actor.core.failed(cause);
                        Err(Failed {
                            returned: (member, returned),
                            cause: self.actor.core.failure(cause),
                        })
                    }
                })
            }
            Request::Retune(_, never, _) => match never {},
            Request::Leave(member, installation, responder) => {
                self.actor
                    .core
                    .leaving(matches!(member, Leaving::Tracked(_)));
                self.actor.core.mint.retire(member);
                let (payload, exit) = {
                    let leaving = core::pin::pin!(
                        self.attachment.exit(&mut self.actor.backend, installation)
                    );
                    leaving.await
                };
                if matches!(self.actor.core.exited(exit), Action::CloseRf) {
                    Boundary::new(self.actor.close_rf()).await;
                }
                responder.answer(Left {
                    payload,
                    domain: self.actor.core.health(),
                })
            }
        }
    }
}
