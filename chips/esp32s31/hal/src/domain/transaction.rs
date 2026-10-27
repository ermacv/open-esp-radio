//! Owned PHY transactions and their affine return to the exact radio domain.
use super::*;
use core::{
    mem::ManuallyDrop,
    sync::atomic::{AtomicBool, Ordering},
};
use oer_memory::OnceValue;
mod record;
pub use record::FaultRecord;
#[cfg(target_arch = "riscv32")]
oer_memory::link_fn! { declare fn __oer_phy_transaction_fault(fault: Fault) -> !; }

/// The admitted operation whose owners must remain quarantined on cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Register,
    Maintain,
    Wake,
    Close,
    Channel,
    Retune,
}

/// Define the image's reporter for the exact signature declared by this HAL.
#[macro_export]
macro_rules! phy_fault_reporter {
    ($report:path) => {
        $crate::domain::transaction::__link_fn! {
            define fn __oer_phy_transaction_fault(fault: $crate::domain::transaction::Fault) -> ! = $report;
        }
    };
}
#[doc(hidden)]
pub use oer_memory::link_fn as __link_fn;

/// Why an owned PHY transaction was quarantined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultReason {
    Cancelled(Operation),
    IdentityClaimed,
    OwnerUnavailable,
}

/// Original observations retained on cancellation. No register is reclaimed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fault {
    pub reason: FaultReason,
    pub registration: Option<PhyRegistrationEpoch>,
}

/// One permanent identity and first-fault cell for a radio domain.
/// Its reporter records the fault and resets without returning.
/// Host cancellation tests catch their own reporter's unwind.
pub struct TransactionSeat {
    claimed: AtomicBool,
    fault: OnceValue<Fault>,
    #[cfg(not(target_arch = "riscv32"))]
    report: fn(Fault) -> !,
}

impl TransactionSeat {
    /// Firmware faults call the image's direct link-time reset binding.
    #[cfg(target_arch = "riscv32")]
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            fault: OnceValue::new(),
        }
    }
    /// One host model's reporter; tests may catch its unwind.
    #[cfg(not(target_arch = "riscv32"))]
    pub const fn for_host(report: fn(Fault) -> !) -> Self {
        Self {
            claimed: AtomicBool::new(false),
            fault: OnceValue::new(),
            report,
        }
    }

    pub fn claim(&'static self) -> Option<PhyIdentity> {
        self.claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(PhyIdentity { seat: self })
    }

    pub fn fault(&self) -> Option<Fault> {
        self.fault.get()
    }

    fn quarantine(&'static self, fault: Fault) -> ! {
        let _first = self.fault.set(fault);
        #[cfg(target_arch = "riscv32")]
        {
            __oer_phy_transaction_fault(fault)
        }
        #[cfg(not(target_arch = "riscv32"))]
        {
            (self.report)(fault)
        }
    }
}

/// The unique domain's right to its transaction seat; never cloned or recycled.
pub struct PhyIdentity {
    seat: &'static TransactionSeat,
}

impl PhyIdentity {
    /// A distinct permanent identity for one hermetic host model.
    #[cfg(all(any(test, feature = "test-support"), not(target_arch = "riscv32")))]
    pub fn for_host_test() -> Self {
        let seat = std::boxed::Box::leak(std::boxed::Box::new(TransactionSeat::for_host(|_| {
            std::process::abort()
        })));
        seat.claimed.store(true, Ordering::Relaxed);
        Self { seat }
    }
}

/// Diagnostic capability only: it grants no register or reunion authority.
#[derive(Clone, Copy)]
pub struct FaultHandle {
    seat: &'static TransactionSeat,
    registration: Option<PhyRegistrationEpoch>,
}
impl FaultHandle {
    /// Require an owner from its storage; absence has one fixed typed cause.
    /// A diagnostic handle cannot select a cancellation or identity fault.
    pub fn require_owner<T>(self, owner: Option<T>) -> T {
        match owner {
            Some(owner) => owner,
            None => self.seat.quarantine(Fault {
                reason: FaultReason::OwnerUnavailable,
                registration: self.registration,
            }),
        }
    }
    pub fn fault(self) -> Option<Fault> {
        self.seat.fault()
    }
}

struct Guard {
    seat: &'static TransactionSeat,
    registration: Option<PhyRegistrationEpoch>,
    reason: FaultReason,
    armed: bool,
}
impl Guard {
    fn new(
        identity: &PhyIdentity,
        registration: Option<PhyRegistrationEpoch>,
        reason: FaultReason,
    ) -> Self {
        Self {
            seat: identity.seat,
            registration,
            reason,
            armed: true,
        }
    }
    fn complete(mut self) {
        self.armed = false;
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if self.armed && !self.seat.fault.is_requested() {
            self.seat.quarantine(Fault {
                reason: self.reason,
                registration: self.registration,
            });
        }
    }
}

/// An owned hardware partition, lent only within its owned transaction.
pub struct PhyHardware {
    pub(crate) registers: crate::phy::RadioPhyRegisters,
    pub(crate) restore: PhyRouteState,
}

struct Payload {
    hardware: PhyHardware,
    coexistence: oer_esp32s31_pac::ownership::CoexistencePeripherals,
    shared_radio: crate::modem::ownership::SharedBasebandParts,
    pti: CoexPtiTable,
    grant_protected: bool,
}

/// Owned shared PHY, restore/registration state and grant-protect registers.
/// Dropping either this value or its return owner quarantines both payloads;
/// only explicit consuming reunion disarms their cancellation guards.
#[must_use = "an owned PHY transaction must reunite with its exact return owner"]
pub struct PhyTransaction {
    guard: Guard,
    payload: ManuallyDrop<Payload>,
}
impl PhyTransaction {
    pub fn hardware_mut(&mut self) -> &mut PhyHardware {
        &mut self.payload.hardware
    }
    pub fn hardware(&self) -> &PhyHardware {
        &self.payload.hardware
    }
    pub fn hardware_and_grant(&mut self) -> (&mut PhyHardware, PhyGrantProtect<'_>) {
        let p = &mut *self.payload;
        let pti = p.pti.pti(PHY_GRANT_PROTECT_EVENT);
        (
            &mut p.hardware,
            PhyGrantProtect::new(
                crate::modem::CoexTimerBankRegisters::new(&mut p.coexistence.coex_hw_timer),
                pti,
                &mut p.grant_protected,
            ),
        )
    }
}

struct ReturnPayload<B: PlatformClockBackend, T> {
    power: CommonRadioPower<B>,
    wifi_power_entry: crate::ieee80211::client::WifiPowerEntry,
    btbb_clients: u8,
    btbb_gain_generation: u32,
    clocks: ModemClocks,
    modem_platform_clocks: ModemPlatformClocks<B>,
    bluetooth_low_power_clock: Option<PlatformClockGuard<B>>,
    external_coex: Option<crate::coex::ExternalCoexConfig>,
    coex_participants: CoexParticipants,
    attachment: T,
    identity: PhyIdentity,
}

/// The rest of one domain while its PHY is detached. It grants no MMIO.
#[must_use = "the detached domain must consume its own PHY transaction"]
pub struct DetachedRadioDomain<B: PlatformClockBackend, T> {
    guard: Guard,
    payload: ManuallyDrop<ReturnPayload<B, T>>,
}

/// Refused foreign reunion, retaining both unchanged owners for exact reunion.
#[must_use = "refused reunion retains both owners"]
pub struct ReuniteFailure<B: PlatformClockBackend, T> {
    pub expected_owner: usize,
    pub received_owner: usize,
    pub domain: DetachedRadioDomain<B, T>,
    pub transaction: PhyTransaction,
}
impl<B: PlatformClockBackend, T> DetachedRadioDomain<B, T> {
    pub fn attachment(&self) -> &T {
        &self.payload.attachment
    }
    pub fn attachment_mut(&mut self) -> &mut T {
        &mut self.payload.attachment
    }

    #[allow(
        clippy::result_large_err,
        reason = "refusal retains the exact affine inputs"
    )]
    pub fn reunite(
        self,
        transaction: PhyTransaction,
    ) -> Result<RadioDomain<B, T>, ReuniteFailure<B, T>> {
        if !core::ptr::eq(self.guard.seat, transaction.guard.seat) {
            return Err(ReuniteFailure {
                expected_owner: core::ptr::from_ref(self.guard.seat).addr(),
                received_owner: core::ptr::from_ref(transaction.guard.seat).addr(),
                domain: self,
                transaction,
            });
        }
        Ok(combine(self, transaction))
    }
}

/// One exact domain/PHY pair, owned across an async operation. Splitting it
/// permits transfer; reuniting a split pair checks the original identity.
#[must_use = "finish the owned radio transaction explicitly"]
pub struct RadioTransaction<B: PlatformClockBackend, T> {
    domain: DetachedRadioDomain<B, T>,
    phy: PhyTransaction,
}
impl<B: PlatformClockBackend, T> RadioTransaction<B, T> {
    pub fn split(self) -> (DetachedRadioDomain<B, T>, PhyTransaction) {
        (self.domain, self.phy)
    }
    pub fn reunite(self) -> RadioDomain<B, T> {
        combine(self.domain, self.phy)
    }
    pub fn hardware_mut(&mut self) -> &mut PhyHardware {
        self.phy.hardware_mut()
    }
    pub fn hardware(&self) -> &PhyHardware {
        self.phy.hardware()
    }
    pub fn attachment(&self) -> &T {
        self.domain.attachment()
    }
    pub fn attachment_mut(&mut self) -> &mut T {
        self.domain.attachment_mut()
    }
    pub fn hardware_with_attachment(&mut self) -> (&mut PhyHardware, &mut T) {
        (self.phy.hardware_mut(), self.domain.attachment_mut())
    }
    pub fn hardware_with_attachment_and_grant(
        &mut self,
    ) -> (&mut PhyHardware, &mut T, PhyGrantProtect<'_>) {
        let (hardware, grant) = self.phy.hardware_and_grant();
        (hardware, self.domain.attachment_mut(), grant)
    }
}

fn combine<B: PlatformClockBackend, T>(
    domain: DetachedRadioDomain<B, T>,
    transaction: PhyTransaction,
) -> RadioDomain<B, T> {
    let DetachedRadioDomain { guard, payload } = domain;
    let PhyTransaction {
        guard: phy_guard,
        payload: phy_payload,
    } = transaction;
    guard.complete();
    phy_guard.complete();
    let ReturnPayload {
        power,
        wifi_power_entry,
        btbb_clients,
        btbb_gain_generation,
        clocks,
        modem_platform_clocks,
        bluetooth_low_power_clock,
        external_coex,
        coex_participants,
        attachment,
        identity,
    } = ManuallyDrop::into_inner(payload);
    let Payload {
        hardware:
            PhyHardware {
                registers: radio_phy,
                restore: phy,
            },
        coexistence,
        shared_radio,
        pti: coex_pti,
        grant_protected: phy_grant_protected,
    } = ManuallyDrop::into_inner(phy_payload);
    RadioDomain::from_state(SharedRadioState {
        registers: SharedRadioRegisters {
            radio_phy,
            coexistence,
            shared_radio,
        },
        phy,
        power,
        wifi_power_entry,
        btbb_clients,
        btbb_gain_generation,
        clocks,
        modem_platform_clocks,
        coex_pti,
        phy_grant_protected,
        bluetooth_low_power_clock,
        external_coex,
        coex_participants,
        attachment,
        identity,
    })
}

impl<B: PlatformClockBackend, T> RadioDomain<B, T> {
    /// Observe or report faults without register or reunion authority.
    pub fn fault_handle(&self) -> FaultHandle {
        FaultHandle {
            seat: self.state.identity.seat,
            registration: self.registration_epoch(),
        }
    }

    /// Consume the domain without MMIO; an affine return owner serializes it
    /// with the movable PHY transaction. Registration and restore stay intact.
    pub fn detach(self, operation: Operation) -> RadioTransaction<B, T> {
        let SharedRadioState {
            registers:
                SharedRadioRegisters {
                    radio_phy,
                    coexistence,
                    shared_radio,
                },
            phy,
            power,
            wifi_power_entry,
            btbb_clients,
            btbb_gain_generation,
            clocks,
            modem_platform_clocks,
            coex_pti,
            phy_grant_protected,
            bluetooth_low_power_clock,
            external_coex,
            coex_participants,
            attachment,
            identity,
        } = self.state;
        let registration = phy.registration_epoch();
        let guard = Guard::new(&identity, registration, FaultReason::Cancelled(operation));
        let phy_guard = Guard::new(&identity, registration, FaultReason::Cancelled(operation));
        RadioTransaction {
            domain: DetachedRadioDomain {
                guard,
                payload: ManuallyDrop::new(ReturnPayload {
                    power,
                    wifi_power_entry,
                    btbb_clients,
                    btbb_gain_generation,
                    clocks,
                    modem_platform_clocks,
                    bluetooth_low_power_clock,
                    external_coex,
                    coex_participants,
                    attachment,
                    identity,
                }),
            },
            phy: PhyTransaction {
                guard: phy_guard,
                payload: ManuallyDrop::new(Payload {
                    hardware: PhyHardware {
                        registers: radio_phy,
                        restore: phy,
                    },
                    coexistence,
                    shared_radio,
                    pti: coex_pti,
                    grant_protected: phy_grant_protected,
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(target_arch = "riscv32")]
impl Default for TransactionSeat {
    fn default() -> Self {
        Self::new()
    }
}
