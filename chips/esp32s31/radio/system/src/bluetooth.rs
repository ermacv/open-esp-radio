//! Multiplex two clients through the same actor and shared state.

use super::*;
use oer_esp32s31_radio_client::bluetooth::Bluetooth;

impl<S: LpSource, B: PhyDomain, T: oer_time::Timer> RadioSystem<Wifi, S, B, T> {
    /// Attach the Bluetooth cell to this actor; no second RF owner is made.
    pub fn with_bluetooth(
        self,
        inbox: ClientInbox<Bluetooth>,
    ) -> RadioSystem<Wifi, S, B, T, ClientInbox<Bluetooth>> {
        RadioSystem {
            backend: self.backend,
            timer: self.timer,
            lp: self.lp,
            doorbell: self.doorbell,
            inbox: self.inbox,
            mint: self.mint,
            state: self.state,
            extra: inbox,
        }
    }
}

impl<S, B, T> RadioSystem<Wifi, S, B, T, ClientInbox<Bluetooth>>
where
    S: LpSource,
    B: ClientBackend<Wifi, S> + ClientBackend<Bluetooth, S>,
    T: oer_time::Timer,
{
    /// Both cells are served by this one future. Visit each once per pass so
    /// a client cannot starve its sibling's posted stop or join.
    pub async fn run(&mut self) -> Infallible {
        loop {
            let mut served = false;
            if let Some((request, responder)) = self.inbox.take() {
                responder.answer(self.serve(request).await);
                served = true;
            }
            if let Some((request, responder)) = self.extra.take() {
                responder.answer(self.serve(request).await);
                served = true;
            }
            if served {
                continue;
            }
            let deadline = (!self.state.poisoned && self.state.tracked > 0)
                .then(|| self.backend.tracking_deadline())
                .flatten();
            let doorbell = self.doorbell;
            let rung = poll_fn(|cx| doorbell.poll_rung(cx));
            match deadline {
                None => {
                    rung.await;
                }
                Some(deadline) => {
                    if let Either::Second(()) = select(rung, self.timer.wait_until(deadline)).await
                    {
                        self.tick().await;
                    }
                }
            }
        }
    }
}
