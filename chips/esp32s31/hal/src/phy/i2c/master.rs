//! Owned master atoms and the legacy calibration bus binding.
//! Serial-byte sequencing belongs to `oer_espressif_phy::serial`.
#![forbid(unsafe_code)]
use super::commands::{
    PHY_I2C_COMMAND_MEMORY_DYNAMIC_INDICES, PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT,
    PHY_I2C_COMMAND_MEMORY_TEMPLATE, PhyI2cCommandMemoryInputs,
};
use oer_esp32s31_pac::{
    analog_i2c::{Address, Host, Profile, Route},
    i2c_ana_mst::I2cAnaMst,
    i2c_ana_mst_mem::PhyI2cCommandRam,
};
use oer_espressif_phy::serial::{AnalogBus, PhyI2cAccessError, Port, SamplePort};
use oer_mmio::{Bits, Index, Placement};

/// Exclusive atom loan from the master owner.
pub struct MasterPort<'a, P: Placement> {
    master: &'a mut I2cAnaMst<P>,
}
impl<'a, P: Placement> MasterPort<'a, P> {
    pub fn new(master: &'a mut I2cAnaMst<P>) -> Self {
        Self { master }
    }
    pub fn pulse_master_reset(&mut self, host: Host) {
        match host {
            Host::Host0 => self.master.write_i2c0_ctrl(|w| {
                w.set_start_or_reset(true);
            }),
            Host::Host1 => self.master.write_i2c1_ctrl(|w| {
                w.set_start_or_reset(true);
            }),
        }
    }
}

/// Read-only observations of the same owned master, with no command authority.
pub struct MasterSample<'a, P: Placement> {
    master: &'a I2cAnaMst<P>,
}
impl<'a, P: Placement> MasterSample<'a, P> {
    pub fn new(master: &'a I2cAnaMst<P>) -> Self {
        Self { master }
    }
}
/// An address admitted by this master's production routing dictionary.
/// Its fields are private so a route from a fake port cannot command this master.
///
/// ```compile_fail
/// use oer_esp32s31_hal::phy::i2c::{Address, Route, master::MasterAddress};
/// fn forge(address: Address, route: Route) -> MasterAddress {
///     MasterAddress { address, route }
/// }
/// ```
#[derive(Clone, Copy)]
pub struct MasterAddress {
    address: Address,
    route: Route,
}
fn route(address: Address) -> Option<MasterAddress> {
    oer_esp32s31_pac::analog_i2c::route(Profile::LibphyNormal, address.block())
        .map(|route| MasterAddress { address, route })
}
fn data<P: Placement>(master: &I2cAnaMst<P>, host: Host) -> u8 {
    match host {
        Host::Host0 => master.read_i2c0_ctrl().data(),
        Host::Host1 => master.read_i2c1_ctrl().data(),
    }
}
impl<P: Placement> SamplePort for MasterSample<'_, P> {
    type Address = MasterAddress;
    fn route(&self, address: Address) -> Option<MasterAddress> {
        route(address)
    }
    fn host(address: MasterAddress) -> Host {
        address.route.host
    }
    fn is_busy(&self, host: Host) -> bool {
        is_busy(self.master, host)
    }
    fn data(&self, host: Host) -> u8 {
        data(self.master, host)
    }
}
impl<P: Placement> SamplePort for MasterPort<'_, P> {
    type Address = MasterAddress;
    fn route(&self, address: Address) -> Option<MasterAddress> {
        route(address)
    }
    fn host(address: MasterAddress) -> Host {
        address.route.host
    }
    fn is_busy(&self, host: Host) -> bool {
        is_busy(self.master, host)
    }
    fn data(&self, host: Host) -> u8 {
        data(self.master, host)
    }
}
impl<P: Placement> Port for MasterPort<'_, P> {
    fn configure_host_map(&mut self) {
        configure_host_map(self.master);
    }
    fn publish_read_mask(&mut self, address: MasterAddress) {
        publish_read_mask(self.master, address.route);
    }
    fn publish_command(&mut self, address: MasterAddress, value: u8, write: bool) {
        publish_command(
            self.master,
            address.route.host,
            address.address.block(),
            address.address.index(),
            value,
            write,
        );
    }
}
impl<P: Placement> oer_espressif_phy::i2c::PhyI2cParallelAccess for MasterPort<'_, P> {
    type Pair = oer_espressif_phy::i2c::PhyI2cParallelWrite<Address>;
    fn is_busy(&self, host: Host) -> bool {
        is_busy(self.master, host)
    }
    /// Select the parallel host map of the retained-wake stage two.
    fn select_parallel_host_map(&mut self) {
        // SOURCE: BLOB_LIBPHY_PHY_I2C_INIT2_20260907, S31_ESP_PACS_SVD.
        self.master.modify_ana_conf2(|w| {
            w.set_bbtop_mst0_select(false)
                .set_txrf_mst0_select(true)
                .set_sdm_mst0_select(false)
                .set_rfpll_mst0_select(true)
                .set_bias_mst0_select(false)
                .set_bbpll_mst0_select(false)
                .set_ulp_bod_mst0_select(false)
                .set_bits_11_12_opaque(Bits::constant::<0>())
                .set_saradc_mst0_select(false)
                .set_bits_14_16_opaque(Bits::constant::<0>())
                .set_dig_reg_mst0_select(false);
        });
    }

    /// Restore the normal radio host map with one fresh RMW.
    fn restore_radio_host_map(&mut self) {
        configure_host_map(self.master);
    }

    /// Start both writes of one parallel pair, host zero first.
    fn start_pair(&mut self, pair: oer_espressif_phy::i2c::PhyI2cParallelWrite<Address>) {
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
}
/// Install the complete reviewed PHY-I²C host map with one fresh RMW.
fn configure_host_map<P: Placement>(master: &mut I2cAnaMst<P>) {
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_ana_conf2(|w| {
        w.set_bbtop_mst0_select(false)
            .set_txrf_mst0_select(false)
            .set_sdm_mst0_select(false)
            .set_rfpll_mst0_select(false)
            .set_bias_mst0_select(false)
            .set_bbpll_mst0_select(true)
            .set_ulp_bod_mst0_select(false)
            .set_bits_11_12_opaque(Bits::constant::<3>())
            .set_saradc_mst0_select(true)
            .set_bits_14_16_opaque(Bits::constant::<7>())
            .set_dig_reg_mst0_select(true);
    });
}

/// Sample the busy bit of one host once.
pub fn is_busy<P: Placement>(master: &I2cAnaMst<P>, host: Host) -> bool {
    match host {
        Host::Host0 => master.read_i2c0_ctrl().i2c0_busy(),
        Host::Host1 => master.read_i2c1_ctrl().i2c1_busy(),
    }
}

/// Publish the complete complemented read mask used by the vendor leaf.
fn publish_read_mask<P: Placement>(master: &mut I2cAnaMst<P>, block: Route) {
    // SOURCE: ROM_REV0_PHY_I2C, BLOB_LIBPHY_PHY_I2C.
    master.publish_phy_i2c_read_mask(Bits::truncate(block.complemented_read_select()));
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
        Host::Host0 => master.write_i2c0_ctrl(|w| {
            w.set_slave_addr(block)
                .set_slave_reg_addr(register)
                .set_data(value)
                .set_read_write(write)
                .set_start_or_reset(true);
        }),
        Host::Host1 => master.write_i2c1_ctrl(|w| {
            w.set_slave_addr(block)
                .set_slave_reg_addr(register)
                .set_data(value)
                .set_read_write(write)
                .set_start_or_reset(true);
        }),
    }
}

/// Apply all six timing RMWs of complete rev0 ROM `phy_i2c_clk_sel` in
/// order.
pub fn configure_clock_selection<P: Placement>(master: &mut I2cAnaMst<P>) {
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_i2c0_ctrl1(|w| {
        w.set_i2c0_sda_side_guard(Bits::constant::<0x2>());
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_i2c0_ctrl1(|w| {
        w.set_i2c0_scl_pulse_dur(Bits::constant::<0x4>());
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_i2c1_ctrl1(|w| {
        w.set_i2c1_sda_side_guard(Bits::constant::<0x2>());
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_i2c1_ctrl1(|w| {
        w.set_i2c1_scl_pulse_dur(Bits::constant::<0x4>());
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_hw_i2c_ctrl(|w| {
        w.set_hw_i2c_sda_side_guard(Bits::constant::<0x2>());
    });
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_hw_i2c_ctrl(|w| {
        w.set_hw_i2c_scl_pulse_dur(Bits::constant::<0x4>());
    });
}

/// Select register mode two, then enable it with a separate fresh RMW
/// (complete rev0 ROM `phy_i2cmst_reg_init`).
pub fn configure_master_registers<P: Placement>(master: &mut I2cAnaMst<P>) {
    // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
    master.modify_ana_conf0(|w| {
        w.set_phy_register_mode(Bits::constant::<2>());
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
    use oer_esp32s31_pac::i2c_ana_mst::ana_conf0::BbpllCalMode;
    let mode = if enabled {
        BbpllCalMode::Enabled
    } else {
        BbpllCalMode::Disabled
    };
    master.modify_ana_conf0(|w| {
        w.set_bbpll_cal_mode(Bits::truncate(mode.bits()));
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
        command_ram.write_command_memory(index, |w| {
            w.set_block(address.block())
                .set_register(address.index())
                .set_data(value);
        });
    }
}

/// Binding for the calibration machines that still consume the analog transition port.
/// The admitted command tables and its existing table-defect assertion remain here.
pub struct ConfigurationBus<'a, P: Placement>(AnalogBus<MasterPort<'a, P>>);
impl<'a, P: Placement> ConfigurationBus<'a, P> {
    pub(crate) fn new(master: &'a mut I2cAnaMst<P>) -> Self {
        Self(AnalogBus::new(MasterPort::new(master)))
    }
}
impl<P: Placement> oer_espressif_analog::AnalogRegisterBus for ConfigurationBus<'_, P> {
    type Address = Address;
    fn try_start_read(&mut self, address: Address) -> Result<(), oer_espressif_analog::Busy> {
        self.0.try_start_read(address).map_err(port_busy)
    }
    fn try_finish_read(&self, address: Address) -> Result<u8, oer_espressif_analog::Busy> {
        self.0.try_finish_read(address).map_err(port_busy)
    }
    fn try_start_write(
        &mut self,
        address: Address,
        value: u8,
    ) -> Result<(), oer_espressif_analog::Busy> {
        self.0.try_start_write(address, value).map_err(port_busy)
    }
    fn try_finish_write(&self, address: Address) -> Result<(), oer_espressif_analog::Busy> {
        self.0.try_finish_write(address).map_err(port_busy)
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
