# Analog register space vocabulary (analog-i2c)

`oer-chips-family-registers-analog-i2c` holds the values the Espressif
analog register space is described in: `AnalogField` (bits `msb..=lsb` of
one byte register, with checked insertion and extraction), `BlockCode`,
`ByteAddress`, the analog I2C master's two hosts (`Host`), a block's
`Route` and a `CaptureRegister` of a reviewed read epoch. They perform no
access.

A chip's PAC writes its generated `analog_i2c` dictionary in these types,
so the crate sits below the PAC and depends on nothing. The bus driver
(`oer-chips-family-drivers-regi2c`) takes its fields and hosts.

`AnalogField::insert_truncated` keeps only the low field bits of a byte
input, as `Bits::truncate` does for a register field; the layer test
counts its calls (`TRUNCATIONS`), which R2's area PRs remove.
