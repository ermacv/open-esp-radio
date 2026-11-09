//! The capture's life over a fake router: each start, ending and teardown
//! path, the order of its steps and the results each keeps.

use super::*;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

type Log = Rc<RefCell<Vec<String>>>;

#[derive(Default)]
struct Script {
    existing: Vec<String>,
    on_radio: Vec<String>,
    create_fails: bool,
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

    fn create(&mut self, phy: &str, name: &str, channel: u8) -> Result<()> {
        self.log
            .borrow_mut()
            .push(format!("create {phy} {name} {channel}"));
        // A partial create leaves the interface behind.
        self.created = true;
        failing(self.script.borrow().create_fails, "create")
    }

    fn start(&mut self, name: &str) -> Result<FakeProcess> {
        self.log.borrow_mut().push(format!("start {name}"));
        failing(self.script.borrow().start_fails, "start")?;
        let stream = std::mem::take(&mut self.script.borrow_mut().stream).into();
        Ok(FakeProcess {
            script: self.script.clone(),
            stream,
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
    fn ready(&mut self, _: Duration) -> Result<()> {
        self.log.borrow_mut().push("ready".into());
        failing(self.script.borrow().ready_fails, "ready")
    }

    fn next(&mut self, _: Duration) -> Next {
        match self.stream.pop_front() {
            Some(next) => next,
            None if self.script.borrow().idle_forever => Next::Idle,
            None => Next::Ended,
        }
    }

    fn stop(&mut self) -> Result<()> {
        self.log.borrow_mut().push("stop".into());
        failing(self.script.borrow().stop_fails, "stop")
    }

    fn wait_exit(&mut self, _: Duration) -> Result<String> {
        self.log.borrow_mut().push("wait".into());
        failing(self.script.borrow().exit_fails, "wait")?;
        Ok("exited (0)".into())
    }

    fn diagnostics(&mut self) -> String {
        "listening on oercap0, link-type IEEE802_11_RADIO".into()
    }
}

/// A reader that cancels after `cancel_after` writes, or fails write
/// number `fail_at`.
struct FakeSink {
    bytes: Vec<u8>,
    writes: usize,
    cancel_after: Option<usize>,
    fail_at: Option<usize>,
}

impl Sink for FakeSink {
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
    let report = capture(&mut host, 13, &mut sink).unwrap();
    assert_eq!(sink.bytes, b"pcap-headerrecord");
    assert_eq!(report.bytes, 17);
    assert_eq!(report.interface, INTERFACE);
    assert!(report.diagnostics.contains("listening"));
    let mut expected = vec![
        "listing".to_owned(),
        "create phy0 oercap0 13".to_owned(),
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
    assert_eq!(capture(&mut host, 13, &mut sink).unwrap().bytes, 0);
    assert!(log_of(&log).ends_with(&TEARDOWN.map(str::to_owned)));
}

#[test]
fn an_existing_interface_of_its_name_is_never_taken_over() {
    let (mut host, log) = fake(Script {
        existing: vec![INTERFACE.to_owned()],
        ..Script::default()
    });
    assert!(capture(&mut host, 13, &mut sink()).is_err());
    assert_eq!(log_of(&log), ["listing"]);
}

#[test]
fn an_interface_already_on_the_radio_refuses_the_start() {
    let (mut host, log) = fake(Script {
        on_radio: vec!["mon0".to_owned()],
        ..Script::default()
    });
    let error = capture(&mut host, 13, &mut sink()).unwrap_err();
    assert!(error.contains("mon0"), "{error}");
    assert_eq!(log_of(&log), ["listing"]);
}

#[test]
fn a_partial_create_removes_the_interface_it_left() {
    let (mut host, log) = fake(Script {
        create_fails: true,
        ..Script::default()
    });
    let error = capture(&mut host, 13, &mut sink()).unwrap_err();
    assert!(
        error.contains("create failed") && error.contains("confirmed"),
        "{error}"
    );
    assert_eq!(
        log_of(&log),
        [
            "listing",
            "create phy0 oercap0 13",
            "remove oercap0",
            "listing"
        ]
    );
}

#[test]
fn a_process_that_does_not_start_or_listen_is_stopped_and_its_interface_removed() {
    let (mut host, log) = fake(Script {
        start_fails: true,
        ..Script::default()
    });
    assert!(
        capture(&mut host, 13, &mut sink())
            .unwrap_err()
            .contains("start failed")
    );
    assert!(log_of(&log).ends_with(&["remove oercap0".to_owned(), "listing".to_owned()]));
    let (mut host, log) = fake(Script {
        ready_fails: true,
        ..Script::default()
    });
    assert!(
        capture(&mut host, 13, &mut sink())
            .unwrap_err()
            .contains("ready failed")
    );
    assert!(log_of(&log).ends_with(&TEARDOWN.map(str::to_owned)));
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
    let error = capture(&mut host, 13, &mut sink).unwrap_err();
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
    let error = capture(&mut host, 13, &mut sink()).unwrap_err();
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
    let error = capture(&mut host, 13, &mut sink).unwrap_err();
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
    assert!(capture(&mut host, 13, &mut sink).is_ok());
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
