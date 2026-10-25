//! Hardware ownership, capabilities, and affine lifecycle transitions.

use crate::generated::{
    ieee802154_mac::Ieee802154Mac,
    peripheral_ownership::{
        self, Claims, PeripheralPartitions, WifiInterruptPeripherals, WifiMacPeripherals,
    },
};

/// Physical owners used by one IEEE 802.15.4 task register set.
///
/// The MAC and its ETM channels are the whole IEEE 802.15.4 authority; its
/// interrupt route word is the interrupt matrix's. Its clocks and resets
/// belong to the modem clock owner; the common PHY it depends on is not
/// modelled by this crate yet.
pub(crate) struct Ieee802154TaskPeripheralOwners {
    pub(crate) ieee802154_mac: crate::ieee802154::ownership::TaskRegisters,
    pub(crate) etm: crate::modem::etm::Ieee802154EtmChannels,
}

/// Opaque IEEE 802.15.4 MAC register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct Ieee802154Partition {
    mac: Ieee802154Mac,
    etm: crate::modem::etm::Ieee802154EtmChannels,
}

/// Opaque Wi-Fi MAC register partition.
///
/// No driver owns it yet: it only reserves the reviewed Wi-Fi MAC register
/// views so that no other partition can reach them.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiMacPartition(#[allow(dead_code)] WifiMacPeripherals);

/// Opaque Wi-Fi MAC and power interrupt bank partition.
///
/// No driver owns it yet: it only reserves the two interrupt banks.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiInterruptPartition(#[allow(dead_code)] WifiInterruptPeripherals);

/// Proof that the platform transferred every ESP32-C5 radio singleton.
#[must_use = "the radio root needs the consumed platform ownership"]
pub struct RadioHandoff(());

impl RadioHandoff {
    /// Assert the platform's unique, permanent radio ownership handoff.
    ///
    /// # Safety
    /// Call at most once. The caller consumed the platform's analog-I2C,
    /// modem clock, Wi-Fi, Bluetooth and IEEE 802.15.4 singletons, and no
    /// other code may access their radio register partitions thereafter.
    #[allow(
        unsafe_code,
        reason = "the platform proves exclusive radio MMIO ownership"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// Every reviewed ESP32-C5 radio register partition, from the platform handoff.
///
/// ```compile_fail,E0061
/// use oer_esp32c5_pac::RadioPartitions;
/// let partitions = RadioPartitions::from_handoff();
/// ```
///
/// ```compile_fail,E0382
/// use oer_esp32c5_pac::{RadioHandoff, RadioPartitions};
/// fn duplicate(handoff: RadioHandoff) {
///     let first = RadioPartitions::from_handoff(handoff);
///     let second = RadioPartitions::from_handoff(handoff);
/// }
/// ```
#[must_use = "dropping the radio partitions permanently loses the unique hardware capability"]
pub struct RadioPartitions {
    pub ieee802154: Ieee802154Partition,
    pub modem_clock: crate::modem::clock::ModemClockRegisters,
    pub phy_i2c: crate::phy::i2c::PhyI2cRegisters,
    pub phy_radio: crate::phy::radio::PhyRadioRegisters,
    pub wifi_mac: WifiMacPartition,
    pub wifi_interrupts: WifiInterruptPartition,
}

impl RadioPartitions {
    /// Adopt the platform's consumed radio owners without a second singleton.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: RadioHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the radio root claims the typed blocks once, from the handoff"
        )]
        // SAFETY: `RadioHandoff`'s constructor guarantees exclusive, permanent
        // ownership of every radio register partition, so no other code
        // accesses the words the typed blocks list. The generated `const`
        // assertion beside `Claims` checks that no two blocks list the same
        // word; the unexposed observation views own no words. Words esp-hal
        // also writes are reviewed with their coordination in
        // `registers/shared-words.toml`.
        let claims = unsafe { oer_mmio::assume_owned::<Claims>() };
        Self::from_claims(claims)
    }

    /// Bind the claimed typed blocks to the opaque partition owners.
    pub(crate) fn from_claims(claims: Claims) -> Self {
        let PeripheralPartitions {
            ieee802154,
            modem_etm,
            modem_clock,
            phy_i2c,
            phy_radio,
            wifi_mac,
            wifi_interrupts,
        } = peripheral_ownership::partition(claims);
        Self {
            ieee802154: Ieee802154Partition {
                mac: ieee802154.ieee802154_mac,
                etm: crate::modem::etm::ieee802154_channels(modem_etm),
            },
            modem_clock: crate::modem::clock::ModemClockRegisters::new(modem_clock),
            phy_i2c: crate::phy::i2c::PhyI2cRegisters::new(phy_i2c),
            phy_radio: crate::phy::radio::PhyRadioRegisters::new(phy_radio),
            wifi_mac: WifiMacPartition(wifi_mac),
            wifi_interrupts: WifiInterruptPartition(wifi_interrupts),
        }
    }

    /// Construct every partition inside one isolated validation image.
    #[cfg(any(test, oer_probe))]
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "a host-test root is fictitious; each test needs its own fresh claim"
    )]
    pub fn for_validation() -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "a validation root claims the typed blocks once"
        )]
        // SAFETY: only host tests and validation probes build this root. In
        // a host test it is fictitious: no device register exists behind it
        // and no test accesses one, so the claim need not be unique; a probe
        // image claims the blocks once.
        let claims = unsafe { oer_mmio::assume_owned::<Claims>() };
        Self::from_claims(claims)
    }
}

#[inline]
pub(crate) fn device_fence() {
    oer_mmio::fence();
}

/// Ordinary task-side owner of the IEEE 802.15.4 MAC partition.
#[must_use = "the IEEE 802.15.4 task owner must be reunited before release"]
pub struct Ieee802154TaskRegisters {
    pub(crate) peripherals: Ieee802154TaskPeripheralOwners,
}

impl Ieee802154TaskRegisters {
    /// Assemble the task register set and split the MAC block into its
    /// disjoint task and interrupt owners. This performs no MMIO.
    pub fn new(partition: Ieee802154Partition) -> (Self, Ieee802154InterruptSetup) {
        let Ieee802154Partition { mac, etm } = partition;
        let (task_mac, interrupt_mac) = crate::ieee802154::ownership::split(mac);
        (
            Self {
                peripherals: Ieee802154TaskPeripheralOwners {
                    ieee802154_mac: task_mac,
                    etm,
                },
            },
            Ieee802154InterruptSetup {
                registers: interrupt_mac,
            },
        )
    }

    /// Reunite the MAC block with its inactive interrupt owner and return the
    /// partition. This performs no MMIO.
    pub fn into_partition(self, interrupts: Ieee802154InterruptSetup) -> Ieee802154Partition {
        let Ieee802154TaskPeripheralOwners {
            ieee802154_mac: task_mac,
            etm,
        } = self.peripherals;
        Ieee802154Partition {
            mac: crate::ieee802154::ownership::reunite(task_mac, interrupts.registers),
            etm,
        }
    }

    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        device_fence();
    }
}

/// Task-side setup token before one IEEE 802.15.4 hard-IRQ epoch.
#[must_use = "the IEEE 802.15.4 interrupt setup must remain paired with its task owner"]
pub struct Ieee802154InterruptSetup {
    pub(crate) registers: crate::ieee802154::ownership::InterruptRegisters,
}

/// Disjoint IEEE 802.15.4 event/status capability for the hard ISR.
#[must_use = "the IEEE 802.15.4 interrupt owner must be deactivated and reunited"]
pub struct Ieee802154InterruptRegisters {
    pub(crate) registers: crate::ieee802154::ownership::InterruptRegisters,
}

#[cfg(test)]
pub(crate) mod test_support;
