use std::vec::Vec;

use super::*;

/// A dependency table shaped after the ESP32-S31 vendor device order: one
/// dependency that is not refcounted and five retained while Wi-Fi is
/// initialized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fixture {
    ModemAdcCommonFe,
    ModemPrivateFe,
    Pll160AndModemSource,
    Coexistence,
    AnalogI2cMaster,
    WifiApb,
    WifiBb44m,
    WifiMac,
    WifiBb,
    WifiBb80x1,
    Etm,
    BtMac,
    BtPeripheral,
    BtApbAndSecurity,
    BtIeee802154CommonBaseband,
    Ieee802154ApbAndMac,
}

impl ModemClockDependency for Fixture {
    const LOW_BIT_FIRST: &'static [Self] = &[
        Self::ModemAdcCommonFe,
        Self::ModemPrivateFe,
        Self::Pll160AndModemSource,
        Self::Coexistence,
        Self::AnalogI2cMaster,
        Self::WifiApb,
        Self::WifiBb44m,
        Self::WifiMac,
        Self::WifiBb,
        Self::WifiBb80x1,
        Self::Etm,
        Self::BtMac,
        Self::BtPeripheral,
        Self::BtApbAndSecurity,
        Self::BtIeee802154CommonBaseband,
        Self::Ieee802154ApbAndMac,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn refcounted(self) -> bool {
        !matches!(self, Self::AnalogI2cMaster)
    }

    fn retained_while_wifi_initialized(self) -> bool {
        matches!(
            self,
            Self::WifiApb | Self::WifiBb44m | Self::WifiMac | Self::WifiBb | Self::WifiBb80x1
        )
    }
}

type Planner<'identity> = ModemClockPlanner<'identity, Fixture>;

/// Module sets shaped after the ESP32-S31 ones.
#[derive(Clone, Copy)]
enum Module {
    AnalogI2cMaster,
    Phy,
    Wifi,
    Bluetooth,
    Ieee802154,
}

impl Module {
    fn dependencies(self) -> DependencySet<Fixture> {
        use Fixture::*;
        DependencySet::of(match self {
            Self::AnalogI2cMaster => &[AnalogI2cMaster],
            Self::Phy => &[
                ModemAdcCommonFe,
                ModemPrivateFe,
                Pll160AndModemSource,
                WifiBb80x1,
                AnalogI2cMaster,
            ],
            Self::Wifi => &[
                WifiMac,
                WifiApb,
                WifiBb,
                WifiBb44m,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
            ],
            Self::Bluetooth => &[
                BtMac,
                BtIeee802154CommonBaseband,
                Etm,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
                BtApbAndSecurity,
                BtPeripheral,
            ],
            Self::Ieee802154 => &[
                Ieee802154ApbAndMac,
                BtIeee802154CommonBaseband,
                Etm,
                Coexistence,
                WifiBb80x1,
                Pll160AndModemSource,
                BtApbAndSecurity,
            ],
        })
    }
}

/// The IEEE 802.15.4 module set, low bit first.
const IEEE802154_EDGES: [Fixture; 7] = [
    Fixture::Pll160AndModemSource,
    Fixture::Coexistence,
    Fixture::WifiBb80x1,
    Fixture::Etm,
    Fixture::BtApbAndSecurity,
    Fixture::BtIeee802154CommonBaseband,
    Fixture::Ieee802154ApbAndMac,
];

#[test]
fn the_fixture_table_is_consistent() {
    assert!(table_is_consistent::<Fixture>());
}

/// Counts after `count` IEEE 802.15.4 leases and nothing else.
fn ieee802154_counts(count: u16) -> [u16; DEPENDENCY_CAPACITY] {
    let mut counts = [0; DEPENDENCY_CAPACITY];
    for &dependency in Fixture::LOW_BIT_FIRST {
        if Module::Ieee802154.dependencies().contains(dependency) {
            counts[dependency.index()] = count;
        }
    }
    counts
}

/// Counts with exactly the listed dependencies set.
fn counts_of(counts: &[(Fixture, u16)]) -> [u16; DEPENDENCY_CAPACITY] {
    let mut all = [0; DEPENDENCY_CAPACITY];
    for (dependency, count) in counts {
        all[dependency.index()] = *count;
    }
    all
}

fn finish_acquire<'identity>(
    mut prepared: PreparedModemClockAcquire<'identity, Fixture>,
) -> (
    ModemClockPlanner<'identity, Fixture>,
    ModemClockLease<'identity, Fixture>,
    Vec<Fixture>,
) {
    let mut edges = Vec::new();
    loop {
        match prepared.advance() {
            ModemClockAcquireStep::Physical(pending) => {
                edges.push(pending.dependency);
                prepared = pending.complete();
            }
            ModemClockAcquireStep::CommitReady(ready) => {
                let (planner, lease) = ready.commit();
                return (planner, lease, edges);
            }
        }
    }
}

fn finish_release<'planner, 'lease>(
    mut prepared: PreparedModemClockRelease<'planner, 'lease, Fixture>,
) -> (ModemClockPlanner<'planner, Fixture>, Vec<Fixture>) {
    let mut edges = Vec::new();
    loop {
        match prepared.advance() {
            ModemClockReleaseStep::Physical(pending) => {
                edges.push(pending.dependency);
                prepared = pending.complete();
            }
            ModemClockReleaseStep::CommitReady(ready) => {
                return (ready.commit(), edges);
            }
        }
    }
}

fn duplicate_for_adversarial_test<'identity>(
    lease: &ModemClockLease<'identity, Fixture>,
) -> ModemClockLease<'identity, Fixture> {
    ModemClockLease {
        identity: lease.identity,
        slot: lease.slot,
        generation: lease.generation,
        dependencies: lease.dependencies,
    }
}

#[test]
fn exact_ieee_set_uses_low_bit_first_order_for_acquire_and_release() {
    let identity = ModemClockPlannerIdentity::new();
    let planner = Planner::managed(&identity);
    let prepared = planner
        .prepare_acquire(Module::Ieee802154.dependencies())
        .expect("known zero baseline");
    let (planner, lease, acquire_edges) = finish_acquire(prepared);
    assert_eq!(acquire_edges, IEEE802154_EDGES);
    assert_eq!(planner.counts, ieee802154_counts(1));

    let release = planner.prepare_release(lease).expect("valid exact lease");
    let (planner, release_edges) = finish_release(release);
    assert_eq!(release_edges, IEEE802154_EDGES);
    assert_eq!(planner.counts, ieee802154_counts(0));
}

#[test]
fn overlapping_leases_emit_only_zero_one_and_one_zero_boundaries() {
    let identity = ModemClockPlannerIdentity::new();
    let planner = Planner::managed(&identity);
    let (planner, first, first_edges) = finish_acquire(
        planner
            .prepare_acquire(Module::Ieee802154.dependencies())
            .expect("first acquisition"),
    );
    let (planner, second, second_edges) = finish_acquire(
        planner
            .prepare_acquire(Module::Ieee802154.dependencies())
            .expect("overlapping acquisition"),
    );
    assert_eq!(first_edges, IEEE802154_EDGES);
    assert!(second_edges.is_empty());
    assert_eq!(planner.counts, ieee802154_counts(2));

    let (planner, first_release_edges) = finish_release(
        planner
            .prepare_release(first)
            .expect("first overlapping release"),
    );
    assert!(first_release_edges.is_empty());
    assert_eq!(planner.counts, ieee802154_counts(1));

    let (planner, last_release_edges) = finish_release(
        planner
            .prepare_release(second)
            .expect("last overlapping release"),
    );
    assert_eq!(last_release_edges, IEEE802154_EDGES);
    assert_eq!(planner.counts, ieee802154_counts(0));
}

#[test]
fn local_lease_capacity_fails_closed_and_preserves_all_owners() {
    let identity = ModemClockPlannerIdentity::new();
    let mut planner = Planner::managed(&identity);
    let mut leases = Vec::new();

    for _ in 0..MAX_ACTIVE_LEASES {
        let prepared = planner
            .prepare_acquire(Module::Ieee802154.dependencies())
            .expect("one slot remains");
        let (next_planner, lease, _) = finish_acquire(prepared);
        planner = next_planner;
        leases.push(lease);
    }

    let failure = match planner.prepare_acquire(Module::Ieee802154.dependencies()) {
        Ok(_) => panic!("the local fixed-capacity table must reject a seventeenth lease"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        ModemClockAcquirePreparationError::LeaseCapacityReached
    );
    let mut planner = failure.into_planner();
    assert_eq!(planner.counts, ieee802154_counts(MAX_ACTIVE_LEASES as u16));
    assert_eq!(
        planner.slots.iter().filter(|slot| slot.active).count(),
        MAX_ACTIVE_LEASES
    );

    for lease in leases {
        let prepared = planner.prepare_release(lease).expect("exact live lease");
        let (next_planner, _) = finish_release(prepared);
        planner = next_planner;
    }
    assert_eq!(planner.counts, ieee802154_counts(0));
    assert!(planner.slots.iter().all(|slot| !slot.active));
}

#[test]
fn exhausted_slot_generation_fails_closed_without_count_changes() {
    let identity = ModemClockPlannerIdentity::new();
    let mut planner = Planner::managed(&identity);
    planner.slots[0].generation = u64::MAX;

    let failure = match planner.prepare_acquire(Module::Ieee802154.dependencies()) {
        Ok(_) => panic!("an exhausted slot generation must not be reused"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        ModemClockAcquirePreparationError::LeaseGenerationExhausted
    );
    let planner = failure.into_planner();
    assert_eq!(planner.counts, ieee802154_counts(0));
    assert_eq!(planner.slots[0].generation, u64::MAX);
    assert!(planner.slots.iter().all(|slot| !slot.active));
}

#[test]
fn partial_dependency_overlap_preserves_order_and_independent_counts() {
    let identity = ModemClockPlannerIdentity::new();
    let planner = Planner::managed(&identity);
    let first_set = DependencySet::of(&[
        Fixture::Pll160AndModemSource,
        Fixture::WifiBb80x1,
        Fixture::BtIeee802154CommonBaseband,
    ]);
    let second_set = DependencySet::of(&[
        Fixture::WifiBb80x1,
        Fixture::Etm,
        Fixture::BtIeee802154CommonBaseband,
    ]);

    let (planner, first, first_edges) =
        finish_acquire(planner.prepare_acquire(first_set).expect("first set"));
    assert_eq!(
        first_edges,
        [
            Fixture::Pll160AndModemSource,
            Fixture::WifiBb80x1,
            Fixture::BtIeee802154CommonBaseband,
        ]
    );

    let (planner, second, second_edges) =
        finish_acquire(planner.prepare_acquire(second_set).expect("second set"));
    assert_eq!(second_edges, [Fixture::Etm]);
    assert_eq!(
        planner.counts,
        counts_of(&[
            (Fixture::Pll160AndModemSource, 1),
            (Fixture::WifiBb80x1, 2),
            (Fixture::Etm, 1),
            (Fixture::BtIeee802154CommonBaseband, 2),
        ])
    );

    let (planner, first_release) =
        finish_release(planner.prepare_release(first).expect("first release"));
    assert_eq!(first_release, [Fixture::Pll160AndModemSource]);
    assert_eq!(
        planner.counts,
        counts_of(&[
            (Fixture::WifiBb80x1, 1),
            (Fixture::Etm, 1),
            (Fixture::BtIeee802154CommonBaseband, 1),
        ])
    );

    let (planner, second_release) =
        finish_release(planner.prepare_release(second).expect("second release"));
    assert_eq!(
        second_release,
        [
            Fixture::WifiBb80x1,
            Fixture::Etm,
            Fixture::BtIeee802154CommonBaseband,
        ]
    );
    assert_eq!(planner.counts, ieee802154_counts(0));
}

#[test]
fn preparation_and_commit_are_transactionally_separate() {
    let identity = ModemClockPlannerIdentity::new();
    let planner = Planner::managed(&identity);
    let prepared = planner
        .prepare_acquire(Module::Ieee802154.dependencies())
        .expect("managed baseline");
    assert_eq!(prepared.planner.counts, ieee802154_counts(0));
    assert!(prepared.planner.slots.iter().all(|slot| !slot.active));

    let pending = match prepared.advance() {
        ModemClockAcquireStep::Physical(pending) => pending,
        ModemClockAcquireStep::CommitReady(_) => panic!("fresh acquire needs edges"),
    };
    assert_eq!(pending.dependency, Fixture::Pll160AndModemSource);
    assert_eq!(pending.transaction.planner.counts, ieee802154_counts(0));

    let poisoned = pending.fail();
    assert_eq!(poisoned.completed_edges(), 0);
    assert_eq!(poisoned.dependency(), Fixture::Pll160AndModemSource);
    let pending = poisoned.reexpose_for_test();
    assert_eq!(pending.transaction.planner.counts, ieee802154_counts(0));

    let (planner, _lease, edges) = finish_acquire(pending.complete());
    assert_eq!(edges, IEEE802154_EDGES[1..]);
    assert_eq!(planner.counts, ieee802154_counts(1));
}

#[test]
fn release_failure_is_poisoned_and_keeps_counts_and_lease_until_commit() {
    let identity = ModemClockPlannerIdentity::new();
    let planner = Planner::managed(&identity);
    let (planner, lease, _) = finish_acquire(
        planner
            .prepare_acquire(Module::Ieee802154.dependencies())
            .expect("managed baseline"),
    );
    let prepared = planner.prepare_release(lease).expect("valid lease");
    assert_eq!(prepared.planner.counts, ieee802154_counts(1));
    assert!(prepared.planner.slots[usize::from(prepared.lease.slot)].active);

    let pending = match prepared.advance() {
        ModemClockReleaseStep::Physical(pending) => pending,
        ModemClockReleaseStep::CommitReady(_) => panic!("last release needs edges"),
    };
    let poisoned = pending.fail();
    assert_eq!(poisoned.completed_edges(), 0);
    assert_eq!(poisoned.dependency(), Fixture::Pll160AndModemSource);
    let pending = poisoned.reexpose_for_test();
    assert_eq!(pending.transaction.planner.counts, ieee802154_counts(1));
    assert!(pending.transaction.planner.slots[usize::from(pending.transaction.lease.slot)].active);

    let (planner, edges) = finish_release(pending.complete());
    assert_eq!(edges, IEEE802154_EDGES[1..]);
    assert_eq!(planner.counts, ieee802154_counts(0));
}

#[test]
fn source_refcount_boundary_accepts_max_then_rejects_overflow() {
    let identity = ModemClockPlannerIdentity::new();
    let mut planner = Planner::managed(&identity);
    planner.counts[Fixture::WifiBb80x1.index()] = MAX_REFCOUNT - 1;
    let identity_address = planner.identity as *const _;

    let prepared = planner
        .prepare_acquire(Module::Ieee802154.dependencies())
        .expect("MAX_REFCOUNT - 1 may advance to the source maximum");
    let (planner, _lease, edges) = finish_acquire(prepared);
    assert!(!edges.contains(&Fixture::WifiBb80x1));
    assert_eq!(planner.counts[Fixture::WifiBb80x1.index()], MAX_REFCOUNT);

    let failure = match planner.prepare_acquire(Module::Ieee802154.dependencies()) {
        Ok(_) => panic!("MAX_REFCOUNT must not exceed the source contract"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        ModemClockAcquirePreparationError::RefcountOverflow(Fixture::WifiBb80x1)
    );
    let planner = failure.into_planner();
    assert_eq!(planner.identity as *const _, identity_address);
    assert_eq!(planner.counts[Fixture::WifiBb80x1.index()], MAX_REFCOUNT);
    assert_eq!(planner.slots.iter().filter(|slot| slot.active).count(), 1);
}

#[test]
fn underflow_fails_transactionally_and_returns_both_opaque_owners() {
    let identity = ModemClockPlannerIdentity::new();
    let planner = Planner::managed(&identity);
    let (mut planner, lease, _) = finish_acquire(
        planner
            .prepare_acquire(Module::Ieee802154.dependencies())
            .expect("managed baseline"),
    );
    planner.counts[Fixture::Etm.index()] = 0;
    let lease_slot = lease.slot;

    let failure = match planner.prepare_release(lease) {
        Ok(_) => panic!("underflow must fail"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        ModemClockReleasePreparationError::RefcountUnderflow(Fixture::Etm)
    );
    let (planner, lease) = failure.into_owners();
    assert_eq!(lease.slot, lease_slot);
    assert_eq!(planner.counts[Fixture::Etm.index()], 0);
    assert!(planner.slots[usize::from(lease.slot)].active);
}

#[test]
fn duplicate_and_stale_leases_are_rejected_without_owner_loss() {
    let identity = ModemClockPlannerIdentity::new();
    let planner = Planner::managed(&identity);
    let (planner, first, _) = finish_acquire(
        planner
            .prepare_acquire(Module::Ieee802154.dependencies())
            .expect("first acquisition"),
    );
    let duplicate = duplicate_for_adversarial_test(&first);
    let stale = duplicate_for_adversarial_test(&first);
    let (planner, _) = finish_release(planner.prepare_release(first).expect("first release"));

    let duplicate_failure = match planner.prepare_release(duplicate) {
        Ok(_) => panic!("duplicate release must fail"),
        Err(failure) => failure,
    };
    assert_eq!(
        duplicate_failure.error(),
        ModemClockReleasePreparationError::DuplicateRelease
    );
    let (planner, duplicate) = duplicate_failure.into_owners();
    assert_eq!(planner.counts, ieee802154_counts(0));
    drop(duplicate);

    let (planner, current, _) = finish_acquire(
        planner
            .prepare_acquire(Module::Ieee802154.dependencies())
            .expect("slot generation advances"),
    );
    let stale_failure = match planner.prepare_release(stale) {
        Ok(_) => panic!("stale generation must fail"),
        Err(failure) => failure,
    };
    assert_eq!(
        stale_failure.error(),
        ModemClockReleasePreparationError::StaleLease
    );
    let (planner, stale) = stale_failure.into_owners();
    assert_eq!(planner.counts, ieee802154_counts(1));
    drop(stale);

    let (planner, edges) = finish_release(planner.prepare_release(current).expect("current lease"));
    assert_eq!(edges, IEEE802154_EDGES);
    assert_eq!(planner.counts, ieee802154_counts(0));
}

#[test]
fn cross_manager_lease_is_rejected_and_both_epochs_are_retained() {
    let first_identity = ModemClockPlannerIdentity::new();
    let second_identity = ModemClockPlannerIdentity::new();
    let first_planner = Planner::managed(&first_identity);
    let second_planner = Planner::managed(&second_identity);
    let (first_planner, lease, _) = finish_acquire(
        first_planner
            .prepare_acquire(Module::Ieee802154.dependencies())
            .expect("first manager"),
    );

    let failure = match second_planner.prepare_release(lease) {
        Ok(_) => panic!("cross-manager lease must fail"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        ModemClockReleasePreparationError::CrossManagerLease
    );
    let (second_planner, lease) = failure.into_owners();
    assert_eq!(second_planner.counts, ieee802154_counts(0));
    assert_eq!(first_planner.counts, ieee802154_counts(1));
    assert!(!ptr::eq(second_planner.identity, lease.identity));

    let (first_planner, edges) = finish_release(
        first_planner
            .prepare_release(lease)
            .expect("original manager accepts exact lease"),
    );
    assert_eq!(edges, IEEE802154_EDGES);
    assert_eq!(first_planner.counts, ieee802154_counts(0));
}

#[test]
fn externally_retained_baseline_cannot_issue_acquire_or_release_plans() {
    let mut external_identity = ModemClockPlannerIdentity::new();
    let external = Planner::externally_retained(&mut external_identity);
    let failure = match external.prepare_acquire(Module::Ieee802154.dependencies()) {
        Ok(_) => panic!("unknown baseline cannot plan acquisition"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        ModemClockAcquirePreparationError::UnknownBaseline
    );
    let external = failure.into_planner();
    assert_eq!(external.baseline, Baseline::ExternallyRetained);
    assert_eq!(external.counts, ieee802154_counts(0));

    let managed_identity = ModemClockPlannerIdentity::new();
    let managed = Planner::managed(&managed_identity);
    let (managed, lease, _) = finish_acquire(
        managed
            .prepare_acquire(Module::Ieee802154.dependencies())
            .expect("test managed baseline"),
    );

    let failure = match external.prepare_release(lease) {
        Ok(_) => panic!("unknown baseline cannot plan release"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        ModemClockReleasePreparationError::UnknownBaseline
    );
    let (external, lease) = failure.into_owners();
    assert_eq!(external.baseline, Baseline::ExternallyRetained);

    let (managed, edges) = finish_release(
        managed
            .prepare_release(lease)
            .expect("managed owner retains release authority"),
    );
    assert_eq!(edges, IEEE802154_EDGES);
    assert_eq!(managed.counts, ieee802154_counts(0));
}

#[test]
fn a_second_module_enables_only_its_missing_dependencies() {
    let identity = ModemClockPlannerIdentity::new();
    let planner = Planner::managed(&identity);
    let (planner, wifi, _) = finish_acquire(
        planner
            .prepare_acquire(Module::Wifi.dependencies())
            .expect("Wi-Fi"),
    );
    // Coexistence, the PLL source and the 80x1 clock are already on for Wi-Fi.
    let (planner, bluetooth, edges) = finish_acquire(
        planner
            .prepare_acquire(Module::Bluetooth.dependencies())
            .expect("Bluetooth"),
    );
    assert_eq!(
        edges,
        [
            Fixture::Etm,
            Fixture::BtMac,
            Fixture::BtPeripheral,
            Fixture::BtApbAndSecurity,
            Fixture::BtIeee802154CommonBaseband,
        ]
    );
    // Leaving Wi-Fi keeps every dependency Bluetooth still uses.
    let (planner, edges) = finish_release(planner.prepare_release(wifi).expect("Wi-Fi release"));
    assert_eq!(
        edges,
        [
            Fixture::WifiApb,
            Fixture::WifiBb44m,
            Fixture::WifiMac,
            Fixture::WifiBb,
        ]
    );
    let (planner, _) = finish_release(
        planner
            .prepare_release(bluetooth)
            .expect("Bluetooth release"),
    );
    assert_eq!(planner.counts, [0; DEPENDENCY_CAPACITY]);
}

#[test]
fn the_analog_i2c_master_edge_follows_every_module_request() {
    let identity = ModemClockPlannerIdentity::new();
    let planner = Planner::managed(&identity);
    let (planner, first, first_edges) = finish_acquire(
        planner
            .prepare_acquire(Module::Phy.dependencies())
            .expect("PHY"),
    );
    assert!(first_edges.contains(&Fixture::AnalogI2cMaster));
    let (planner, second, second_edges) = finish_acquire(
        planner
            .prepare_acquire(Module::AnalogI2cMaster.dependencies())
            .expect("analog I2C"),
    );
    // The platform owner counts analog-I2C references, so each request
    // reaches it even while the dependency is already enabled.
    assert_eq!(second_edges, [Fixture::AnalogI2cMaster]);
    let (planner, released) =
        finish_release(planner.prepare_release(second).expect("analog I2C release"));
    assert_eq!(released, [Fixture::AnalogI2cMaster]);
    let (_planner, released) = finish_release(planner.prepare_release(first).expect("PHY release"));
    assert!(released.contains(&Fixture::AnalogI2cMaster));
}

#[test]
fn initialized_wifi_keeps_its_clocks_and_re_enables_them_on_the_next_request() {
    let identity = ModemClockPlannerIdentity::new();
    let mut planner = Planner::managed(&identity);
    planner.set_wifi_initialized(true);
    let (planner, wifi, _) = finish_acquire(
        planner
            .prepare_acquire(Module::Wifi.dependencies())
            .expect("Wi-Fi"),
    );
    let (planner, released) = finish_release(planner.prepare_release(wifi).expect("release"));
    // Only the dependencies outside the Wi-Fi clock group are switched off.
    assert_eq!(
        released,
        [Fixture::Pll160AndModemSource, Fixture::Coexistence,]
    );
    assert_eq!(planner.counts, [0; DEPENDENCY_CAPACITY]);
    // The next zero-to-one request enables them again, including the
    // baseband reset carried by the WifiBb edge.
    let (_planner, _lease, edges) = finish_acquire(
        planner
            .prepare_acquire(Module::Wifi.dependencies())
            .expect("Wi-Fi again"),
    );
    assert!(edges.contains(&Fixture::WifiBb));
}

#[test]
fn the_executor_performs_edges_in_order_and_poisons_at_a_failed_edge() {
    let identity = ModemClockPlannerIdentity::new();
    let planner = Planner::managed(&identity);
    let mut performed = Vec::new();
    let (planner, lease) = execute_acquire(
        planner
            .prepare_acquire(Module::Ieee802154.dependencies())
            .expect("managed baseline"),
        |dependency| {
            performed.push(dependency);
            Ok::<(), ()>(())
        },
    )
    .expect("every edge performed");
    assert_eq!(performed, IEEE802154_EDGES);
    assert_eq!(planner.counts, ieee802154_counts(1));

    let mut attempted = Vec::new();
    let poisoned = match execute_release(
        planner.prepare_release(lease).expect("valid lease"),
        |dependency| {
            attempted.push(dependency);
            if dependency == Fixture::WifiBb80x1 {
                Err(())
            } else {
                Ok(())
            }
        },
    ) {
        Ok(_) => panic!("the refused edge must poison the release"),
        Err(poisoned) => poisoned,
    };
    assert_eq!(attempted, IEEE802154_EDGES[..3]);
    assert_eq!(poisoned.dependency(), Fixture::WifiBb80x1);
    assert_eq!(poisoned.completed_edges(), 2);
    // Nothing was committed: the lease is still recorded as held.
    let pending = poisoned.reexpose_for_test();
    assert_eq!(pending.transaction.planner.counts, ieee802154_counts(1));
}
