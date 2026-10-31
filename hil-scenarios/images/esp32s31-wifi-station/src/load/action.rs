//! Which requests the load server answers by itself, idle or running.

use oer_services_load::{
    Collect, CollectCounters, CollectParts, CollectSamples, LoadError, MAX_DELAY_MS, MAX_DRAIN_MS,
    MAX_DURATION_MS, MAX_SAMPLE_PERIOD_US, MIN_SAMPLE_PERIOD_US, Request, Start, Started, header,
};

/// The largest UDP payload the image sends or receives: one 1514-byte
/// Ethernet frame of the stack's packet buffers less its 14-byte Ethernet,
/// 20-byte IPv4 and 8-byte UDP headers. The stack fragments nothing.
pub(crate) const MAX_PAYLOAD: u16 = 1472;

/// Where the load server is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Phase {
    /// No run runs; `kept` names the run whose report the server keeps.
    Idle { kept: Option<u32> },
    /// Run `run` runs: its window or drain has not ended.
    Running { run: u32 },
}

/// What the load server does with one request.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Action {
    /// Answer it now.
    Answer(Answer),
    /// Answer with the CPU report.
    Cpu,
    /// Start this run, once the station has its address.
    Start(Start),
    /// Answer with what the run kept.
    Kept(Kept),
}

/// What a run keeps once it ended: its report, its parts' window, its
/// counters and its samples.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kept {
    Report,
    /// The parts from the one numbered `from`.
    Parts {
        from: u8,
    },
    Counters,
    /// The samples from the one numbered `from`.
    Samples {
        from: u16,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Answer {
    Started(Started),
    /// `Running` of the collected kind: the collected run has not ended.
    Running(Kept),
    /// `Refused(UnknownRun)` of the collected kind.
    UnknownRun(Kept),
}

/// What the server does with `request` in `phase`: one run at a time; a
/// collect of the running run answers that it runs, of the kept one what
/// it kept.
pub(crate) fn action(phase: Phase, request: Request) -> Action {
    let refuse_start = |error| Action::Answer(Answer::Started(Started::Refused(error)));
    match (phase, request) {
        (_, Request::GetCpu(_)) => Action::Cpu,
        (Phase::Idle { .. }, Request::Start(start)) => match validate(&start) {
            Ok(()) => Action::Start(start),
            Err(error) => refuse_start(error),
        },
        (Phase::Running { .. }, Request::Start(_)) => refuse_start(LoadError::Busy),
        (phase, Request::Collect(Collect { run })) => collect(phase, run, Kept::Report),
        (phase, Request::CollectParts(CollectParts { run, from })) => {
            collect(phase, run, Kept::Parts { from })
        }
        (phase, Request::CollectCounters(CollectCounters { run })) => {
            collect(phase, run, Kept::Counters)
        }
        (phase, Request::CollectSamples(CollectSamples { run, from })) => {
            collect(phase, run, Kept::Samples { from })
        }
    }
}

fn collect(phase: Phase, asked: u32, kind: Kept) -> Action {
    match phase {
        Phase::Running { run } if asked == run => Action::Answer(Answer::Running(kind)),
        Phase::Idle { kept: Some(run) } if asked == run => Action::Kept(kind),
        _ => Action::Answer(Answer::UnknownRun(kind)),
    }
}

/// A run's parameters: nonzero ports, a window, delay and drain within
/// their maxima, a transmitted payload that holds the header and fits one
/// datagram of the image, and a sampling period within its range.
fn validate(start: &Start) -> Result<(), LoadError> {
    let period = start.profile.map(|profile| profile.period_us);
    if start.port == 0
        || start.delay_ms > MAX_DELAY_MS
        || start.duration_ms > MAX_DURATION_MS
        || start.drain_ms > MAX_DRAIN_MS
        || period
            .is_some_and(|period| !(MIN_SAMPLE_PERIOD_US..=MAX_SAMPLE_PERIOD_US).contains(&period))
    {
        return Err(LoadError::InvalidParameters);
    }
    match start.transmit {
        Some(transmit)
            if transmit.peer_port == 0
                || usize::from(transmit.payload) < header::LENGTH
                || transmit.payload > MAX_PAYLOAD =>
        {
            Err(LoadError::InvalidParameters)
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_services_load::{GetCpu, Profile, Transmit};

    const IDLE: Phase = Phase::Idle { kept: None };
    const RUNNING: Phase = Phase::Running { run: 7 };

    fn start(transmit: Option<Transmit>) -> Start {
        Start {
            run: 7,
            port: 5001,
            delay_ms: 500,
            duration_ms: 10_000,
            drain_ms: 1_000,
            transmit,
            profile: None,
        }
    }

    fn transmit(payload: u16) -> Transmit {
        Transmit {
            peer: [192, 168, 1, 2],
            peer_port: 5002,
            payload,
            rate_bps: 0,
        }
    }

    fn refused(error: LoadError) -> Action {
        Action::Answer(Answer::Started(Started::Refused(error)))
    }

    #[test]
    fn an_idle_server_starts_a_valid_run_and_refuses_invalid_parameters() {
        assert_eq!(
            action(IDLE, Request::Start(start(None))),
            Action::Start(start(None))
        );
        let longest = Start {
            delay_ms: MAX_DELAY_MS,
            duration_ms: MAX_DURATION_MS,
            drain_ms: MAX_DRAIN_MS,
            ..start(None)
        };
        assert_eq!(
            action(IDLE, Request::Start(longest)),
            Action::Start(longest)
        );
        for payload in [header::LENGTH as u16, MAX_PAYLOAD] {
            let valid = start(Some(transmit(payload)));
            assert_eq!(action(IDLE, Request::Start(valid)), Action::Start(valid));
        }
        for period_us in [MIN_SAMPLE_PERIOD_US, MAX_SAMPLE_PERIOD_US] {
            let sampled = Start {
                profile: Some(Profile { period_us }),
                ..start(None)
            };
            assert_eq!(
                action(IDLE, Request::Start(sampled)),
                Action::Start(sampled)
            );
        }
        let mut invalid = [0, header::LENGTH as u16 - 1, MAX_PAYLOAD + 1]
            .map(|payload| start(Some(transmit(payload))))
            .to_vec();
        invalid.extend([
            Start {
                port: 0,
                ..start(None)
            },
            start(Some(Transmit {
                peer_port: 0,
                ..transmit(1400)
            })),
            Start {
                delay_ms: MAX_DELAY_MS + 1,
                ..start(None)
            },
            Start {
                duration_ms: MAX_DURATION_MS + 1,
                ..start(None)
            },
            Start {
                drain_ms: MAX_DRAIN_MS + 1,
                ..start(None)
            },
            Start {
                profile: Some(Profile {
                    period_us: MIN_SAMPLE_PERIOD_US - 1,
                }),
                ..start(None)
            },
            Start {
                profile: Some(Profile {
                    period_us: MAX_SAMPLE_PERIOD_US + 1,
                }),
                ..start(None)
            },
        ]);
        for start in invalid {
            assert_eq!(
                action(IDLE, Request::Start(start)),
                refused(LoadError::InvalidParameters),
                "{start:?}"
            );
        }
    }

    #[test]
    fn a_running_server_refuses_a_second_start_even_an_invalid_one() {
        assert_eq!(
            action(RUNNING, Request::Start(start(None))),
            refused(LoadError::Busy)
        );
        let invalid = Start {
            port: 0,
            ..start(None)
        };
        assert_eq!(
            action(RUNNING, Request::Start(invalid)),
            refused(LoadError::Busy)
        );
    }

    #[test]
    fn a_collect_answers_running_then_what_its_own_run_kept_only() {
        type Asks = fn(u32) -> Request;
        let requests: [(Kept, Asks); 4] = [
            (Kept::Report, |run| Request::Collect(Collect { run })),
            (Kept::Parts { from: 12 }, |run| {
                Request::CollectParts(CollectParts { run, from: 12 })
            }),
            (Kept::Counters, |run| {
                Request::CollectCounters(CollectCounters { run })
            }),
            (Kept::Samples { from: 40 }, |run| {
                Request::CollectSamples(CollectSamples { run, from: 40 })
            }),
        ];
        for (kind, collect) in requests {
            let unknown = Action::Answer(Answer::UnknownRun(kind));
            assert_eq!(
                action(RUNNING, collect(7)),
                Action::Answer(Answer::Running(kind))
            );
            assert_eq!(action(RUNNING, collect(6)), unknown);
            let kept = Phase::Idle { kept: Some(7) };
            assert_eq!(action(kept, collect(7)), Action::Kept(kind));
            assert_eq!(action(kept, collect(8)), unknown);
            assert_eq!(action(IDLE, collect(7)), unknown);
        }
        let kept = Phase::Idle { kept: Some(7) };
        assert_eq!(
            action(kept, Request::Start(start(None))),
            Action::Start(start(None)),
            "a kept report does not hold off the next run"
        );
    }

    #[test]
    fn the_cpu_report_is_answered_in_every_phase() {
        for phase in [IDLE, Phase::Idle { kept: Some(7) }, RUNNING] {
            assert_eq!(action(phase, Request::GetCpu(GetCpu)), Action::Cpu);
        }
    }
}
