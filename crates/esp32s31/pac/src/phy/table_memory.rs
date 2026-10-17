//! Ownership-bound access to the shared PHY table-memory aperture.

#![forbid(unsafe_code)]

use crate::generated::{
    PbusMemoryGroupBoundaryInput, PhyForcedPowerState, PhyGainMemoryCommandInput,
    PhyGeneratedReceiveGainData0, PhyGeneratedReceiveGainData1, PhyGeneratedReceiveGainData2,
    PhyMemoryIndex, PhyPbusMemoryCommand, PhyReceiveTableGainData1, PhyReceiveTableGainData2,
    PhyTransmitGainData0, PhyTransmitGainData1, PhyTransmitGainData2,
};
use crate::{PhyForcedPowerIndex, RadioPhyRegisters};

/// One packed PBUS-memory group boundary.
///
/// Complete rev0 ROM `phy_write_pbus_mem` packs groups 0..11 into six
/// generated `GROUP_BOUNDARY` register words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PbusMemoryGroupBoundary {
    pub group: u8,
    pub first_entry: u8,
    pub last_entry: u8,
}

/// One complete gain-memory transaction encoded only inside the PAC.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PhyGainMemoryEntry {
    words: PhyGainMemoryWords,
    index: u8,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PhyGainMemoryWords {
    GeneratedReceive {
        data_0: PhyGeneratedReceiveGainData0,
        data_1: PhyGeneratedReceiveGainData1,
        data_2: PhyGeneratedReceiveGainData2,
    },
    ReceiveTable {
        data_1: PhyReceiveTableGainData1,
        data_2: PhyReceiveTableGainData2,
    },
    Transmit {
        data_0: PhyTransmitGainData0,
        data_1: PhyTransmitGainData1,
        data_2: PhyTransmitGainData2,
    },
}

impl core::fmt::Debug for PhyGainMemoryEntry {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("PhyGainMemoryEntry(..)")
    }
}

impl PhyGainMemoryEntry {
    /// Encode one generated receive-gain entry from semantic calibration data.
    ///
    /// `dc` contains the I and Q calibration components in that order.
    pub const fn generated_receive(
        index: u8,
        dc: [u16; 2],
        index_dc: [u16; 2],
        auxiliary: u16,
        encoded_gain: u32,
        mixer_digital_gain: u8,
        parameter_002: u8,
    ) -> Self {
        let [dc_i, dc_q] = dc;
        Self {
            words: PhyGainMemoryWords::GeneratedReceive {
                data_0: PhyGeneratedReceiveGainData0::compose(dc_i, dc_q, index_dc[1], auxiliary),
                data_1: PhyGeneratedReceiveGainData1::compose(
                    encoded_gain,
                    encoded_gain,
                    index_dc[0],
                    dc_i,
                    mixer_digital_gain,
                ),
                data_2: PhyGeneratedReceiveGainData2::compose(
                    parameter_002,
                    encoded_gain,
                    encoded_gain,
                ),
            },
            index,
        }
    }

    /// Encode one fixed-form receive-table initialization entry.
    pub const fn receive_table(index: u8, parameter_002: u8) -> Self {
        Self {
            words: PhyGainMemoryWords::ReceiveTable {
                data_1: PhyReceiveTableGainData1::compose(parameter_002),
                data_2: PhyReceiveTableGainData2::compose(parameter_002),
            },
            index,
        }
    }

    /// Encode one transmit-gain entry from the reviewed table components.
    pub const fn transmit(
        index: u8,
        gain_72: u16,
        gain_64: u16,
        gain_32: u8,
        seed: [u16; 4],
        config: u16,
    ) -> Self {
        let [seed_0, seed_1, seed_2, seed_3] = seed;
        Self {
            words: PhyGainMemoryWords::Transmit {
                data_0: PhyTransmitGainData0::compose(config, seed_2, seed_1, seed_3),
                data_1: PhyTransmitGainData1::compose(seed_0, seed_1, gain_64, gain_72, gain_64),
                data_2: PhyTransmitGainData2::compose(gain_72, gain_72, gain_32),
            },
            index,
        }
    }
}

/// A shared PHY-memory operation did not fit the recovered layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyMemoryError {
    /// Complete ROM `phy_set_pbus_mem` constructs exactly twelve groups.
    PbusGroupOutOfRange,
    /// Complete ROM `phy_write_pbus_mem` publishes a ten-bit command.
    PbusCommandOutOfRange,
}

const fn forced_power_state(enabled: bool) -> PhyForcedPowerState {
    if enabled {
        PhyForcedPowerState::Enabled
    } else {
        PhyForcedPowerState::Disabled
    }
}

impl RadioPhyRegisters {
    /// Restore one packed PBUS-memory group boundary without rewriting table
    /// memory.
    pub fn configure_pbus_memory_group_boundary(
        &mut self,
        boundary: PbusMemoryGroupBoundary,
    ) -> Result<(), PhyMemoryError> {
        let value =
            PbusMemoryGroupBoundaryInput::compose(boundary.first_entry, boundary.last_entry);
        match boundary.group {
            0 => crate::generated::configure_even_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                0,
                value,
            ),
            1 => crate::generated::configure_odd_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                0,
                value,
            ),
            2 => crate::generated::configure_even_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                1,
                value,
            ),
            3 => crate::generated::configure_odd_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                1,
                value,
            ),
            4 => crate::generated::configure_even_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                2,
                value,
            ),
            5 => crate::generated::configure_odd_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                2,
                value,
            ),
            6 => crate::generated::configure_even_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                3,
                value,
            ),
            7 => crate::generated::configure_odd_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                3,
                value,
            ),
            8 => crate::generated::configure_even_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                4,
                value,
            ),
            9 => crate::generated::configure_odd_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                4,
                value,
            ),
            10 => crate::generated::configure_even_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                5,
                value,
            ),
            11 => crate::generated::configure_odd_pbus_memory_group_boundary(
                &self.peripherals.phy_memory,
                5,
                value,
            ),
            _ => return Err(PhyMemoryError::PbusGroupOutOfRange),
        }
        Ok(())
    }

    /// Publish one PBUS-memory entry in complete rev0 ROM access order.
    pub fn program_pbus_memory_entry(
        &mut self,
        boundary: Option<PbusMemoryGroupBoundary>,
        data: u32,
        command: u16,
    ) -> Result<(), PhyMemoryError> {
        let command = PhyPbusMemoryCommand::new(u32::from(command))
            .ok_or(PhyMemoryError::PbusCommandOutOfRange)?;

        if let Some(boundary) = boundary {
            self.configure_pbus_memory_group_boundary(boundary)?;
        }

        crate::generated::phy_memory_data_0(
            &self.peripherals.phy_memory,
            crate::generated::PhyMemoryData0::new(data),
        );
        crate::generated::publish_pbus_memory_command(&self.peripherals.phy_memory, command);
        Ok(())
    }

    /// Sample the shared CFR/gain-memory base index exactly once.
    pub fn read_table_memory_base_index(&self) -> u8 {
        crate::svd::field_read::observe_table_memory_base_index(&self.peripherals.phy_clock_oracle)
    }

    /// Configure the shared CFR/gain-memory base index through one fresh RMW.
    pub fn configure_table_memory_base_index(&mut self, index: u8) {
        crate::generated::configure_table_memory_base_index(
            &self.peripherals.phy_clock_oracle,
            PhyMemoryIndex::new(u32::from(index))
                .expect("u8 always fits the reviewed PHY memory-index domain"),
        );
    }

    /// Apply complete rev0 ROM `phy_force_pwr_index` through its two ordered
    /// fresh-read field replacements.
    pub fn configure_forced_power_index(&mut self, enabled: bool, index: PhyForcedPowerIndex) {
        let control = &self.peripherals.phy_clock_oracle;
        crate::generated::configure_forced_power_state(control, forced_power_state(enabled));
        crate::generated::configure_forced_power_index(control, index);
    }

    /// Publish one TX-CFR entry and its complete set/clear commit pulse.
    pub fn program_tx_cfr_entry(&mut self, data: u32, index: u8) {
        crate::generated::phy_memory_data_0(
            &self.peripherals.phy_memory,
            crate::generated::PhyMemoryData0::new(data),
        );
        let memory = &self.peripherals.phy_memory;
        crate::generated::configure_tx_cfr_memory_index(
            memory,
            PhyMemoryIndex::new(u32::from(index))
                .expect("u8 always fits the reviewed PHY memory-index domain"),
        );
        crate::generated::set_tx_cfr_commit(memory);
        crate::generated::clear_tx_cfr_commit(memory);
    }

    /// Publish one three-word gain-memory entry in complete ROM order.
    pub fn program_gain_memory_entry(&mut self, entry: PhyGainMemoryEntry) {
        let memory = &self.peripherals.phy_memory;
        match entry.words {
            PhyGainMemoryWords::GeneratedReceive {
                data_0,
                data_1,
                data_2,
            } => {
                crate::generated::publish_generated_receive_gain_data_0(memory, data_0);
                crate::generated::publish_generated_receive_gain_data_1(memory, data_1);
                crate::generated::publish_generated_receive_gain_data_2(memory, data_2);
            }
            PhyGainMemoryWords::ReceiveTable { data_1, data_2 } => {
                crate::svd::fixed_register_image::publish_receive_table_gain_data_0(memory);
                crate::generated::publish_receive_table_gain_data_1(memory, data_1);
                crate::generated::publish_receive_table_gain_data_2(memory, data_2);
            }
            PhyGainMemoryWords::Transmit {
                data_0,
                data_1,
                data_2,
            } => {
                crate::generated::publish_transmit_gain_data_0(memory, data_0);
                crate::generated::publish_transmit_gain_data_1(memory, data_1);
                crate::generated::publish_transmit_gain_data_2(memory, data_2);
            }
        }

        crate::generated::publish_gain_memory_command(
            memory,
            PhyGainMemoryCommandInput::compose(entry.index),
        );
    }
}
