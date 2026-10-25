//! The analog-I²C bus driver over the generated I²C master and its command
//! memory.
//!
//! The master is one serial resource: one [`AnalogBus`] borrows the whole
//! block, so one owner starts and completes every byte command. Each start
//! refuses, writing nothing, while its host is busy; each completion is one
//! sample of the busy bit. Nothing here waits: the bounded waits and their
//! wake-ups belong to the callers' transitions. The command and reset words
//! and the read-mask word have no class accessor (a write-only lane, a
//! read-only lane the vendor writes ones to); they stay sourced PAC word
//! operations.

#![forbid(unsafe_code)]

use oer_esp32s31_pac::{
    PhyI2cReadMask,
    analog_i2c::{Address, Host, Route},
    i2c_ana_mst::{self, I2cAnaMst},
    phy_i2c_command_ram::{PhyI2cCommandRam, command_memory},
};
use oer_mmio::{Bits, Index, Placement};

use super::commands::{
    BluetoothTxPowerControlRegister, PHY_I2C_COMMAND_MEMORY_DYNAMIC_INDICES,
    PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT, PHY_I2C_COMMAND_MEMORY_TEMPLATE, PhyI2cAccessError,
    PhyI2cCommandMemoryInputs, PhyI2cParallelWrite, normal_route,
};

/// The block that holds the Bluetooth TX-power control registers.
///
/// SOURCE: BLOB_LIBPHY_PHY_I2C. They are BBTOP bytes on host one.
const BLUETOOTH_TX_POWER_CONTROL_BLOCK: u8 = 0x67;

/// The analog-I²C bus over the master's block: the one owner of every byte
/// command for as long as it is borrowed.
pub struct AnalogBus<'a, P: Placement> {
    master: &'a mut I2cAnaMst<P>,
}

impl<'a, P: Placement> AnalogBus<'a, P> {
    /// Borrow the master for a sequence of byte commands.
    pub fn new(master: &'a mut I2cAnaMst<P>) -> Self {
        Self { master }
    }

    /// Install the reviewed host map and return the address's host.
    pub fn configure_and_select_host(&mut self, block: Route) -> Host {
        configure_host_map(self.master);
        block.host
    }

    /// Start one analog-register read; refused while its host is busy.
    pub fn try_start_read(&mut self, address: Address) -> Result<(), PhyI2cAccessError> {
        let route = normal_route(address)?;
        let host = self.configure_and_select_host(route);
        if is_busy(self.master, host) {
            return Err(PhyI2cAccessError::Busy);
        }
        publish_read_mask(self.master, route);
        publish_command(
            self.master,
            host,
            address.block(),
            address.index(),
            0,
            false,
        );
        Ok(())
    }

    /// Consume one externally delivered completion edge of a read.
    pub fn try_finish_read(&self, address: Address) -> Result<u8, PhyI2cAccessError> {
        try_finish_read(self.master, address)
    }

    /// Start one analog-register write; refused while its host is busy.
    pub fn try_start_write(
        &mut self,
        address: Address,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        let host = self.configure_and_select_host(normal_route(address)?);
        if is_busy(self.master, host) {
            return Err(PhyI2cAccessError::Busy);
        }
        publish_command(
            self.master,
            host,
            address.block(),
            address.index(),
            value,
            true,
        );
        Ok(())
    }

    /// Consume one externally delivered completion edge of a write.
    pub fn try_finish_write(&self, address: Address) -> Result<(), PhyI2cAccessError> {
        try_finish_write(self.master, address)
    }

    /// Start one Bluetooth TX-power control read with its block's read mask.
    pub fn start_bluetooth_tx_power_control_read(
        &mut self,
        register: BluetoothTxPowerControlRegister,
    ) -> Result<(), PhyI2cAccessError> {
        configure_host_map(self.master);
        if is_busy(self.master, Host::Host1) {
            return Err(PhyI2cAccessError::Busy);
        }
        publish_read_mask(
            self.master,
            normal_route(oer_esp32s31_pac::analog_i2c::registers::BBTOP_RF_CLOSE_CONTROL)?,
        );
        publish_command(
            self.master,
            Host::Host1,
            BLUETOOTH_TX_POWER_CONTROL_BLOCK,
            register.address(),
            0,
            false,
        );
        Ok(())
    }

    /// Start one Bluetooth TX-power control write.
    pub fn start_bluetooth_tx_power_control_write(
        &mut self,
        register: BluetoothTxPowerControlRegister,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        configure_host_map(self.master);
        if is_busy(self.master, Host::Host1) {
            return Err(PhyI2cAccessError::Busy);
        }
        publish_command(
            self.master,
            Host::Host1,
            BLUETOOTH_TX_POWER_CONTROL_BLOCK,
            register.address(),
            value,
            true,
        );
        Ok(())
    }

    /// Consume one completion edge of a read on `host`.
    pub fn finish_host_read(&self, host: Host) -> Result<u8, PhyI2cAccessError> {
        finish_host_read(self.master, host)
    }

    /// Consume one completion edge of a write on `host`.
    pub fn finish_host_write(&self, host: Host) -> Result<(), PhyI2cAccessError> {
        finish_host_write(self.master, host)
    }

    /// Select the parallel host map of the retained-wake stage two.
    pub fn select_parallel_host_map(&mut self) {
        // SOURCE: BLOB_LIBPHY_PHY_I2C_INIT2_20260907, S31_ESP_PACS_SVD.
        self.master.modify_ana_conf2(|w| {
            w.set_phy_host_map(i2c_ana_mst::ana_conf2::PhyHostMap::ParallelStageTwoMap);
        });
    }

    /// Restore the normal radio host map with one fresh RMW.
    pub fn restore_radio_host_map(&mut self) {
        configure_host_map(self.master);
    }

    /// Start both writes of one parallel pair, host zero first.
    pub fn start_parallel_pair(&mut self, pair: PhyI2cParallelWrite) {
        let (address, value) = pair.first;
        publish_command(
            self.master,
            Host::Host0,
            address.block(),
            address.index(),
            value,
            true,
        );
        let (address, value) = pair.second;
        publish_command(
            self.master,
            Host::Host1,
            address.block(),
            address.index(),
            value,
            true,
        );
    }

    /// Sample the busy bit of one host once.
    pub fn is_busy(&self, host: Host) -> bool {
        is_busy(self.master, host)
    }

    /// Publish the finite reset command of one host.
    ///
    /// The complete ROM parent writes only bit 26 to a busy host and then
    /// polls bit 25; the caller's transition owns that bounded poll.
    pub fn pulse_master_reset(&mut self, host: Host) {
        match host {
            Host::Host0 => self.master.pulse_phy_i2c_host0_reset(),
            Host::Host1 => self.master.pulse_phy_i2c_host1_reset(),
        }
    }
}

/// Install the complete reviewed PHY-I²C host map with one fresh RMW.
fn configure_host_map<P: Placement>(master: &mut I2cAnaMst<P>) {
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_ana_conf2(|w| {
        w.set_phy_host_map(i2c_ana_mst::ana_conf2::PhyHostMap::ReviewedRadioMap);
    });
}

/// Consume one externally delivered completion edge of a read: one sample
/// of its host's busy bit, then its data byte.
pub fn try_finish_read<P: Placement>(
    master: &I2cAnaMst<P>,
    address: Address,
) -> Result<u8, PhyI2cAccessError> {
    finish_host_read(master, normal_route(address)?.host)
}

/// Consume one externally delivered completion edge of a write: one sample
/// of its host's busy bit.
pub fn try_finish_write<P: Placement>(
    master: &I2cAnaMst<P>,
    address: Address,
) -> Result<(), PhyI2cAccessError> {
    finish_host_write(master, normal_route(address)?.host)
}

/// Sample the busy bit of one host once.
pub fn is_busy<P: Placement>(master: &I2cAnaMst<P>, host: Host) -> bool {
    match host {
        Host::Host0 => master.read_i2c0_ctrl().busy(),
        Host::Host1 => master.read_i2c1_ctrl().busy(),
    }
}

fn finish_host_read<P: Placement>(
    master: &I2cAnaMst<P>,
    host: Host,
) -> Result<u8, PhyI2cAccessError> {
    if is_busy(master, host) {
        return Err(PhyI2cAccessError::Busy);
    }
    Ok(match host {
        Host::Host0 => master.read_i2c0_ctrl().data(),
        Host::Host1 => master.read_i2c1_ctrl().data(),
    })
}

fn finish_host_write<P: Placement>(
    master: &I2cAnaMst<P>,
    host: Host,
) -> Result<(), PhyI2cAccessError> {
    if is_busy(master, host) {
        Err(PhyI2cAccessError::Busy)
    } else {
        Ok(())
    }
}

/// Publish the complete complemented read mask used by the vendor leaf.
fn publish_read_mask<P: Placement>(master: &mut I2cAnaMst<P>, block: Route) {
    let Some(mask) = PhyI2cReadMask::new(block.complemented_read_select()) else {
        unreachable!("a complemented read select fits the low 24 bits")
    };
    master.publish_phy_i2c_read_mask(mask);
}

/// Publish one complete host command in the reviewed vendor order.
fn publish_command<P: Placement>(
    master: &mut I2cAnaMst<P>,
    host: Host,
    block: u8,
    register: u8,
    value: u8,
    write: bool,
) {
    match host {
        Host::Host0 => master.publish_phy_i2c_host0_command(block, register, value, write, true),
        Host::Host1 => master.publish_phy_i2c_host1_command(block, register, value, write, true),
    }
}

/// Apply all six timing RMWs of complete rev0 ROM `phy_i2c_clk_sel` in
/// order.
pub fn configure_clock_selection<P: Placement>(master: &mut I2cAnaMst<P>) {
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_i2c0_ctrl1(|w| {
        w.set_sda_side_guard(Bits::constant::<0x2>());
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_i2c0_ctrl1(|w| {
        w.set_scl_pulse_duration(Bits::constant::<0x4>());
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_i2c1_ctrl1(|w| {
        w.set_sda_side_guard(Bits::constant::<0x2>());
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_i2c1_ctrl1(|w| {
        w.set_scl_pulse_duration(Bits::constant::<0x4>());
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_hw_i2c_ctrl(|w| {
        w.set_sda_side_guard(Bits::constant::<0x2>());
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_hw_i2c_ctrl(|w| {
        w.set_scl_pulse_duration(Bits::constant::<0x4>());
    });
}

/// Select register mode two, then enable it with a separate fresh RMW
/// (complete rev0 ROM `phy_i2cmst_reg_init`).
pub fn configure_master_registers<P: Placement>(master: &mut I2cAnaMst<P>) {
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_ana_conf0(|w| {
        w.set_phy_register_mode(i2c_ana_mst::ana_conf0::PhyRegisterMode::RegisterMode);
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_ana_conf0(|w| {
        w.set_phy_register_enable(true);
    });
}

/// Select one of the two complete-ROM BBPLL calibration encodings.
///
/// SOURCE: ROM_REV0_PHY_I2C, BLOB_LIBPHY_PHY_I2C.
pub fn set_bbpll_calibration<P: Placement>(master: &mut I2cAnaMst<P>, enabled: bool) {
    use i2c_ana_mst::ana_conf0::BbpllCalMode;
    let mode = if enabled {
        BbpllCalMode::Enabled
    } else {
        BbpllCalMode::Disabled
    };
    master.modify_ana_conf0(|w| {
        w.set_bbpll_cal_mode(mode);
    });
}

/// Program the complete reviewed 45-entry PHY-I²C command memory.
///
/// SOURCE: BLOB_LIBPHY_PHY_I2C (`phy_i2c_master_cmd_mem_init`). Callers
/// provide only the six retained vendor-parameter facts.
pub fn configure_command_memory<P: Placement>(
    command_ram: &mut PhyI2cCommandRam<P>,
    inputs: PhyI2cCommandMemoryInputs,
) {
    let mut dynamic = PHY_I2C_COMMAND_MEMORY_DYNAMIC_INDICES
        .into_iter()
        .zip(inputs.dynamic_values())
        .peekable();
    for (slot, (address, fixed_value)) in PHY_I2C_COMMAND_MEMORY_TEMPLATE.into_iter().enumerate() {
        let value = match dynamic.next_if(|(index, _)| *index == slot) {
            Some((_, value)) => value,
            None => fixed_value,
        };
        let Ok(index) = Index::<PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT>::new(slot) else {
            unreachable!("the template has one entry per command slot")
        };
        let Some(block) = command_memory::Block::from_bits(u32::from(address.block())) else {
            unreachable!("every template block is a declared command-memory block")
        };
        command_ram.write_command_memory(index, |w| {
            w.set_block(block)
                .set_register(address.index())
                .set_data(value);
        });
    }
}

impl<P: Placement> oer_espressif_analog::AnalogRegisterBus for AnalogBus<'_, P> {
    type Address = Address;

    fn try_start_read(&mut self, address: Address) -> Result<(), oer_espressif_analog::Busy> {
        AnalogBus::try_start_read(self, address).map_err(port_busy)
    }

    fn try_finish_read(&self, address: Address) -> Result<u8, oer_espressif_analog::Busy> {
        AnalogBus::try_finish_read(self, address).map_err(port_busy)
    }

    fn try_start_write(
        &mut self,
        address: Address,
        value: u8,
    ) -> Result<(), oer_espressif_analog::Busy> {
        AnalogBus::try_start_write(self, address, value).map_err(port_busy)
    }

    fn try_finish_write(&self, address: Address) -> Result<(), oer_espressif_analog::Busy> {
        AnalogBus::try_finish_write(self, address).map_err(port_busy)
    }
}

/// The port's refusal of a busy host. Configuration operations use only
/// routed addresses, so an unrouted one is a table defect.
fn port_busy(error: PhyI2cAccessError) -> oer_espressif_analog::Busy {
    match error {
        PhyI2cAccessError::Busy => oer_espressif_analog::Busy,
        PhyI2cAccessError::UnroutedAddress => {
            panic!("a configuration operation uses only admitted analog routes")
        }
    }
}

#[cfg(test)]
mod tests;
