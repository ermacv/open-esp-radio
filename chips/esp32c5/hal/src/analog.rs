//! The ESP32-C5 analog register bus of the PHY.
//!
//! [`AnalogI2c`] implements the chip-neutral
//! [`oer_espressif_analog::AnalogRegisterBus`] over the PAC's analog I2C master
//! owner, whose addresses carry the libphy host map, host set, read masks and
//! block alias.

use oer_esp32c5_pac::{
    PhyI2cAccessError, PhyI2cAddress, PhyI2cConfigurationCommand, PhyI2cRegisters,
};
use oer_espressif_analog::{
    AnalogField, AnalogRegisterBus, Busy, Configuration, ConfigurationCommand, ParallelAnalogBus,
    ParallelHost, ParallelWrites,
};

pub use oer_esp32c5_pac::{
    PhyI2cBlock, PhyI2cClockSelection, PhyI2cConfiguration, PhyI2cHost, PhyI2cInitializationInputs,
    PhyI2cParallelWrite, PhyI2cRcCalibration, PhyI2cSar2Code,
};

/// Unique owner of the ESP32-C5 analog register bus.
#[must_use = "dropping the analog register bus loses the analog I2C master"]
pub struct AnalogI2c {
    registers: PhyI2cRegisters,
}

impl AnalogI2c {
    pub const fn new(registers: PhyI2cRegisters) -> Self {
        Self { registers }
    }

    pub fn into_registers(self) -> PhyI2cRegisters {
        self.registers
    }
}

const fn busy(_: PhyI2cAccessError) -> Busy {
    Busy
}

impl AnalogRegisterBus for AnalogI2c {
    type Address = PhyI2cAddress;

    fn try_start_read(&mut self, address: PhyI2cAddress) -> Result<(), Busy> {
        self.registers.try_start_read(address).map_err(busy)
    }

    fn try_finish_read(&self, address: PhyI2cAddress) -> Result<u8, Busy> {
        self.registers.try_finish_read(address).map_err(busy)
    }

    fn try_start_write(&mut self, address: PhyI2cAddress, value: u8) -> Result<(), Busy> {
        self.registers.try_start_write(address, value).map_err(busy)
    }

    fn try_finish_write(&self, address: PhyI2cAddress) -> Result<(), Busy> {
        self.registers.try_finish_write(address).map_err(busy)
    }
}

impl ParallelAnalogBus for AnalogI2c {
    type Pair = PhyI2cParallelWrite;

    fn select_parallel_host_map(&mut self) {
        self.registers.select_parallel_host_map();
    }

    fn restore_host_map(&mut self) {
        self.registers.restore_host_map();
    }

    fn start_pair(&mut self, pair: PhyI2cParallelWrite) {
        self.registers.start_parallel_pair(pair);
    }

    fn is_busy(&self, host: ParallelHost) -> bool {
        self.registers.is_busy(match host {
            ParallelHost::First => PhyI2cHost::Host0,
            ParallelHost::Second => PhyI2cHost::Host1,
        })
    }
}

/// The parallel analog I2C initialization `phy_i2c_init1` performs, over
/// the `phy_param` fields it reads; poll it on an [`AnalogI2c`].
pub fn initialization(
    inputs: PhyI2cInitializationInputs,
) -> ParallelWrites<impl Fn(usize) -> Option<PhyI2cParallelWrite>> {
    ParallelWrites::new(move |index| inputs.pair(index))
}

fn configuration_command(
    command: PhyI2cConfigurationCommand,
) -> Option<ConfigurationCommand<PhyI2cAddress>> {
    Some(match command {
        PhyI2cConfigurationCommand::Write(address, value) => {
            ConfigurationCommand::Write(address, value)
        }
        PhyI2cConfigurationCommand::Modify {
            address,
            msb,
            lsb,
            value,
        } => ConfigurationCommand::Modify(AnalogField::new(address, msb, lsb)?, value),
    })
}

/// A vendor analog configuration leaf; poll it on an [`AnalogI2c`].
pub fn configuration(
    leaf: PhyI2cConfiguration,
) -> Configuration<impl Fn(usize) -> Option<ConfigurationCommand<PhyI2cAddress>>, PhyI2cAddress> {
    Configuration::new(move |index| leaf.command(index).and_then(configuration_command))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAVES: [PhyI2cConfiguration; 10] = [
        PhyI2cConfiguration::Band,
        PhyI2cConfiguration::CrystalRegisters,
        PhyI2cConfiguration::DacRate,
        PhyI2cConfiguration::AdcRate(false),
        PhyI2cConfiguration::AdcRate(true),
        PhyI2cConfiguration::BiasRegisters,
        PhyI2cConfiguration::PeakDetector,
        PhyI2cConfiguration::FilterCapacitors(PhyI2cInitializationInputs {
            parameter_f5: 0xff,
            parameter_f6: 0xff,
            parameter_f7: 0xff,
            parameter_f8: 0xff,
            parameter_f9: 0xff,
            parameter_fa: 0xff,
            parameter_fb: 0xff,
            parameter_fc: 0xff,
            parameter_410: 0,
            parameter_412: 0,
            parameter_416: 0,
        }),
        PhyI2cConfiguration::RcCalibration(match PhyI2cRcCalibration::new(3, 31, 15) {
            Some(arguments) => arguments,
            None => panic!("largest arguments"),
        }),
        PhyI2cConfiguration::Sar2InitializationCode(match PhyI2cSar2Code::new(0xfff) {
            Some(code) => code,
            None => panic!("largest code"),
        }),
    ];

    /// Every command of every leaf maps to a portable command whose value
    /// fits its field, so no configuration stops on an invalid command.
    #[test]
    fn every_configuration_command_is_a_valid_portable_command() {
        for leaf in LEAVES {
            let mut index = 0;
            while let Some(command) = leaf.command(index) {
                match configuration_command(command) {
                    Some(ConfigurationCommand::Modify(field, value)) => {
                        assert!(field.insert(0, value).is_some(), "{leaf:?} {index}");
                    }
                    Some(ConfigurationCommand::Write(..) | ConfigurationCommand::Read(_)) => {}
                    None => panic!("{leaf:?} {index} has no valid field"),
                }
                index += 1;
            }
            assert!(index > 0, "{leaf:?} has commands");
        }
    }

    #[test]
    fn the_adc_rate_selects_the_inverted_analog_bit() {
        let bit = |rate| match PhyI2cConfiguration::AdcRate(rate).command(0) {
            Some(PhyI2cConfigurationCommand::Modify { value, .. }) => value,
            other => panic!("{other:?}"),
        };
        assert_eq!((bit(false), bit(true)), (1, 0));
    }

    #[test]
    fn modeled_configuration_writes_match_the_old_byte_packing() {
        // Leaf, command, old mask, old shift, old logical value. These are
        // the pre-model configuration expressions, exercised through the HAL.
        for (leaf, index, mask, shift, old_value) in [
            (1, 0, 0x10, 4, 0),
            (1, 1, 0x20, 5, 1),
            (2, 0, 0x10, 4, 0),
            (3, 0, 0x04, 2, 1),
            (4, 0, 0x04, 2, 0),
            (5, 0, 0x0f, 0, 15),
            (5, 1, 0xf0, 4, 7),
            (5, 2, 0xf0, 4, 9),
            (5, 3, 0x0f, 0, 15),
            (6, 0, 0x80, 7, 1),
            (6, 1, 0x70, 4, 4),
            (6, 2, 0x70, 4, 4),
            (7, 0, 0x0c, 2, 0),
            (7, 1, 0x40, 6, 1),
            (7, 2, 0x08, 3, 1),
            (7, 3, 0x20, 5, 1),
            (8, 0, 0x30, 4, 3),
            (8, 1, 0xf8, 3, 31),
            (8, 2, 0x3c, 2, 15),
            (9, 0, 0x0f, 0, 15),
        ] {
            let command = LEAVES[leaf].command(index).expect("configuration command");
            let Some(ConfigurationCommand::Modify(field, value)) = configuration_command(command)
            else {
                panic!("expected a masked analog write");
            };
            for raw in 0..=u8::MAX {
                assert_eq!(
                    field.insert(raw, value),
                    Some((raw & !mask) | (old_value << shift))
                );
            }
        }
    }
}
