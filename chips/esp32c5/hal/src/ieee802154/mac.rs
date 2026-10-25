//! Driver-facing owners of the dedicated IEEE 802.15.4 MAC registers.
//!
//! The task owner holds every task-side word; the armed interrupt owner is
//! the MAC's interrupt part. The interrupt table binds the interrupt owner
//! by value and the task enables the events with the binding's proof, or the
//! engine pair of the [`ll`](crate::ieee802154::ll) backend holds both and
//! polls the events. The owners are made from and returned to the PAC
//! partition; the MAC lifecycle that will own clocks, reset and foundation
//! around them is not part of this crate yet.

use oer_esp32c5_pac::{
    Ieee802154InterruptRegisters as PacInterruptRegisters,
    Ieee802154InterruptSetup as PacInterruptSetup, Ieee802154RegisterLease as PacRegisterLease,
    Ieee802154TaskRegisters as PacTaskRegisters,
};
use oer_interrupt_table::Bound;
use oer_mmio::InterruptSource;

/// Exclusive task-side owner of the IEEE 802.15.4 MAC registers.
///
/// Every operation is expressed in the HAL's semantic vocabulary; drivers
/// never name a register-level value type.
#[must_use = "the IEEE 802.15.4 task owner must be returned to its foundation owner"]
pub struct Ieee802154TaskOwner {
    registers: PacTaskRegisters,
}

impl Ieee802154TaskOwner {
    /// Split the IEEE 802.15.4 partition into the task owner and its
    /// inactive interrupt owner. This performs no MMIO.
    pub fn new(
        partition: oer_esp32c5_pac::Ieee802154Partition,
    ) -> (Self, Ieee802154InterruptSetupOwner) {
        let (registers, interrupts) = PacTaskRegisters::new(partition);
        (
            Self { registers },
            Ieee802154InterruptSetupOwner {
                registers: interrupts,
            },
        )
    }

    /// Reunite the task owner with its inactive interrupt owner and return
    /// the partition. This performs no MMIO.
    pub fn into_partition(
        self,
        interrupts: Ieee802154InterruptSetupOwner,
    ) -> oer_esp32c5_pac::Ieee802154Partition {
        self.registers.into_partition(interrupts.registers)
    }

    /// `ieee802154_txon_delay_set` of the ESP32-C5 libbtbb: the vendor
    /// transmit-on, turnaround, receive-on and transmit-off delays, in the
    /// vendor order. `ieee802154_mac_init` applies them after the register
    /// part of the MAC initialization, before interrupts are allocated, so
    /// the interrupt owner must still be inactive.
    pub fn apply_txon_delay(&mut self, interrupts: &mut Ieee802154InterruptSetupOwner) {
        interrupts
            .registers
            .polled_register_lease(&mut self.registers)
            .apply_txon_delay();
    }

    /// Borrow the PAC task lease for one low-level accessor.
    pub(crate) fn lease(&mut self) -> PacRegisterLease<'_> {
        self.registers.ieee802154_register_lease()
    }
}

/// Inactive IEEE 802.15.4 interrupt ownership.
#[must_use = "the inactive IEEE 802.15.4 interrupt owner must be returned to its foundation owner"]
pub struct Ieee802154InterruptSetupOwner {
    registers: PacInterruptSetup,
}

impl Ieee802154InterruptSetupOwner {
    /// Install the reviewed abort baseline with every event masked,
    /// acknowledge one stale snapshot and return the interrupt owner.
    ///
    /// The owner is the MAC's interrupt part: the interrupt table binds it
    /// by value (`oer_interrupt_table::Router::bind`), and the binding's
    /// [`Bound`] proof is what [`Ieee802154TaskOwner::enable_events`] takes,
    /// so no event reaches the CPU before the handler holds the owner. The
    /// engine pair ([`Ieee802154MacOwners`](crate::ieee802154::ll::Ieee802154MacOwners))
    /// holds the owner itself instead: its source is never bound, so the
    /// matrix never routes it and the engine polls its events.
    pub fn arm(self, task: &mut Ieee802154TaskOwner) -> Ieee802154InterruptOwner {
        Ieee802154InterruptOwner {
            registers: self.registers.arm(&mut task.registers),
        }
    }
}

impl Ieee802154TaskOwner {
    /// Publish the runtime events of the reviewed ISR: from here the MAC
    /// raises its interrupt. `bound` proves that the interrupt table's
    /// handler holds the armed interrupt owner and its line and route are
    /// written.
    ///
    /// ```compile_fail,E0061
    /// # use oer_esp32c5_hal::ieee802154::mac::Ieee802154TaskOwner;
    /// fn enable(task: &mut Ieee802154TaskOwner) {
    ///     // Enabling the interrupt takes the binding's proof.
    ///     task.enable_events();
    /// }
    /// ```
    ///
    /// ```compile_fail,E0308
    /// # use oer_esp32c5_hal::ieee802154::mac::Ieee802154TaskOwner;
    /// # use oer_interrupt_table::Bound;
    /// struct OtherIrq;
    /// impl oer_mmio::InterruptSource for OtherIrq {
    ///     const SOURCE: u16 = 3;
    /// }
    /// fn enable(task: &mut Ieee802154TaskOwner, bound: &Bound<OtherIrq>) {
    ///     // Another part's binding proves nothing about this interrupt.
    ///     task.enable_events(bound);
    /// }
    /// ```
    pub fn enable_events(&mut self, bound: &Bound<Ieee802154InterruptOwner>) {
        let _bound = bound;
        self.registers.enable_runtime_events();
    }
}

/// Armed IEEE 802.15.4 interrupt owner: the MAC's interrupt part.
///
/// An image's interrupt table binds it by value, its source (`ZB_MAC`) from
/// its type; the task then enables the events with the binding's proof:
///
/// ```no_run
/// use oer_esp32c5_hal::ieee802154::mac::{
///     Ieee802154InterruptOwner, Ieee802154InterruptSetupOwner, Ieee802154TaskOwner,
/// };
/// use oer_esp32c5_hal::platform::InterruptController;
/// use oer_interrupt_table::Router;
/// # const LINES: oer_interrupt_table::Lines =
/// #     oer_interrupt_table::Lines { first: 16, count: 48, levels: 8, harts: 1 };
/// pub fn on_mac(_: &mut Ieee802154InterruptOwner) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: LINES,
///     handler_attributes: [];
///     mac: MacToken = Ieee802154InterruptOwner => on_mac, line 20, level 3, hart 0, Hardware;
/// }
/// fn start(
///     router: &mut Router<InterruptController>,
///     token: MacToken,
///     task: &mut Ieee802154TaskOwner,
///     setup: Ieee802154InterruptSetupOwner,
/// ) {
///     let interrupts = setup.arm(task);
///     if let Ok(bound) = router.bind(token, interrupts) {
///         task.enable_events(&bound);
///     }
/// }
/// # fn main() {}
/// ```
///
/// The owner moves into its one binding:
///
/// ```compile_fail,E0382
/// # use oer_esp32c5_hal::ieee802154::mac::{Ieee802154InterruptOwner, Ieee802154TaskOwner};
/// # use oer_esp32c5_hal::platform::InterruptController;
/// # use oer_interrupt_table::Router;
/// # const LINES: oer_interrupt_table::Lines =
/// #     oer_interrupt_table::Lines { first: 16, count: 48, levels: 8, harts: 1 };
/// # pub fn on_mac(_: &mut Ieee802154InterruptOwner) {}
/// # oer_interrupt_table::interrupt_table! {
/// #     lines: LINES,
/// #     handler_attributes: [];
/// #     mac: MacToken = Ieee802154InterruptOwner => on_mac, line 20, level 3, hart 0, Hardware;
/// # }
/// fn start(
///     router: &mut Router<InterruptController>,
///     token: MacToken,
///     task: &mut Ieee802154TaskOwner,
///     interrupts: Ieee802154InterruptOwner,
/// ) {
///     let _bound = router.bind(token, interrupts);
///     // The handler holds the owner now; the task cannot take it back.
///     let _setup = interrupts.deactivate(task);
/// }
/// # fn main() {}
/// ```
///
/// and the MAC's token binds the MAC's owner alone:
///
/// ```compile_fail,E0308
/// # use oer_esp32c5_hal::ieee802154::mac::Ieee802154InterruptOwner;
/// # use oer_esp32c5_hal::platform::InterruptController;
/// # use oer_interrupt_table::Router;
/// # const LINES: oer_interrupt_table::Lines =
/// #     oer_interrupt_table::Lines { first: 16, count: 48, levels: 8, harts: 1 };
/// # pub fn on_mac(_: &mut Ieee802154InterruptOwner) {}
/// # oer_interrupt_table::interrupt_table! {
/// #     lines: LINES,
/// #     handler_attributes: [];
/// #     mac: MacToken = Ieee802154InterruptOwner => on_mac, line 20, level 3, hart 0, Hardware;
/// # }
/// pub struct ConsoleIrq;
/// impl oer_mmio::InterruptSource for ConsoleIrq {
///     const SOURCE: u16 = 54;
/// }
/// fn start(router: &mut Router<InterruptController>, token: MacToken) {
///     let _bound = router.bind(token, ConsoleIrq);
/// }
/// # fn main() {}
/// ```
#[must_use = "the armed IEEE 802.15.4 interrupt owner must be bound or deactivated"]
pub struct Ieee802154InterruptOwner {
    registers: PacInterruptRegisters,
}

impl InterruptSource for Ieee802154InterruptOwner {
    /// The MAC's source (`ZB_MAC`), generated for its interrupt side.
    const SOURCE: u16 = <PacInterruptRegisters as InterruptSource>::SOURCE;
}

impl Ieee802154InterruptOwner {
    pub(crate) const fn registers(&self) -> &PacInterruptRegisters {
        &self.registers
    }

    pub(crate) fn registers_mut(&mut self) -> &mut PacInterruptRegisters {
        &mut self.registers
    }

    /// Close one polled epoch of the engine pair and return inactive setup
    /// ownership. A bound owner never comes back: its handler holds it.
    pub fn deactivate(self, task: &mut Ieee802154TaskOwner) -> Ieee802154InterruptSetupOwner {
        Ieee802154InterruptSetupOwner {
            registers: self.registers.deactivate(&mut task.registers),
        }
    }
}
