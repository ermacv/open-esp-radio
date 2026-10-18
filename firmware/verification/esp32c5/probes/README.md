# ESP32-C5 compiled verification probes

This isolated workspace builds retained Rust entry points that the
[vendor scenarios](../../harness/README.md) compare with the ESP32-C5 vendor
artifacts. The probes depend on production crates; no production crate or
firmware depends on them.

```console
cargo verification probes --chip esp32c5
```

builds `oer-esp32c5-probe-radio-elf` for `riscv32imac-unknown-none-elf` into
`target/verification/esp32c5-probes/` and validates its `.blobray.probes`
catalog against the image's executable symbols.

`radio/library` declares the entries with `oer_probe_macros::probe!`;
`radio/elf` owns only the image entry point and linker layout. Each analog
I2C entry is named `open_phy_i2c_trace_<vendor function>` and takes that
`libphy.a[phy_i2c.o]` function's integer arguments in its order:

| Entry | Production path |
| --- | --- |
| `phy_get_i2c_hostid_(block)` | `PhyI2cRegisters::configure_and_select_host` |
| `phy_get_i2c_read_mask_(block)` | `PhyI2cBlock::read_mask_complement_low` |
| `phy_chip_i2c_readReg(block, host_id, reg_add)` | `AnalogI2c` start and completion of a read |
| `phy_chip_i2c_writeReg(block, host_id, reg_add, data)` | `AnalogI2c` start and completion of a write |
| `phy_i2c_readReg_Mask(block, host_id, reg_add, msb, lsb)` | `oer_radio_analog::FieldRead` over `AnalogI2c` |
| `phy_i2c_writeReg_Mask(block, host_id, reg_add, msb, lsb, data)` | `oer_radio_analog::FieldWrite` over `AnalogI2c` |
| `phy_i2c_paral_write(block0, reg0, data0, block1, reg1, data1, flag)` | `AnalogI2c` parallel pair, then each host polled idle; only `flag == 0` |
| `phy_i2c_init1(parameters)` | `oer_esp32c5_hal::analog::initialization` over the `phy_param` image at `parameters` |

The vendor `phy_i2c_init1` takes no argument and reads `phy_param`; its
entry takes the address of a 1080-byte parameter image instead. Like the
vendor leaves, the entries ignore `host_id` and derive the host from
the block. They return `0x10000` for a block outside the libphy tables, a
field outside one byte, data wider than a byte or, for a field write, data
wider than the field; and `0x10001` when 64 bus actions do not complete the
transaction. That bound limits the harness only; it is not hardware time.
