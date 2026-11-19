//! Each injected fault owns its own entry, allocation and counter.
use oer_chips_family_platform_interrupt_table::{
    BindFaultCode, Controller, Entry, Error, LineExpectation, LineObservation, Quarantined, Router,
};
use oer_portable_riscv::clic::{Control, LevelBits};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

oer_chips_family_platform_interrupt_table::__fake_parts!();
oer_chips_family_platform_interrupt_table::__fake_controller!();

pub struct Owned<const SOURCE: u16>(Arc<AtomicUsize>);
impl<const SOURCE: u16> Drop for Owned<SOURCE> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
impl<const SOURCE: u16> oer_chips_family_platform_interrupt_table::InterruptSource
    for Owned<SOURCE>
{
    const SOURCE: u16 = SOURCE;
}
fn on_owned<const SOURCE: u16>(_: &mut Owned<SOURCE>) {}
/// A level task's part: no source.
pub struct OwnedTask(Arc<AtomicUsize>);
impl Drop for OwnedTask {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
impl oer_chips_family_platform_interrupt_table::InterruptSources for OwnedTask {
    const SOURCES: &'static [u16] = &[];
}
fn on_task(_: &mut OwnedTask) {}
oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [];
    contexts { hardware: 1; keeper: 3; }
    storage: StorageToken = Owned<0> => on_owned::<0>, line 20, context hardware, hart 0, Hardware;
    handler: HandlerToken = Owned<1> => on_owned::<1>, line 21, context hardware, hart 0, Hardware;
    task: TaskToken = OwnedTask => on_task, line 22, context keeper, hart 0, LevelTask;
}

/// The fake controller, shared with the test that reads its writes.
struct Injected {
    inner: Arc<Mutex<FakeController>>,
}
impl Controller for Injected {
    type Source = u16;
    type Line = u8;
    const SOURCES: &'static [u16] = FakeController::SOURCES;
    const LINES: &'static [u8] = FakeController::LINES;
    const LEVEL_BITS: LevelBits = FakeController::LEVEL_BITS;
    fn hart(&self) -> u8 {
        self.inner.lock().unwrap().hart()
    }
    fn verify_configuration(&self) -> Result<(), Error> {
        self.inner.lock().unwrap().verify_configuration()
    }
    fn route(&mut self, source: u16, line: u8) {
        self.inner.lock().unwrap().route(source, line)
    }
    fn silence(&mut self, source: u16) {
        self.inner.lock().unwrap().silence(source)
    }
    fn routed(&self, source: u16) -> Option<u8> {
        self.inner.lock().unwrap().routed(source)
    }
    fn bind(&mut self, line: u8, control: Control) {
        self.inner.lock().unwrap().bind(line, control)
    }
    fn bind_task(&mut self, line: u8, control: Control) {
        self.inner.lock().unwrap().bind_task(line, control)
    }
    fn disable(&mut self, line: u8) {
        self.inner.lock().unwrap().disable(line)
    }
    fn check(&self, line: u8, expected: LineExpectation) -> Result<(), LineObservation> {
        self.inner.lock().unwrap().check(line, expected)
    }
}
fn router() -> (
    Router<Injected, 0, InterruptTable>,
    Arc<Mutex<FakeController>>,
) {
    let inner = Arc::new(Mutex::new(FakeController::new(0)));
    let router = Router::<_, 0, _>::install(
        Injected {
            inner: Arc::clone(&inner),
        },
        InterruptTable,
    )
    .unwrap();
    inner.lock().unwrap().writes.clear();
    (router, inner)
}
fn drops() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}
fn fault<E: Entry>(
    result: Result<oer_chips_family_platform_interrupt_table::Bound<E>, Quarantined<E>>,
) -> Quarantined<E> {
    result.unwrap_err()
}

#[test]
fn each_failed_binding_retains_owners_and_stops_before_later_writes() {
    let tokens = Interrupts::take().unwrap();
    let (mut router, observed) = router();
    assert!(observed.lock().unwrap().writes.is_empty());

    let _claimed = StorageToken::storage().try_uninit().unwrap();
    let storage_drops = drops();
    let error = fault(router.bind(tokens.storage, Owned(Arc::clone(&storage_drops))));
    assert_eq!(error.fault.source, Some(0), "the entry's first source");
    assert_eq!(
        error.fault.diagnostic(),
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
    assert_eq!(error.fault.source, Some(1), "the entry's first source");
    assert_eq!(
        error.fault.diagnostic(),
        (BindFaultCode::HandlerOccupied, [(21 << 16) | 1, 0, 0, 0])
    );
    drop(error);
    assert_eq!(handler_drops.load(Ordering::Relaxed), 0);
    assert_eq!(previous_drops.load(Ordering::Relaxed), 0);
    assert!(HandlerToken::cell().with(|_| ()).is_some());
    assert_eq!(observed.lock().unwrap().routed(1), None);
    assert!(observed.lock().unwrap().writes.is_empty());

    // A level task's entry has no source: its fault names none, and its
    // record's source field is the number no source has.
    let _claimed = TaskToken::storage().try_uninit().unwrap();
    let task_drops = drops();
    let error = fault(router.bind(tokens.task, OwnedTask(Arc::clone(&task_drops))));
    assert_eq!(error.fault.source, None);
    assert_eq!(
        error.fault.diagnostic(),
        (
            BindFaultCode::StorageClaimed,
            [
                u32::from(oer_chips_family_platform_interrupt_table::NO_SOURCE) | (22 << 16),
                0,
                0,
                0
            ]
        )
    );
    assert_eq!(oer_chips_family_platform_interrupt_table::NO_SOURCE, 0xffff);
    drop(error);
    assert_eq!(task_drops.load(Ordering::Relaxed), 0);
    assert!(observed.lock().unwrap().writes.is_empty());
}
