//! S31 byte-transport binding over the family driver and the admitted HAL owner.
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::phy::i2c::{self as hal_phy_i2c, Address};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cError {
    Busy,
    UnroutedAddress,
}

/// Publish one complete-register PHY-I2C read without waiting for completion.
///
/// Unlike the ROM read leaf, this function also rejects an already-busy host
/// before publishing the command. This is a deliberate fail-fast ownership
/// check, not a claim that the ROM performed the same pre-command check.
///
/// The caller must keep borrowing the same shared-PHY register owner until
/// [`try_finish_read`] succeeds.
#[cfg(target_arch = "riscv32")]
pub fn try_start_read(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    address: Address,
) -> Result<(), PhyI2cError> {
    oer_espressif_phy::serial::AnalogBus::new(hal_phy_i2c::serial_port(registers))
        .try_start_read(address)
        .map_err(|error| match error {
            oer_espressif_phy::serial::PhyI2cAccessError::Busy => PhyI2cError::Busy,
            oer_espressif_phy::serial::PhyI2cAccessError::UnroutedAddress => {
                PhyI2cError::UnroutedAddress
            }
        })
}

/// Observe one previously published PHY-I2C read exactly once.
///
/// The caller may invoke this once after an independently delivered hardware
/// or timer completion edge. `Busy` is then an incomplete/timeout result; it
/// must not be converted into a self-waking retry loop. This function never
/// loops, delays, or schedules itself.
///
/// `address` must name the in-flight command started by [`try_start_read`]
/// under the same borrowed radio ownership.
#[cfg(target_arch = "riscv32")]
pub fn try_finish_read(
    registers: &impl oer_esp32s31_hal::owner::SharedPhyAccess,
    address: Address,
) -> Result<u8, PhyI2cError> {
    oer_espressif_phy::serial::try_finish_read(&hal_phy_i2c::serial_sample(registers), address)
        .map_err(|error| match error {
            oer_espressif_phy::serial::PhyI2cAccessError::Busy => PhyI2cError::Busy,
            oer_espressif_phy::serial::PhyI2cAccessError::UnroutedAddress => {
                PhyI2cError::UnroutedAddress
            }
        })
}

/// Publish one complete-register PHY-I2C write after observing the
/// pre-command busy state once. It never waits or loops on that state and
/// leaves post-command completion to [`try_finish_write`].
///
/// The caller must keep borrowing the same shared-PHY register owner until
/// [`try_finish_write`] succeeds.
#[cfg(target_arch = "riscv32")]
pub fn try_start_write(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    address: Address,
    value: u8,
) -> Result<(), PhyI2cError> {
    oer_espressif_phy::serial::AnalogBus::new(hal_phy_i2c::serial_port(registers))
        .try_start_write(address, value)
        .map_err(|error| match error {
            oer_espressif_phy::serial::PhyI2cAccessError::Busy => PhyI2cError::Busy,
            oer_espressif_phy::serial::PhyI2cAccessError::UnroutedAddress => {
                PhyI2cError::UnroutedAddress
            }
        })
}

/// Observe one previously published PHY-I2C write exactly once.
///
/// The caller may invoke this once after an independently delivered hardware
/// or timer completion edge. `Busy` is an incomplete/timeout result and must
/// not be converted into a self-waking retry loop.
///
/// `address` must name the in-flight command started by [`try_start_write`]
/// under the same borrowed radio ownership.
#[cfg(target_arch = "riscv32")]
pub fn try_finish_write(
    registers: &impl oer_esp32s31_hal::owner::SharedPhyAccess,
    address: Address,
) -> Result<(), PhyI2cError> {
    oer_espressif_phy::serial::try_finish_write(&hal_phy_i2c::serial_sample(registers), address)
        .map_err(|error| match error {
            oer_espressif_phy::serial::PhyI2cAccessError::Busy => PhyI2cError::Busy,
            oer_espressif_phy::serial::PhyI2cAccessError::UnroutedAddress => {
                PhyI2cError::UnroutedAddress
            }
        })
}
