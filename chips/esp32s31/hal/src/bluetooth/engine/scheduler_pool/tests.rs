use bytemuck::Zeroable;
mod foreign;
use crate::layouts::*;
use std::boxed::Box;

use super::{
    SchedulerAllocationConfig, SchedulerItemSpace, SchedulerPoolError, SchedulerPoolPlaceError,
    SchedulerRoleKind, SchedulerRolePool, SchedulerRolePoolStorage, SchedulerRoleStorage, sealed,
};
use crate::bluetooth::engine::{
    scheduler_item::SchedulerItemCompletionStatus, sram_link::SchedulerItemLink,
};

/// Two items and a marker word that reinitialization writes.
#[repr(C)]
#[derive(bytemuck::Zeroable)]
struct TestStorage<const DTM: bool = false> {
    items: [BleSchedulerItem; 2],
    marker: u32,
}

#[derive(Clone, Copy)]
struct TestBinding {
    items: [SchedulerItemLink; 2],
    number: u16,
}

impl<const DTM: bool> sealed::Sealed for TestStorage<DTM> {}

impl<const DTM: bool> SchedulerRoleStorage for TestStorage<DTM> {
    const KIND: SchedulerRoleKind = if DTM {
        SchedulerRoleKind::DirectTestMode
    } else {
        SchedulerRoleKind::LegacyScanning
    };
    const ITEMS: usize = 2;
    const NUMBERS: usize = 1;
    const NEW: Self = Self {
        items: [const { BleSchedulerItem::zeroed() }; 2],
        marker: 0,
    };
    type Binding = TestBinding;
    type State = u8;
    const INITIAL_STATE: u8 = 0;
    fn addresses<D: crate::bluetooth::engine::ControllerDmaSpace>(
        &self,
        placement: &crate::bluetooth::engine::Placement<D>,
        first_number: u16,
    ) -> TestBinding {
        TestBinding {
            items: self
                .items
                .each_ref()
                .map(|item| SchedulerItemLink::new(placement.reference(item))),
            number: first_number,
        }
    }
    fn admits(state: &u8, item: usize) -> bool {
        *state & (1 << item) != 0
    }

    fn item_layout(&self, item: usize) -> &BleSchedulerItem {
        &self.items[item]
    }

    fn item_link(binding: &TestBinding, item: usize) -> SchedulerItemLink {
        binding.items[item]
    }

    fn reinitialize(&mut self, binding: &TestBinding) -> u8 {
        self.marker = 0x100 + u32::from(binding.number);
        for item in &self.items {
            crate::bluetooth::engine::shared_layout::clear_scheduler_item(item);
        }
        0
    }
}

type Pool = SchedulerRolePool<'static, 'static, TestStorage, 2>;

/// Prepare an event for both items, as a role does.
fn prepare(pool: &mut Pool, instance: &super::SchedulerRoleInstance<'static, 'static>) {
    *pool.cpu(instance).expect("the instance is quiescent").state = 0b11;
}

fn config() -> SchedulerAllocationConfig {
    SchedulerAllocationConfig::new(2, 3, 1).expect("test limits fit")
}

fn pool() -> Pool {
    let storage = Box::leak(Box::new(
        SchedulerRolePoolStorage::<TestStorage, 2>::zeroed(),
    ));
    Pool::place::<crate::bluetooth::engine::HostDma<0x2f00_1000>>(
        crate::memory::dma_static::Placed::host(storage),
        config().connections(),
    )
    .expect("test pool binds")
}

#[test]
fn an_empty_pool_cannot_hide_a_populated_pools_submitted_item() {
    let mut populated = pool();
    let instance = populated.acquire().expect("test pool has room");
    prepare(&mut populated, &instance);
    let id = populated.submit(&instance, 0).expect("prepared item");
    let empty = SchedulerRolePool::<'static, 'static, TestStorage, 0>::place::<
        crate::bluetooth::engine::HostDma<0x2f00_2000>,
    >(
        crate::memory::dma_static::Placed::host(Box::leak(Box::new(SchedulerRolePoolStorage::<
            TestStorage,
            0,
        >::zeroed()))),
        config().connections(),
    )
    .expect("empty pool fits");
    let initial = SchedulerItemSpace::new().with(&populated);
    let link = initial.link(id);
    assert_eq!(initial.listed_item(link), Some(id));
    let with_empty = initial.with(&empty);
    assert_eq!(with_empty.listed_item(link), Some(id));
    assert_eq!(with_empty.link(id), link);
    assert!(
        SchedulerItemSpace::new()
            .with(&empty)
            .listed_item(link)
            .is_none()
    );
}

#[test]
fn instances_are_bound_at_their_offsets_and_reinitialized() {
    let mut pool = pool();
    let first = pool.acquire().unwrap();
    let second = pool.acquire().unwrap();
    assert!(pool.acquire().is_none());
    let stride = core::mem::size_of::<TestStorage>() as u32;
    let space = SchedulerItemSpace::new().with(&pool);
    assert_eq!(space.link(first.item(1)).controller_address(), 0x2f00_1060);
    assert_eq!(
        space.link(second.item(0)).controller_address(),
        0x2f00_1000 + stride
    ); // Connections are numbered after the advertising instances and one
    // reserved number.
    let (graph, _, _) = pool.shared(&second).unwrap();
    assert_eq!(graph.marker, 0x100 + 4);
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

    super::super::with_epoch(|epoch| {
        let mut pool = pool
            .reset_after_controller_reset(
                &crate::bluetooth::BluetoothControllerReset::for_host_test(),
                epoch,
            )
            .ok()
            .expect("epoch space remains");

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
    });
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
    assert!(graph.items[0].hardware_next().read().skip());
    assert!(graph.items[0].control().read().deleted());
    assert_eq!(graph.items[1].previous().read().address(), 0);
    graph.items[1].status().write(BleSchedulerItemStatus::ZERO);
    assert_eq!(
        space.completion_status(second),
        Some(SchedulerItemCompletionStatus::Zero)
    );
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
    let storage = Box::leak(Box::new(
        SchedulerRolePoolStorage::<TestStorage, 2>::zeroed(),
    ));
    let one_connection = SchedulerAllocationConfig::new(2, 1, 0)
        .unwrap()
        .connections();
    assert!(matches!(
        Pool::place::<crate::bluetooth::engine::HostDma<0x2f00_1000>>(
            crate::memory::dma_static::Placed::host(storage),
            one_connection
        ),
        Err(SchedulerPoolPlaceError::AllocationNumbers)
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

#[test]
fn the_last_populated_pool_of_a_kind_resolves_without_falling_back() {
    let mut first = pool();
    let storage = Box::leak(Box::new(
        SchedulerRolePoolStorage::<TestStorage, 2>::zeroed(),
    ));
    let mut last = Pool::place::<crate::bluetooth::engine::HostDma<0x2f00_3000>>(
        crate::memory::dma_static::Placed::host(storage),
        config().connections(),
    )
    .expect("second pool fits");
    let a = first.acquire().expect("first slot");
    let b = last.acquire().expect("last slot");
    prepare(&mut first, &a);
    prepare(&mut last, &b);
    let a = first.submit(&a, 0).expect("prepared first");
    let b = last.submit(&b, 0).expect("prepared last");
    let a_link = SchedulerItemSpace::new().with(&first).link(a);
    let b_link = SchedulerItemSpace::new().with(&last).link(b);
    let space = SchedulerItemSpace::new().with(&first).with(&last);
    assert_eq!(space.listed_item(b_link), Some(b));
    assert_eq!(space.link(b), b_link);
    assert_eq!(space.listed_item(a_link), None);
    let reversed = SchedulerItemSpace::new().with(&last).with(&first);
    assert_eq!(reversed.listed_item(a_link), Some(a));
    assert_eq!(reversed.listed_item(b_link), None);
}

#[test]
fn lookup_follows_role_order_independent_of_composition_order() {
    let mut scanner = pool();
    let instance = scanner.acquire().expect("scanner slot");
    prepare(&mut scanner, &instance);
    let scan = scanner.submit(&instance, 0).expect("scanner prepared");
    // Two independent host models intentionally give their storage the same
    // controller link, to make the tie-breaking order observable.
    let mut dtm = SchedulerRolePool::<'static, 'static, TestStorage<true>, 1>::place::<
        crate::bluetooth::engine::HostDma<0x2f00_1000>,
    >(
        crate::memory::dma_static::Placed::host(Box::leak(Box::new(SchedulerRolePoolStorage::<
            TestStorage<true>,
            1,
        >::zeroed()))),
        config().direct_test_mode(),
    )
    .expect("DTM pool fits");
    let instance = dtm.acquire().expect("DTM slot");
    *dtm.cpu(&instance).expect("quiescent").state = 1;
    let test = dtm.submit(&instance, 0).expect("DTM prepared");
    let forward = SchedulerItemSpace::new().with(&dtm).with(&scanner);
    let reverse = SchedulerItemSpace::new().with(&scanner).with(&dtm);
    let link = forward.link(test);
    assert_eq!(forward.link(scan), link);
    assert_eq!(forward.listed_item(link), Some(test));
    assert_eq!(reverse.listed_item(link), Some(test));
}
