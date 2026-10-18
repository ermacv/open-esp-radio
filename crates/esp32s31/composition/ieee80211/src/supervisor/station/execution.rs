//! Same-Core0 execution and owner return for the connected station datapath.
//!
//! The supervisor transfers its non-`Send` runner to one permanent child task on its
//! recorded executor. The non-`Sync` mailbox retains the exact returned runner;
//! signals carry stop and completion notifications. Stop stays latched and
//! retains ordinary terminal teardown.

use core::cell::{Cell, RefCell};

mod exchange;
#[cfg(feature = "connected-datapath-cycle-telemetry")]
use core::future::poll_fn;

use embassy_executor::Spawner;

use core::{convert::Infallible, future::Future, pin::pin};

use embassy_futures::select::{Either, Either3, select, select3};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use exchange::Exchange;

use oer_esp32s31_ieee80211_runtime::{
    datapath::{DatapathRunnerExit, execution::Control},
    roles::station::{
        connected::{StationCommand, StationCommandReceiver},
        power::StationPowerFailure,
    },
};

use oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason;

use static_cell::StaticCell;

use super::{ConnectedDatapathError, ConnectedDatapathRunner};

pub(super) struct ConnectedDatapathTaskReturn {
    pub(super) runner: ConnectedDatapathRunner,
    pub(super) result:
        Result<DatapathRunnerExit<ConnectedDisconnectReason>, ConnectedDatapathError>,
}

/// Same-executor rendezvous for the non-`Send` connected radio owner.
///
/// `StaticCell` provides stable storage, while this type deliberately does
/// not implement `Sync`: both participants are spawned by the one Core0
/// executor recorded before the physical supervisor starts.
// CAPABILITY: station-lifecycle-owners
pub(crate) struct ConnectedDatapathMailbox {
    bound: Cell<bool>,
    exchange: Exchange<ConnectedDatapathRunner, ConnectedDatapathTaskReturn>,
    control: RefCell<Control<CriticalSectionRawMutex>>,
    #[cfg(feature = "connected-datapath-cycle-telemetry")]
    poll_observer: Option<crate::ConnectedDatapathPollObserver>,
}

impl ConnectedDatapathMailbox {
    const fn new(
        #[cfg(feature = "connected-datapath-cycle-telemetry")] poll_observer: Option<
            crate::ConnectedDatapathPollObserver,
        >,
    ) -> Self {
        Self {
            bound: Cell::new(false),
            exchange: Exchange::new(),
            control: RefCell::new(Control::new()),
            #[cfg(feature = "connected-datapath-cycle-telemetry")]
            poll_observer,
        }
    }

    pub(in crate::supervisor) fn bind(&'static self, spawner: Spawner) {
        assert!(
            !self.bound.replace(true),
            "connected datapath executor is bound once"
        );
        spawner
            .spawn(connected_datapath_task(self).expect("one permanent connected datapath worker"));
    }

    #[inline(never)]
    pub(super) fn start(&self, runner: &mut Option<ConnectedDatapathRunner>) {
        assert!(
            self.bound.get(),
            "radio runner binds its Core0 executor before station start"
        );
        let runner = runner.take().expect("live connected runner");
        if self.exchange.submit(runner).is_err() {
            panic!("previous connected datapath owner must be reclaimed");
        }
        // No await: the same-core worker cannot poll before this new epoch
        // replaces control. Failed submission must never clear an old Stop.
        *self.control.borrow_mut() = Control::new();
    }

    pub(super) fn request_stop(&self) {
        self.control.borrow().request_stop();
    }

    fn finish(&self, returned: ConnectedDatapathTaskReturn) {
        self.exchange.finish(returned);
    }

    async fn wait_completed(&self) {
        self.exchange.wait_completed().await;
    }

    pub(super) fn take_return(&self) -> ConnectedDatapathTaskReturn {
        self.exchange.take_return()
    }
}

static CONNECTED_DATAPATH_MAILBOX: StaticCell<ConnectedDatapathMailbox> = StaticCell::new();

pub(in crate::supervisor) fn initialize_connected_datapath_mailbox(
    #[cfg(feature = "connected-datapath-cycle-telemetry")] poll_observer: Option<
        crate::ConnectedDatapathPollObserver,
    >,
) -> &'static ConnectedDatapathMailbox {
    CONNECTED_DATAPATH_MAILBOX.init_with(|| {
        ConnectedDatapathMailbox::new(
            #[cfg(feature = "connected-datapath-cycle-telemetry")]
            poll_observer,
        )
    })
}

#[allow(
    clippy::await_holding_refcell_ref,
    reason = "shared control borrow permits concurrent requests and prevents replacing the epoch while the worker runs"
)]
#[embassy_executor::task(pool_size = 1)]
async fn connected_datapath_task(mailbox: &'static ConnectedDatapathMailbox) {
    loop {
        let mut runner = mailbox.exchange.next().await;
        let control = mailbox.control.borrow();
        #[cfg(feature = "connected-datapath-cycle-telemetry")]
        let result = if let Some(observer) = mailbox.poll_observer {
            const POLLS_PER_BATCH: u32 = 256;
            let cycles_per_micro = observer.cycles_per_micro();
            let mut batch = crate::ConnectedDatapathPollBatch::default();
            let mut run = core::pin::pin!(runner.run_controlled(&control));
            let result = poll_fn(|context| {
                let started = riscv::register::mcycle::read() as u32;
                let result = run.as_mut().poll(context);
                let elapsed_cycles = (riscv::register::mcycle::read() as u32).wrapping_sub(started);
                let elapsed_micros = elapsed_cycles.div_ceil(cycles_per_micro);
                batch.polls = batch.polls.saturating_add(1);
                batch.poll_micros = batch.poll_micros.saturating_add(elapsed_micros);
                batch.maximum_poll_micros = batch.maximum_poll_micros.max(elapsed_micros);
                batch.over_100_micros = batch
                    .over_100_micros
                    .saturating_add(u32::from(elapsed_micros > 100));
                batch.over_500_micros = batch
                    .over_500_micros
                    .saturating_add(u32::from(elapsed_micros > 500));
                batch.over_1_000_micros = batch
                    .over_1_000_micros
                    .saturating_add(u32::from(elapsed_micros > 1_000));
                batch.over_5_000_micros = batch
                    .over_5_000_micros
                    .saturating_add(u32::from(elapsed_micros > 5_000));
                if batch.polls == POLLS_PER_BATCH {
                    observer.record(batch);
                    batch = crate::ConnectedDatapathPollBatch::default();
                }
                result
            })
            .await;
            if batch.polls != 0 {
                observer.record(batch);
            }
            result
        } else {
            runner.run_controlled(&control).await
        };
        #[cfg(not(feature = "connected-datapath-cycle-telemetry"))]
        let result = runner.run_controlled(&control).await;
        drop(control);
        mailbox.finish(ConnectedDatapathTaskReturn { runner, result });
    }
}

/// Keep active execution separate from connection assembly and terminal
/// teardown. A station command stops the worker at its next TX-idle boundary.
///
/// `power` is the station's power agent. It runs until the worker returns;
/// when it fails, control reads the failure and ends the association, so the
/// agent then only waits.
pub(super) async fn run(
    mailbox: &'static ConnectedDatapathMailbox,
    station_control: &mut StationCommandReceiver<'_, CriticalSectionRawMutex>,
    runner: &mut Option<ConnectedDatapathRunner>,
    power: impl Future<Output = StationPowerFailure>,
) -> (
    Result<DatapathRunnerExit<ConnectedDisconnectReason>, ConnectedDatapathError>,
    Option<StationCommand>,
) {
    // This arm only stops racing once power save fails; it ends no port.
    #[allow(
        clippy::disallowed_methods,
        reason = "a select arm that stays out of the race, not a port body"
    )]
    let power = async {
        let _failure = power.await;
        core::future::pending::<Infallible>().await
    };
    let mut power = pin!(power);
    mailbox.start(runner);
    let command = match select3(
        mailbox.wait_completed(),
        station_control.wait(),
        power.as_mut(),
    )
    .await
    {
        Either3::First(()) => None,
        Either3::Second(command) => {
            mailbox.request_stop();
            match select(mailbox.wait_completed(), power.as_mut()).await {
                Either::First(()) => {}
                Either::Second(never) => match never {},
            }
            Some(command)
        }
        Either3::Third(never) => match never {},
    };
    (take_runner(mailbox, runner), command)
}

/// Transfer once into caller-owned storage instead of keeping a second complete
/// return value in the execution future's poll frame.
#[inline(never)]
fn take_runner(
    mailbox: &ConnectedDatapathMailbox,
    runner: &mut Option<ConnectedDatapathRunner>,
) -> Result<DatapathRunnerExit<ConnectedDisconnectReason>, ConnectedDatapathError> {
    let returned = mailbox.take_return();
    *runner = Some(returned.runner);
    returned.result
}
