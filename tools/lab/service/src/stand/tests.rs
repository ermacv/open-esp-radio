use super::*;

#[test]
fn operations_on_one_board_take_turns_in_the_order_they_asked() {
    let owner = std::sync::Arc::new(BoardOwner::default());
    let never = AtomicBool::new(false);
    let first = owner.take(&never).unwrap();
    // Another board is not held up.
    drop(
        std::sync::Arc::new(BoardOwner::default())
            .take(&never)
            .unwrap(),
    );
    let order = std::sync::Arc::new(Mutex::new(Vec::new()));
    let waiters = (1..=2)
        .map(|waiter| {
            let (asking, order) = (owner.clone(), order.clone());
            let waiting = std::thread::spawn(move || {
                let _turn = asking.take(&AtomicBool::new(false)).unwrap();
                lock(&order).push(waiter);
            });
            // The second asks only once the first waits in the queue.
            while lock(&owner.queue).1.len() != waiter + 1 {
                std::thread::yield_now();
            }
            waiting
        })
        .collect::<Vec<_>>();
    assert!(lock(&order).is_empty());
    drop(first);
    for waiting in waiters {
        waiting.join().unwrap();
    }
    assert_eq!(*lock(&order), vec![1, 2]);
}

#[test]
fn a_stopped_operation_leaves_the_queue_without_its_turn() {
    let owner = std::sync::Arc::new(BoardOwner::default());
    let _first = owner.take(&AtomicBool::new(false)).unwrap();
    let refused = owner.take(&AtomicBool::new(true)).err().unwrap();
    assert!(refused.contains("stopped"), "{refused}");
    assert_eq!(lock(&owner.queue).1.len(), 1);
}

#[test]
fn only_a_router_operation_needs_the_air() {
    use oer_tools_lab_client::protocol::RouterOperation;
    let board = || "02:00:00:00:00:01".to_owned();
    let alone = [Claim::board(&board()), Claim::shared(ROUTER)];
    let router = Operation::Router {
        request: RouterOperation::VerifyRuntime,
    };
    assert!(air_refusal(&router, &alone).is_some());
    assert!(air_refusal(&router, &[Claim::shared(ROUTER), Claim::shared(AIR)]).is_none());
    // A board's radio works only on its image's command.
    for operation in [
        Operation::Flash {
            board: board(),
            bundle: "bundle".to_owned(),
            image: oer_tools_lab_arbiter::journal::ImageIdentity {
                name: "aes".to_owned(),
                sha256: String::new(),
                commit: None,
                dirty: None,
                origin: "test".to_owned(),
            },
            owner: "ann".to_owned(),
            via: FlashVia::Usb,
            skip_carried: false,
        },
        Operation::FlashConsole {
            board: board(),
            bundle: "bundle".to_owned(),
            image: oer_tools_lab_arbiter::journal::ImageIdentity {
                name: "aes".to_owned(),
                sha256: String::new(),
                commit: None,
                dirty: None,
                origin: "test".to_owned(),
            },
            owner: "ann".to_owned(),
        },
        Operation::Reset {
            board: board(),
            path: None,
        },
        Operation::Console {
            board: board(),
            reset: true,
        },
        Operation::PowerCycle { board: board() },
    ] {
        assert!(air_refusal(&operation, &alone).is_none(), "{operation:?}");
    }
}

/// A write into a console on a host seam: every stage logs itself and
/// fails when told; its console logs when it is released.
struct Steps {
    log: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
    fails: Option<&'static str>,
}

struct HeldConsole(std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>);

impl Drop for HeldConsole {
    fn drop(&mut self) {
        self.0.borrow_mut().push("console released");
    }
}

impl Steps {
    fn step(&self, stage: &'static str) -> Result<()> {
        self.log.borrow_mut().push(stage);
        if self.fails == Some(stage) {
            Err(format!("{stage} refused"))
        } else {
            Ok(())
        }
    }
}

impl IntoConsole for Steps {
    type Written = &'static str;
    type Console = HeldConsole;

    fn write(&mut self) -> Result<&'static str> {
        self.step("write").map(|()| "digest-of-the-bundle")
    }

    fn receipt(&self, written: &&'static str) -> serde_json::Value {
        serde_json::json!({ "digest": written })
    }

    fn start(&mut self) -> Result<HeldConsole> {
        self.step("start").map(|()| HeldConsole(self.log.clone()))
    }

    fn confirm(&mut self, _: &'static str) -> Result<()> {
        self.step("confirm")
    }

    fn journal(&mut self) -> Result<()> {
        self.step("journal")
    }

    fn stream(&mut self, console: HeldConsole) -> Result<()> {
        let streamed = self.step("stream");
        drop(console);
        streamed
    }
}

fn into_console_failing(
    fails: Option<&'static str>,
) -> (Result<serde_json::Value>, Vec<&'static str>) {
    let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let result = into_console(&mut Steps {
        log: log.clone(),
        fails,
    });
    let log = log.borrow().clone();
    (result, log)
}

#[test]
fn a_write_into_a_console_runs_its_stages_in_order_and_answers_its_receipt() {
    let (result, log) = into_console_failing(None);
    assert_eq!(
        result,
        Ok(serde_json::json!({ "written": true, "receipt": { "digest": "digest-of-the-bundle" } }))
    );
    assert_eq!(
        log,
        [
            "write",
            "start",
            "confirm",
            "journal",
            "stream",
            "console released"
        ]
    );
}

#[test]
fn every_failure_after_the_write_keeps_its_stage_and_the_receipt() {
    let (refused, log) = into_console_failing(Some("write"));
    let refused = refused.unwrap_err();
    assert!(
        refused.contains("the write failed") && !refused.contains("receipt"),
        "{refused}"
    );
    assert_eq!(log, ["write"]);
    for (stage, named, reached, steps) in [
        (
            "start",
            "the start failed",
            "written, not started",
            &["write", "start"][..],
        ),
        (
            "confirm",
            "the start's confirmation failed",
            "its start unconfirmed",
            &["write", "start", "confirm", "console released"][..],
        ),
        (
            "journal",
            "the journal failed",
            "written and started, not journaled",
            &["write", "start", "confirm", "journal", "console released"][..],
        ),
        (
            "stream",
            "the console failed",
            "written, started and journaled",
            &[
                "write",
                "start",
                "confirm",
                "journal",
                "stream",
                "console released",
            ][..],
        ),
    ] {
        let (result, log) = into_console_failing(Some(stage));
        let error = result.unwrap_err();
        assert!(
            error.contains(named)
                && error.contains(reached)
                && error.contains("digest-of-the-bundle"),
            "{stage}: {error}"
        );
        // Nothing after the failed stage runs; a console it opened is released.
        assert_eq!(log, steps, "{stage}");
    }
}
