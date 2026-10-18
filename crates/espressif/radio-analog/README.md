# Analog register bus

`oer-radio-analog` is the chip-neutral contract the Espressif PHY uses to
reach its analog blocks: byte registers behind an analog I2C master. Where
the vendor leaves `phy_chip_i2c_readReg` and `phy_chip_i2c_writeReg`
busy-wait, this crate separates what a transaction does from how an
executor waits for it.

The vendor transactions are transitions. `FieldReadTransition` and
`FieldWriteTransition` are `phy_i2c_readReg_Mask` and
`phy_i2c_writeReg_Mask`; `ConfigurationTransition` runs a configuration
leaf's reads, writes and field writes in order from a `ConfigurationCommands`
source. Each exposes `action()`, the next whole-byte read or write or its
result, and `advance()`, which accepts the matching completion. A field
write reads the register, replaces bits `msb..=lsb` and writes the byte
back. The vendor ORs an unmasked value into the byte, so a field write
rejects a value wider than its field instead of reproducing that leak.
`ParallelTransition` is `phy_i2c_paral_write_num`: install the parallel host
map, publish each pair of host commands, wait for host 0 and then host 1
idle, and restore the normal map.

A chip's HAL implements `AnalogRegisterBus`, which splits each command into
a start and an observed completion, over its PAC. The address type is the
chip's own: host selection, read masks and block aliases stay behind it, and
completion is always observed with the address that started the command.
`Driver` runs any `ByteTransition` over that bus, at most one bus action per
`poll`; `FieldRead`, `FieldWrite`, `Configuration` and `ParallelWrites` are
the polled forms. A chip may instead drive the transitions with its own
executor, which then owns every wait and deadline.

The ESP32-C5 bus is `oer_esp32c5_hal::analog::AnalogI2c`, polled through the
drivers. The ESP32-S31 PHY drives the transitions from its own executor; its
HAL configuration transaction and retained-wake parallel stage use
`ConfigurationTransition` and `ParallelTransition` over the PAC bus.
