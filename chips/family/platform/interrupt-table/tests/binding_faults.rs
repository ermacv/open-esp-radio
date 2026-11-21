#![allow(clippy::unwrap_used, reason = "test code: a test may panic")]
//! Each injected fault owns its own entry, allocation and counter. An
//! occupied handler cell, which only the crate reaches, is its own test
//! (`src/tests.rs`).
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
    assert_eq!((error.fault.line, error.fault.hart), (20, 0));
    assert_eq!(
        error.fault.diagnostic(),
        (BindFaultCode::StorageClaimed, [20, 0, 0, 0])
    );
    drop(error);
    assert_eq!(storage_drops.load(Ordering::Relaxed), 0);
    assert!(observed.lock().unwrap().writes.is_empty());

    // A level task's entry is named as any entry: by its line and hart.
    let _claimed = TaskToken::storage().try_uninit().unwrap();
    let task_drops = drops();
    let error = fault(router.bind(tokens.task, OwnedTask(Arc::clone(&task_drops))));
    assert_eq!((error.fault.line, error.fault.hart), (22, 0));
    assert_eq!(
        error.fault.diagnostic(),
        (BindFaultCode::StorageClaimed, [22, 0, 0, 0])
    );
    drop(error);
    assert_eq!(task_drops.load(Ordering::Relaxed), 0);
    assert!(observed.lock().unwrap().writes.is_empty());
}
