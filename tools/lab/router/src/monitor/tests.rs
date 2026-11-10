//! The capture's life over a fake router: each start, ending and teardown
//! path, the order of its steps and the results each keeps. The fake
//! process never waits; its clock never needs to move.

use super::*;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

type Log = Rc<RefCell<Vec<String>>>;

#[derive(Default)]
struct Script {
    existing: Vec<String>,
    on_radio: Vec<String>,
    create_fails: bool,
    isolate_fails: bool,
    up_fails: bool,
    start_fails: bool,
    ready_fails: bool,
    stop_fails: bool,
    exit_fails: bool,
    remove_fails: bool,
    /// The interface stays listed after its removal.
    stays: bool,
    /// The stream's looks; `Ended` once they run out unless `idle_forever`.
    stream: Vec<Next>,
    idle_forever: bool,
}

struct FakeHost {
    script: Rc<RefCell<Script>>,
    created: bool,
    log: Log,
}

struct FakeProcess {
    script: Rc<RefCell<Script>>,
    stream: VecDeque<Next>,
    stopped: bool,
    log: Log,
}

fn failing(fail: bool, what: &str) -> Result<()> {
    if fail {
        Err(format!("{what} failed"))
    } else {
        Ok(())
    }
}

impl Host for FakeHost {
    type Process = FakeProcess;

    fn listing(&mut self) -> Result<Listing> {
        self.log.borrow_mut().push("listing".into());
        let script = self.script.borrow();
        let mut all = script.existing.clone();
        all.extend(script.on_radio.iter().cloned());
        if self.created {
            all.push(INTERFACE.to_owned());
        }
        Ok(Listing {
            phy: "phy0".into(),
            on_radio: script.on_radio.clone(),
            all,
        })
    }

    fn create(&mut self, phy: &str, name: &str) -> Result<()> {
        self.log.borrow_mut().push(format!("create {phy} {name}"));
        // A partial create leaves the interface behind.
        self.created = true;
        failing(self.script.borrow().create_fails, "create")
    }

    fn isolate(&mut self, name: &str) -> Result<()> {
        self.log.borrow_mut().push(format!("ipv6 off {name}"));
        failing(self.script.borrow().isolate_fails, "ipv6 off")
    }

    fn up(&mut self, name: &str, channel: u8) -> Result<()> {
        self.log.borrow_mut().push(format!("up {name} {channel}"));
        failing(self.script.borrow().up_fails, "up")
    }

    fn start(&mut self, name: &str) -> Result<FakeProcess> {
        self.log.borrow_mut().push(format!("start {name}"));
        failing(self.script.borrow().start_fails, "start")?;
        let stream = std::mem::take(&mut self.script.borrow_mut().stream).into();
        Ok(FakeProcess {
            script: self.script.clone(),
            stream,
            stopped: false,
            log: self.log.clone(),
        })
    }

    fn remove(&mut self, name: &str) -> Result<()> {
        self.log.borrow_mut().push(format!("remove {name}"));
        let script = self.script.borrow();
        if !script.stays {
            self.created = false;
        }
        failing(script.remove_fails, "remove")
    }
}

impl Process for FakeProcess {
    fn ready<C: Clock>(&mut self, _: &C, _: C::Instant) -> Result<()> {
        self.log.borrow_mut().push("ready".into());
        failing(self.script.borrow().ready_fails, "ready")
    }

    fn next<C: Clock>(&mut self, _: &C, _: C::Instant) -> Next {
        match self.stream.pop_front() {
            Some(next) => next,
            // Idle while it runs; a stopped process's stream ends.
            None if self.script.borrow().idle_forever && !self.stopped => Next::Idle,
            None => Next::Ended,
        }
    }

    fn stop(&mut self) -> Result<()> {
        self.log.borrow_mut().push("stop".into());
        self.stopped = true;
        failing(self.script.borrow().stop_fails, "stop")
    }

    fn wait_exit<C: Clock>(&mut self, _: &C, _: C::Instant) -> Result<String> {
        self.log.borrow_mut().push("wait".into());
        failing(self.script.borrow().exit_fails, "wait")?;
        Ok("exited (0)".into())
    }

    fn diagnostics(&mut self) -> Diagnostics {
        Diagnostics {
            text: "listening on oercap0, link-type IEEE802_11_RADIO".into(),
            error: None,
        }
    }
}

/// A reader that cancels after `cancel_after` writes, or fails write
/// number `fail_at`.
struct FakeSink {
    bytes: Vec<u8>,
    writes: usize,
    /// The writes made before each readiness.
    readied: Vec<usize>,
    cancel_after: Option<usize>,
    fail_at: Option<usize>,
}

impl Sink for FakeSink {
    fn ready(&mut self) {
        self.readied.push(self.writes);
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writes += 1;
        if self.fail_at == Some(self.writes) {
            return Err("the client is gone".into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn cancelled(&self) -> bool {
        self.cancel_after.is_some_and(|after| self.writes >= after)
    }
}

fn sink() -> FakeSink {
    FakeSink {
        bytes: Vec::new(),
        writes: 0,
        readied: Vec::new(),
        cancel_after: None,
        fail_at: None,
    }
}

fn fake(script: Script) -> (FakeHost, Log) {
    let log = Log::default();
    (
        FakeHost {
            script: Rc::new(RefCell::new(script)),
            created: false,
            log: log.clone(),
        },
        log,
    )
}

fn chunks() -> Vec<Next> {
    vec![
        Next::Bytes(b"pcap-header".to_vec()),
        Next::Idle,
        Next::Bytes(b"record".to_vec()),
    ]
}

const TEARDOWN: [&str; 4] = ["stop", "wait", "remove oercap0", "listing"];

fn log_of(log: &Log) -> Vec<String> {
    log.borrow().clone()
}

#[test]
fn a_cancelled_capture_streams_only_pcap_and_tears_down_in_order() {
    let (mut host, log) = fake(Script {
        stream: chunks(),
        idle_forever: true,
        ..Script::default()
    });
    let mut sink = FakeSink {
        cancel_after: Some(2),
        ..sink()
    };
    let report = capture(&mut host, 13, &mut sink, &clock::ManualClock::new()).unwrap();
    assert_eq!(sink.bytes, b"pcap-headerrecord");
    assert_eq!(report.bytes, 17);
    // Ready once, after the readiness and before the first byte.
    assert_eq!(sink.readied, [0]);
    assert_eq!(report.interface, INTERFACE);
    assert!(report.diagnostics.contains("listening"));
    let mut expected = vec![
        "listing".to_owned(),
        "create phy0 oercap0".to_owned(),
        "ipv6 off oercap0".to_owned(),
        "up oercap0 13".to_owned(),
        "start oercap0".to_owned(),
        "ready".to_owned(),
    ];
    expected.extend(TEARDOWN.iter().map(|step| (*step).to_owned()));
    assert_eq!(log_of(&log), expected);
}

#[test]
fn a_client_disconnect_before_any_byte_is_a_cancellation_with_the_same_teardown() {
    let (mut host, log) = fake(Script {
        idle_forever: true,
        ..Script::default()
    });
    let mut sink = FakeSink {
        cancel_after: Some(0),
        ..sink()
    };
    assert_eq!(
        capture(&mut host, 13, &mut sink, &clock::ManualClock::new())
            .unwrap()
            .bytes,
        0
    );
    assert!(log_of(&log).ends_with(&TEARDOWN.map(str::to_owned)));
}

#[test]
fn an_existing_interface_of_its_name_is_never_taken_over() {
    let (mut host, log) = fake(Script {
        existing: vec![INTERFACE.to_owned()],
        ..Script::default()
    });
    assert!(capture(&mut host, 13, &mut sink(), &clock::ManualClock::new()).is_err());
    assert_eq!(log_of(&log), ["listing"]);
}

#[test]
fn an_interface_already_on_the_radio_refuses_the_start() {
    let (mut host, log) = fake(Script {
        on_radio: vec!["mon0".to_owned()],
        ..Script::default()
    });
    let error = capture(&mut host, 13, &mut sink(), &clock::ManualClock::new()).unwrap_err();
    assert!(error.contains("mon0"), "{error}");
    assert_eq!(log_of(&log), ["listing"]);
}

#[test]
fn a_refused_setup_step_removes_the_interface_and_starts_nothing() {
    // Each step before the process exists: the steps up to the refused
    // one, in order, then the interface's removal and its check; never a
    // process, never a readiness.
    let steps = ["create phy0 oercap0", "ipv6 off oercap0", "up oercap0 13"];
    for (refused, failure) in ["create failed", "ipv6 off failed", "up failed"]
        .into_iter()
        .enumerate()
    {
        let (mut host, log) = fake(Script {
            create_fails: refused == 0,
            isolate_fails: refused == 1,
            up_fails: refused == 2,
            ..Script::default()
        });
        let mut unready = sink();
        let error = capture(&mut host, 13, &mut unready, &clock::ManualClock::new()).unwrap_err();
        assert!(
            error.contains(failure) && error.contains("interface removal: confirmed"),
            "{error}"
        );
        let mut expected = vec!["listing".to_owned()];
        expected.extend(steps[..=refused].iter().map(|step| (*step).to_owned()));
        expected.extend(["remove oercap0".to_owned(), "listing".to_owned()]);
        assert_eq!(log_of(&log), expected);
        assert!(unready.readied.is_empty());
    }
}

#[test]
fn the_setup_turns_ipv6_off_before_the_monitor_comes_up_then_starts_tcpdump() {
    let (mut host, log) = fake(Script {
        idle_forever: true,
        ..Script::default()
    });
    let mut sink = FakeSink {
        cancel_after: Some(0),
        ..sink()
    };
    capture(&mut host, 13, &mut sink, &clock::ManualClock::new()).unwrap();
    assert_eq!(
        log_of(&log)[..6],
        [
            "listing",
            "create phy0 oercap0",
            "ipv6 off oercap0",
            "up oercap0 13",
            "start oercap0",
            "ready"
        ]
    );
}

#[test]
fn the_router_scripts_are_exactly_these() {
    assert_eq!(
        create_script("phy1", INTERFACE).unwrap(),
        "set -e\niw phy phy1 interface add oercap0 type monitor\n"
    );
    // The interface's own sysctl, read back; nothing global.
    assert_eq!(
        isolate_script(INTERFACE).unwrap(),
        "set -e\necho 1 > /proc/sys/net/ipv6/conf/oercap0/disable_ipv6\n\
         [ \"$(cat /proc/sys/net/ipv6/conf/oercap0/disable_ipv6)\" = 1 ]\n"
    );
    assert_eq!(
        up_script(INTERFACE, 13).unwrap(),
        "set -e\nip link set oercap0 up\niw dev oercap0 set channel 13\n"
    );
    assert_eq!(
        capture_script(INTERFACE).unwrap(),
        "echo \"oer-capture-pid $$\" >&2\n\
         exec tcpdump -i oercap0 -Q in -y IEEE802_11_RADIO -s 0 -U -w -\n"
    );
    // A name the shell would split is quoted, never spliced.
    assert!(
        isolate_script("a b")
            .unwrap()
            .contains("'/proc/sys/net/ipv6/conf/a b/disable_ipv6'")
    );
}

#[test]
fn a_tcpdump_that_cannot_set_its_direction_never_announces_readiness() {
    // tcpdump ends with its error before its listening line when libpcap
    // refuses the direction (tcpdump.c: "%s: pcap_setdirection() failed:
    // %s"); the reader ends, so the readiness channel closes unsent.
    const STDERR: &str = "oer-capture-pid 14719\n\
                          tcpdump: oercap0: pcap_setdirection() failed: \
                          Setting direction is not supported on this device\n";
    let pid = std::sync::Arc::new(std::sync::Mutex::new(None));
    let (announce, ready) = std::sync::mpsc::sync_channel(1);
    let seen = pid.clone();
    let reader = StderrReader::spawn(STDERR.as_bytes(), move |line| {
        ssh::observe(line, &seen, &announce);
    });
    let diagnostics = reader.join();
    assert_eq!(
        ready.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    );
    assert_eq!(diagnostics.text, STDERR);
}

#[test]
fn a_process_that_does_not_start_or_listen_is_stopped_and_its_interface_removed() {
    let (mut host, log) = fake(Script {
        start_fails: true,
        ..Script::default()
    });
    assert!(
        capture(&mut host, 13, &mut sink(), &clock::ManualClock::new())
            .unwrap_err()
            .contains("start failed")
    );
    assert!(log_of(&log).ends_with(&["remove oercap0".to_owned(), "listing".to_owned()]));
    let (mut host, log) = fake(Script {
        ready_fails: true,
        ..Script::default()
    });
    let mut unready = sink();
    assert!(
        capture(&mut host, 13, &mut unready, &clock::ManualClock::new())
            .unwrap_err()
            .contains("ready failed")
    );
    assert!(log_of(&log).ends_with(&TEARDOWN.map(str::to_owned)));
    // A capture that never listened is never reported ready.
    assert!(unready.readied.is_empty());
}

#[test]
fn a_write_error_ends_the_capture_through_the_teardown() {
    let (mut host, log) = fake(Script {
        stream: chunks(),
        idle_forever: true,
        ..Script::default()
    });
    let mut sink = FakeSink {
        fail_at: Some(2),
        ..sink()
    };
    let error = capture(&mut host, 13, &mut sink, &clock::ManualClock::new()).unwrap_err();
    assert!(error.contains("the client is gone"), "{error}");
    assert_eq!(sink.bytes, b"pcap-header");
    assert!(log_of(&log).ends_with(&TEARDOWN.map(str::to_owned)));
}

#[test]
fn a_process_that_ends_by_itself_is_an_error_and_still_torn_down() {
    let (mut host, log) = fake(Script {
        stream: chunks(),
        ..Script::default()
    });
    let error = capture(&mut host, 13, &mut sink(), &clock::ManualClock::new()).unwrap_err();
    assert!(error.contains("ended by itself"), "{error}");
    assert!(log_of(&log).ends_with(&TEARDOWN.map(str::to_owned)));
}

#[test]
fn failed_teardown_steps_do_not_skip_the_next_and_every_cause_is_kept() {
    let (mut host, log) = fake(Script {
        idle_forever: true,
        stop_fails: true,
        exit_fails: true,
        remove_fails: true,
        stays: true,
        ..Script::default()
    });
    let mut sink = FakeSink {
        cancel_after: Some(0),
        ..sink()
    };
    let error = capture(&mut host, 13, &mut sink, &clock::ManualClock::new()).unwrap_err();
    for cause in [
        "stop failed",
        "wait failed",
        "remove failed",
        "still on the router",
    ] {
        assert!(error.contains(cause), "{cause}: {error}");
    }
    assert!(log_of(&log).ends_with(&TEARDOWN.map(str::to_owned)));
}

#[test]
fn a_removal_the_router_confirms_counts_even_if_its_command_complained() {
    let (mut host, _) = fake(Script {
        idle_forever: true,
        remove_fails: true,
        ..Script::default()
    });
    let mut sink = FakeSink {
        cancel_after: Some(0),
        ..sink()
    };
    assert!(capture(&mut host, 13, &mut sink, &clock::ManualClock::new()).is_ok());
}

#[test]
fn a_listing_names_the_radio_and_every_interface() {
    let listing = parse_listing(
        "phy0\n0\nphy#1\n\tInterface phy1-ap0\n\t\ttype AP\nphy#0\n\tInterface mon0\n\t\ttype monitor\n",
    )
    .unwrap();
    assert_eq!(listing.phy, "phy0");
    assert_eq!(listing.on_radio, ["mon0"]);
    assert_eq!(listing.all, ["phy1-ap0", "mon0"]);
    assert_eq!(
        parse_listing("phy0\n0\n").unwrap().all,
        Vec::<String>::new()
    );
    assert!(parse_listing("").is_err());
    assert!(parse_listing("phy0\nx\n").is_err());
    assert!(parse_listing("phy0\n0\nInterface a\n").is_err());
}

mod clock;
mod real;

#[test]
fn tcpdumps_recorded_listening_line_is_its_readiness() {
    // tcpdump's stderr on the lab router, recorded in lease #1316.
    const RECORDED: &str = "tcpdump: listening on oercap0, link-type IEEE802_11_RADIO \
                            (802.11 plus radiotap header), snapshot length 262144 bytes";
    assert!(announces_listening(RECORDED));
    assert!(announces_listening(
        "listening on oercap0, link-type IEEE802_11_RADIO"
    ));
    for other in [
        "tcpdump: data link type IEEE802_11_RADIO",
        "oer-capture-pid 14183",
        "1 packet captured",
        "tcpdump: oercap0: No such device exists",
    ] {
        assert!(!announces_listening(other), "{other}");
    }
}

#[test]
fn tcpdumps_recorded_stderr_names_its_process_and_its_readiness_unchanged() {
    // The capture's whole stderr in lease #1316, as the service logged it.
    const STDERR: &str = "oer-capture-pid 14183\n\
                          tcpdump: data link type IEEE802_11_RADIO\n\
                          tcpdump: listening on oercap0, link-type IEEE802_11_RADIO (802.11 plus \
                          radiotap header), snapshot length 262144 bytes\n\
                          1 packet captured\n\
                          1 packet received by filter\n\
                          0 packets dropped by kernel\n";
    let pid = std::sync::Arc::new(std::sync::Mutex::new(None));
    let (announce, ready) = std::sync::mpsc::sync_channel(1);
    let seen = pid.clone();
    let reader = StderrReader::spawn(STDERR.as_bytes(), move |line| {
        ssh::observe(line, &seen, &announce);
    });
    let diagnostics = reader.join();
    assert_eq!(ready.try_recv(), Ok(()));
    assert_eq!(*pid.lock().unwrap(), Some(14183));
    // The diagnostics are the stderr as read, unchanged.
    assert_eq!(
        diagnostics,
        Diagnostics {
            text: STDERR.to_owned(),
            error: None
        }
    );
}
