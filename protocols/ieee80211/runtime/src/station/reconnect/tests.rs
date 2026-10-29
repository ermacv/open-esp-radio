use super::*;
use oer_time::Clock;
use oer_time_virtual::VirtualClock;
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
#[test]
fn initial_connect_and_retries_follow_the_policy_tick_and_terminal_credit() {
    let time = clock();
    let mut policy = Policy::new(ReconnectPolicy::every(Duration::from_secs(1)).unwrap());
    assert_eq!(connect::<()>(policy.command(time.now(), false).unwrap()), 1);
    time.advance(Duration::from_secs(1)).unwrap();
    assert!(policy.command::<()>(time.now(), false).unwrap().is_none());
    policy.observe(
        &ServiceOutput::<()>::Notice(super::super::contract::Notice::Disconnected(
            super::super::contract::DisconnectReason::AuthenticationFailed,
        )),
        false,
    );
    assert!(policy.command::<()>(time.now(), false).unwrap().is_none());
    policy.observe(
        &ServiceOutput::<()>::CommandCompleted(super::super::CommandTerminal {
            request: Request::new(1, Command::Connect),
            outcome: super::super::contract::CommandOutcome::ConnectFailed(
                super::super::contract::DisconnectReason::AuthenticationFailed,
            ),
        }),
        false,
    );
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
    policy.ready = true;
    policy.observe(
        &ServiceOutput::<()>::Notice(super::super::contract::Notice::Disconnected(
            super::super::contract::DisconnectReason::HandshakeFailed,
        )),
        false,
    );
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
    assert!(matches!(
        policy.command::<()>(Instant::from_micros(u64::MAX), false),
        Err(DeadlineOverflow)
    ));
}
