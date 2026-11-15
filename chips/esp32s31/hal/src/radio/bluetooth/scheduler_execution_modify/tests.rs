use std::{collections::VecDeque, vec::Vec};

use crate::radio::bluetooth::registers::{
    BluetoothSchedulerExecutionModifyDisposition as Disposition,
    BluetoothSchedulerHardwareListIndex,
};

use super::{Control, ExecutionModifyKind, Phase, Repeat, State, Unsettled};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Busy,
    EnginesIdle,
    SelectProgress,
    Publish(ExecutionModifyKind),
    Repeats,
    Ready,
    SelectSettle,
    Settled,
    ClearStart,
    Rejected,
}

#[derive(Default)]
struct Model {
    ops: Vec<Op>,
    busy: VecDeque<bool>,
    engines_idle: VecDeque<bool>,
    repeats: VecDeque<bool>,
    ready: VecDeque<bool>,
    settled: VecDeque<bool>,
    /// Every diagnostic sample exhausts its attempt budget.
    unsettled: Samples,
    rejected: Verdict,
    defer_samples: Replies,
}

/// Whether the model's diagnostic samples settle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Samples {
    #[default]
    Settle,
    Unsettled,
}

/// What the hardware says of the request once it settles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Verdict {
    #[default]
    Accepted,
    Rejected,
}

/// Whether the model's P3 replies come at once or after a yield.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Replies {
    #[default]
    Immediate,
    Deferred,
}

impl Model {
    async fn before_reply(&self) {
        if self.defer_samples == Replies::Deferred {
            let mut yielded = false;
            core::future::poll_fn(|_| {
                if core::mem::replace(&mut yielded, true) {
                    core::task::Poll::Ready(())
                } else {
                    core::task::Poll::Pending
                }
            })
            .await;
        }
    }
    fn sample(&mut self, value: impl FnOnce(&mut Self) -> bool) -> Result<bool, Unsettled> {
        if self.unsettled == Samples::Unsettled {
            Err(Unsettled)
        } else {
            Ok(value(self))
        }
    }
}

impl Control for Model {
    type Published = ();
    type Error = Unsettled;

    async fn busy(&mut self) -> Result<bool, Unsettled> {
        self.before_reply().await;
        self.ops.push(Op::Busy);
        self.sample(|model| model.busy.pop_front().unwrap_or(false))
    }
    fn engines_idle(&mut self) -> bool {
        self.ops.push(Op::EnginesIdle);
        self.engines_idle.pop_front().unwrap_or(true)
    }
    async fn select_progress(&mut self) -> Result<(), Unsettled> {
        self.before_reply().await;
        self.ops.push(Op::SelectProgress);
        Ok(())
    }
    fn publish(&mut self, _: BluetoothSchedulerHardwareListIndex, kind: ExecutionModifyKind) {
        self.ops.push(Op::Publish(kind));
    }
    async fn repeats(&mut self) -> Result<bool, Unsettled> {
        self.before_reply().await;
        self.ops.push(Op::Repeats);
        self.sample(|model| model.repeats.pop_front().unwrap_or(false))
    }
    fn ready(&mut self) -> bool {
        self.ops.push(Op::Ready);
        self.ready.pop_front().unwrap_or(true)
    }
    async fn select_settle(&mut self) -> Result<(), Unsettled> {
        self.before_reply().await;
        self.ops.push(Op::SelectSettle);
        Ok(())
    }
    async fn settled(&mut self) -> Result<bool, Unsettled> {
        self.before_reply().await;
        self.ops.push(Op::Settled);
        self.sample(|model| model.settled.pop_front().unwrap_or(true))
    }
    fn clear_start(&mut self, (): ()) {
        self.ops.push(Op::ClearStart);
    }
    fn rejected(&mut self) -> bool {
        self.ops.push(Op::Rejected);
        self.rejected == Verdict::Rejected
    }
}

struct Request {
    phase: Phase,
    repeat: Repeat,
    published: Option<()>,
}

impl Request {
    fn new() -> Self {
        Self {
            phase: Phase::Preamble,
            repeat: Repeat::No,
            published: None,
        }
    }

    fn step(&mut self, model: &mut Model, kind: ExecutionModifyKind) -> Disposition {
        self.try_step(model, kind).unwrap()
    }

    fn try_step(
        &mut self,
        model: &mut Model,
        kind: ExecutionModifyKind,
    ) -> Result<Disposition, Unsettled> {
        crate::radio::bluetooth::test_ready(super::step(
            State {
                index: BluetoothSchedulerHardwareListIndex::ZERO,
                kind,
                phase: &mut self.phase,
                repeat: &mut self.repeat,
                published: &mut self.published,
            },
            model,
        ))
    }
}

#[test]
fn an_idle_scheduler_publishes_and_settles_in_the_vendor_order() {
    let mut model = Model::default();
    let mut request = Request::new();
    assert_eq!(
        request.step(&mut model, ExecutionModifyKind::ListDeletion),
        Disposition::Ready
    );
    assert_eq!(
        model.ops,
        [
            Op::Busy,
            Op::SelectProgress,
            Op::Publish(ExecutionModifyKind::ListDeletion),
            Op::Repeats,
            Op::Busy,
            Op::SelectSettle,
            Op::Settled,
            Op::Rejected,
        ]
    );
    assert!(request.published.is_some(), "the owner clears START");
}

#[test]
fn delayed_p3_replies_preserve_publication_order_without_repeating_writes() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let mut model = Model {
        defer_samples: Replies::Deferred,
        ..Model::default()
    };
    let mut request = Request::new();
    {
        let mut future = core::pin::pin!(super::step(
            State {
                index: BluetoothSchedulerHardwareListIndex::ZERO,
                kind: ExecutionModifyKind::ListDeletion,
                phase: &mut request.phase,
                repeat: &mut request.repeat,
                published: &mut request.published,
            },
            &mut model
        ));
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..6 {
            assert!(future.as_mut().poll(&mut cx).is_pending());
        }
        assert_eq!(
            future.as_mut().poll(&mut cx),
            Poll::Ready(Ok(Disposition::Ready))
        );
    }
    assert_eq!(
        model.ops,
        [
            Op::Busy,
            Op::SelectProgress,
            Op::Publish(ExecutionModifyKind::ListDeletion),
            Op::Repeats,
            Op::Busy,
            Op::SelectSettle,
            Op::Settled,
            Op::Rejected
        ]
    );
    assert!(request.published.is_some());
}

#[test]
fn busy_engines_hold_the_request_and_a_busy_scheduler_waits_for_ready() {
    let mut model = Model {
        busy: VecDeque::from([true, true, true, true]),
        engines_idle: VecDeque::from([false, true]),
        ready: VecDeque::from([false, true]),
        ..Model::default()
    };
    let mut request = Request::new();
    assert_eq!(
        request.step(&mut model, ExecutionModifyKind::Modify),
        Disposition::Pending
    );
    assert_eq!(model.ops, [Op::Busy, Op::EnginesIdle]);
    model.ops.clear();
    assert_eq!(
        request.step(&mut model, ExecutionModifyKind::Modify),
        Disposition::Pending
    );
    assert_eq!(
        model.ops,
        [
            Op::Busy,
            Op::EnginesIdle,
            Op::SelectProgress,
            Op::Publish(ExecutionModifyKind::Modify),
            Op::Repeats,
            Op::Busy,
            Op::Ready,
        ]
    );
    model.ops.clear();
    assert_eq!(
        request.step(&mut model, ExecutionModifyKind::Modify),
        Disposition::Ready
    );
    assert_eq!(
        model.ops,
        [
            Op::Busy,
            Op::Ready,
            Op::SelectSettle,
            Op::Settled,
            Op::Rejected
        ]
    );
}

#[test]
fn a_conflict_repeats_the_request_after_it_settles() {
    let mut model = Model {
        repeats: VecDeque::from([true, false, false]),
        settled: VecDeque::from([false, true, true]),
        ..Model::default()
    };
    let mut request = Request::new();
    // The conflict is observed right after publication.
    assert_eq!(
        request.step(&mut model, ExecutionModifyKind::Modify),
        Disposition::Pending
    );
    model.ops.clear();
    // A resumed observation selects the progress signal again.
    assert_eq!(
        request.step(&mut model, ExecutionModifyKind::Modify),
        Disposition::Pending
    );
    assert_eq!(
        model.ops,
        [
            Op::SelectProgress,
            Op::Repeats,
            Op::Busy,
            Op::SelectSettle,
            Op::Settled,
        ]
    );
    model.ops.clear();
    assert_eq!(
        request.step(&mut model, ExecutionModifyKind::Modify),
        Disposition::Ready
    );
    assert_eq!(
        model.ops,
        [
            Op::SelectSettle,
            Op::Settled,
            Op::ClearStart,
            Op::Busy,
            Op::SelectProgress,
            Op::Publish(ExecutionModifyKind::Modify),
            Op::Repeats,
            Op::Busy,
            Op::SelectSettle,
            Op::Settled,
            Op::Rejected,
        ]
    );
}

#[test]
fn a_set_status_19_is_a_hardware_rejection() {
    let mut model = Model {
        rejected: Verdict::Rejected,
        ..Model::default()
    };
    assert_eq!(
        Request::new().step(&mut model, ExecutionModifyKind::Modify),
        Disposition::HardwareRejected
    );
}

#[test]
fn an_unsettled_diagnostic_sample_fails_the_step_without_publishing() {
    let mut model = Model {
        unsettled: Samples::Unsettled,
        ..Model::default()
    };
    let mut request = Request::new();
    assert_eq!(
        request.try_step(&mut model, ExecutionModifyKind::Modify),
        Err(Unsettled)
    );
    assert_eq!(model.ops, [Op::Busy]);
    assert!(request.published.is_none());
}
