use super::*;

struct Initializing {
    owner: Owner,
    split_failure: bool,
    lend_failure: bool,
}
impl<Id: SlotIdentity> SlotOwner<Id> for Initializing {
    type Pools = ();
    type Queues = (bool, bool);
    const VACANT_QUEUES: Self::Queues = (false, false);
    type Vacant = ();
    type Frontend<'s>
        = bool
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = ()
    where
        Self: 's,
        Id: 's;
    fn prepare_queues(&mut self, q: &mut Self::Queues, mint: crate::Mint<'static, (), Id>) {
        mint.into_pools();
        *q = (self.split_failure, self.lend_failure);
    }
    fn split<'s>(q: &'s mut Self::Queues, _: SlotWakes<'s, Id>) -> Result<(bool, ()), SlotInitError>
    where
        Self: 's,
        Id: 's,
    {
        if q.0 {
            Err(SlotInitError::QueuesUnprepared)
        } else {
            Ok((q.1, ()))
        }
    }
    async fn run<'s>(
        &mut self,
        _: &mut (),
        retire: SlotRetire<'_, Id, Self>,
    ) -> SlotStopped<Id, Self>
    where
        Self: 's,
        Id: 's,
    {
        self.owner.0.starts.set(self.owner.0.starts.get() + 1);
        retire.requested().await
    }

    fn finish<'s>(&mut self, invocation: &mut ())
    where
        Self: 's,
        Id: 's,
    {
        <Owner as SlotOwner<Id>>::finish(&mut self.owner, invocation)
    }
}
impl<Id: SlotIdentity> MockCleanup<Id> for Initializing {
    fn abandon<'s>(&mut self, invocation: Option<&mut ()>, q: Quarantine) -> Abandon
    where
        Self: 's,
        Id: 's,
    {
        <Owner as MockCleanup<Id>>::abandon(&mut self.owner, invocation, q)
    }
}

impl<Id: SlotIdentity> SlotBodyOwner<Id> for Initializing {
    type Body<'p>
        = ()
    where
        Self: 'p,
        Id: 'p;
    fn lend<'p, 's>(f: &'p mut bool, _: crate::BodyLoan<'p, Id, Self>) -> Result<(), SlotInitError>
    where
        Self: 's,
        Id: 's,
        's: 'p,
    {
        if *f {
            Err(SlotInitError::MissingResource("initial resource"))
        } else {
            Ok(())
        }
    }
}
impl<Id: SlotIdentity> ModelSlotOwner<Id> for Initializing {
    fn end_world(&mut self, _: Option<&mut Self::Queues>) {}
    fn end_vacant_world(_: &mut ()) {}
}
struct UnreachableBody;
impl<Id: SlotIdentity> SlotBody<Id, Initializing> for UnreachableBody {
    type Output = ();
    async fn run<'p>(self, _: ())
    where
        Initializing: 'p,
        Id: 'p,
    {
        panic!("failed construction never invokes a body")
    }
}

#[test]
fn split_failure_returns_its_cause_and_retains_p2_cleanup_without_publishing_endpoints() {
    let observed = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        {
            let Err(error) = slot.register(Initializing {
                owner: Owner(observed.clone()),
                split_failure: true,
                lend_failure: false,
            }) else {
                panic!("split must refuse")
            };
            let SlotRegisterError::Initialization {
                cause,
                registration,
            } = error
            else {
                panic!("initialization error")
            };
            assert_eq!(cause, SlotInitError::QueuesUnprepared);
            assert!(registration.failure().is_some());
            assert_eq!(observed.abandons.get(), 0, "caller still holds P2's guard");
            assert_eq!(observed.drops.get(), 0);
            assert!(MockRegistration::new(registration).run().await.is_err());
            assert_eq!(observed.starts.get(), 0);
            assert_eq!(observed.abandons.get(), 1);
            assert_eq!(observed.finishes.get(), 0);
            assert_eq!(observed.drops.get(), 0, "quarantine retains the owner");
        }
        slot.end_model_world();
        assert_eq!(observed.drops.get(), 1);
    }));
}

#[test]
fn lend_failure_returns_its_cause_and_quarantines_before_calling_the_body_or_driver() {
    let observed = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let registered = slot
            .register(Initializing {
                owner: Owner(observed.clone()),
                split_failure: false,
                lend_failure: true,
            })
            .unwrap();
        let result = crate::LocalExecution
            .scope(guarded(registered), async |mut frontend| {
                observed.executor.set(Executor::P1);
                let result = frontend.run_body(UnreachableBody).await;
                let join = frontend.abandon();
                observed.executor.set(Executor::P2);
                let Err(quarantine) = join.wait().await else {
                    panic!("no ordinary join after lend failure")
                };
                assert!(matches!(quarantine.outcome(), Abandon::Stopped(_)));
                result
            })
            .await;
        assert_eq!(
            result,
            Err(SlotInitError::MissingResource("initial resource"))
        );
        assert_eq!(observed.starts.get(), 0);
        assert_eq!(observed.abandons.get(), 1);
        assert_eq!(observed.finishes.get(), 0);
        assert_eq!(observed.drops.get(), 0);
        slot.end_model_world();
        assert_eq!(observed.drops.get(), 1);
    }));
}
