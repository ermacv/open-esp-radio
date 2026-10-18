//! The unbranded driver retained by the lifecycle core.
//!
//! The lifecycle core owns this value independently of its backend. All
//! branded commands and contexts remain in `Held<'id>`; neither this driver
//! nor its permanent registration can address them. The driving future only
//! borrows this value. Consuming teardown is separate from retirement, so the
//! core can quarantine Driver without calling Drop after Unconfirmed.

use super::*;
use oer_radio_port::Retire;

/// An installation's independent driver, owning only its pinned SRAM lease,
/// cancellation-output loan and drain bound. No hardware-reachable memory
/// lives inside this movable value: all registered regions are in the backing.
/// H is RadioRuntimeOwner (and unbranded execution state) in the S31 binding.
/// Its only lifetime belongs to the composition's cancellation output latch;
/// it has no port installation brand or accepted-context lifetime.
#[must_use = "move the driver once into Installed<B>"]
pub struct Driver<
    'latch,
    H: OwnsDma + Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    scope: ManuallyDrop<DriverScope<'latch, H, I, P, E, S>>,
}

/// Explicit result of consuming teardown, after the driving loan has ended.
/// `Released` holds owners only after a stop that read DMA quiescent, whatever
/// the stop path; dropping it drops them. `Quarantined` never releases.
#[must_use = "handle retirement or the reset-only quarantine"]
pub enum State<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> {
    Released(ReleasedScope<H, I, P, E, S>),
    Quarantined(Quarantined<ScopeReturn<H, I, P, E, S>>),
}

impl<
    'latch,
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> Driver<'latch, H, I, P, E, S>
{
    pub fn new(scope: DriverScope<'latch, H, I, P, E, S>) -> Self {
        Self {
            scope: ManuallyDrop::new(scope),
        }
    }

    /// Wait through an exclusive driver loan and the one retirement receiver.
    /// Hardware steps run on the separately registered driver executor. This
    /// future owns neither Driver nor its lease and performs no owner recovery.
    /// After a route error, the binding publishes Held's fault and synchronously
    /// abandons the installation. Unconfirmed requires the core to retain Driver
    /// without Drop; only a confirmed teardown may consume it through finish.
    ///
    /// At every await, Driver remains in a state accepted by retirement and
    /// synchronous abandonment. Its two suspension points are the core's
    /// retirement request and the executor's completed retirement. All owners,
    /// descriptors and hardware work remain in permanent backing; no ownership
    /// is parked in this future. Dropping it only ends the loan. The core must
    /// then retire, abandon, or consume finish before reclaiming any owners.
    /// Successful finish drains actors and stops once; timeout retains the
    /// complete backing in reset-only quarantine without calling stop.
    pub async fn drive(&self, retire: &Retire<'_>) -> Result<(), E> {
        self.scope.start()?;
        retire.wait().await;
        self.scope.retire().await;
        Ok(())
    }

    /// Consume the driver after the lifecycle core ends its unique driving
    /// loan. A stop that read DMA quiescent returns the owners and the lease,
    /// whether the driver retired or `finish` itself closed and stopped an
    /// unretired installation (a cancelled `drive`, a requested abandon, or no
    /// drive at all). An unconfirmed stop or a drain timeout
    /// returns explicit quarantine. The common core's abandon paths
    /// (fail-stop, whole-run cancel) never call this method or Drop, whatever
    /// the stop proved ().
    pub fn finish(self) -> State<H, I, P, E, S> {
        let mut driver = ManuallyDrop::new(self);
        #[allow(
            unsafe_code,
            reason = "explicit finish consumes the sole registered scope"
        )]
        // SAFETY: ManuallyDrop suppresses cancellation after the single take;
        // this path returns its explicit outcome, without a second Drop.
        let scope = unsafe { ManuallyDrop::take(&mut driver.scope) };
        match scope.finish() {
            ScopeReturn::Released(released)
                if matches!(&released.owners, DetachedOwners::Stopped(_)) =>
            {
                State::Released(released)
            }
            returned => State::Quarantined(Quarantined::new(returned)),
        }
    }
}

impl<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> Drop for Driver<'_, H, I, P, E, S>
{
    fn drop(&mut self) {
        // Safety net for an owned driver discarded outside core quarantine.
        // The core's abandon paths (fail-stop, whole-run cancel) retain Driver
        // without invoking this destructor, whatever the stop proved ().
        // A real destructor cannot await and uses the same synchronous domain.
        let quiescence = self.scope.lease.storage().abandon(self.scope.bound);
        // The DMA fact decides here as on every path ( ): a
        // quiescent stop releases the lease and owners to the latch; anything
        // else stays quarantined and the latch receives a reset-only receipt.
        self.scope.latch.returned = Some(match self.scope.complete_after(quiescence) {
            ScopeReturn::Released(released)
                if matches!(&released.owners, DetachedOwners::Stopped(_)) =>
            {
                ScopeReturn::Released(released)
            }
            returned => {
                let _retained = Quarantined::new(returned);
                ScopeReturn::Poisoned(ScopePoisoned { _private: () })
            }
        });
    }
}
