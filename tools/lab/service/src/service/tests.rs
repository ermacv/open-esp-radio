//! The service with fake drivers in the test's own process; clients talk to
//! it over socket pairs.

use std::sync::atomic::AtomicU64;

use super::*;
use crate::client::{Client, Event};

const A: &str = "02:00:00:00:00:0a";

fn board(mac: &str) -> String {
    format!("board:{mac}")
}

/// A gate a fake holds until the test opens it.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    fn opened() -> Self {
        let gate = Self::default();
        gate.open();
        gate
    }

    fn open(&self) {
        *lock(&self.open) = true;
        self.changed.notify_all();
    }

    fn close(&self) {
        *lock(&self.open) = false;
    }

    fn pass(&self) {
        let mut open = lock(&self.open);
        while !*open {
            open = self
                .changed
                .wait(open)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Recoveries pass `recovery` and confirm `confirmation`; a `hold`
/// operation keeps its hardware busy until `hardware` opens, whether or not
/// it was asked to stop; `flood` writes `flood` bytes at once.
struct Fake {
    recovery: Gate,
    confirmation: Mutex<Confirmation>,
    hardware: Gate,
    recoveries: AtomicU64,
}

impl Fake {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            recovery: Gate::opened(),
            confirmation: Mutex::new(Confirmation::Ready),
            hardware: Gate::opened(),
            recoveries: AtomicU64::new(0),
        })
    }
}

const FLOOD: usize = 4 * OUTPUT_BUDGET;

impl Drivers for Fake {
    fn recover(&self, _: &Recovery) -> Confirmation {
        self.recovery.pass();
        self.recoveries.fetch_add(1, Ordering::SeqCst);
        lock(&self.confirmation).clone()
    }

    fn run(
        &self,
        operation: &Operation,
        _: &[Claim],
        io: OperationIo,
    ) -> Result<serde_json::Value, String> {
        let Operation::Command { argv, .. } = operation else {
            return Err("the fake runs commands only".to_owned());
        };
        match argv.first().map(String::as_str) {
            Some("hold") => {
                self.hardware.pass();
                Ok(serde_json::json!("released"))
            }
            Some("wait") => {
                while !io.stop.load(Ordering::SeqCst) {
                    std::thread::park_timeout(std::time::Duration::from_millis(1));
                }
                Ok(serde_json::json!("stopped"))
            }
            Some("flood") => {
                for _ in 0..FLOOD / 1024 {
                    io.output.write(&[b'x'; 1024]);
                }
                Ok(serde_json::json!("flooded"))
            }
            Some("echo") => {
                for chunk in io.input.iter() {
                    io.output.write(&chunk);
                }
                Ok(serde_json::json!("echoed"))
            }
            other => Err(format!("no fake operation {other:?}")),
        }
    }
}

fn command(name: &str) -> Operation {
    Operation::Command {
        argv: vec![name.to_owned()],
        directory: "/".to_owned(),
        environment: Vec::new(),
    }
}

const START_MS: u64 = 1_700_000_000_000;

fn open(directory: &std::path::Path, fake: &Arc<Fake>) -> Service {
    open_at(directory, fake, Arc::new(AtomicU64::new(START_MS)))
}

/// A service whose clock reads `now`.
fn open_at(directory: &std::path::Path, fake: &Arc<Fake>, now: Arc<AtomicU64>) -> Service {
    Service::open(
        oer_lab_arbiter::Arbiter::at(directory).unwrap(),
        vec![board(A)],
        fake.clone(),
        Box::new(move || now.load(Ordering::SeqCst)),
    )
    .unwrap()
}

fn client(service: &Service) -> Client {
    Client::connect(service.connect().unwrap(), "test").unwrap()
}

fn request(owner: &str) -> LeaseRequest {
    LeaseRequest {
        owner: owner.to_owned(),
        work: "test".to_owned(),
        scenarios: Vec::new(),
        run: None,
        claims: vec![Claim::board(A)],
        maintenance: None,
        divisible: false,
    }
}

fn ready(status: &Status) -> bool {
    status.resources.iter().all(|(_, readiness)| {
        matches!(
            readiness,
            oer_lab_arbiter::readiness::Readiness::Ready { .. }
        )
    })
}

fn readiness(service: &Service) -> oer_lab_arbiter::readiness::Readiness {
    service
        .status()
        .resources
        .into_iter()
        .find(|(resource, _)| *resource == board(A))
        .map(|(_, readiness)| readiness)
        .unwrap()
}

#[test]
fn a_client_of_another_protocol_is_refused_with_the_reason() {
    let directory = tempfile::tempdir().unwrap();
    let service = open(directory.path(), &Fake::new());
    let mut stream = service.connect().unwrap();
    Frame::message(&Request::Hello {
        protocol: PROTOCOL + 1,
        client: "newer".to_owned(),
    })
    .unwrap()
    .write(&mut stream)
    .unwrap();
    let Some(Frame::Message(json)) = Frame::read(&mut stream).unwrap() else {
        panic!("no reply");
    };
    let Reply::Refused { reason } = serde_json::from_slice(&json).unwrap() else {
        panic!("not refused");
    };
    assert!(reason.contains("cargo stand install"), "{reason}");
    assert!(
        Frame::read(&mut stream).unwrap().is_none(),
        "the connection ends"
    );
}

#[test]
fn a_lost_client_releases_nothing_while_its_hardware_work_runs() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let service = open(directory.path(), &fake);
    service.wait_until(ready);
    fake.hardware.close();

    let mut lost = client(&service);
    lost.acquire(request("ann"), &mut |_| {}).unwrap();
    let id = 1;
    lost.send(&Request::Start {
        id,
        operation: command("hold"),
    })
    .unwrap();
    service.wait_until(|status| status.holders.iter().any(|h| h.operations == 1));
    drop(lost);
    service.wait_until(|status| status.holders.iter().all(|h| h.ending));

    let waiter = service.clone();
    let next = std::thread::spawn(move || {
        let mut next = client(&waiter);
        next.acquire(request("bob"), &mut |_| {}).map(|_| next)
    });
    // Either the next client waits, or (wrongly) it was granted the board.
    service.wait_until(|status| {
        !status.waiting.is_empty() || status.holders.iter().any(|holder| !holder.ending)
    });
    let status = service.status();
    assert_eq!(
        status.holders.len(),
        1,
        "the lost lease still holds the board"
    );
    assert!(status.holders[0].ending);
    assert!(matches!(
        readiness(&service),
        oer_lab_arbiter::readiness::Readiness::InUse { .. }
    ));
    assert_eq!(
        fake.recoveries.load(Ordering::SeqCst),
        1,
        "only the first recovery"
    );

    fake.hardware.open();
    let _next = next.join().unwrap().unwrap();
    assert_eq!(fake.recoveries.load(Ordering::SeqCst), 2);
    let history = std::fs::read_to_string(directory.path().join("history.jsonl")).unwrap();
    assert!(history.contains("\"abandoned\""), "{history}");
}

#[test]
fn a_failed_recovery_holds_the_board_until_a_person_returns_it() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let service = open(directory.path(), &fake);
    service.wait_until(ready);
    *lock(&fake.confirmation) = Confirmation::Unavailable {
        reason: "neither its image nor its ROM answered".to_owned(),
        quiet: false,
    };
    let mut first = client(&service);
    first.acquire(request("ann"), &mut |_| {}).unwrap();
    first.release().unwrap();
    service.wait_until(|status| !ready(status));
    service.wait_until(|status| {
        status.resources.iter().any(|(_, readiness)| {
            matches!(
                readiness,
                oer_lab_arbiter::readiness::Readiness::Unavailable { .. }
            )
        })
    });

    let waiter = service.clone();
    let next = std::thread::spawn(move || {
        let mut next = client(&waiter);
        next.acquire(request("bob"), &mut |_| {}).map(|_| next)
    });
    service.wait_until(|status| !status.waiting.is_empty());
    assert!(service.status().holders.is_empty());
    *lock(&fake.confirmation) = Confirmation::Ready;
    let mut person = client(&service);
    assert!(person.return_resource("board:unknown").is_err());
    person.return_resource(&board(A)).unwrap();
    let _next = next.join().unwrap().unwrap();
}

#[test]
fn a_restart_with_a_lease_held_recovers_its_board_before_the_next_grant() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let before = open(directory.path(), &fake);
    before.wait_until(ready);
    let mut holder = client(&before);
    holder.acquire(request("ann"), &mut |_| {}).unwrap();

    // The service stops with the lease held; a new one opens its state.
    fake.recovery.close();
    let after = open(directory.path(), &fake);
    let waiter = after.clone();
    let next = std::thread::spawn(move || {
        let mut next = client(&waiter);
        next.acquire(request("bob"), &mut |_| {}).map(|_| next)
    });
    after.wait_until(|status| !status.waiting.is_empty());
    assert!(after.status().holders.is_empty());
    assert!(matches!(
        readiness(&after),
        oer_lab_arbiter::readiness::Readiness::Recovering { .. }
    ));
    fake.recovery.open();
    let _next = next.join().unwrap().unwrap();
    let history = std::fs::read_to_string(directory.path().join("history.jsonl")).unwrap();
    assert!(history.contains("\"abandoned\""), "{history}");
}

#[test]
fn a_slow_reader_loses_output_visibly_and_never_holds_the_operation() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let service = open(directory.path(), &fake);
    service.wait_until(ready);
    let mut slow = client(&service);
    slow.acquire(request("ann"), &mut |_| {}).unwrap();
    let mut flood = slow.start(command("flood")).unwrap();
    // The operation ends before the client read a byte of its output.
    service.wait_until(|status| status.holders.iter().any(|h| h.operations == 1));
    service.wait_until(|status| status.holders.iter().all(|h| h.operations == 0));

    let (mut received, mut lost, mut finished) = (0usize, 0u64, None);
    while finished.is_none() {
        match flood.event() {
            Event::Output(bytes) => received += bytes.len(),
            Event::Lost(bytes) => lost = bytes,
            Event::Finished(result) => finished = Some(result),
        }
    }
    assert_eq!(finished, Some(Ok(serde_json::json!("flooded"))));
    assert!(lost > 0);
    assert_eq!(received as u64 + lost, FLOOD as u64);
}

#[test]
fn a_command_inside_the_lease_joins_it_and_its_loss_stops_only_its_work() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let service = open(directory.path(), &fake);
    service.wait_until(ready);
    let mut holder = client(&service);
    let granted = holder.acquire(request("ann"), &mut |_| {}).unwrap();

    let mut outsider = client(&service);
    assert!(outsider.join("not-a-token", vec![Claim::board(A)]).is_err());
    assert!(
        outsider
            .join(&granted.token, vec![Claim::board("02:00:00:00:00:0b")])
            .is_err(),
        "a claim the lease does not hold"
    );

    let mut inside = client(&service);
    assert_eq!(
        inside.join(&granted.token, vec![Claim::board(A)]).unwrap(),
        granted
    );
    let mut output = Vec::new();
    let echoed = inside
        .run(
            command("echo"),
            &mut output,
            Some(Box::new(&b"through the service"[..])),
        )
        .unwrap();
    assert_eq!(echoed, serde_json::json!("echoed"));
    assert_eq!(output, b"through the service");

    fake.hardware.close();
    inside
        .send(&Request::Start {
            id: 9,
            operation: command("hold"),
        })
        .unwrap();
    service.wait_until(|status| status.holders.iter().any(|h| h.operations == 1));
    drop(inside);
    fake.hardware.open();
    service.wait_until(|status| status.holders.iter().all(|h| h.operations == 0));
    let status = service.status();
    assert_eq!(status.holders.len(), 1);
    assert!(
        !status.holders[0].ending,
        "the enclosing lease is still held"
    );
    holder.release().unwrap();
}

#[test]
fn a_released_lease_waits_for_its_operations() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let service = open(directory.path(), &fake);
    service.wait_until(ready);
    fake.hardware.close();
    let mut holder = client(&service);
    holder.acquire(request("ann"), &mut |_| {}).unwrap();
    holder
        .send(&Request::Start {
            id: 1,
            operation: command("hold"),
        })
        .unwrap();
    service.wait_until(|status| status.holders.iter().any(|h| h.operations == 1));
    holder.send(&Request::Release).unwrap();
    service.wait_until(|status| status.holders.iter().all(|h| h.ending));
    assert_eq!(
        service.status().holders.len(),
        1,
        "the lease waits for its operation"
    );
    assert!(matches!(
        readiness(&service),
        oer_lab_arbiter::readiness::Readiness::InUse { .. }
    ));
    fake.hardware.open();
    holder.ended().unwrap();
}

fn history(directory: &std::path::Path) -> String {
    std::fs::read_to_string(directory.join("history.jsonl")).unwrap_or_default()
}

#[test]
fn a_lease_at_the_hard_limit_is_revoked_and_its_work_stopped() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let now = Arc::new(AtomicU64::new(START_MS));
    let service = open_at(directory.path(), &fake, now.clone());
    service.wait_until(ready);
    let mut holder = client(&service);
    holder.acquire(request("ann"), &mut |_| {}).unwrap();
    holder
        .send(&Request::Start {
            id: 1,
            operation: command("wait"),
        })
        .unwrap();
    service.wait_until(|status| status.holders.iter().any(|h| h.operations == 1));
    service.tick();
    assert!(!service.status().holders[0].ending, "not before the limit");
    now.fetch_add(HARD_LIMIT.as_millis() as u64, Ordering::SeqCst);
    service.tick();
    holder.ended().unwrap();
    assert!(holder.revoked().unwrap().contains("hard limit"));
    assert!(history(directory.path()).contains("\"hard-limit\""));
}

#[test]
fn waiting_maintenance_preempts_the_holders_it_conflicts_with() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let service = open(directory.path(), &fake);
    service.wait_until(ready);
    let mut holder = client(&service);
    holder.acquire(request("ann"), &mut |_| {}).unwrap();
    let waiter = service.clone();
    let maintenance = std::thread::spawn(move || {
        let mut maintenance = client(&waiter);
        let lease = LeaseRequest {
            maintenance: Some("install fixture software".to_owned()),
            claims: Vec::new(),
            ..request("bob")
        };
        maintenance.acquire(lease, &mut |_| {}).map(|_| maintenance)
    });
    holder.ended().unwrap();
    assert!(
        holder
            .revoked()
            .unwrap()
            .contains("install fixture software")
    );
    let _maintenance = maintenance.join().unwrap().unwrap();
    assert!(history(directory.path()).contains("\"preempted-on-request\""));
}

#[test]
fn a_person_preempts_a_lease_and_its_client_hears_why() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let service = open(directory.path(), &fake);
    service.wait_until(ready);
    let mut holder = client(&service);
    let granted = holder.acquire(request("ann"), &mut |_| {}).unwrap();
    let mut person = client(&service);
    assert!(person.preempt(granted.lease, "bob", " ").is_err());
    assert!(person.preempt(granted.lease + 100, "bob", "stuck").is_err());
    person.preempt(granted.lease, "bob", "stuck run").unwrap();
    holder.ended().unwrap();
    assert_eq!(
        holder.revoked().as_deref(),
        Some("preempted by bob: stuck run")
    );
}

#[test]
fn divisible_work_is_asked_to_yield_to_a_waiter_with_a_higher_balance() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let now = Arc::new(AtomicU64::new(START_MS));
    let service = open_at(directory.path(), &fake, now.clone());
    service.wait_until(ready);
    let mut holder = client(&service);
    let divisible = LeaseRequest {
        divisible: true,
        ..request("ann")
    };
    holder.acquire(divisible, &mut |_| {}).unwrap();
    let waiter = service.clone();
    let next = std::thread::spawn(move || {
        let mut next = client(&waiter);
        next.acquire(request("bob"), &mut |_| {}).map(|_| next)
    });
    service.wait_until(|status| !status.waiting.is_empty());
    now.fetch_add(MIN_SLICE.as_millis() as u64, Ordering::SeqCst);
    service.tick();
    // The notice arrives with the next reply the holder reads.
    holder.status().unwrap();
    assert!(holder.yield_requested().unwrap().contains("bob"));
    holder.release().unwrap();
    let _next = next.join().unwrap().unwrap();
}

#[test]
fn a_recovery_past_its_deadline_leaves_the_board_unavailable_and_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    fake.recovery.close();
    let now = Arc::new(AtomicU64::new(START_MS));
    let service = Service::open_with(
        oer_lab_arbiter::Arbiter::at(directory.path()).unwrap(),
        vec![board(A)],
        fake.clone(),
        Box::new(move || now.load(Ordering::SeqCst)),
        std::time::Duration::from_millis(1),
    )
    .unwrap();
    service.wait_until(|status| {
        status.resources.iter().any(|(_, readiness)| {
            matches!(
                readiness,
                oer_lab_arbiter::readiness::Readiness::Unavailable { reason, .. }
                    if reason.contains("did not finish")
            )
        })
    });
    let mut person = client(&service);
    let refused = person.return_resource(&board(A)).unwrap_err();
    assert!(refused.contains("still runs"), "{refused}");
    fake.recovery.open();
    service.wait_until(|status| status.recovering.is_empty());
    // The driver returned: a person may return the board.
    person.return_resource(&board(A)).unwrap();
}

#[test]
fn a_grant_the_service_cannot_record_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let service = open(directory.path(), &fake);
    service.wait_until(ready);
    let mode = |mode| {
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(mode)).unwrap()
    };
    mode(0o500);
    let refused = client(&service).acquire(request("ann"), &mut |_| {});
    mode(0o700);
    assert_eq!(
        refused.unwrap_err(),
        "the LAB service cannot record its state"
    );
    assert!(service.status().holders.is_empty());
    service.wait_until(ready);
    client(&service)
        .acquire(request("ann"), &mut |_| {})
        .unwrap();
}

#[test]
fn an_operation_is_a_duplex_stream_for_its_client() {
    use std::io::{Read as _, Write as _};
    let directory = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let service = open(directory.path(), &fake);
    service.wait_until(ready);
    let mut holder = client(&service);
    holder.acquire(request("ann"), &mut |_| {}).unwrap();
    let mut echo = holder.start(command("echo")).unwrap();
    echo.write_all(b"console line\n").unwrap();
    echo.input().close().unwrap();
    let mut read = String::new();
    echo.read_to_string(&mut read).unwrap();
    assert_eq!(read, "console line\n");
}
