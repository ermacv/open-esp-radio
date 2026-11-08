use super::*;
use oer_portable_time::Clock;
use oer_portable_time_virtual::VirtualClock;
fn clock() -> VirtualClock {
    VirtualClock::starting_at(Instant::from_micros(0))
}
fn connect<K>(command: Option<ServiceCommand<K>>) -> u64 {
    match command {
        Some(ServiceCommand::Station(request)) if request.command() == Command::Connect => {
            request.correlation()
        }
        _ => panic!("expected connect"),
    }
}
fn failed(request: u64) -> ServiceOutput<()> {
    ServiceOutput::CommandCompleted(super::super::CommandTerminal {
        request: Request::new(request, Command::Connect),
        outcome: super::super::contract::CommandOutcome::ConnectFailed(
            super::super::contract::DisconnectReason::HandshakeFailed,
        ),
    })
}
fn linked(request: u64) -> ServiceOutput<()> {
    ServiceOutput::CommandCompleted(super::super::CommandTerminal {
        request: Request::new(request, Command::Connect),
        outcome: super::super::contract::CommandOutcome::Connected {
            bssid: [2; 6],
            channel: 6,
            phy: crate::station::contract::LinkPhy::Legacy,
        },
    })
}
fn lost() -> ServiceOutput<()> {
    ServiceOutput::Notice(super::super::contract::Notice::Disconnected(
        super::super::contract::DisconnectReason::Peer { reason_code: 15 },
    ))
}
#[test]
fn retries_follow_the_applications_linked_connect_the_policy_tick_and_terminal_credit() {
    let time = clock();
    let mut policy = Policy::new(ReconnectPolicy::every(Duration::from_secs(1)).unwrap());
    // Before the application's connect named a network, nothing connects.
    assert!(policy.command::<()>(time.now(), false).unwrap().is_none());
    time.advance(Duration::from_secs(10)).unwrap();
    assert!(policy.command::<()>(time.now(), false).unwrap().is_none());
    policy.admitted(Command::Connect);
    policy.observe(&linked(7), true);
    // The link is lost: one policy attempt per tick, after the terminal.
    policy.observe(&lost(), false);
    time.advance(Duration::from_secs(1)).unwrap();
    assert_eq!(connect::<()>(policy.command(time.now(), false).unwrap()), 1);
    policy.ready = false;
    policy.observe(&lost(), false);
    time.advance(Duration::from_secs(1)).unwrap();
    assert!(policy.command::<()>(time.now(), false).unwrap().is_none());
    // A failed policy attempt keeps the policy for the next tick.
    policy.observe(&failed(1), false);
    time.advance(Duration::from_secs(1)).unwrap();
    assert_eq!(connect::<()>(policy.command(time.now(), false).unwrap()), 1);
}
#[test]
fn an_application_connect_without_a_link_leaves_the_station_idle_for_the_next_connect() {
    let time = clock();
    let mut policy = Policy::new(ReconnectPolicy::every(Duration::from_secs(1)).unwrap());
    policy.admitted(Command::Connect);
    policy.observe(&lost(), false);
    policy.observe(&failed(7), false);
    for _ in 0..5 {
        time.advance(Duration::from_secs(1)).unwrap();
        assert!(policy.command::<()>(time.now(), false).unwrap().is_none());
        assert!(policy.wanted(false).is_none());
    }
    // Also after a link: the application's failed connect ends the retries.
    policy.admitted(Command::Connect);
    policy.observe(&linked(8), true);
    policy.admitted(Command::Connect);
    policy.observe(&lost(), false);
    policy.observe(&failed(9), false);
    time.advance(Duration::from_secs(1)).unwrap();
    assert!(policy.command::<()>(time.now(), false).unwrap().is_none());
    // The next linked connect activates it again.
    policy.admitted(Command::Connect);
    policy.observe(&linked(10), true);
    policy.observe(&lost(), false);
    time.advance(Duration::from_secs(1)).unwrap();
    assert_eq!(connect::<()>(policy.command(time.now(), false).unwrap()), 1);
}
#[test]
fn a_refused_application_connect_keeps_the_policy_of_the_established_link() {
    let time = clock();
    let mut policy = Policy::new(ReconnectPolicy::every(Duration::from_secs(1)).unwrap());
    policy.admitted(Command::Connect);
    policy.observe(&linked(7), true);
    policy.admitted(Command::Connect);
    policy.observe(
        &ServiceOutput::<()>::CommandRefused(super::super::CommandRefused {
            request: Request::new(8, Command::Connect),
            reason: super::super::contract::CommandRefusal::AlreadyConnected,
        }),
        true,
    );
    policy.observe(&lost(), false);
    time.advance(Duration::from_secs(1)).unwrap();
    assert_eq!(connect::<()>(policy.command(time.now(), false).unwrap()), 1);
}
#[test]
fn explicit_disconnect_pauses_reconnect_and_connect_resumes_it() {
    let time = clock();
    let mut policy = Policy::new(ReconnectPolicy::every(Duration::from_secs(1)).unwrap());
    policy.admitted(Command::Disconnect);
    time.advance(Duration::from_secs(10)).unwrap();
    assert!(!policy.ready(time.now(), false));
    policy.admitted(Command::Connect);
    assert!(policy.command::<()>(time.now(), false).unwrap().is_none());
    policy.observe(&linked(7), true);
    policy.observe(&lost(), false);
    time.advance(Duration::from_secs(1)).unwrap();
    assert_eq!(connect::<()>(policy.command(time.now(), false).unwrap()), 1);
}
#[test]
fn graceful_stop_disconnects_once_waits_for_link_down_and_never_reconnects() {
    let time = clock();
    let mut policy = Policy::new(ReconnectPolicy::every(Duration::from_secs(1)).unwrap());
    policy.stop();
    assert!(
        matches!(policy.command::<()>(time.now(), true).unwrap(), Some(ServiceCommand::Station(request)) if request.command() == Command::Disconnect && request.correlation() == 0xffff)
    );
    assert!(!policy.ready(time.now(), true));
    time.advance(Duration::from_secs(10)).unwrap();
    assert!(policy.command::<()>(time.now(), true).unwrap().is_none());
    assert!(matches!(
        policy.command::<()>(time.now(), false).unwrap(),
        Some(ServiceCommand::Stop)
    ));
}
#[test]
fn disabled_policy_has_no_timer_or_initial_connect_and_deadline_exhaustion_is_typed() {
    let time = clock();
    let mut policy = Policy::new(ReconnectPolicy::DISABLED);
    assert!(policy.command::<()>(time.now(), false).unwrap().is_none());
    assert!(policy.deadline().is_none());
    assert_eq!(
        ReconnectPolicy::every(Duration::from_micros(0)),
        Err(ZeroReconnectInterval)
    );
    let mut policy = Policy::new(ReconnectPolicy::every(Duration::from_secs(1)).unwrap());
    policy.admitted(Command::Connect);
    policy.observe(&linked(7), true);
    assert!(matches!(
        policy.command::<()>(Instant::from_micros(u64::MAX), false),
        Err(DeadlineOverflow)
    ));
}

/// `pending` tells exactly whether `command` gives one, at every step of a
/// retry and of a stop: its Disconnect, its wait for the link and its Stop.
#[test]
fn pending_tells_whether_the_next_command_gives_one() {
    let time = clock();
    let agrees = |policy: &Policy, linked: bool| {
        let mut copy = *policy;
        let gives = copy.command::<()>(time.now(), linked).unwrap().is_some();
        assert_eq!(policy.pending(time.now(), linked), gives);
        gives
    };
    let mut policy = Policy::new(ReconnectPolicy::every(Duration::from_secs(1)).unwrap());
    assert!(!agrees(&policy, false));
    policy.admitted(Command::Connect);
    assert!(!agrees(&policy, false));
    policy.observe(&linked(7), true);
    assert!(!agrees(&policy, true));
    policy.observe(&lost(), false);
    assert!(agrees(&policy, false), "the first retry goes at once");
    assert_eq!(connect::<()>(policy.command(time.now(), false).unwrap()), 1);
    assert!(!agrees(&policy, false), "the attempt runs");
    policy.observe(&lost(), false);
    policy.observe(&failed(1), false);
    assert!(!agrees(&policy, false), "the retry waits for its tick");
    time.advance(Duration::from_secs(1)).unwrap();
    assert!(agrees(&policy, false));
    // A stop: its Disconnect, then the Stop once the link is down.
    policy.stop();
    assert!(agrees(&policy, true));
    assert!(matches!(
        policy.command::<()>(time.now(), true).unwrap(),
        Some(ServiceCommand::Station(request)) if request.command() == Command::Disconnect
    ));
    assert!(!agrees(&policy, true), "the link is still up");
    assert!(agrees(&policy, false));
    assert!(matches!(
        policy.command::<()>(time.now(), false).unwrap(),
        Some(ServiceCommand::Stop)
    ));
}
