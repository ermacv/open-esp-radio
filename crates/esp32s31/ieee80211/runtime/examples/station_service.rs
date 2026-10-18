//! Host-runnable shape of a non-HIL Embassy station application.
//!
//! A board firmware replaces `ExampleRunner` with
//! `StaAttemptTargetPort` composition. Connected RX protocol work is
//! owned by the radio DATAPATH runner, so stopping that finite runner returns
//! the complete RX owner graph without a second executor-task handshake.

use core::future::{Future, ready};

use embassy_futures::block_on;

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_esp32s31_ieee80211_runtime::roles::station::{
    StationAttemptRunner, StationCommandReceiver, StationConfiguration, StationControlResources,
    StationExit, StationStartResources, StationStopReason, prepare_esp32s31_station_task,
};

use oer_time_virtual::SkipClock;

use oer_ieee80211_sta::station::{StaAttemptContext, StaAttemptOutcome, StaReconnectPolicy};

#[derive(Debug, Eq, PartialEq)]
struct StationOwner {
    dma_generation: u32,
}

struct ExampleRunner {
    clock: SkipClock,
}

impl StationAttemptRunner<NoopRawMutex> for ExampleRunner {
    type Owner = StationOwner;
    type Error = ();
    type Fault = core::convert::Infallible;
    type Timer = SkipClock;

    fn timer(&self) -> &SkipClock {
        &self.clock
    }

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + 'a {
        ready(StaAttemptOutcome::Stopped { owner })
    }
}

fn main() {
    let policy = StaReconnectPolicy::new(3, 100, 1_000, 100).unwrap();
    let control = StationControlResources::<NoopRawMutex>::new();
    let (controller, mut runner) = prepare_esp32s31_station_task(
        StationConfiguration::new(policy),
        StationStartResources::new(StationOwner { dma_generation: 7 }),
        &control,
        ExampleRunner {
            clock: SkipClock::new(),
        },
    )
    .expect("fresh station control resources must accept one task");

    assert!(controller.request_stop());
    let StationExit::Stopped {
        resources, reason, ..
    } = block_on(runner.run())
    else {
        panic!("finite station stop did not return its owner");
    };
    assert!(matches!(reason, StationStopReason::Requested(_)));
    let (owner, _runner) = resources.into_parts();
    assert_eq!(owner.dma_generation, 7);
}
