//! The installation's rights (`design/ble-port-contract-2026-11-19.md`,
//! sections 2 to 4) and the mint that makes them.
//!
//! ```text
//!             start             a confirmed start
//!  Stopped ──────────► Starting ─────────────────► Running
//!     ▲  ▲                 │ failed: the radio is stopped  │ stop
//!     │  └─────────────────┘                               ▼
//!     └──────────────── a confirmed stop ◄──────────── Stopping
//! ```
//!
//! Each right is a value of at most two words, branded with the
//! installation (`'id`), neither `Copy` nor `Clone`. Only this crate builds
//! one. A port makes the next right from the one it was given with its
//! [`PortKey`], which the installation's mint gives the port alone
//! ([`PortPools`]); a core holds the rights the machine and the controller
//! hold and never the key.
//!
//! A role's right and its places carry the index of their instance, and an
//! event or transmit place the index of its slot, given at the mint and
//! read by `index()` and `slot()`. The port takes the instance from the
//! place it is given.

use core::marker::PhantomData;

use oer_protocols_radio_port::{PoolBuilder, PoolPath, PoolSet};

use crate::LeCapabilities;

/// The brand of one installation, invariant in `'id`.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// The most instances or slots a role may have: an index is one octet.
pub const MAX_INDEX: usize = 256;

macro_rules! right {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[must_use = "a right is the only way to the call that takes it"]
        #[derive(Debug)]
        pub struct $name<'id> {
            brand: Brand<'id>,
        }
    };
}

macro_rules! instance_right {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[must_use = "a right is the only way to the call that takes it"]
        #[derive(Debug)]
        pub struct $name<'id> {
            brand: Brand<'id>,
            instance: u8,
        }

        impl $name<'_> {
            /// The index of its instance.
            pub const fn index(&self) -> u8 {
                self.instance
            }
        }
    };
}

macro_rules! slot_right {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[must_use = "a right is the only way to the call that takes it"]
        #[derive(Debug)]
        pub struct $name<'id> {
            brand: Brand<'id>,
            instance: u8,
            slot: u8,
        }

        impl $name<'_> {
            /// The index of its instance.
            pub const fn index(&self) -> u8 {
                self.instance
            }

            /// The index of its slot in its instance.
            pub const fn slot(&self) -> u8 {
                self.slot
            }
        }
    };
}

right!(
    /// The radio is stopped: what the install, a confirmed stop and a failed
    /// start give, and what a start and a retirement take.
    Stopped
);
right!(
    /// The right to stop the radio, held by the installation machine from
    /// the install to the release.
    StopRight
);
right!(
    /// A start in progress.
    Starting
);
right!(
    /// The radio runs: a confirmed start.
    Running
);
right!(
    /// A stop in progress. It always ends in [`Stopped`].
    Stopping
);
right!(
    /// The installation is retired: what a release takes.
    Retired
);
right!(
    /// The right to write the filter accept list to the hardware. The list
    /// itself is the controller's state.
    AcceptList
);

instance_right!(
    /// An advertising set's place, closed.
    AdvPlace
);
instance_right!(
    /// The scanner's place, closed.
    ScanPlace
);
instance_right!(
    /// A connection's place, closed.
    ConnPlace
);
instance_right!(
    /// The test session's place, closed.
    TestPlace
);
instance_right!(
    /// An open advertising set.
    Advertiser
);
instance_right!(
    /// The open scanner.
    Scanner
);
instance_right!(
    /// An open connection.
    Connection
);
instance_right!(
    /// The open test session.
    Test
);
instance_right!(
    /// An open advertising set's event place: home at its role, or out with
    /// its event.
    AdvEvent
);
instance_right!(
    /// The open scanner's event place.
    ScanEvent
);
instance_right!(
    /// The open test session's event place.
    TestEvent
);
slot_right!(
    /// One of an open connection's event places.
    ConnEvent
);
slot_right!(
    /// One of an open connection's transmit places: home at the ACL engine,
    /// or in the connection's chain with its PDU.
    TxPlace
);

/// A received PDU held in its receive node, from its taking in a round to
/// its release in a round.
#[must_use = "a held PDU's node comes back only by its release"]
#[derive(Debug)]
pub struct PduHandle<'id> {
    brand: Brand<'id>,
    node: u16,
}

impl PduHandle<'_> {
    /// The index of its receive node.
    pub const fn node(&self) -> u16 {
        self.node
    }
}

/// An event's place back from a round, typed by its role.
#[must_use = "an event's place is the only way to the role's next event and its closing"]
#[derive(Debug)]
pub enum EventPlace<'id> {
    /// An advertising set's.
    Advertising(AdvEvent<'id>),
    /// The scanner's.
    Scan(ScanEvent<'id>),
    /// A connection's.
    Connection(ConnEvent<'id>),
    /// The test session's.
    Test(TestEvent<'id>),
}

/// A role whose events are withdrawn, borrowed.
#[derive(Debug)]
pub enum RoleRef<'a, 'id> {
    /// An advertising set.
    Advertiser(&'a Advertiser<'id>),
    /// The scanner.
    Scanner(&'a Scanner<'id>),
    /// A connection: its events and its chain.
    Connection(&'a Connection<'id>),
    /// The test session.
    Test(&'a Test<'id>),
}

/// Where a stop begins: a running radio or a start in progress.
#[must_use = "a stop begins from it"]
#[derive(Debug)]
pub enum StopFrom<'id> {
    /// The radio runs.
    Running(Running<'id>),
    /// A start is in progress.
    Starting(Starting<'id>),
}

/// The empty places of every role and the accept list's right. The machine
/// holds them while the roles are stopped, the controller while the radio
/// runs; a retirement takes them whole.
#[must_use = "the roles' rights each have one holder"]
#[derive(Debug)]
pub struct RoleRights<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const TEST: usize,
> {
    /// The advertising sets' places.
    pub advertising: [AdvPlace<'id>; ADV],
    /// The scanner's place.
    pub scanners: [ScanPlace<'id>; SCAN],
    /// The connections' places.
    pub connections: [ConnPlace<'id>; CONN],
    /// The test session's place.
    pub tests: [TestPlace<'id>; TEST],
    /// The right to write the accept list.
    pub accept_list: AcceptList<'id>,
}

/// The index of position `position` of an array of at most [`MAX_INDEX`]
/// rights: exact, since every count is checked against `MAX_INDEX` at the
/// mint.
const fn index(position: usize) -> u8 {
    position as u8
}

/// The port's authority to make the next right from the one it was given.
/// The installation's mint gives it to the port alone; it is neither
/// `Copy` nor `Clone`, and no core holds one. Its geometry is the
/// installation's, so the arrays it makes have the lengths the mint
/// checked.
#[must_use = "the port keeps its key for the whole installation"]
#[derive(Debug)]
pub struct PortKey<'id, const EVENTS: usize, const TX: usize> {
    brand: Brand<'id>,
}

impl<'id, const EVENTS: usize, const TX: usize> PortKey<'id, EVENTS, TX> {
    /// A start began.
    pub const fn start(&self, stopped: Stopped<'id>) -> Starting<'id> {
        let Stopped { brand } = stopped;
        Starting { brand }
    }

    /// A start is confirmed: the radio runs.
    pub const fn started(&self, starting: Starting<'id>) -> Running<'id> {
        let Starting { brand } = starting;
        Running { brand }
    }

    /// A start failed: the port took down what it began, and the radio is
    /// stopped.
    pub const fn start_failed(&self, starting: Starting<'id>) -> Stopped<'id> {
        let Starting { brand } = starting;
        Stopped { brand }
    }

    /// A stop began.
    pub const fn stop(&self, from: StopFrom<'id>) -> Stopping<'id> {
        let (StopFrom::Running(Running { brand }) | StopFrom::Starting(Starting { brand })) = from;
        Stopping { brand }
    }

    /// A stop is confirmed.
    pub const fn stopped(&self, stopping: Stopping<'id>) -> Stopped<'id> {
        let Stopping { brand } = stopping;
        Stopped { brand }
    }

    /// The installation is retired, with every role's place home.
    pub fn retire<const ADV: usize, const SCAN: usize, const CONN: usize, const TEST: usize>(
        &self,
        stopped: Stopped<'id>,
        roles: RoleRights<'id, ADV, SCAN, CONN, TEST>,
    ) -> Retired<'id> {
        let Stopped { brand } = stopped;
        let RoleRights {
            advertising,
            scanners,
            connections,
            tests,
            accept_list,
        } = roles;
        let AcceptList { brand: _ } = accept_list;
        for AdvPlace { .. } in advertising {}
        for ScanPlace { .. } in scanners {}
        for ConnPlace { .. } in connections {}
        for TestPlace { .. } in tests {}
        Retired { brand }
    }

    /// The installation is released: the retirement's and the stop's
    /// rights end here.
    pub const fn release(&self, retired: Retired<'id>, right: StopRight<'id>) {
        let Retired { brand: _ } = retired;
        let StopRight { brand: _ } = right;
    }

    /// An advertising set opened in its place.
    pub const fn open_advertising(&self, place: AdvPlace<'id>) -> (Advertiser<'id>, AdvEvent<'id>) {
        let AdvPlace { brand, instance } = place;
        (Advertiser { brand, instance }, AdvEvent { brand, instance })
    }

    /// An advertising set closed: its role and its event's place give its
    /// place back. The instance is the role's.
    pub const fn close_advertising(
        &self,
        role: Advertiser<'id>,
        home: AdvEvent<'id>,
    ) -> AdvPlace<'id> {
        let Advertiser { brand, instance } = role;
        let AdvEvent { .. } = home;
        AdvPlace { brand, instance }
    }

    /// The scanner opened in its place.
    pub const fn open_scanner(&self, place: ScanPlace<'id>) -> (Scanner<'id>, ScanEvent<'id>) {
        let ScanPlace { brand, instance } = place;
        (Scanner { brand, instance }, ScanEvent { brand, instance })
    }

    /// The scanner closed.
    pub const fn close_scanner(&self, role: Scanner<'id>, home: ScanEvent<'id>) -> ScanPlace<'id> {
        let Scanner { brand, instance } = role;
        let ScanEvent { .. } = home;
        ScanPlace { brand, instance }
    }

    /// The test session opened in its place.
    pub const fn open_test(&self, place: TestPlace<'id>) -> (Test<'id>, TestEvent<'id>) {
        let TestPlace { brand, instance } = place;
        (Test { brand, instance }, TestEvent { brand, instance })
    }

    /// The test session closed.
    pub const fn close_test(&self, role: Test<'id>, home: TestEvent<'id>) -> TestPlace<'id> {
        let Test { brand, instance } = role;
        let TestEvent { .. } = home;
        TestPlace { brand, instance }
    }

    /// A connection opened in its place, with its event and transmit places.
    pub fn open_connection(
        &self,
        place: ConnPlace<'id>,
    ) -> (
        Connection<'id>,
        [ConnEvent<'id>; EVENTS],
        [TxPlace<'id>; TX],
    ) {
        let ConnPlace { brand, instance } = place;
        (
            Connection { brand, instance },
            core::array::from_fn(|slot| ConnEvent {
                brand,
                instance,
                slot: index(slot),
            }),
            core::array::from_fn(|slot| TxPlace {
                brand,
                instance,
                slot: index(slot),
            }),
        )
    }

    /// A connection closed: its role and every one of its places give its
    /// place back. The instance is the role's.
    pub fn close_connection(
        &self,
        role: Connection<'id>,
        events: [ConnEvent<'id>; EVENTS],
        tx: [TxPlace<'id>; TX],
    ) -> ConnPlace<'id> {
        let Connection { brand, instance } = role;
        for ConnEvent { .. } in events {}
        for TxPlace { .. } in tx {}
        ConnPlace { brand, instance }
    }

    /// A received PDU taken in a round: its node is held.
    pub const fn hold(&self, node: u16) -> PduHandle<'id> {
        PduHandle {
            brand: PhantomData,
            node,
        }
    }

    /// A held PDU released: the index of the node that goes back.
    pub const fn release_pdu(&self, handle: PduHandle<'id>) -> u16 {
        let PduHandle { brand: _, node } = handle;
        node
    }
}

/// What an installation's mint gives: the port's key, the radio stopped,
/// the stop's right and the roles' rights. The port keeps the key; the
/// machine takes the rest.
#[must_use = "an installation's rights each have one holder"]
#[derive(Debug)]
pub struct Install<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const TEST: usize,
> {
    /// The port's key.
    pub key: PortKey<'id, EVENTS, TX>,
    /// The radio is stopped.
    pub stopped: Stopped<'id>,
    /// The right to stop the radio.
    pub right: StopRight<'id>,
    /// The roles' rights.
    pub roles: RoleRights<'id, ADV, SCAN, CONN, TEST>,
}

/// The pool set of an installation of the backend capabilities `K` with
/// its geometry: `ADV` advertising sets, `SCAN` scanners, `CONN`
/// connections of `EVENTS` event places, `TX` transmit places and `RX`
/// receive nodes each, and `TEST` test sessions.
///
/// Its mint checks the geometry at build: `EVENTS` at least 2, each count
/// inside `K`'s limits and at most [`MAX_INDEX`], a scanner and a test
/// session one at most, and a role's count zero where `K` serves no such
/// role.
pub struct PortPools<
    K,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
>(PhantomData<fn() -> K>);

impl<
    K: LeCapabilities,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
> PortPools<K, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>
{
    /// The geometry's check: naming it fails the build where the geometry
    /// does not fit `K`. The mint names it.
    pub const GEOMETRY: () =
        crate::geometry::Geometry::<K, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>::CHECKED;
}

impl<
    K: LeCapabilities,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
> PoolSet for PortPools<K, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>
{
    type Pools<'id, Path: PoolPath> = Install<'id, ADV, SCAN, CONN, EVENTS, TX, TEST>;

    fn build<'id, Path: PoolPath>(builder: PoolBuilder<'id, Path>) -> Self::Pools<'id, Path> {
        const { Self::GEOMETRY };
        let () = builder.into_pools::<()>();
        minted()
    }
}

/// An installation's rights, for the mint and the tests' forge.
fn minted<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const TEST: usize,
>() -> Install<'id, ADV, SCAN, CONN, EVENTS, TX, TEST> {
    Install {
        key: PortKey { brand: PhantomData },
        stopped: Stopped { brand: PhantomData },
        right: StopRight { brand: PhantomData },
        roles: RoleRights {
            advertising: core::array::from_fn(|instance| AdvPlace {
                brand: PhantomData,
                instance: index(instance),
            }),
            scanners: core::array::from_fn(|instance| ScanPlace {
                brand: PhantomData,
                instance: index(instance),
            }),
            connections: core::array::from_fn(|instance| ConnPlace {
                brand: PhantomData,
                instance: index(instance),
            }),
            tests: core::array::from_fn(|instance| TestPlace {
                brand: PhantomData,
                instance: index(instance),
            }),
            accept_list: AcceptList { brand: PhantomData },
        },
    }
}

/// An installation's rights made without a mint and without its checks:
/// for the tests of a core that holds rights, never a product build.
#[cfg(feature = "test-support")]
pub fn forge<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const TEST: usize,
>() -> Install<'id, ADV, SCAN, CONN, EVENTS, TX, TEST> {
    minted()
}

#[cfg(test)]
mod tests;
