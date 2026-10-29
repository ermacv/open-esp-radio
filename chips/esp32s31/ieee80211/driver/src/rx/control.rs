//! The application's (P1) handle on one receive epoch: a stop request in,
//! the exit report out.

use core::{
    future::poll_fn,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

use crate::mac::rx::{RxRingError, pool::RxStageTransactionError};
use atomic_waker::AtomicWaker;
use oer_memory::{AffineSpscQueue, AffineSpscReceiver, AffineSpscSender};
use oer_sync::PriorityWaker;

use super::frontier::RxFrontierError;

/// Why a receive epoch ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRxCause {
    /// The application asked it to stop.
    StopRequested,
    /// The stopped ring could not be prepared; nothing was started.
    Prepare(RxRingError),
    /// The interrupt cell already held a line; the new epoch was masked
    /// directly and never routed.
    LineRefused,
    /// The walker did not start.
    Start(RxFrontierError),
    /// One RX service transaction failed.
    Service(RxStageTransactionError),
    /// The runner ran outside the executor its interrupt signals belong
    /// to: no interrupt fact could ever wake it.
    ForeignExecutor,
}

/// Why a teardown left its owners quarantined until a radio reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRxQuarantine {
    /// The cause was a fault: the arena was poisoned before the stop.
    Fault,
    /// The interrupt line did not confirm the mask in time.
    MaskTimeout,
    /// Staged leases or the MAC runtime did not drain in time.
    DrainTimeout,
    /// The single stop attempt was refused; it is never repeated.
    StopRefused(RxFrontierError),
}

/// How a teardown left the walker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRxEnd {
    /// The single stop attempt halted the walker.
    Halted,
    /// The walker was never enabled.
    NotStarted,
    /// The owners are parked until a radio reset.
    Quarantined {
        reason: WifiRxQuarantine,
        /// Whether the single stop attempt halted the walker anyway.
        walker_stopped: bool,
    },
}

/// The report a receive epoch publishes before it parks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiRxExit {
    pub cause: WifiRxCause,
    pub end: WifiRxEnd,
}

/// The shared words of one stop handle and its receive runner.
pub struct ReceiveControl {
    stop: AtomicBool,
    runner: PriorityWaker,
    exit: AffineSpscQueue<WifiRxExit, 1>,
    reporter: AtomicWaker,
}

impl ReceiveControl {
    /// Initialize the control words and cached exit queue in caller storage.
    pub fn init_in_place(storage: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        oer_memory::init_in_place!(storage, Self {
            stop: AtomicBool::new(false),
            runner: PriorityWaker::new(),
            exit => AffineSpscQueue::init_in_place,
            reporter: AtomicWaker::new(),
        })
    }

    pub const fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            runner: PriorityWaker::new(),
            exit: AffineSpscQueue::new(),
            reporter: AtomicWaker::new(),
        }
    }

    /// Lend the runner's side and the application's side.
    pub fn split(&mut self) -> (ReceiveRunnerControl<'_>, ReceiveStopHandle<'_>) {
        let Self {
            stop,
            runner,
            exit,
            reporter,
        } = self;
        let (sender, receiver) = exit.split();
        let (stop, runner, reporter) = (&*stop, &*runner, &*reporter);
        (
            ReceiveRunnerControl {
                stop,
                runner,
                exit: sender,
                reporter,
            },
            ReceiveStopHandle {
                stop,
                runner,
                exit: receiver,
                reporter,
            },
        )
    }
}

impl Default for ReceiveControl {
    fn default() -> Self {
        Self::new()
    }
}

/// The application's handle: request the stop, then wait for the report.
pub struct ReceiveStopHandle<'a> {
    stop: &'a AtomicBool,
    runner: &'a PriorityWaker,
    exit: AffineSpscReceiver<'a, WifiRxExit, 1>,
    reporter: &'a AtomicWaker,
}

impl ReceiveStopHandle<'_> {
    /// Ask the runner to tear the epoch down. Repeating it changes nothing.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.runner.wake();
    }

    /// The report, once published.
    pub fn try_exit(&mut self) -> Option<WifiRxExit> {
        self.exit.try_receive().ok()
    }

    /// Wait for the report.
    pub async fn exit(&mut self) -> WifiRxExit {
        poll_fn(|cx| {
            self.reporter.register(cx.waker());
            match self.exit.try_receive() {
                Ok(exit) => Poll::Ready(exit),
                Err(_) => Poll::Pending,
            }
        })
        .await
    }
}

/// The runner's side: observe the stop request, publish the report once.
pub struct ReceiveRunnerControl<'a> {
    stop: &'a AtomicBool,
    runner: &'a PriorityWaker,
    exit: AffineSpscSender<'a, WifiRxExit, 1>,
    reporter: &'a AtomicWaker,
}

impl ReceiveRunnerControl<'_> {
    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// Registration precedes the check, so a request is never missed.
    pub fn poll_stop(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.runner.register(cx.waker());
        if self.stop_requested() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Publish the report. The runner publishes one report per epoch; the
    /// queue holds it, so a later one is refused and dropped.
    pub fn publish(&self, exit: WifiRxExit) {
        let _refused = self.exit.try_send(exit);
        self.reporter.wake();
    }
}
