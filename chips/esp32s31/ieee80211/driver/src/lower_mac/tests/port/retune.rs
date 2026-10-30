//! A start that retunes the radio: its failure keeps its cause, and the
//! port's periodic clock sample never reads a device whose registers the
//! radio actor holds.
use super::*;
use oer_esp32s31_radio_client::{
    Failure, PhyFault, Poison, PoisonCause, PoisonOrigin, RequestKind,
};
use oer_ieee80211_wire::channel::Channel;
use oer_time::Duration;

async fn yield_once() {
    let mut yielded = false;
    core::future::poll_fn(|cx| {
        if core::mem::replace(&mut yielded, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await
}

/// Enable on the configured channel, disable, then enable on channel 1:
/// the second start retunes.
struct Retune {
    observed: Rc<RefCell<Observed>>,
    /// Before the second enable, advance the clock by the first value; while
    /// it is held, by the second.
    clock: Option<(TestTimer, Duration, Duration)>,
    fail: Option<Fault>,
    /// The cause a poisoned enable published.
    cause: Rc<Cell<Option<Fault>>>,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for Retune {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        _: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let owner = owner.enable().await.ok().expect("first enable");
        let mut owner = owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
            .ok()
            .expect("disable");
        owner.setup().set_channel(
            SupportedChannel::new(
                oer_radio_regulatory::RegulatoryDomain::of(
                    oer_radio_regulatory::Country::new(*b"DE").expect("test country"),
                )
                .expect("domain fixture")
                .permit(oer_ieee80211_port::RegulatedChannel::from(
                    Channel::from_wifi_channel(channel(1)),
                ))
                .expect("permitted test channel"),
            )
            .expect("supported test channel"),
        );
        self.observed.borrow_mut().fail_start = self.fail;
        let enabled = if let Some((timer, before, held)) = self.clock {
            timer.0.advance(before).expect("finite test clock");
            self.observed.borrow_mut().hold_start = true;
            let mut enable = pin!(owner.enable());
            assert!(
                core::future::poll_fn(|cx| Poll::Ready(enable.as_mut().poll(cx).is_pending()))
                    .await,
                "the retune is held"
            );
            timer.0.advance(held).expect("finite test clock");
            for _ in 0..8 {
                yield_once().await;
            }
            self.observed.borrow_mut().hold_start = false;
            enable.await
        } else {
            owner.enable().await
        };
        match enabled {
            Ok(owner) => {
                let owner = owner
                    .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                    .await
                    .ok()
                    .expect("disable after the retune");
                radio::Ended::uninstall(owner, (), ())
            }
            Err(radio::Failed::Poisoned(owner)) => {
                self.cause.set(Some(owner.cause()));
                radio::Ended::fail_stop(owner.fail_stop(), ())
            }
            Err(radio::Failed::Refused(..)) => panic!("a retune failure poisons the port"),
        }
    }
}

#[test]
fn a_failed_retune_poisons_the_port_with_its_named_cause() {
    // The radio actor refused the retune because a tracking tick had failed.
    let tick = Poison {
        origin: PoisonOrigin::TrackingTick,
        cause: PoisonCause::Tracking(PhyFault::EdgeTimedOut),
    };
    for cause in [
        WifiRetuneError::Radio(Failure::Poisoned(tick)),
        WifiRetuneError::Radio(Failure::Poisoned(Poison::request(
            oer_esp32s31_hal::domain::RadioClient::Wifi,
            RequestKind::Retune,
            PoisonCause::Channel(PhyFault::UnexpectedBinding),
        ))),
        WifiRetuneError::Radio(Failure::Refused),
        WifiRetuneError::NotStopped(oer_esp32s31_hal::owner::maintenance::Error::MacActive {
            state: 3,
        }),
    ] {
        let (parts, observed, _) = installed(Vec::new());
        crate::port::FIRST_POISON.with(|first| first.set(None));
        let published = Rc::new(Cell::new(None));
        let report = block_on(run_body(
            parts,
            Retune {
                observed: observed.clone(),
                clock: None,
                fail: Some(Fault::Retune(cause)),
                cause: published.clone(),
            },
        ));
        assert!(
            !matches!(report, Report::Uninstalled { .. }),
            "the failed retune fail-stops the port"
        );
        // The body reads the cause unless the quarantine cancelled it first.
        assert!(
            published
                .get()
                .is_none_or(|seen| seen == Fault::Retune(cause))
        );
        assert_eq!(
            crate::port::FIRST_POISON.with(Cell::get),
            Some(Fault::Retune(cause))
        );
        assert_eq!(observed.borrow().start_channels, [channel(6)]);
    }
}

#[test]
fn the_clock_sample_falling_due_during_a_retune_waits_for_its_end() {
    let (parts, observed, timer) = installed(Vec::new());
    crate::port::FIRST_POISON.with(|first| first.set(None));
    let published = Rc::new(Cell::new(None));
    // The installation armed the sample 60 s ahead; it falls due while the
    // radio actor holds the registers for the retune.
    let report = block_on(run_body(
        parts,
        Retune {
            observed: observed.clone(),
            clock: Some((
                timer.clone(),
                Duration::from_millis(59_900),
                Duration::from_millis(200),
            )),
            fail: None,
            cause: published.clone(),
        },
    ));
    assert_eq!(published.get(), None);
    assert_eq!(
        crate::port::FIRST_POISON.with(Cell::get),
        None,
        "no fault poisons the port"
    );
    assert!(
        matches!(report, Report::Uninstalled { .. }),
        "the retune completes and the port uninstalls"
    );
    assert_eq!(observed.borrow().start_channels, [channel(6), channel(1)]);
}
