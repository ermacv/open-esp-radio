use super::*;
use crate::layouts::WifiRxDmaDescriptorControl;
use crate::radio::ieee80211::dma::rx_dma::RxDescriptorAddresses;
use crate::radio::ieee80211::dma::{
    rx_ring::{RxArenaWords, RxDescriptorTable},
    rx_storage::RxDmaStorage,
    space::{HOST_ARENA_BASE, HostDma},
};

extern crate std;

#[test]
fn the_model_follows_the_pac_walker_and_logs_in_order() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(
        RxDmaStorage::<2, 16, 16, HostDma>::new(),
    ));
    let placed =
        RxDmaStorage::place(crate::system::memory::dma_static::Placed::host(storage)).unwrap();
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let reserved = base + 2 * DESCRIPTOR_BYTES;
    let mut walker = RxWalkerModel::new();
    let live = placed
        .prepare_ring(&mut walker)
        .and_then(|ring| ring.core_ready_for_test().publish(&mut walker))
        .unwrap()
        .try_start(&mut walker)
        .map_err(|(_, error)| error)
        .unwrap();
    assert_eq!(walker.walker_enabled, WalkerEnable::Enabled);
    // BASE names the reserved descriptor, and the started walker fetches it.
    assert_eq!(walker.descriptor_base, reserved);
    // The diagnostic read reports the written base and logs nothing.
    let logged = walker.operations().len();
    assert_eq!(walker.descriptor_base_register(), Some(reserved));
    assert_eq!(walker.operations().len(), logged);
    assert_eq!(walker.next_descriptor_low, reserved & CURSOR_LOW_MASK);
    assert_eq!(
        walker.operation_count(RxWalkerOperation::WriteDescriptorBase(reserved)),
        1
    );
    assert_eq!(storage.reserved_descriptor().next_address(), base);

    walker.clear_operations();
    walker.set_cursor(base, 1, None);
    assert_eq!(
        walker.with_ordered_cursor(|cursor| (
            cursor.last_descriptor_low(),
            cursor.next_descriptor_low()
        )),
        ((base + DESCRIPTOR_BYTES) & CURSOR_LOW_MASK, 0)
    );
    assert_eq!(
        walker.operations(),
        [
            RxWalkerOperation::ObserveLast,
            RxWalkerOperation::Fence,
            RxWalkerOperation::ObserveNext,
            RxWalkerOperation::Fence,
        ]
    );

    walker.reload_pending_observations = 2;
    assert!(walker.reload_pending());
    assert!(walker.try_with_reload_settled(|_| ()).is_none());
    assert!(walker.try_with_reload_settled(|_| ()).is_some());

    assert!(live.try_stop(&mut walker).is_ok());
    assert_eq!(walker.walker_enabled, WalkerEnable::Disabled);
}

const BASE: u32 = HOST_ARENA_BASE;

/// A 16-byte descriptor the walker completed with `length` bytes as a
/// unit's terminal.
fn completed(length: u32) -> WifiRxDmaDescriptorControl {
    let armed = crate::radio::ieee80211::dma::descriptor::rx_armed_word(16).unwrap();
    rx_completed_word(armed, length, true).unwrap()
}

/// A bound four-descriptor table at [`BASE`] whose descriptor 3 links to
/// descriptor 1 and whose reserved descriptor links to descriptor 2.
fn linked_binding() -> RxDmaBinding<'static> {
    let table = std::boxed::Box::leak(std::boxed::Box::new(RxDescriptorTable::<4>::new()));
    let arena = std::boxed::Box::leak(std::boxed::Box::new(RxArenaWords::new()));
    table.ring()[3].inject_corrupt_words(
        WifiRxDmaDescriptorControl::ZERO,
        0,
        BASE + DESCRIPTOR_BYTES,
    );
    table.reserved().inject_corrupt_words(
        WifiRxDmaDescriptorControl::ZERO,
        0,
        BASE + 2 * DESCRIPTOR_BYTES,
    );
    RxDmaBinding::new(
        table,
        RxDescriptorAddresses::for_host_model(BASE, 5).unwrap(),
        arena,
    )
    .unwrap()
}

fn exhausted_at(last: usize) -> RxWalkerModel {
    let mut walker = RxWalkerModel::new();
    walker.walker_enabled = WalkerEnable::Enabled;
    walker.set_cursor(BASE, last, None);
    walker
}

#[test]
fn a_doorbell_on_an_exhausted_walker_resumes_it_at_last_next_when_it_clears() {
    let binding = linked_binding();
    let mut walker = exhausted_at(3);
    walker.reload_latency = 2;
    walker.request_reload(&binding);
    assert!(walker.reload_pending());
    assert_eq!(
        walker.next_descriptor_low, 0,
        "not before the doorbell clears"
    );
    assert!(walker.reload_pending());
    assert!(!walker.reload_pending());
    assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 1));

    // LAST naming the reserved descriptor resumes at its link.
    let mut walker = exhausted_at(4);
    walker.request_reload(&binding);
    assert!(walker.try_with_reload_settled(|_| ()).is_some());
    assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 2));
}

#[test]
fn a_doorbell_resumes_nothing_with_next_nonzero_a_stopped_walker_or_a_zero_link() {
    let binding = linked_binding();
    let mut walker = exhausted_at(3);
    walker.set_cursor(BASE, 3, Some(0));
    walker.request_reload(&binding);
    assert!(!walker.reload_pending());
    assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 0));

    let mut walker = exhausted_at(3);
    walker.walker_enabled = WalkerEnable::Disabled;
    walker.request_reload(&binding);
    assert!(!walker.reload_pending());
    assert_eq!(walker.next_descriptor_low, 0);

    // Descriptor 0's link is zero: there is nothing to resume at.
    let mut walker = exhausted_at(0);
    walker.request_reload(&binding);
    assert!(!walker.reload_pending());
    assert_eq!(walker.next_descriptor_low, 0);
}

#[test]
fn lost_doorbells_leave_next_zero() {
    let binding = linked_binding();
    let mut walker = exhausted_at(3);
    walker.lose_reloads = 1;
    walker.request_reload(&binding);
    assert!(!walker.reload_pending());
    assert_eq!(walker.next_descriptor_low, 0, "the first doorbell is lost");
    walker.request_reload(&binding);
    assert!(!walker.reload_pending());
    assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 1));

    let mut walker = exhausted_at(3);
    walker.lose_reloads = u32::MAX;
    for _ in 0..3 {
        walker.request_reload(&binding);
        assert!(!walker.reload_pending());
    }
    assert_eq!(walker.next_descriptor_low, 0, "every doorbell is lost");
    assert_eq!(walker.lose_reloads, u32::MAX);
}

/// A bound four-descriptor table at [`BASE`] linked 0 → 1 → 2 → 3 → 0,
/// every descriptor armed with a 16-byte buffer, and its binding.
fn armed_table() -> (RxWalkerTable<'static>, RxDmaBinding<'static>) {
    let table = std::boxed::Box::leak(std::boxed::Box::new(RxDescriptorTable::<4>::new()));
    let arena = std::boxed::Box::leak(std::boxed::Box::new(RxArenaWords::new()));
    let armed = crate::radio::ieee80211::dma::descriptor::rx_armed_word(16).unwrap();
    for (index, descriptor) in table.ring().iter().enumerate() {
        let next = BASE + ((index as u32 + 1) % 4) * DESCRIPTOR_BYTES;
        descriptor.inject_corrupt_words(armed, 0x2f01_0000, next);
    }
    let walker_table = RxWalkerTable::new(BASE, table.ring(), table.reserved());
    (
        walker_table,
        RxDmaBinding::new(
            table,
            RxDescriptorAddresses::for_host_model(BASE, 5).unwrap(),
            arena,
        )
        .unwrap(),
    )
}

fn walking_at(next: usize) -> RxWalkerModel {
    let mut walker = RxWalkerModel::new();
    walker.walker_enabled = WalkerEnable::Enabled;
    walker.set_cursor(BASE, 3, Some(next));
    walker
}

#[test]
fn a_frame_lands_where_next_points_and_the_walker_follows_the_link() {
    let (table, _) = armed_table();
    let mut walker = walking_at(1);
    assert_eq!(
        walker.land(table, 5, false),
        RxWalkerLanding::Landed(descriptor_low(BASE, 1))
    );
    let first = table.ring[1].word0();
    assert_eq!((first.owner(), first.rx_done()), (true, false));
    assert_eq!(walker.last_descriptor_low, descriptor_low(BASE, 3));
    assert_eq!(
        walker.land(table, 7, true),
        RxWalkerLanding::Landed(descriptor_low(BASE, 2))
    );
    let terminal = table.ring[2].word0();
    assert_eq!(terminal, completed(7));
    assert_eq!(walker.last_descriptor_low, descriptor_low(BASE, 2));
    assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 3));

    let mut walker = walking_at(1);
    walker.next_descriptor_low = 0;
    assert_eq!(walker.land(table, 5, true), RxWalkerLanding::Exhausted);
}

#[test]
fn the_walker_stops_on_a_clear_owner_bit_without_writing_it() {
    let (table, _) = armed_table();
    let detached = crate::radio::ieee80211::dma::descriptor::rx_detached_word(completed(9));
    table.ring[1].write_word0(detached);
    let mut walker = walking_at(1);
    for _ in 0..3 {
        assert_eq!(
            walker.land(table, 5, true),
            RxWalkerLanding::StoppedOnOwner(descriptor_low(BASE, 1))
        );
    }
    assert_ne!(walker.owner_stop, OwnerStop::Running);
    assert_eq!(table.ring[1].word0(), detached, "never written");
    assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 1));
    assert_eq!(walker.last_descriptor_low, descriptor_low(BASE, 3));
}

#[test]
fn a_doorbell_rung_while_detached_does_not_resume_the_walker_once_rearmed() {
    let (table, binding) = armed_table();
    let completed = completed(9);
    table.ring[1].write_word0(crate::radio::ieee80211::dma::descriptor::rx_detached_word(
        completed,
    ));
    let mut walker = walking_at(1);
    let stopped = RxWalkerLanding::StoppedOnOwner(descriptor_low(BASE, 1));
    assert_eq!(walker.land(table, 5, true), stopped);

    // The doorbell rings and clears while the descriptor is detached.
    walker.reload_latency = 1;
    walker.request_reload(&binding);
    assert!(walker.reload_pending());
    assert!(!walker.reload_pending());

    // Rearmed after that doorbell: the walker waits for the next one.
    table.ring[1]
        .write_word0(crate::radio::ieee80211::dma::descriptor::rx_rearm_word(completed).unwrap());
    assert_eq!(walker.land(table, 5, true), stopped);
    assert_eq!(walker.owner_stop, OwnerStop::Stopped);
}

#[test]
fn a_rearmed_descriptor_and_one_doorbell_resume_the_stopped_walker_in_place() {
    let (table, binding) = armed_table();
    let completed = completed(9);
    table.ring[1].write_word0(crate::radio::ieee80211::dma::descriptor::rx_detached_word(
        completed,
    ));
    let mut walker = walking_at(1);
    let stopped = RxWalkerLanding::StoppedOnOwner(descriptor_low(BASE, 1));
    assert_eq!(walker.land(table, 5, true), stopped);

    // A doorbell while the descriptor is still detached resumes nothing.
    walker.request_reload(&binding);
    assert!(!walker.reload_pending());
    assert_eq!(walker.land(table, 5, true), stopped);

    // Rearmed but no doorbell: still stopped.
    table.ring[1]
        .write_word0(crate::radio::ieee80211::dma::descriptor::rx_rearm_word(completed).unwrap());
    assert_eq!(walker.land(table, 5, true), stopped);

    // One doorbell resumes it in place, once it clears.
    walker.reload_latency = 1;
    walker.request_reload(&binding);
    assert!(walker.reload_pending());
    assert_eq!(walker.land(table, 5, true), stopped);
    assert!(!walker.reload_pending());
    assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 1));
    assert_eq!(
        walker.land(table, 5, true),
        RxWalkerLanding::Landed(descriptor_low(BASE, 1))
    );
    assert_eq!(walker.owner_stop, OwnerStop::Running);
}

#[test]
fn a_doorbell_follows_last_next_whatever_last_s_owner_bit() {
    let (table, binding) = armed_table();
    // LAST, descriptor 3, is detached: its owner bit is clear.
    table.ring[3].write_word0(crate::radio::ieee80211::dma::descriptor::rx_detached_word(
        completed(9),
    ));
    let mut walker = exhausted_at(3);
    walker.request_reload(&binding);
    assert!(!walker.reload_pending());
    assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 0));
    assert_eq!(
        walker.land(table, 5, true),
        RxWalkerLanding::Landed(descriptor_low(BASE, 0))
    );
}

/// A test's event log the walker journals into, beside its own events.
struct Events(core::cell::RefCell<std::vec::Vec<&'static str>>);

impl RxWalkerJournal for Events {
    fn record(&self, operation: RxWalkerOperation) {
        self.0.borrow_mut().push(match operation {
            RxWalkerOperation::Stop => "walker stop",
            RxWalkerOperation::RequestReload => "doorbell",
            _ => "walker",
        });
    }
}

#[test]
fn a_shared_walker_journals_its_operations_among_the_test_s_events() {
    let binding = linked_binding();
    let events: &'static Events = std::boxed::Box::leak(std::boxed::Box::new(Events(
        core::cell::RefCell::new(std::vec::Vec::new()),
    )));
    let model = core::cell::RefCell::new(exhausted_at(3));
    model.borrow_mut().set_journal(events);
    let mut walker = SharedRxWalker(&model);

    events.0.borrow_mut().push("mac stop");
    walker.request_reload(&binding);
    // The test plays the hardware between two operations.
    model.borrow_mut().set_cursor(BASE, 3, Some(0));
    assert_eq!(walker.next_descriptor_low(), descriptor_low(BASE, 0));
    assert!(walker.try_with_walker_stopped(&binding, |_| ()).is_some());
    events.0.borrow_mut().push("mac resume");

    assert_eq!(
        *events.0.borrow(),
        [
            "mac stop",
            "doorbell",
            "walker",
            "walker stop",
            "mac resume"
        ]
    );
    // The model's own log holds the same operations.
    assert_eq!(
        model.borrow().operations(),
        [
            RxWalkerOperation::RequestReload,
            RxWalkerOperation::ObserveNext,
            RxWalkerOperation::Stop,
        ]
    );
}
