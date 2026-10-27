//! Each injected fault owns its own entry, allocation and counter.
use oer_interrupt_table::{
    BindError, BindFault, BindFaultCode, BindFaultKind, Controller, Entry, Error, LineState, Router,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

oer_interrupt_table::__fake_parts!();
oer_interrupt_table::__fake_controller!();

pub struct Owned<const SOURCE: u16>(Arc<AtomicUsize>);
impl<const SOURCE: u16> Drop for Owned<SOURCE> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
impl<const SOURCE: u16> oer_interrupt_table::InterruptSource for Owned<SOURCE> {
    const SOURCE: u16 = SOURCE;
}
pub struct Pair {
    _owner: Owned<3>,
}
impl oer_interrupt_table::InterruptSources for Pair {
    const SOURCES: &'static [u16] = &[3, 7];
}
fn on_owned<const SOURCE: u16>(_: &mut Owned<SOURCE>) {}
fn on_pair(_: &mut Pair) {}
oer_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [];
    contexts { hardware: 1; }
    storage: StorageToken = Owned<0> => on_owned::<0>, line 20, context hardware, hart 0, Hardware;
    handler: HandlerToken = Owned<1> => on_owned::<1>, line 21, context hardware, hart 0, Hardware;
    line: LineToken = Owned<2> => on_owned::<2>, line 22, context hardware, hart 0, Hardware;
    route: RouteToken = Pair => on_pair, line 23, context hardware, hart 0, Hardware;
}

#[derive(Clone, Copy)]
enum Refuse {
    Line,
    Route,
}
struct Injected {
    inner: Arc<Mutex<FakeController>>,
    refusal: Option<Refuse>,
}
impl Controller for Injected {
    const SOURCES: u16 = FakeController::SOURCES;
    const LINES: u8 = FakeController::LINES;
    const LEVEL_BITS: u8 = FakeController::LEVEL_BITS;
    fn hart(&self) -> u8 {
        self.inner.lock().unwrap().hart()
    }
    fn verify_configuration(&self) -> Result<(), Error> {
        self.inner.lock().unwrap().verify_configuration()
    }
    fn route(&mut self, source: u16, line: u8) -> Result<(), Error> {
        if matches!(self.refusal, Some(Refuse::Route)) && source == 7 {
            return Err(Error::WrongHart {
                source,
                hart: 0,
                current: u8::MAX,
            });
        }
        self.inner.lock().unwrap().route(source, line)
    }
    fn silence(&mut self, source: u16) -> Result<(), Error> {
        self.inner.lock().unwrap().silence(source)
    }
    fn routed(&self, source: u16) -> Option<u8> {
        self.inner.lock().unwrap().routed(source)
    }
    fn set_line(&mut self, line: u8, state: LineState) -> Result<(), Error> {
        if matches!(self.refusal, Some(Refuse::Line)) && line == 22 && state.enabled {
            return Err(Error::NoSuchLine { line });
        }
        self.inner.lock().unwrap().set_line(line, state)
    }
    fn line(&self, line: u8) -> LineState {
        self.inner.lock().unwrap().line(line)
    }
}
fn router(refusal: Option<Refuse>) -> (Router<Injected>, Arc<Mutex<FakeController>>) {
    let inner = Arc::new(Mutex::new(FakeController::new(0)));
    let router = Router::install(
        Injected {
            inner: Arc::clone(&inner),
            refusal,
        },
        INTERRUPT_TABLE,
    )
    .unwrap();
    inner.lock().unwrap().writes.clear();
    (router, inner)
}
fn drops() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}
fn fault<E: Entry>(result: Result<oer_interrupt_table::Bound<E>, BindError<E>>) -> BindError<E> {
    let error = result.unwrap_err();
    assert!(matches!(error, BindError::Fault(_)), "{error:?}");
    error
}

#[test]
fn each_failed_binding_retains_owners_and_stops_before_later_writes() {
    let tokens = Interrupts::take().unwrap();
    let (mut router, observed) = router(None);
    assert!(observed.lock().unwrap().writes.is_empty());

    let _claimed = StorageToken::storage().try_uninit().unwrap();
    let storage_drops = drops();
    let error = fault(router.bind(tokens.storage, Owned(Arc::clone(&storage_drops))));
    assert_eq!(
        error.diagnostic(),
        (BindFaultCode::StorageClaimed, [20 << 16, 0, 0, 0])
    );
    drop(error);
    assert_eq!(storage_drops.load(Ordering::Relaxed), 0);
    assert!(observed.lock().unwrap().writes.is_empty());

    let previous_drops = drops();
    let previous = Box::leak(Box::new(Owned(Arc::clone(&previous_drops))));
    let _installed = HandlerToken::cell().install(previous).ok().unwrap();
    let handler_drops = drops();
    let error = fault(router.bind(tokens.handler, Owned(Arc::clone(&handler_drops))));
    assert_eq!(
        error.diagnostic(),
        (BindFaultCode::HandlerOccupied, [(21 << 16) | 1, 0, 0, 0])
    );
    drop(error);
    assert_eq!(handler_drops.load(Ordering::Relaxed), 0);
    assert_eq!(previous_drops.load(Ordering::Relaxed), 0);
    assert!(HandlerToken::cell().with(|_| ()).is_some());
    assert_eq!(observed.lock().unwrap().routed(1), None);
    assert!(observed.lock().unwrap().writes.is_empty());

    let (mut router, observed) = router_with(Refuse::Line);
    let line_drops = drops();
    let error = fault(router.bind(tokens.line, Owned(Arc::clone(&line_drops))));
    assert_eq!(
        error.diagnostic(),
        (BindFaultCode::LineRefused, [(22 << 16) | 2, 9, 22, 0])
    );
    drop(error);
    assert_eq!(line_drops.load(Ordering::Relaxed), 0);
    assert_eq!(observed.lock().unwrap().routed(2), None);
    assert!(observed.lock().unwrap().writes.is_empty());

    let (mut router, observed) = router_with(Refuse::Route);
    let route_drops = drops();
    let error = fault(router.bind(
        tokens.route,
        Pair {
            _owner: Owned(Arc::clone(&route_drops)),
        },
    ));
    assert_eq!(
        error.diagnostic(),
        (
            BindFaultCode::RouteRefused,
            [(23 << 16) | 7, 4, 7, 255 << 8]
        )
    );
    drop(error);
    assert_eq!(route_drops.load(Ordering::Relaxed), 0);
    assert_eq!(
        observed.lock().unwrap().routed(3),
        Some(23),
        "the preceding route remains owned"
    );
    assert_eq!(
        observed.lock().unwrap().routed(7),
        None,
        "the failed route was not published"
    );
    assert_eq!(
        observed.lock().unwrap().writes.len(),
        2,
        "line and first route only"
    );
}

fn router_with(refusal: Refuse) -> (Router<Injected>, Arc<Mutex<FakeController>>) {
    // Installation writes line 22 too, so enable the failure only on a bound line.
    router(Some(refusal))
}

#[test]
fn diagnostic_keeps_both_line_states_and_full_entry_identity() {
    let state = LineState {
        enabled: true,
        vectored: false,
        trigger: oer_interrupt_table::Trigger::NegativeEdge,
        control: 0xfe,
    };
    let expected = LineState {
        enabled: false,
        vectored: true,
        trigger: oer_interrupt_table::Trigger::NegativeLevel,
        control: 0xfd,
    };
    let fault = BindFault {
        source: u16::MAX,
        line: u8::MAX,
        hart: u8::MAX,
        kind: BindFaultKind::LineRefused(Error::WrongLine {
            line: 254,
            state,
            expected,
        }),
    };
    assert_eq!(
        fault.diagnostic(),
        (BindFaultCode::LineRefused, [u32::MAX, 7, 254, 0xfd0a_fe0d])
    );
}
