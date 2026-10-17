use crate::{
    ieee80211::client::{WifiClocked, WifiCold},
    owner::{MacInterruptSetup, RadioRuntimeOwner},
    root::RadioHardware,
    shared_radio::SharedRadio,
};

use super::*;

/// A running Wi-Fi client over a validation split, beside its arbiter.
fn running() -> (SharedRadio<()>, RadioRuntimeOwner, MacInterruptSetup) {
    let (shared, partitions) = RadioHardware::for_validation().into_concurrent(());
    let (owner, interrupts, _clocks) =
        WifiClocked::for_validation(WifiCold::from_partition(partitions.wifi)).into_running();
    (shared, owner, interrupts)
}

#[test]
fn stable_publication_reclaims_exactly_once_and_drop_poison_is_sticky() {
    let (_shared, owner, _interrupt_setup) = running();
    let arena = RadioOwnerArena::new();
    let published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept its first owner"));
    assert_eq!(arena.state(), RadioOwnerArenaState::Published);

    let borrowed = arena.registers.borrow();
    let published = match published.try_reclaim() {
        Ok(_) => panic!("an outstanding transaction must prevent reclaim"),
        Err((published, error)) => {
            assert_eq!(error, RadioOwnerArenaError::Borrowed);
            published
        }
    };
    drop(borrowed);
    let reclaimed = published
        .try_reclaim_with_republish()
        .unwrap_or_else(|_| panic!("a returned transaction must permit exact reclaim"));
    assert_eq!(arena.state(), RadioOwnerArenaState::Empty);

    let poisoned = RadioOwnerArena::new();
    let published = poisoned
        .publish(reclaimed.into_owner())
        .unwrap_or_else(|_| panic!("the second empty arena must accept the reclaimed owner"));
    drop(published);
    assert_eq!(poisoned.state(), RadioOwnerArenaState::ResetRequired);
}

#[test]
fn reclaimed_owner_republishes_only_through_its_exact_arena_binding() {
    let (_shared, owner, _interrupt_setup) = running();
    let arena = RadioOwnerArena::new();
    let published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
    let access = published.access();
    let reclaimed = published
        .try_reclaim_with_republish()
        .unwrap_or_else(|_| panic!("a quiescent lease must retain its arena binding"));
    assert_eq!(arena.state(), RadioOwnerArenaState::Empty);
    assert!(
        matches!(
            access.try_wifi_mac_hal(),
            Err(RadioOwnerArenaError::MissingOwner)
        ),
        "previous child handles must be revoked throughout maintenance"
    );

    let published = reclaimed
        .try_republish()
        .unwrap_or_else(|_| panic!("the exact empty arena must accept republication"));
    assert_eq!(arena.state(), RadioOwnerArenaState::Published);
    let mac = access
        .try_wifi_mac_hal()
        .expect("the exact arena restores existing child handles");
    drop(mac);
    let _registers = published
        .try_reclaim()
        .unwrap_or_else(|_| panic!("the republished owner must remain reclaimable"));
}

#[test]
fn published_channel_capability_holds_the_arena_serialization_guard() {
    let (shared, owner, _interrupt_setup) = running();
    let arena = RadioOwnerArena::new();
    let published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
    let access = published.access();
    let mut platform = ();
    let mut lease = shared
        .try_acquire()
        .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
    let channel = access
        .try_channel_hal(&mut platform, &mut lease)
        .unwrap_or_else(|_| panic!("published registers must yield a channel capability"));

    let published = match published.try_reclaim() {
        Ok(_) => panic!("a live channel capability must prevent reclaim"),
        Err((published, error)) => {
            assert_eq!(error, RadioOwnerArenaError::Borrowed);
            published
        }
    };
    drop(channel);
    let _registers = published
        .try_reclaim()
        .unwrap_or_else(|_| panic!("dropping the channel capability must release the guard"));
}

#[test]
fn published_wifi_mac_capability_holds_the_arena_serialization_guard() {
    let (_shared, owner, _interrupt_setup) = running();
    let arena = RadioOwnerArena::new();
    let published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
    let access = published.access();
    let wifi_mac = access
        .try_wifi_mac_hal()
        .unwrap_or_else(|_| panic!("published registers must yield a Wi-Fi MAC capability"));

    let published = match published.try_reclaim() {
        Ok(_) => panic!("a live Wi-Fi MAC capability must prevent reclaim"),
        Err((published, error)) => {
            assert_eq!(error, RadioOwnerArenaError::Borrowed);
            published
        }
    };
    drop(wifi_mac);
    let _registers = published
        .try_reclaim()
        .unwrap_or_else(|_| panic!("dropping the Wi-Fi MAC capability must release the guard"));
}

#[test]
fn stale_access_cannot_mutate_a_reset_required_arena() {
    let (_shared, owner, mut interrupt_setup) = running();
    let arena = RadioOwnerArena::new();
    let published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
    let access = published.access();

    drop(published);
    assert_eq!(arena.state(), RadioOwnerArenaState::ResetRequired);
    assert!(matches!(
        access.try_prepare_connected_sta_without_power_save(&mut interrupt_setup),
        Err(RadioOwnerArenaError::ResetRequired)
    ));
}

#[test]
fn the_unique_lease_writes_the_station_tsf_in_one_serialized_transaction() {
    let (_shared, owner, _interrupt_setup) = running();
    let arena = RadioOwnerArena::new();
    let mut published = arena
        .publish(owner)
        .unwrap_or_else(|_| panic!("an empty arena must accept the runtime owner"));
    let access = published.access();
    let wifi_mac = access
        .try_wifi_mac_hal()
        .unwrap_or_else(|_| panic!("published registers must yield a Wi-Fi MAC capability"));
    // The write is one serialized transaction: a live one refuses it
    // before any register access.
    assert_eq!(
        published.set_station_tsf(1_000_000),
        Err(RadioOwnerArenaError::Borrowed)
    );
    drop(wifi_mac);
}
