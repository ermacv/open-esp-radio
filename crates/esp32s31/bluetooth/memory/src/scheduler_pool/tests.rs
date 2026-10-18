use std::boxed::Box;

use vcell::VolatileCell;

use super::{
    SchedulerAllocationConfig, SchedulerItemSpace, SchedulerPoolBindError, SchedulerPoolError,
    SchedulerPoolModelAddress, SchedulerRoleKind, SchedulerRolePool, SchedulerRolePoolStorage,
    SchedulerRoleStorage, sealed,
};
use crate::{scheduler_item::SchedulerItemCompletionStatus, sram_link::ControllerSramLinkAddress};

const ITEM_WORDS: usize = 0x60 / 4;

/// Two items and a marker word that reinitialization writes.
#[repr(C)]
struct TestStorage {
    items: [[VolatileCell<u32>; ITEM_WORDS]; 2],
    marker: VolatileCell<u32>,
}

#[derive(Clone, Copy)]
struct TestBinding {
    items: [ControllerSramLinkAddress; 2],
    number: u16,
}

impl sealed::Sealed for TestStorage {}

impl SchedulerRoleStorage for TestStorage {
    const KIND: SchedulerRoleKind = SchedulerRoleKind::LegacyScanning;
    const ITEMS: usize = 2;
    const NUMBERS: usize = 1;
    const NEW: Self = Self {
        items: [const { [const { VolatileCell::new(0) }; ITEM_WORDS] }; 2],
        marker: VolatileCell::new(0),
    };
    type Binding = TestBinding;
    type State = u8;
    const INITIAL_STATE: u8 = 0;
    fn bind(base: u32, first_number: u16) -> Result<TestBinding, SchedulerPoolBindError> {
        let link = |offset: u32| {
            ControllerSramLinkAddress::new(base + offset)
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)
        };
        Ok(TestBinding {
            items: [link(0)?, link(0x60)?],
            number: first_number,
        })
    }
    fn admits(state: &u8, item: usize) -> bool {
        *state & (1 << item) != 0
    }

    fn item_words(&self, item: usize) -> &[VolatileCell<u32>] {
        &self.items[item]
    }

    fn item_link(binding: &TestBinding, item: usize) -> ControllerSramLinkAddress {
        binding.items[item]
    }

    fn reinitialize(&mut self, binding: &TestBinding) -> u8 {
        self.marker.set(0x100 + u32::from(binding.number));
        for item in &self.items {
            for word in item {
                word.set(0);
            }
        }
        0
    }
}

type Pool = SchedulerRolePool<TestStorage, 2>;

/// Prepare an event for both items, as a role does.
fn prepare(pool: &mut Pool, instance: &super::SchedulerRoleInstance) {
    *pool.cpu(instance).expect("the instance is quiescent").state = 0b11;
}

fn config() -> SchedulerAllocationConfig {
    SchedulerAllocationConfig::new(2, 3, 1).expect("test limits fit")
}

fn pool() -> Pool {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<TestStorage, 2>::new()));
    Pool::bind_model(
        storage,
        SchedulerPoolModelAddress::new(0x2f00_1000).expect("test base is aligned"),
        config().connections(),
    )
    .expect("test pool binds")
}

#[test]
fn instances_are_bound_at_their_offsets_and_reinitialized() {
    let mut pool = pool();
    let first = pool.acquire().unwrap();
    let second = pool.acquire().unwrap();
    assert!(pool.acquire().is_none());
    let stride = core::mem::size_of::<TestStorage>() as u32;
    let space = SchedulerItemSpace::new().with(&pool);
    assert_eq!(
        space.link(first.item(1)).controller_address().address(),
        0x2f00_1060
    );
    assert_eq!(
        space.link(second.item(0)).controller_address().address(),
        0x2f00_1000 + stride
    ); // Connections are numbered after the advertising instances and one
    // reserved number.
    let (graph, _, _) = pool.shared(&second).unwrap();
    assert_eq!(graph.marker.get(), 0x100 + 4);
}

#[test]
fn listed_items_block_the_cpu_and_release() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    assert_eq!(
        pool.submit(&instance, 1),
        Err(SchedulerPoolError::NotPrepared)
    );
    prepare(&mut pool, &instance);
    let id = pool.submit(&instance, 1).unwrap();
    assert_eq!(id.item(), 1);
    assert_eq!(pool.submit(&instance, 1), Err(SchedulerPoolError::Listed));
    assert_eq!(
        pool.submit(&instance, 2),
        Err(SchedulerPoolError::NoSuchItem)
    );
    assert!(pool.is_listed(id));
    assert!(matches!(
        pool.cpu(&instance),
        Err(SchedulerPoolError::InstanceBusy)
    ));
    let failure = pool.release(instance).unwrap_err();
    assert_eq!(failure.error, SchedulerPoolError::InstanceBusy);
    let instance = failure.instance;

    pool.retire(id).unwrap();
    assert_eq!(pool.retire(id), Err(SchedulerPoolError::NotListed));
    assert!(pool.cpu(&instance).is_ok());
    pool.release(instance).unwrap();
    assert!(pool.acquire().is_some());
}

#[test]
fn a_withheld_item_keeps_its_instance_forever() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    prepare(&mut pool, &instance);
    let id = pool.submit(&instance, 0).unwrap();
    pool.withhold(id).unwrap();
    pool.retire(id).unwrap();
    assert_eq!(
        pool.submit(&instance, 1),
        Err(SchedulerPoolError::InstanceBusy)
    );
    assert!(pool.release(instance).is_err());
}

#[test]
fn a_controller_reset_frees_even_withheld_instances() {
    let mut pool = pool();
    let withheld = pool.acquire().unwrap();
    prepare(&mut pool, &withheld);
    let id = pool.submit(&withheld, 0).unwrap();
    pool.withhold(id).unwrap();
    let listed = pool.acquire().unwrap();
    prepare(&mut pool, &listed);
    let listed_id = pool.submit(&listed, 1).unwrap();

    pool.reset_after_controller_reset(
        &oer_esp32s31_hal::bluetooth::BluetoothControllerReset::for_validation(),
    );

    assert!(!pool.is_listed(id));
    assert!(!pool.is_listed(listed_id));
    let first = pool
        .acquire()
        .expect("the reset freed the withheld instance");
    let second = pool.acquire().expect("the reset freed the listed instance");
    assert_ne!(first.index(), second.index());
    assert!(
        pool.submit(&first, 0).is_err(),
        "no event survives the reset"
    );
}

#[test]
fn the_space_edits_only_scheduler_fields_of_listed_items() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    prepare(&mut pool, &instance);
    let first = pool.submit(&instance, 0).unwrap();
    let second = pool.submit(&instance, 1).unwrap();
    let space = SchedulerItemSpace::new().with(&pool);
    let second_link = space.link(second);

    space.prepare_for_list(first, None, Some(second_link));
    space.prepare_for_list(second, Some(space.link(first)), None);
    assert_eq!(space.completion_status(first), None);
    space.mark_deleted(first);
    space.set_next(first, None);
    space.set_previous(second, None);

    let (graph, _, _) = pool.shared(&instance).unwrap();
    assert_eq!(graph.items[0][0].get(), 1 << 25);
    assert_eq!(graph.items[0][0x4c / 4].get(), 1 << 25);
    assert_eq!(graph.items[1][0x50 / 4].get(), 0);
    graph.items[1][0x38 / 4].set(0);
    assert_eq!(
        space.completion_status(second),
        Some(SchedulerItemCompletionStatus::Zero)
    );
}

#[test]
fn storage_outside_controller_sram_is_refused() {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<TestStorage, 2>::new()));
    assert!(matches!(
        Pool::bind_model(
            storage,
            SchedulerPoolModelAddress::new(0x2f07_ff00).expect("test base is aligned"),
            config().connections(),
        ),
        Err(SchedulerPoolBindError::ExtentOutsidePhysicalSram)
    ));
}

#[test]
fn allocation_numbers_follow_the_vendor_order() {
    let config = config();
    assert_eq!(
        config.advertising(0, 2).map(|n| (n.first(), n.count())),
        Some((0, 2))
    );
    assert_eq!(config.advertising(1, 2), None);
    assert_eq!(
        (config.connections().first(), config.connections().count()),
        (3, 3)
    );
    assert_eq!(
        (config.scanning().first(), config.scanning().count()),
        (6, 3)
    );
    // Two advertising instances, one reserved number, three connections,
    // nine private numbers and one periodic synchronization.
    assert_eq!(config.direct_test_mode().first(), 2 + 1 + 3 + 9 + 1);
    assert_eq!(SchedulerAllocationConfig::new(0x0ff0, 0x10, 0), None);
}

#[test]
fn a_pool_larger_than_its_numbers_is_refused() {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<TestStorage, 2>::new()));
    let one_connection = SchedulerAllocationConfig::new(2, 1, 0)
        .unwrap()
        .connections();
    assert!(matches!(
        Pool::bind_model(
            storage,
            SchedulerPoolModelAddress::new(0x2f00_1000).expect("test base is aligned"),
            one_connection,
        ),
        Err(SchedulerPoolBindError::AllocationNumbers)
    ));
}

#[test]
fn only_submitted_items_resolve_from_their_link() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    prepare(&mut pool, &instance);
    let listed = pool.submit(&instance, 1).unwrap();
    let space = SchedulerItemSpace::new().with(&pool);
    assert_eq!(space.listed_item(space.link(listed)), Some(listed));
    assert_eq!(space.listed_item(space.link(instance.item(0))), None);
    pool.retire(listed).unwrap();
    let space = SchedulerItemSpace::new().with(&pool);
    assert_eq!(space.listed_item(space.link(listed)), None);
}
