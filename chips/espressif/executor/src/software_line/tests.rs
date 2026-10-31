use core::cell::Cell;

use super::{SoftwareLine, serve};

/// A level line whose handler runs while it is raised.
struct Line {
    raised: Cell<bool>,
}

impl SoftwareLine for Line {
    fn raise(&self) {
        self.raised.set(true);
    }
    fn lower(&self) {
        self.raised.set(false);
    }
}

#[test]
fn a_raise_during_the_drain_leads_to_another_round() {
    let line = Line {
        raised: Cell::new(false),
    };
    let pending = Cell::new(1_u32);
    line.raise();
    let mut rounds = 0;
    // The hardware runs the handler while the line is raised.
    while line.raised.get() {
        rounds += 1;
        serve(&line, || {
            let work = pending.replace(0);
            if rounds == 1 {
                // A waker posts more work and raises while the first round
                // takes the work it found.
                pending.set(pending.get() + 1);
                line.raise();
            }
            work
        });
        assert!(rounds < 4, "the line settles");
    }
    assert_eq!(rounds, 2, "the second raise ran a second round");
    assert_eq!(pending.get(), 0, "no work was left behind");
}

#[test]
fn a_raise_after_the_drain_is_not_lowered_by_it() {
    let line = Line {
        raised: Cell::new(true),
    };
    serve(&line, || ());
    line.raise();
    assert!(line.raised.get());
}

/// A line that records its raises and lowers beside the fences around them.
struct Recorded<'a> {
    log: &'a core::cell::RefCell<std::vec::Vec<&'static str>>,
}

impl SoftwareLine for Recorded<'_> {
    fn raise(&self) {
        self.log.borrow_mut().push("raise");
    }
    fn lower(&self) {
        self.log.borrow_mut().push("lower");
    }
}

extern crate std;

#[test]
fn a_raiser_fences_its_published_facts_before_the_raise() {
    let log = core::cell::RefCell::new(std::vec::Vec::new());
    let line = Recorded { log: &log };
    log.borrow_mut().push("publish");
    super::raise_with(&line, || log.borrow_mut().push("fence w,o"));
    assert_eq!(*log.borrow(), ["publish", "fence w,o", "raise"]);
}

#[test]
fn a_handler_lowers_then_fences_before_it_takes_the_facts() {
    let log = core::cell::RefCell::new(std::vec::Vec::new());
    let line = Recorded { log: &log };
    super::serve_with(
        &line,
        || log.borrow_mut().push("fence o,rw"),
        || log.borrow_mut().push("take"),
    );
    assert_eq!(*log.borrow(), ["lower", "fence o,rw", "take"]);
}
