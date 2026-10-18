use core::future::ready;

use crate::test_support::block_on;
use oer_ieee80211_sta::scan::{ActiveProbeOutcome, StaScanChannelContext};

use crate::scan::StaCandidateScanService;
use oer_ieee80211_sta::scan::StaCandidateScanExit;

use std::vec::Vec;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Begin,
    Switch(u8),
    Start(u8),
    Probe(u8),
    Observe(u8),
    Wait,
    Stop(u8),
    Prepare(u8),
    Select,
}

struct Owner {
    identity: u32,
    actions: Vec<Action>,
    fail: Option<Action>,
    probe_fallback: bool,
    candidate: Option<u8>,
    dwell_ticks: Option<u16>,
}

impl Owner {
    fn new(identity: u32) -> Self {
        Self {
            identity,
            actions: Vec::new(),
            fail: None,
            probe_fallback: false,
            candidate: Some(11),
            dwell_ticks: None,
        }
    }

    fn record(&mut self, action: Action) -> Result<(), Action> {
        self.actions.push(action);
        if self.fail == Some(action) {
            Err(action)
        } else {
            Ok(())
        }
    }
}

impl StaScanPort for Owner {
    type Channel = u8;
    type Candidate = u8;
    type Error = Action;

    fn begin_scan(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        ready(self.record(Action::Begin))
    }

    fn switch_channel(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
        requested_dwell_ticks: u16,
    ) -> impl Future<Output = Result<u16, Self::Error>> + '_ {
        let dwell_ticks = self.dwell_ticks.unwrap_or(requested_dwell_ticks);
        ready(
            self.record(Action::Switch(context.channel))
                .map(|()| dwell_ticks),
        )
    }

    fn start_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        ready(self.record(Action::Start(context.channel)))
    }

    fn transmit_active_probe(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<ActiveProbeOutcome, Self::Error>> + '_ {
        self.actions.push(Action::Probe(context.channel));
        let outcome = if self.probe_fallback {
            ActiveProbeOutcome::PassiveFallback
        } else {
            ActiveProbeOutcome::Transmitted
        };
        ready(if self.fail == Some(Action::Probe(context.channel)) {
            Err(Action::Probe(context.channel))
        } else {
            Ok(outcome)
        })
    }

    fn observe_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        self.record(Action::Observe(context.channel))
    }

    fn wait_dwell_tick(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        ready(self.record(Action::Wait))
    }

    fn stop_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        ready(self.record(Action::Stop(context.channel)))
    }

    fn prepare_next_ring(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        self.record(Action::Prepare(context.channel))
    }

    fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error> {
        self.record(Action::Select)?;
        Ok(self.candidate)
    }
}

fn backend() -> StaScanBackend<Owner> {
    StaScanBackend::new(StaScanConfig::new(2).unwrap())
}

#[test]
fn two_channels_preserve_the_complete_transaction_order() {
    let mut service = StaCandidateScanService::new(backend());
    let exit = block_on(service.run(Owner::new(41), &[1, 6]));
    let StaCandidateScanExit::Selected {
        owner, candidate, ..
    } = exit
    else {
        panic!("scan must select the planned candidate")
    };

    assert_eq!(owner.identity, 41);
    assert_eq!(candidate, 11);
    assert_eq!(
        owner.actions,
        [
            Action::Begin,
            Action::Switch(1),
            Action::Start(1),
            Action::Probe(1),
            Action::Observe(1),
            Action::Wait,
            Action::Observe(1),
            Action::Wait,
            Action::Stop(1),
            Action::Prepare(1),
            Action::Switch(6),
            Action::Start(6),
            Action::Probe(6),
            Action::Observe(6),
            Action::Wait,
            Action::Observe(6),
            Action::Wait,
            Action::Stop(6),
            Action::Select,
        ]
    );
}

#[test]
fn the_port_chooses_each_channel_dwell() {
    let mut owner = Owner::new(3);
    owner.dwell_ticks = Some(1);
    let mut service = StaCandidateScanService::new(backend());
    let StaCandidateScanExit::Selected { owner, .. } = block_on(service.run(owner, &[6])) else {
        panic!("scan must select the planned candidate")
    };
    assert_eq!(
        owner.actions,
        [
            Action::Begin,
            Action::Switch(6),
            Action::Start(6),
            Action::Probe(6),
            Action::Observe(6),
            Action::Wait,
            Action::Stop(6),
            Action::Select,
        ]
    );
}

#[test]
fn passive_probe_fallback_does_not_abort_the_receive_dwell() {
    let mut owner = Owner::new(7);
    owner.probe_fallback = true;
    let mut service = StaCandidateScanService::new(backend());

    let exit = block_on(service.run(owner, &[3]));

    assert!(matches!(
        exit,
        StaCandidateScanExit::Selected {
            owner: Owner { identity: 7, .. },
            candidate: 11,
            ..
        }
    ));
}

#[test]
fn fatal_probe_failure_still_closes_the_live_rx_epoch() {
    let mut owner = Owner::new(17);
    owner.fail = Some(Action::Probe(3));
    let mut service = StaCandidateScanService::new(backend());

    let exit = block_on(service.run(owner, &[3]));
    let StaCandidateScanExit::Failed { owner, error, .. } = exit else {
        panic!("fatal active-probe failure must be returned")
    };

    assert_eq!(error, StaScanError::ActiveProbe(Action::Probe(3)));
    assert_eq!(
        owner.actions,
        [
            Action::Begin,
            Action::Switch(3),
            Action::Start(3),
            Action::Probe(3),
            Action::Stop(3),
        ]
    );
}

#[test]
fn dwell_failure_still_stops_rx_and_returns_the_exact_owner() {
    let mut owner = Owner::new(99);
    owner.fail = Some(Action::Observe(4));
    let mut service = StaCandidateScanService::new(backend());

    let exit = block_on(service.run(owner, &[4]));
    let StaCandidateScanExit::Failed { owner, error, .. } = exit else {
        panic!("planned dwell failure must be returned")
    };

    assert_eq!(owner.identity, 99);
    assert_eq!(error, StaScanError::ReceiveObserve(Action::Observe(4)));
    assert_eq!(
        owner.actions,
        [
            Action::Begin,
            Action::Switch(4),
            Action::Start(4),
            Action::Probe(4),
            Action::Observe(4),
            Action::Stop(4),
        ]
    );
}

#[test]
fn stop_failure_takes_precedence_over_an_earlier_dwell_failure() {
    struct StopFailureOwner(Owner);

    impl StaScanPort for StopFailureOwner {
        type Channel = u8;
        type Candidate = u8;
        type Error = Action;

        fn begin_scan(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            self.0.begin_scan()
        }

        fn switch_channel(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
            requested_dwell_ticks: u16,
        ) -> impl Future<Output = Result<u16, Self::Error>> + '_ {
            self.0.switch_channel(context, requested_dwell_ticks)
        }

        fn start_receive(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            self.0.start_receive(context)
        }

        fn transmit_active_probe(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> impl Future<Output = Result<ActiveProbeOutcome, Self::Error>> + '_ {
            self.0.transmit_active_probe(context)
        }

        fn observe_receive(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> Result<(), Self::Error> {
            self.0.record(Action::Observe(context.channel))?;
            Err(Action::Observe(context.channel))
        }

        fn wait_dwell_tick(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            self.0.wait_dwell_tick()
        }

        fn stop_receive(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
            self.0.actions.push(Action::Stop(context.channel));
            ready(Err(Action::Stop(context.channel)))
        }

        fn prepare_next_ring(
            &mut self,
            context: StaScanChannelContext<Self::Channel>,
        ) -> Result<(), Self::Error> {
            self.0.prepare_next_ring(context)
        }

        fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error> {
            self.0.select_candidate()
        }
    }

    let owner = StopFailureOwner(Owner::new(123));
    let backend = StaScanBackend::new(StaScanConfig::new(1).unwrap());
    let mut service = StaCandidateScanService::new(backend);
    let exit = block_on(service.run(owner, &[9]));

    assert!(matches!(
        exit,
        StaCandidateScanExit::Failed {
            owner: StopFailureOwner(Owner { identity: 123, .. }),
            error: StaScanError::ReceiveStop(Action::Stop(9)),
            ..
        }
    ));
}
