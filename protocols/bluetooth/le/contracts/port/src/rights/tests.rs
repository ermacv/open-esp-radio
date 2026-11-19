#![allow(clippy::disallowed_macros)]

use super::{Install, PortKey, minted};

/// An installation of two advertising sets, a scanner, three connections of
/// two events and four transmit places, and a test session.
fn install<'id>() -> Install<'id, 2, 1, 3, 2, 4, 1> {
    minted()
}

#[test]
fn the_mint_names_every_instance_by_its_position() {
    let Install { roles, .. } = install();
    assert_eq!(
        roles.advertising.each_ref().map(|place| place.index()),
        [0, 1]
    );
    assert_eq!(roles.scanners.each_ref().map(|place| place.index()), [0]);
    assert_eq!(
        roles.connections.each_ref().map(|place| place.index()),
        [0, 1, 2]
    );
    assert_eq!(roles.tests.each_ref().map(|place| place.index()), [0]);
}

#[test]
fn an_opening_and_its_closing_keep_the_instance() {
    let Install { key, roles, .. } = install();
    let [_, second] = roles.advertising;
    let (role, event) = key.open_advertising(second);
    assert_eq!((role.index(), event.index()), (1, 1));
    assert_eq!(key.close_advertising(role, event).index(), 1);

    let [scanner] = roles.scanners;
    let (role, event) = key.open_scanner(scanner);
    assert_eq!((role.index(), event.index()), (0, 0));
    assert_eq!(key.close_scanner(role, event).index(), 0);

    let [test] = roles.tests;
    let (role, event) = key.open_test(test);
    assert_eq!((role.index(), event.index()), (0, 0));
    assert_eq!(key.close_test(role, event).index(), 0);
}

#[test]
fn a_connection_names_its_instance_and_each_slot() {
    let Install { key, roles, .. } = install();
    let [_, _, third] = roles.connections;
    let (role, events, tx) = key.open_connection(third);
    assert_eq!(role.index(), 2);
    assert_eq!(
        events.each_ref().map(|place| (place.index(), place.slot())),
        [(2, 0), (2, 1)]
    );
    assert_eq!(
        tx.each_ref().map(|place| (place.index(), place.slot())),
        [(2, 0), (2, 1), (2, 2), (2, 3)]
    );
    assert_eq!(key.close_connection(role, events, tx).index(), 2);
}

#[test]
fn the_radio_runs_its_life_through_the_key() {
    let Install {
        key,
        stopped,
        right,
        roles,
    } = install();
    let starting = key.start(stopped);
    let stopped = key.start_failed(starting);
    let running = key.started(key.start(stopped));
    let stopping = key.stop(super::StopFrom::Running(running));
    let stopping_again = key.stop(super::StopFrom::Starting(key.start(key.stopped(stopping))));
    let retired = key.retire(key.stopped(stopping_again), roles);
    key.release(retired, right);
}

#[test]
fn a_held_pdu_names_its_node() {
    let key: PortKey<'_, 2, 4> = install().key;
    let handle = key.hold(513);
    assert_eq!(handle.node(), 513);
    assert_eq!(key.release_pdu(handle), 513);
}
