//! Compile-time declarations of the contexts that raise a signal.

use core::marker::PhantomData;

use crate::{Class, Context, Entry, InterruptSource};

use super::SignalExecutor;

/// An interrupt table handler that raises facts.
pub struct FromHandler<E>(PhantomData<E>);

/// A task of executor `X`, whose interrupt or thread-wake entry is `E`.
pub struct FromTask<X, E>(PhantomData<(X, E)>);

mod sealed {
    pub trait Sealed {}
    impl<E: crate::Entry> Sealed for super::FromHandler<E> {}
    impl<X: super::SignalExecutor, E: crate::Entry> Sealed for super::FromTask<X, E> where
        E::Part: crate::InterruptSource
    {
    }
    impl<A: super::Raisers, B: super::Raisers> Sealed for (A, B) {}
}

/// The declared raising contexts: a handler, a task, or a pair of these.
/// Pairs can be nested when several contexts raise the same signal.
pub trait Raisers: sealed::Sealed {
    #[doc(hidden)]
    const CONTEXTS: Raising;
}

/// Const-only scheduling metadata for a signal's declared raisers.
#[doc(hidden)]
pub enum Raising {
    Handler(Context),
    Task(Context),
    Pair(&'static Raising, &'static Raising),
}

impl<E: Entry> Raisers for FromHandler<E> {
    const CONTEXTS: Raising = Raising::Handler(E::CONTEXT);
}

impl<X: SignalExecutor, E: Entry> Raisers for FromTask<X, E>
where
    E::Part: InterruptSource,
{
    const CONTEXTS: Raising = Raising::Task(executor_context::<X, E>());
}

impl<A: Raisers, B: Raisers> Raisers for (A, B) {
    const CONTEXTS: Raising = Raising::Pair(&A::CONTEXTS, &B::CONTEXTS);
}

/// Link the scheduling entry to the executor's actual software source.
pub(super) const fn executor_context<X: SignalExecutor, E: Entry>() -> Context
where
    E::Part: InterruptSource,
{
    assert!(
        <E::Part as InterruptSource>::SOURCE == X::SOURCE,
        "the signal entry must name its executor's software interrupt"
    );
    if X::INTERRUPT_MODE {
        assert!(
            matches!(E::CLASS, Class::Executor),
            "the signal's executor entry must poll tasks"
        );
        E::CONTEXT
    } else {
        assert!(
            matches!(E::CLASS, Class::Hardware),
            "a thread executor needs its wake entry"
        );
        Context {
            level: 0,
            hart: E::HART,
        }
    }
}

impl Raising {
    pub(super) const fn check(&self, executor: Context) {
        match self {
            Self::Handler(from) => {
                assert!(
                    from.hart == executor.hart,
                    "a signal and its raiser must share a hart"
                );
                assert!(
                    from.level > executor.level,
                    "a signal handler must be above its executor"
                );
            }
            Self::Task(from) => {
                assert!(
                    from.hart == executor.hart,
                    "a signal and its raiser must share a hart"
                );
                assert!(
                    from.level < executor.level,
                    "a signal task must be below its executor"
                );
            }
            Self::Pair(a, b) => {
                a.check(executor);
                b.check(executor);
            }
        }
    }
}
