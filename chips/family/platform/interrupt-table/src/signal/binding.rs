//! Compile-time declarations of the contexts that raise a signal.

use core::marker::PhantomData;

use crate::{Class, Context, Entry, InterruptSource};

use super::SignalExecutor;

/// An interrupt table handler strictly above the receiving executor.
pub struct FromHandler<E>(PhantomData<E>);

/// An `Exempt(ExecutorLevel)` handler on the receiving executor's level
/// and hart. It cannot preempt that executor's round; its facts are serviced
/// by the next round, without calling a task waker in the handler.
pub struct FromExecutorLevel<E>(PhantomData<E>);

/// A task of executor `X`, whose interrupt or thread-wake entry is `E`.
pub struct FromTask<X, E>(PhantomData<(X, E)>);

/// A task of the receiving executor `X` itself (interrupt or thread-wake
/// entry `E`). It cannot preempt that executor's round: as an
/// `Exempt(ExecutorLevel)` handler's, its facts are serviced by the next
/// round, which the raise pends.
pub struct FromOwnTask<X, E>(PhantomData<(X, E)>);

/// A task of executor `X` (interrupt or thread-wake entry `E`) on another
/// hart than the receiving executor's. No level relation holds across harts:
/// the raise only publishes facts and pends the receiver, whose own handler
/// alone lowers its line and services them. The raiser never calls the
/// receiver's waker; wakers stay on their executor's hart.
pub struct FromPeerHart<X, E>(PhantomData<(X, E)>);

mod sealed {
    pub trait Sealed {}
    impl<E: crate::Entry> Sealed for super::FromHandler<E> {}
    impl<E: crate::Entry> Sealed for super::FromExecutorLevel<E> {}
    impl<X: super::SignalExecutor, E: crate::Entry> Sealed for super::FromTask<X, E> where
        E::Part: crate::InterruptSource
    {
    }
    impl<X: super::SignalExecutor, E: crate::Entry> Sealed for super::FromOwnTask<X, E> where
        E::Part: crate::InterruptSource
    {
    }
    impl<X: super::SignalExecutor, E: crate::Entry> Sealed for super::FromPeerHart<X, E> where
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
    ExecutorLevel(Context),
    Task(Context),
    OwnTask(Context),
    PeerHart(Context),
    Pair(&'static Raising, &'static Raising),
}

impl<E: Entry> Raisers for FromHandler<E> {
    const CONTEXTS: Raising = Raising::Handler(E::CONTEXT);
}

impl<E: Entry> Raisers for FromExecutorLevel<E> {
    const CONTEXTS: Raising = {
        if !matches!(E::CLASS, Class::ExemptExecutorLevel) {
            panic!("a same-level signal handler must have class Exempt(ExecutorLevel)")
        }
        Raising::ExecutorLevel(E::CONTEXT)
    };
}

impl<X: SignalExecutor, E: Entry> Raisers for FromTask<X, E>
where
    E::Part: InterruptSource,
{
    const CONTEXTS: Raising = match executor_context::<X, E>() {
        Ok(context) => Raising::Task(context),
        Err(why) => panic!("{}", why),
    };
}

impl<X: SignalExecutor, E: Entry> Raisers for FromOwnTask<X, E>
where
    E::Part: InterruptSource,
{
    const CONTEXTS: Raising = match executor_context::<X, E>() {
        Ok(context) => Raising::OwnTask(context),
        Err(why) => panic!("{}", why),
    };
}

impl<X: SignalExecutor, E: Entry> Raisers for FromPeerHart<X, E>
where
    E::Part: InterruptSource,
{
    const CONTEXTS: Raising = match executor_context::<X, E>() {
        Ok(context) => Raising::PeerHart(context),
        Err(why) => panic!("{}", why),
    };
}

impl<A: Raisers, B: Raisers> Raisers for (A, B) {
    const CONTEXTS: Raising = Raising::Pair(&A::CONTEXTS, &B::CONTEXTS);
}

/// The context executor `X`'s tasks run in, through its entry `E`: the
/// entry must name `X`'s own software interrupt, an interrupt-mode
/// executor's entry polls its tasks (class `Executor`) and a thread-mode
/// executor's is its wake entry (class `Hardware`), whose tasks run at level
/// zero. A raiser or requester names its executor through it.
pub const fn executor_context<X: SignalExecutor, E: Entry>() -> Result<Context, &'static str>
where
    E::Part: InterruptSource,
{
    if <E::Part as InterruptSource>::SOURCE != X::SOURCE {
        return Err("an executor's entry must name its software interrupt");
    }
    if X::INTERRUPT_MODE {
        if !matches!(E::CLASS, Class::Executor) {
            return Err("an interrupt-mode executor's entry must poll its tasks");
        }
        Ok(E::CONTEXT)
    } else {
        if !matches!(E::CLASS, Class::Hardware) {
            return Err("a thread executor needs its wake entry");
        }
        Ok(Context {
            level: 0,
            hart: E::HART,
        })
    }
}

impl Raising {
    pub(super) const fn check(&self, executor: Context) -> Result<(), &'static str> {
        match self {
            Self::Handler(from) => {
                if from.hart != executor.hart {
                    return Err("a signal and its raiser must share a hart");
                }
                if from.level <= executor.level {
                    return Err("a signal handler must be above its executor");
                }
            }
            Self::Task(from) => {
                if from.hart != executor.hart {
                    return Err("a signal and its raiser must share a hart");
                }
                if from.level >= executor.level {
                    return Err("a signal task must be below its executor");
                }
            }
            Self::OwnTask(from) => {
                if from.hart != executor.hart || from.level != executor.level {
                    return Err("an own-task raiser must be a task of its receiving executor");
                }
            }
            Self::ExecutorLevel(from) => {
                if from.hart != executor.hart {
                    return Err("a signal and its raiser must share a hart");
                }
                if from.level != executor.level {
                    return Err(
                        "a same-level signal handler must be on its receiving executor's level",
                    );
                };
            }
            Self::PeerHart(from) => {
                if from.hart == executor.hart {
                    return Err("a peer-hart raiser must run on another hart than its executor");
                }
            }
            Self::Pair(a, b) => {
                if let Err(why) = a.check(executor) {
                    return Err(why);
                }
                return b.check(executor);
            }
        }
        Ok(())
    }
}
