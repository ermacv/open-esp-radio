//! The cipher's offload (`design/ble-port-contract-2026-11-19.md`, section
//! 6): declared so that the base contract does not shut it. No backend
//! implements it; its form is fixed after a stand run (B8).

use core::marker::PhantomData;

use crate::{LePort, rights::Connection};

/// The brand of one installation, invariant in `'id`.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// A key slot of the hardware, empty.
#[must_use = "a right is the only way to the call that takes it"]
#[derive(Debug)]
pub struct KeySlot<'id> {
    brand: Brand<'id>,
}

/// A key slot holding a connection's session key.
#[must_use = "a right is the only way to the call that takes it"]
#[derive(Debug)]
pub struct Keyed<'id> {
    brand: Brand<'id>,
}

/// A connection's 128-bit session key (Core Vol 6 Part E 1).
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SessionKey(pub [u8; 16]);

/// A connection's 64-bit initialization vector (Core Vol 6 Part E 1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Iv(pub [u8; 8]);

/// Which direction of a connection a key's counter starts from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// From the central to the peripheral.
    CentralToPeripheral,
    /// From the peripheral to the central.
    PeripheralToCentral,
}

/// A backend that deciphers a connection's PDUs in its hardware.
pub trait CipherOffload<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
>: LePort<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>
{
    /// Install a connection's key in a slot.
    fn install_key(
        &mut self,
        role: &Connection<'id>,
        slot: KeySlot<'id>,
        key: &SessionKey,
        iv: &Iv,
        from: Direction,
    ) -> Keyed<'id>;
    /// Remove a connection's key: its slot is empty again.
    fn remove_key(&mut self, role: &Connection<'id>, keyed: Keyed<'id>) -> KeySlot<'id>;
}
