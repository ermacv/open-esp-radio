//! The analog-register I2C transport of the pinned ESP32-C5 `libphy.a`
//! (`phy_i2c.o`) against the compiled production transport.
//!
//! Each leaf runs the vendor function and the production probe over the
//! same argument domains, the radio register fills and an analog command
//! bank that answers the one register the case addresses. Every register
//! effect compares exactly except the completion polling production adds
//! before a read, and each case also meets the expectation below, which
//! derives from the reviewed transport facts and neither execution.
//!
//! SOURCE: `esp32c5/libphy.a[phy_i2c.o]` of `esp-phy-lib` 20f1db05
//! (evidence `C5_BLOB_LIBPHY_PHY_I2C`), with the host and read-mask roles
//! of ESP-IDF 4d59230d `regi2c_impl.c` and `i2c_ana_mst_reg.h` (evidence
//! `ESP_IDF_4D59230D_C5_I2C_ANA_MST`).
use crate::{I2C_HOST_MAP, I2C_PORTS, I2C_READ_MASK, LIBRARY, ROM};
use blobray_domain::{
    CommandBank, CommandCell, CommandPort, DeviceBehavior, DeviceDeclaration, EffectRule,
    RegionLifetime, RegisterCell,
};
use oer_vendor_scenario_engine::evidence::PhyEffect;
use oer_vendor_scenario_engine::harness::Result;
use oer_vendor_scenario_engine::leaf::{
    Domain, Leaf, Objects, Observed, Suite, Vendor, expected, leaf, objects, ruled, stated,
};
use oer_vendor_scenario_engine::phy::contracts::port_polling;
use oer_vendor_scenario_engine::phy::layout::{
    COMMAND_BUSY, COMMAND_DATA_MASK, COMMAND_READ, COMMAND_SELECTOR_MASK, COMMAND_WRITE,
    analog_bank,
};

/// Every analog block the transport admits.
const BLOCKS: &[u32] = &[
    0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f,
];
/// Every value of the vendor's `uint8_t` block argument.
const BLOCK_BYTES: [u32; 256] = {
    let mut values = [0; 256];
    let mut index = 0;
    while index < values.len() {
        values[index] = index as u32;
        index += 1;
    }
    values
};
/// The first and last block, the aliased block and a host-one block, for
/// the field leaves.
const FIELD_BLOCKS: &[u32] = &[0x61, 0x62, 0x66, 0x6f];
/// Host arguments, which both sides ignore.
const HOSTS: &[u32] = &[0, 1];
const HOST: &[u32] = &[0];
/// Registers: the first, an inner and the last.
const REGISTERS: &[u32] = &[0x00, 0x07, 0xff];
/// Written bytes and sampled register values: clear, a pattern and set.
const BYTES: &[u32] = &[0x00, 0x5a, 0xff];
const SAMPLES: &[u32] = &[0x00, 0xa5, 0xff];
/// Field bounds: whole byte, upper and lower nibble-sized fields.
const MOST_SIGNIFICANT: &[u32] = &[7, 4];
const LEAST_SIGNIFICANT: &[u32] = &[0, 3];
/// Field values every admitted field holds.
const FIELD_VALUES: &[u32] = &[0, 1];

/// Blocks whose host is one; every other block uses host zero.
const HOST_ONE_BLOCKS: &[u32] = &[0x63, 0x65, 0x66, 0x68, 0x6a, 0x6b, 0x6e];
/// Read-mask bit of each block from 0x61, before its shift by two.
const READ_MASK_BITS: [u32; 15] = [
    0x40, 0, 0x4, 0, 0x400, 0x20, 0x1, 0x8, 0x80, 0x10, 0x2, 0x800, 0x100, 0x200, 0x1000,
];
const READ_MASK_SHIFT: u32 = 2;
/// The host-map bits every host selection rewrites.
const HOST_MAP_KEPT: u32 = 0xfffe_000f;
const HOST_MAP_SET: u32 = 0x0001_9c10;
/// The block the vendor addresses as another slave.
const ALIASED_BLOCK: u32 = 0x62;
const ALIAS_SLAVE: u32 = 0x65;
/// Command fields: register and data positions.
const REGISTER_SHIFT: u32 = 8;
const DATA_SHIFT: u32 = 16;
/// Busy reads each command reports before it completes, and the busy reads
/// of a port no earlier command occupies.
const BUSY_READS: u32 = 1;
const IDLE: u32 = 0;
/// Completion polls one case may perform per port.
const MAX_POLLS: u32 = 64;

fn host(block: u32) -> usize {
    usize::from(HOST_ONE_BLOCKS.contains(&block))
}

/// The read mask of `block`; a block outside the transport has none.
fn read_mask(block: u32) -> u32 {
    block
        .checked_sub(BLOCKS[0])
        .and_then(|index| READ_MASK_BITS.get(index as usize))
        .map_or(0, |bit| bit << READ_MASK_SHIFT)
}

fn slave(block: u32) -> u32 {
    if block == ALIASED_BLOCK {
        ALIAS_SLAVE
    } else {
        block
    }
}

fn selector(block: u32, register: u32) -> u32 {
    register << REGISTER_SHIFT | slave(block)
}

fn field_mask(most: u32, least: u32) -> u32 {
    (1 << (most - least + 1)) - 1
}

/// An analog bank holding `value` in the one register a case addresses,
/// whose addressed port first reports `initial_busy` busy reads.
fn bank(block: u32, register: u32, value: u32, initial_busy: u32) -> Objects {
    Objects {
        devices: vec![analog_bank(
            "analog",
            "the addressed analog register; commands complete after one busy read",
            BUSY_READS,
            {
                let mut ports = [IDLE; 2];
                ports[host(block)] = initial_busy;
                ports
            },
            vec![CommandCell {
                selector: selector(block, register),
                initial: value,
                reads: None,
            }],
        )],
        ..Default::default()
    }
}

/// `(block, host, register, …, sample)`: the vendor takes every word but
/// the sample, which seeds the addressed register.
fn sampled_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let (sample, arguments) = words.split_last().expect("words and a sample");
    Ok(Objects {
        vendor_words: arguments.to_vec(),
        ..bank(words[0], words[2], *sample, IDLE)
    })
}

/// `(block, host, register, data, port state)` over a cleared register: the
/// port is idle, or still busy with an earlier command, which a write waits
/// for before it issues.
fn written_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let (busy, arguments) = words.split_last().expect("words and a port state");
    Ok(Objects {
        vendor_words: arguments.to_vec(),
        ..bank(words[0], words[2], 0, *busy)
    })
}

/// Port states of a write: idle, and busy with an earlier command.
const PORT_STATES: &[u32] = &[IDLE, BUSY_READS];

/// Production polls a port for completion before it issues a read; the
/// vendor issues at once.
fn polling() -> Vec<EffectRule> {
    port_polling(MAX_POLLS)
}

/// Completion polls of a leaf that issues many commands.
const MAX_LONG_POLLS: u32 = 512;

/// The completion polling production adds before each read of a leaf with
/// many commands.
fn long_polling() -> Vec<EffectRule> {
    port_polling(MAX_LONG_POLLS)
}

/// The host-map read and its rewrite, which every host selection performs.
fn check_host_map(observed: &Observed) -> std::result::Result<(), String> {
    let old = observed
        .effects
        .iter()
        .find_map(|e| match e {
            PhyEffect::Read(address, value) if *address == I2C_HOST_MAP => Some(*value),
            _ => None,
        })
        .ok_or("no host-map read")?;
    let expected = PhyEffect::Write(I2C_HOST_MAP, old & HOST_MAP_KEPT | HOST_MAP_SET);
    if !observed.effects.contains(&expected) {
        return Err(format!("no host-map write {expected:x?}"));
    }
    Ok(())
}

fn check_return(observed: &Observed, value: u32) -> std::result::Result<(), String> {
    if observed.returned != Some(value) {
        return Err(format!(
            "returned {:x?}, expected {value:#x}",
            observed.returned
        ));
    }
    Ok(())
}

/// The command write of one case, with nothing written outside the
/// transport registers.
fn check_command(observed: &Observed, block: u32, command: u32) -> std::result::Result<(), String> {
    let port = I2C_PORTS[host(block)];
    if !observed.effects.contains(&PhyEffect::Write(port, command)) {
        return Err(format!("no command {command:#x} at {port:#x}"));
    }
    let transport = [I2C_PORTS[0], I2C_PORTS[1], I2C_READ_MASK, I2C_HOST_MAP];
    if let Some(stray) = observed
        .effects
        .iter()
        .find(|e| matches!(e, PhyEffect::Write(address, _) if !transport.contains(address)))
    {
        return Err(format!("write outside the transport {stray:x?}"));
    }
    Ok(())
}

fn expect_host(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_return(observed, host(words[0]) as u32)?;
    check_host_map(observed)
}

fn expect_read_mask(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_return(observed, read_mask(words[0]))
}

fn expect_read(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let [block, _, register, sample] = *words else {
        return Err("read words".into());
    };
    check_return(observed, sample)?;
    check_host_map(observed)?;
    if !observed
        .effects
        .contains(&PhyEffect::Write(I2C_READ_MASK, !read_mask(block)))
    {
        return Err("no complemented read mask".into());
    }
    check_command(observed, block, selector(block, register) | COMMAND_READ)
}

fn expect_write(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let [block, _, register, data, _] = *words else {
        return Err("write words".into());
    };
    if observed
        .effects
        .iter()
        .any(|e| matches!(e, PhyEffect::Write(address, _) if *address == I2C_READ_MASK))
    {
        return Err("a write selects a read mask".into());
    }
    check_command(
        observed,
        block,
        selector(block, register) | data << DATA_SHIFT | COMMAND_WRITE,
    )
}

fn expect_read_field(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let [_, _, _, most, least, sample] = *words else {
        return Err("field read words".into());
    };
    check_return(observed, sample >> least & field_mask(most, least))
}

fn expect_write_field(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let [block, _, register, most, least, data, sample] = *words else {
        return Err("field write words".into());
    };
    let mask = field_mask(most, least) << least;
    let byte = (sample & !mask | data << least) & 0xff;
    check_command(
        observed,
        block,
        selector(block, register) | byte << DATA_SHIFT | COMMAND_WRITE,
    )
}

/// The write command of a parallel pair: `phy_i2c_paral_write` publishes it
/// without the start bit.
const PARALLEL_WRITE: u32 = 0x0100_0000;
/// The host map of the parallel initialization, in place.
const PARALLEL_HOST_MAP_SET: u32 = 0x0000_2060;

/// An analog bank over both ports that admits the parallel write command at
/// the `selectors` a case addresses.
fn parallel_bank(selectors: impl IntoIterator<Item = u32>) -> Objects {
    let mut cells: Vec<CommandCell> = selectors
        .into_iter()
        .map(|selector| CommandCell {
            selector,
            initial: 0,
            reads: None,
        })
        .collect();
    cells.sort_by_key(|cell| cell.selector);
    cells.dedup_by_key(|cell| cell.selector);
    Objects {
        devices: vec![DeviceDeclaration {
            id: "analog-parallel".into(),
            applicability: "both analog ports; parallel writes without the start bit complete after one busy read".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::CommandBank(CommandBank {
                selector_mask: COMMAND_SELECTOR_MASK,
                data_mask: COMMAND_DATA_MASK,
                read_command: COMMAND_READ,
                write_command: PARALLEL_WRITE,
                busy_mask: COMMAND_BUSY,
                reset_command: None,
                ports: I2C_PORTS
                    .iter()
                    .map(|address| CommandPort {
                        address: *address,
                        initial: 0,
                        initial_busy_reads: IDLE,
                        busy_reads: BUSY_READS,
                    })
                    .collect(),
                cells,
            }),
        }],
        ..Default::default()
    }
}

/// `(block0, reg0, data0, block1, reg1, data1, flag)`.
fn parallel_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    Ok(Objects {
        vendor_words: words.to_vec(),
        ..parallel_bank([
            words[1] << REGISTER_SHIFT | words[0],
            words[4] << REGISTER_SHIFT | words[3],
        ])
    })
}

fn parallel_command(block: u32, register: u32, data: u32) -> u32 {
    register << REGISTER_SHIFT | block | data << DATA_SHIFT | PARALLEL_WRITE
}

/// The port writes of a case, in order.
fn port_writes(observed: &Observed) -> Vec<(u32, u32)> {
    observed
        .effects
        .iter()
        .filter_map(|e| match e {
            PhyEffect::Write(address, value) if I2C_PORTS.contains(address) => {
                Some((*address, *value))
            }
            _ => None,
        })
        .collect()
}

fn expect_parallel(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let [b0, r0, d0, b1, r1, d1, _] = *words else {
        return Err("parallel words".into());
    };
    let expected = vec![
        (I2C_PORTS[0], parallel_command(b0, r0, d0)),
        (I2C_PORTS[1], parallel_command(b1, r1, d1)),
    ];
    if port_writes(observed) != expected {
        return Err(format!("port writes {:x?}", port_writes(observed)));
    }
    if observed.effects.iter().any(|e| {
        matches!(e, PhyEffect::Write(address, _) if *address == I2C_HOST_MAP || *address == I2C_READ_MASK)
    }) {
        return Err("a parallel write selects a host map or read mask".into());
    }
    Ok(())
}

/// One byte `phy_i2c_init1` writes: an immediate or a `phy_param` field.
#[derive(Clone, Copy)]
enum Param {
    Constant(u32),
    Byte(usize),
    /// `phy_get_data_sat(param[offset], high, low)`.
    Saturated(usize, u32, u32),
    /// Bits 9:2 of the halfword at 0x416 with bit 6 set.
    Shifted416,
    /// Bits 1:0 of the halfword at 0x416 at 5:4.
    Low416,
}

impl Param {
    fn value(self, image: &[u8]) -> u32 {
        let halfword = u32::from(image[0x416]) | u32::from(image[0x417]) << 8;
        match self {
            Self::Constant(value) => value,
            Self::Byte(offset) => u32::from(image[offset]),
            Self::Saturated(offset, low, high) => u32::from(image[offset]).clamp(low, high),
            Self::Shifted416 => (halfword >> 2 | 0x40) & 0xff,
            Self::Low416 => (halfword << 4) & 0x30,
        }
    }
}

/// Independent reading of `phy_i2c_init1`'s stack arrays: the host-0 and
/// host-1 (block, register, data) of each of its 44 pairs.
/// One host's (block, register, data) command of a parallel pair.
type HostCommand = (u32, u32, Param);

const INITIALIZATION: [(HostCommand, HostCommand); 44] = [
    (
        (0x6b, 0x2, Param::Constant(0x72)),
        (0x6a, 0x0, Param::Constant(0xff)),
    ),
    (
        (0x6b, 0x3, Param::Constant(0xaa)),
        (0x6a, 0x1, Param::Constant(0x7f)),
    ),
    (
        (0x6b, 0xe, Param::Constant(0x55)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x6b, 0x7, Param::Constant(0xff)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x6b, 0xa, Param::Constant(0x8)),
        (0x67, 0x2, Param::Constant(0x3)),
    ),
    (
        (0x6b, 0xc, Param::Constant(0x55)),
        (0x67, 0x1, Param::Constant(0x6f)),
    ),
    (
        (0x6b, 0xf, Param::Constant(0x81)),
        (0x67, 0x5, Param::Constant(0x6b)),
    ),
    (
        (0x6b, 0x9, Param::Constant(0x0)),
        (0x67, 0x1d, Param::Constant(0xc2)),
    ),
    (
        (0x6b, 0x5, Param::Constant(0x33)),
        (0x67, 0x6, Param::Byte(0xf5)),
    ),
    (
        (0x6b, 0x6, Param::Constant(0x30)),
        (0x67, 0x8, Param::Saturated(0xf7, 10, 50)),
    ),
    (
        (0x6b, 0xd, Param::Constant(0x57)),
        (0x67, 0xa, Param::Byte(0xf5)),
    ),
    (
        (0x6b, 0x8, Param::Constant(0xfd)),
        (0x67, 0xc, Param::Byte(0xf7)),
    ),
    (
        (0x6b, 0x4, Param::Constant(0xac)),
        (0x67, 0x7, Param::Byte(0xf6)),
    ),
    (
        (0x6e, 0x5, Param::Shifted416),
        (0x67, 0x9, Param::Saturated(0xf8, 10, 60)),
    ),
    (
        (0x6e, 0x7, Param::Constant(0x63)),
        (0x67, 0xb, Param::Byte(0xf6)),
    ),
    (
        (0x6e, 0x8, Param::Constant(0x73)),
        (0x67, 0xd, Param::Byte(0xf8)),
    ),
    (
        (0x6e, 0x9, Param::Constant(0xc)),
        (0x67, 0xe, Param::Byte(0xfb)),
    ),
    (
        (0x6e, 0xd, Param::Constant(0x22)),
        (0x67, 0x10, Param::Byte(0xfb)),
    ),
    (
        (0x6e, 0x1, Param::Constant(0x71)),
        (0x67, 0x12, Param::Byte(0xf9)),
    ),
    (
        (0x6e, 0x10, Param::Constant(0x63)),
        (0x67, 0x14, Param::Byte(0xf9)),
    ),
    (
        (0x6e, 0x11, Param::Constant(0x73)),
        (0x67, 0xf, Param::Byte(0xfc)),
    ),
    (
        (0x6e, 0x4, Param::Constant(0x47)),
        (0x67, 0x11, Param::Byte(0xfc)),
    ),
    ((0x6e, 0xc, Param::Low416), (0x67, 0x13, Param::Byte(0xfa))),
    (
        (0x6e, 0xf, Param::Constant(0x47)),
        (0x67, 0x15, Param::Byte(0xfa)),
    ),
    (
        (0x6e, 0x12, Param::Constant(0x44)),
        (0x6a, 0x3, Param::Constant(0xf)),
    ),
    (
        (0x6e, 0x13, Param::Constant(0x63)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x6e, 0x14, Param::Constant(0x73)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0xf, Param::Constant(0x3f)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x1, Param::Constant(0xab)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x13, Param::Constant(0x90)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x6, Param::Constant(0xe0)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x15, Param::Constant(0x98)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x11, Param::Constant(0xc)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x14, Param::Constant(0xa)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x0, Param::Constant(0xdb)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0xe, Param::Constant(0x0)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x8, Param::Constant(0x61)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x7, Param::Constant(0x0)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x6e, 0xa, Param::Byte(0x410)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x6e, 0xb, Param::Byte(0x412)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x1a, Param::Constant(0x11)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x19, Param::Constant(0x48)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x12, Param::Constant(0x0)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
    (
        (0x63, 0x18, Param::Constant(0xdd)),
        (0x67, 0x3, Param::Constant(0x44)),
    ),
];

/// `phy_param` images of the initialization cases: clear, a pattern with
/// distinct fields, saturation bounds with a full halfword at 0x416, and
/// both saturated fields inside their ranges.
const PARAMETER_PROFILES: &[u32] = &[0, 1, 2, 3];

fn parameter_image(profile: u32) -> Vec<u8> {
    let mut image = vec![0; crate::PHY_PARAM_BYTES as usize];
    match profile {
        1 => {
            for (index, offset) in (0xf5..=0xfc).enumerate() {
                image[offset] = 0x11 * (index as u8 + 1);
            }
            image[0x410..0x418].copy_from_slice(&[0x34, 0x12, 0xcd, 0xab, 0, 0, 0xff, 0x03]);
        }
        2 => {
            image[0xf7] = 0x05;
            image[0xf8] = 0xff;
            image[0x416..0x418].copy_from_slice(&[0xfc, 0xff]);
        }
        3 => {
            image[0xf7] = 30;
            image[0xf8] = 40;
            image[0x416..0x418].copy_from_slice(&[0x5a, 0x01]);
        }
        _ => {}
    }
    image
}

/// `(parameters, profile)`: production reads the image at `parameters`,
/// the vendor its linked `phy_param`, both holding the profile's image.
fn initialization_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [parameters, profile] = *words else {
        return Err(oer_vendor_scenario_engine::harness::invalid(
            "initialization words",
        ));
    };
    let image = parameter_image(profile);
    Ok(Objects {
        vendor_words: vec![],
        production: vec![(parameters, image.clone())],
        image: vec![(vendor.image_symbol("phy_param")?, image)],
        ..parallel_bank(
            INITIALIZATION
                .iter()
                .flat_map(|((b0, r0, _), (b1, r1, _))| {
                    [r0 << REGISTER_SHIFT | b0, r1 << REGISTER_SHIFT | b1]
                }),
        )
    })
}

fn expect_initialization(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let image = parameter_image(words[1]);
    let expected: Vec<(u32, u32)> = INITIALIZATION
        .iter()
        .flat_map(|((b0, r0, d0), (b1, r1, d1))| {
            [
                (I2C_PORTS[0], parallel_command(*b0, *r0, d0.value(&image))),
                (I2C_PORTS[1], parallel_command(*b1, *r1, d1.value(&image))),
            ]
        })
        .collect();
    let observed_writes = port_writes(observed);
    if observed_writes != expected {
        let first = observed_writes
            .iter()
            .zip(&expected)
            .position(|(a, b)| a != b);
        return Err(format!("port writes differ at {first:?}"));
    }
    let maps: Vec<u32> = observed
        .effects
        .iter()
        .filter_map(|e| match e {
            PhyEffect::Write(address, value) if *address == I2C_HOST_MAP => {
                Some(value & !HOST_MAP_KEPT)
            }
            _ => None,
        })
        .collect();
    if maps != [PARALLEL_HOST_MAP_SET, HOST_MAP_SET] {
        return Err(format!("host maps {maps:x?}"));
    }
    Ok(())
}

/// Where production reads the parameter image.
const PARAMETER_IMAGE: &[u32] = &[crate::PARAMETER_COPY];
/// Pair words: host-0 and host-1 blocks, registers and data, and the flag
/// production admits.
const PARALLEL_FIRST_BLOCKS: &[u32] = &[0x63, 0x6b];
const PARALLEL_SECOND_BLOCKS: &[u32] = &[0x67, 0x6a];
const PARALLEL_REGISTERS: &[u32] = &[0x00, 0x15];
const PARALLEL_FLAG: &[u32] = &[0];

/// PMU `RF_PWC` and `IMM_HP_CK_POWER` of ESP-IDF `soc/esp32c5/register/soc/pmu_reg.h`.
const PMU_RF_PWC: u32 = 0x600b_0158;
const PMU_IMM_HP_CK_POWER: u32 = 0x600b_00cc;
/// The RF analog I2C power bits `phy_open_i2c_xpd` sets at once.
const RF_ANALOG_I2C_POWER: u32 = 0xf300_0000;
const TIE_HIGH_XPD_BB_I2C: u32 = 1 << 28;
const XPD_PERIF_I2C: u32 = 1 << 27;
const PERIF_I2C_RSTB: u32 = 1 << 26;
/// Initial `RF_PWC` words: every combination of the peripheral power and
/// reset bits, over otherwise clear and set words.
const RF_PWC_STATES: &[u32] = &[
    0,
    XPD_PERIF_I2C,
    PERIF_I2C_RSTB,
    XPD_PERIF_I2C | PERIF_I2C_RSTB,
    !(XPD_PERIF_I2C | PERIF_I2C_RSTB),
];

fn pmu_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let (state, arguments) = words.split_last().expect("words and a state");
    Ok(Objects {
        vendor_words: arguments.to_vec(),
        devices: vec![DeviceDeclaration {
            id: "pmu".into(),
            applicability: "PMU RF_PWC and IMM_HP_CK_POWER retain writes".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: vec![
                    RegisterCell {
                        address: PMU_IMM_HP_CK_POWER,
                        width: 4,
                        value: 0,
                    },
                    RegisterCell {
                        address: PMU_RF_PWC,
                        width: 4,
                        value: *state,
                    },
                ],
            },
        }],
        ..Default::default()
    })
}

fn writes_to(observed: &Observed, address: u32) -> Vec<u32> {
    observed
        .effects
        .iter()
        .filter_map(|e| match e {
            PhyEffect::Write(a, v) if *a == address => Some(*v),
            _ => None,
        })
        .collect()
}

fn expect_open_i2c_xpd(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let state = words[0];
    let powered = state | RF_ANALOG_I2C_POWER;
    let mut expected = vec![powered];
    let mut word = powered;
    if state & XPD_PERIF_I2C == 0 {
        word |= XPD_PERIF_I2C;
        expected.push(word);
        word &= !PERIF_I2C_RSTB;
        expected.push(word);
        word |= PERIF_I2C_RSTB;
        expected.push(word);
    }
    if word & PERIF_I2C_RSTB == 0 {
        word |= PERIF_I2C_RSTB;
        expected.push(word);
    }
    let rf = writes_to(observed, PMU_RF_PWC);
    if rf != expected {
        return Err(format!("RF_PWC writes {rf:x?}, expected {expected:x?}"));
    }
    if writes_to(observed, PMU_IMM_HP_CK_POWER) != [TIE_HIGH_XPD_BB_I2C] {
        return Err("no BB I2C power tie".into());
    }
    Ok(())
}

/// The host-0, host-1 and hardware-host bus timing words.
const I2C_TIMING: [u32; 3] = [0x600a_f824, 0x600a_f828, 0x600a_f82c];
const CLOCK_SELECTIONS: &[u32] = &[0, 1, 5, 31];

fn expect_clk_sel(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let n = words[0];
    let pulse = if n == 0 { 1 } else { 2 * n };
    let timing: Vec<(u32, u32)> = observed
        .effects
        .iter()
        .filter_map(|e| match e {
            PhyEffect::Write(a, v) if I2C_TIMING.contains(a) => Some((*a, *v)),
            _ => None,
        })
        .collect();
    let addresses: Vec<u32> = timing.iter().map(|(a, _)| *a).collect();
    let order: Vec<u32> = I2C_TIMING.iter().flat_map(|a| [*a, *a]).collect();
    if addresses != order {
        return Err(format!("timing writes {timing:x?}"));
    }
    for pair in timing.chunks(2) {
        let (guard, final_word) = (pair[0].1, pair[1].1);
        if guard >> 6 & 0x1f != n || final_word >> 6 & 0x1f != n || final_word & 0x3f != pulse {
            return Err(format!("timing {pair:x?} for {n}"));
        }
    }
    Ok(())
}

/// Independent reading of the configuration leaves: (block, register, msb,
/// lsb, value); a whole-byte write has msb 7 and lsb 0 and no read.
type Command = (u32, u32, u32, u32, u32);
const BAND: &[Command] = &[(0x6a, 1, 7, 0, 0x7f)];
const CRYSTAL: &[Command] = &[(0x61, 8, 4, 4, 0), (0x61, 7, 5, 5, 1)];
const BIAS: &[Command] = &[
    (0x6a, 0, 3, 0, 0xf),
    (0x6a, 1, 7, 4, 7),
    (0x6a, 0, 7, 4, 9),
    (0x6a, 1, 3, 0, 0xf),
];
const DAC_RATE: &[Command] = &[(0x66, 4, 4, 4, 0)];
const ADC_RATE_ZERO: &[Command] = &[(0x66, 4, 2, 2, 1)];
const ADC_RATE_ONE: &[Command] = &[(0x66, 4, 2, 2, 0)];

/// An analog bank whose addressed registers all hold `sample`.
fn command_bank(commands: &[Command], sample: u32) -> Objects {
    let mut cells: Vec<CommandCell> = commands
        .iter()
        .map(|(block, register, ..)| CommandCell {
            selector: selector(*block, *register),
            initial: sample,
            reads: None,
        })
        .collect();
    cells.sort_by_key(|c| c.selector);
    cells.dedup_by_key(|c| c.selector);
    Objects {
        devices: vec![analog_bank(
            "analog",
            "the addressed analog registers; commands complete after one busy read",
            BUSY_READS,
            [IDLE; 2],
            cells,
        )],
        ..Default::default()
    }
}

/// The command writes `commands` perform over registers that hold `sample`
/// and are updated by each earlier write.
fn expected_commands(commands: &[Command], sample: u32) -> Vec<(u32, u32)> {
    let mut registers = std::collections::BTreeMap::new();
    commands
        .iter()
        .map(|&(block, register, msb, lsb, value)| {
            let current = *registers.get(&(block, register)).unwrap_or(&sample);
            let byte = if (msb, lsb) == (7, 0) {
                value
            } else {
                let mask = field_mask(msb, lsb) << lsb;
                (current & !mask | value << lsb) & 0xff
            };
            registers.insert((block, register), byte);
            (
                I2C_PORTS[host(block)],
                selector(block, register) | byte << DATA_SHIFT | COMMAND_WRITE,
            )
        })
        .collect()
}

fn check_commands(
    observed: &Observed,
    commands: &[Command],
    sample: u32,
) -> std::result::Result<(), String> {
    let writes: Vec<(u32, u32)> = port_writes(observed)
        .into_iter()
        .filter(|(_, v)| v & COMMAND_WRITE == COMMAND_WRITE)
        .collect();
    let expected = expected_commands(commands, sample);
    if writes != expected {
        return Err(format!("commands {writes:x?}, expected {expected:x?}"));
    }
    Ok(())
}

/// The baseband configuration word of `phy_dac_rate_set` and
/// `phy_adc_rate_set`, inside the radio aperture.
const RATE_WORD: u32 = 0x600a_0448;

macro_rules! configuration_leaf {
    ($abi:ident, $expect:ident, $commands:expr, $rate:expr) => {
        /// `(arguments…, state)`: every addressed register holds the state's
        /// low byte and the baseband word starts at the bits above it.
        fn $abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
            let (state, arguments) = words.split_last().expect("a state");
            Ok(Objects {
                vendor_words: arguments.to_vec(),
                registers: vec![(RATE_WORD, state >> 8)],
                ..command_bank(&$commands(arguments), state & 0xff)
            })
        }

        fn $expect(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
            let (state, arguments) = words.split_last().expect("a state");
            let (sample, rate_word) = (state & 0xff, state >> 8);
            check_commands(observed, &$commands(arguments), sample)?;
            let rate: Option<fn(&[u32], u32) -> Vec<u32>> = $rate;
            if let Some(rate) = rate {
                let writes = writes_to(observed, RATE_WORD);
                let expected = rate(arguments, rate_word);
                if writes != expected {
                    return Err(format!(
                        "rate word writes {writes:x?}, expected {expected:x?}"
                    ));
                }
            }
            Ok(())
        }
    };
}

fn dac_rate_word(_: &[u32], word: u32) -> Vec<u32> {
    let high = word & !0x8;
    vec![high, high & !0x4]
}

fn adc_rate_word(arguments: &[u32], word: u32) -> Vec<u32> {
    let rate = arguments[0] & 1;
    let high = word & !0x2 | rate << 1;
    vec![high, high & !0x1 | rate]
}

fn bbpll_rate_word(_: &[u32], word: u32) -> Vec<u32> {
    let mut writes = dac_rate_word(&[], word);
    let last = *writes.last().expect("two writes");
    writes.extend(adc_rate_word(&[0], last));
    writes
}

fn band_commands(_: &[u32]) -> Vec<Command> {
    (BAND).to_vec()
}
fn crystal_commands(_: &[u32]) -> Vec<Command> {
    (CRYSTAL).to_vec()
}
fn bias_commands(_: &[u32]) -> Vec<Command> {
    (BIAS).to_vec()
}
fn dac_commands(_: &[u32]) -> Vec<Command> {
    (DAC_RATE).to_vec()
}
fn adc_commands(arguments: &[u32]) -> &'static [Command] {
    if arguments[0] == 0 {
        ADC_RATE_ZERO
    } else {
        ADC_RATE_ONE
    }
}
const BBPLL: &[Command] = &[(0x66, 4, 4, 4, 0), (0x66, 4, 2, 2, 1)];
fn bbpll_commands(_: &[u32]) -> Vec<Command> {
    (BBPLL).to_vec()
}

configuration_leaf!(band_abi, expect_band, band_commands, None);
configuration_leaf!(crystal_abi, expect_crystal, crystal_commands, None);
configuration_leaf!(bias_abi, expect_bias, bias_commands, None);
configuration_leaf!(dac_abi, expect_dac, dac_commands, Some(dac_rate_word));
configuration_leaf!(adc_abi, expect_adc, adc_commands, Some(adc_rate_word));
configuration_leaf!(
    bbpll_abi,
    expect_bbpll,
    bbpll_commands,
    Some(bbpll_rate_word)
);

/// Samples and rate words of the configuration leaves, as one state word
/// each: sample in the low byte, rate word above.
const CONFIGURATION_STATES: &[u32] = &[0x0_00, 0x0_a5, 0xf_ff, 0xf_5a];
const BANDS: &[u32] = &[0, 1];
const RATES: &[u32] = &[0, 1];
const NO_ARGUMENT: &[(&str, Domain)] = &[];

/// One register operation of an independent leaf reading.
#[derive(Clone, Copy)]
enum Op {
    /// A fresh RMW: the written word is the word just read with `clear`
    /// cleared and `set` set.
    Rmw(u32, u32, u32),
    /// A write of a complete image.
    Image(u32, u32),
}

/// The radio-register writes of a case match `ops` in order, each RMW
/// writing the word it read with its bits replaced.
fn check_ops(observed: &Observed, ops: &[Op]) -> std::result::Result<(), String> {
    let watched: Vec<u32> = ops
        .iter()
        .map(|op| match op {
            Op::Rmw(a, ..) | Op::Image(a, _) => *a,
        })
        .collect();
    let mut last_read = std::collections::BTreeMap::new();
    let mut writes = vec![];
    for effect in &observed.effects {
        match effect {
            PhyEffect::Read(a, v) if watched.contains(a) => {
                last_read.insert(*a, *v);
            }
            PhyEffect::Write(a, v) if watched.contains(a) => {
                writes.push((*a, *v, last_read.get(a).copied()))
            }
            _ => {}
        }
    }
    if writes.len() != ops.len() {
        return Err(format!(
            "{} writes {writes:x?}, expected {}",
            writes.len(),
            ops.len()
        ));
    }
    for (index, (op, (address, value, read))) in ops.iter().zip(&writes).enumerate() {
        let expected = match *op {
            Op::Image(a, image) => (a, image),
            Op::Rmw(a, clear, set) => {
                let Some(read) = read else {
                    return Err(format!("write {index} to {a:#x} without a read"));
                };
                (a, read & !clear | set)
            }
        };
        if (*address, *value) != expected {
            return Err(format!(
                "write {index} {:x?}, expected {expected:x?}",
                (address, value)
            ));
        }
    }
    Ok(())
}

const APB_SARADC_CTRL: u32 = 0x6000_e000;
const LP_AON_SAR_CCT: u32 = 0x600b_1054;
const BB: u32 = 0x600a_0000;
/// Initial SAR ADC and LP_AON words: clear and set.
const OUTSIDE_STATES: &[u32] = &[0, u32::MAX];

fn outside_bank(value: u32) -> DeviceDeclaration {
    DeviceDeclaration {
        id: "sar".into(),
        applicability: "APB_SARADC CTRL and LP_AON SAR_CCT retain writes".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: vec![
                RegisterCell {
                    address: APB_SARADC_CTRL,
                    width: 4,
                    value,
                },
                RegisterCell {
                    address: LP_AON_SAR_CCT,
                    width: 4,
                    value,
                },
            ],
        },
    }
}

/// `(arguments…, state)` of a leaf without parameters: the SAR words start
/// at the state.
fn register_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let (state, arguments) = words.split_last().expect("a state");
    Ok(Objects {
        vendor_words: arguments.to_vec(),
        devices: vec![outside_bank(*state)],
        ..Default::default()
    })
}

/// `phy_param` states of the parameter leaves: the IQ swap byte 0x2A and
/// the RX IQ scale byte 0x28A, as `swap | scale << 8`.
const SWAP_SCALE_STATES: &[u32] = &[0x000, 0x001, 0x100, 0x201, 0x300, 0x0ff];

fn swap_scale_image(state: u32) -> Vec<u8> {
    let mut image = vec![0; crate::PHY_PARAM_BYTES as usize];
    image[0x2a] = state as u8;
    image[0x28a] = (state >> 8) as u8;
    image
}

fn parameter_leaf_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [parameters, state] = *words else {
        return Err(oer_vendor_scenario_engine::harness::invalid(
            "parameter words",
        ));
    };
    let image = swap_scale_image(state);
    Ok(Objects {
        vendor_words: vec![],
        production: vec![(parameters, image.clone())],
        image: vec![(vendor.image_symbol("phy_param")?, image)],
        devices: vec![outside_bank(0)],
        ..Default::default()
    })
}

const OPEN_FE_BB_CLK: &[Op] = &[
    Op::Image(BB + 0x400, 0x1e7),
    Op::Rmw(BB + 0x800, 0, 0x3),
    Op::Image(BB + 0x7c80, 0xffff_ffff),
];
const I2C_ANA_CONF0: u32 = 0x600a_f818;
const I2CMST_REG_INIT: &[Op] = &[
    Op::Rmw(I2C_ANA_CONF0, 0x600, 0x400),
    Op::Rmw(I2C_ANA_CONF0, 0, 0x40),
];
const PWDET_REG_INIT: &[Op] = &[
    Op::Image(BB + 0x810, 0x0f0f_0fff),
    Op::Image(BB + 0x814, 0x00ff_0f64),
    Op::Rmw(BB + 0x808, 0xff0, 0x500),
    Op::Image(BB + 0x818, 0xaaaa),
    Op::Rmw(BB + 0x808, 0x70_0000, 0x20_0000),
    Op::Rmw(APB_SARADC_CTRL, 0, 1 << 29),
    Op::Rmw(LP_AON_SAR_CCT, 0xe000_0000, 0x8000_0000),
];

fn dac_scale_ops(full: bool) -> Vec<Op> {
    let byte = if full { 0xff } else { 0 };
    vec![
        Op::Rmw(BB + 0xc04, 0xff_0000, byte << 16),
        Op::Rmw(BB + 0xc04, 0xff00, byte << 8),
    ]
}

fn rxiq_scale_ops(selection: u32) -> Vec<Op> {
    let (high, low) = match selection {
        1 => (0xfa, 0),
        2 => (0, 0xfa),
        _ => (0, 0),
    };
    vec![
        Op::Rmw(BB + 0x43c, 0xff00, high << 8),
        Op::Rmw(BB + 0x43c, 0xff, low),
    ]
}

fn iq_swap_ops(swap: bool) -> Vec<Op> {
    vec![
        if swap {
            Op::Rmw(BB + 0xc08, 1 << 25, 0)
        } else {
            Op::Rmw(BB + 0xc08, 0, 0x600_0000)
        },
        Op::Rmw(BB + 0x434, 0xc0_0000, 0),
    ]
}

fn fe_reg_init_ops(swap: bool, selection: u32) -> Vec<Op> {
    let mut ops = vec![
        Op::Rmw(BB + 0x894, 0, 1 << 22),
        Op::Rmw(BB + 0x444, 1 << 8, 0),
        Op::Rmw(BB + 0x408, 0xff00_0000, 0xb400_0000),
        Op::Rmw(BB + 0x40c, 0, 0x4),
        Op::Rmw(BB + 0x438, 0, 0xe000_0000),
        Op::Rmw(BB + 0xc0c, 0, 0x6000),
        Op::Rmw(BB + 0x43c, 0xff00, 0),
        Op::Rmw(BB + 0x43c, 0xff, 0),
        if swap {
            Op::Rmw(BB + 0x888, 0, 1 << 29)
        } else {
            Op::Rmw(BB + 0x888, 1 << 29, 0)
        },
        Op::Rmw(BB + 0xc20, 0xff, 0x57),
        Op::Rmw(BB + 0x870, 0xff00, 0x9600),
    ];
    ops.extend(PWDET_REG_INIT);
    ops.extend(dac_scale_ops(true));
    ops.extend(rxiq_scale_ops(selection));
    ops
}

fn pwdet_sar2_init_ops(swap: bool) -> Vec<Op> {
    vec![
        Op::Rmw(BB + 0x80c, 0, 0x3000),
        Op::Rmw(BB + 0x80c, 1 << 9, 0),
        Op::Image(BB + 0x818, 0xaaaa),
        Op::Rmw(BB + 0x808, 0x70_0000, 0x60_0000),
        Op::Rmw(LP_AON_SAR_CCT, 0xe000_0000, if swap { 4 } else { 2 } << 29),
    ]
}

fn swap_scale(words: &[u32]) -> (bool, u32) {
    let state = words[1];
    (state & 0xff != 0, state >> 8 & 0xff)
}

fn expect_open_fe_bb_clk(_: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, OPEN_FE_BB_CLK)
}
fn expect_i2cmst_reg_init(_: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, I2CMST_REG_INIT)
}
fn expect_pwdet_reg_init(_: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, PWDET_REG_INIT)
}
fn expect_dac_scale(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, &dac_scale_ops(words[0] != 0))
}
fn expect_iq_swap(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, &iq_swap_ops(swap_scale(words).0))
}
fn expect_fe_reg_init(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let (swap, selection) = swap_scale(words);
    check_ops(observed, &fe_reg_init_ops(swap, selection))
}
fn expect_rxiq_scale(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, &rxiq_scale_ops(swap_scale(words).1))
}
fn expect_pwdet_sar2_init(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, &pwdet_sar2_init_ops(swap_scale(words).0))
}

const DAC_SCALES: &[u32] = &[0, 1, 0x100];

fn rc_cal_commands(arguments: &[u32]) -> Vec<Command> {
    vec![
        (0x6b, 0x11, 5, 4, arguments[0]),
        (0x6b, 0x0f, 7, 3, arguments[1]),
        (0x6b, 0x13, 5, 2, arguments[2]),
    ]
}
fn pkdet_commands(_: &[u32]) -> Vec<Command> {
    vec![
        (0x67, 0x1d, 7, 7, 1),
        (0x67, 0x1d, 6, 4, 4),
        (0x67, 3, 6, 4, 4),
    ]
}
fn sar2_commands(arguments: &[u32]) -> Vec<Command> {
    vec![
        (0x69, 4, 3, 0, arguments[0] >> 8),
        (0x69, 3, 7, 0, arguments[0] & 0xff),
    ]
}
configuration_leaf!(rc_cal_abi, expect_rc_cal, rc_cal_commands, None);
configuration_leaf!(pkdet_abi, expect_pkdet, pkdet_commands, None);
configuration_leaf!(sar2_abi, expect_sar2, sar2_commands, None);
const RC_FIRST: &[u32] = &[0, 3];
const RC_SECOND: &[u32] = &[0, 31];
const RC_THIRD: &[u32] = &[0, 15];
const SAR2_CODES: &[u32] = &[0, 0x5a5, 0xfff];

/// `phy_filter_dcap_set`'s four field writes and sixteen byte writes over
/// the `phy_param` bytes 0xF5 to 0xFC.
fn filter_dcap_commands(image: &[u8]) -> Vec<Command> {
    let p = |offset: usize| u32::from(image[offset]);
    let sat = |offset: usize, low, high| p(offset).clamp(low, high);
    let mut commands = vec![
        (0x67, 0x1d, 3, 2, 0),
        (0x67, 5, 6, 6, 1),
        (0x67, 5, 3, 3, 1),
        (0x67, 5, 5, 5, 1),
    ];
    for (register, value) in [
        (6, p(0xf5)),
        (8, sat(0xf7, 10, 50)),
        (0xa, p(0xf5)),
        (0xc, p(0xf7)),
        (7, p(0xf6)),
        (9, sat(0xf8, 10, 60)),
        (0xb, p(0xf6)),
        (0xd, p(0xf8)),
        (0xe, p(0xfb)),
        (0x10, p(0xfb)),
        (0x12, p(0xf9)),
        (0x14, p(0xf9)),
        (0xf, p(0xfc)),
        (0x11, p(0xfc)),
        (0x13, p(0xfa)),
        (0x15, p(0xfa)),
    ] {
        commands.push((0x67, register, 7, 0, value));
    }
    commands
}

/// `(parameters, profile)` over the initialization parameter images, with
/// every addressed register holding 0xA5.
fn filter_dcap_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [parameters, profile] = *words else {
        return Err(oer_vendor_scenario_engine::harness::invalid("filter words"));
    };
    let image = parameter_image(profile);
    Ok(Objects {
        vendor_words: vec![],
        production: vec![(parameters, image.clone())],
        image: vec![(vendor.image_symbol("phy_param")?, image.clone())],
        ..command_bank(&filter_dcap_commands(&image), FILTER_SAMPLE)
    })
}

const FILTER_SAMPLE: u32 = 0xa5;

fn expect_filter_dcap(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_commands(
        observed,
        &filter_dcap_commands(&parameter_image(words[1])),
        FILTER_SAMPLE,
    )
}

const PCR_SARADC_CONF: u32 = 0x6009_6088;
const PCR_TSENS_CLK_CONF: u32 = 0x6009_6090;
const APB_SARADC_TSENS_CTRL: u32 = 0x6000_e058;
const APB_SARADC_TSENS_CTRL2: u32 = 0x6000_e05c;

fn tsens_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let (state, arguments) = words.split_last().expect("a state");
    Ok(Objects {
        vendor_words: arguments.to_vec(),
        devices: vec![DeviceDeclaration {
            id: "tsens".into(),
            applicability:
                "PCR SAR ADC and TSENS clocks and the APB_SARADC TSENS controls retain writes"
                    .into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: [
                    PCR_SARADC_CONF,
                    PCR_TSENS_CLK_CONF,
                    APB_SARADC_TSENS_CTRL,
                    APB_SARADC_TSENS_CTRL2,
                ]
                .into_iter()
                .map(|address| RegisterCell {
                    address,
                    width: 4,
                    value: *state,
                })
                .collect(),
            },
        }],
        ..Default::default()
    })
}

fn tsens_power_ops(on: bool) -> Vec<Op> {
    vec![Op::Rmw(APB_SARADC_TSENS_CTRL, 1 << 22, u32::from(on) << 22)]
}

fn tsens_pwr_ops() -> Vec<Op> {
    let mut ops = tsens_power_ops(true);
    ops.push(Op::Rmw(APB_SARADC_TSENS_CTRL2, 0, 1 << 15));
    ops
}

fn tsens_read_init_ops() -> Vec<Op> {
    let mut ops = vec![
        Op::Rmw(PCR_SARADC_CONF, 0, 0x5),
        Op::Rmw(PCR_TSENS_CLK_CONF, 0, 0x50_0000),
        Op::Rmw(PCR_TSENS_CLK_CONF, 1 << 23, 0),
        Op::Rmw(APB_SARADC_TSENS_CTRL2, 0, 1 << 15),
    ];
    ops.extend(tsens_pwr_ops());
    ops
}

fn expect_tsens_power(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, &tsens_power_ops(words[0] != 0))
}
fn expect_tsens_pwr(_: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, &tsens_pwr_ops())
}
fn expect_tsens_read_init(_: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, &tsens_read_init_ops())
}

const TSENS_ON: &[u32] = &[0, 1];
const TSENS_MODES: &[u32] = &[0, 1];

fn bbpll_cal_ops(start: bool) -> Vec<Op> {
    vec![Op::Rmw(I2C_ANA_CONF0, 0xc, if start { 0x8 } else { 0x4 })]
}
fn rxevm_reset_ops() -> Vec<Op> {
    vec![
        Op::Rmw(BB + 0x7a34, 0, 0x2),
        Op::Rmw(BB + 0x7a34, 0x2, 0),
        Op::Rmw(BB + 0x7c50, 0, 0x100),
        Op::Rmw(BB + 0x7c50, 0x100, 0),
    ]
}
fn rxevm_init_ops(words: &[u32]) -> Vec<Op> {
    let (enable, second, third) = (words[0], words[1], words[2]);
    let mut ops = vec![
        Op::Rmw(BB + 0x7a38, 0x7c, 0x60),
        Op::Rmw(BB + 0x7a38, 0x1fc0_0000, third << 22),
        Op::Rmw(BB + 0x7a38, 0x3f_8000, second << 15),
        Op::Rmw(BB + 0x7a34, 0x7, if enable != 0 { 5 } else { 0 }),
        Op::Rmw(BB + 0x7920, 0xf000, 0),
    ];
    ops.extend(rxevm_reset_ops());
    ops
}
fn expect_bbpll_cal(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, &bbpll_cal_ops(words[0] != 0))
}
fn expect_bbpll_recal(_: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    let mut ops = bbpll_cal_ops(true);
    ops.extend(bbpll_cal_ops(false));
    check_ops(observed, &ops)
}
fn expect_rxevm_init(words: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, &rxevm_init_ops(words))
}
fn expect_rxevm_reset(_: &[u32], observed: &Observed) -> std::result::Result<(), String> {
    check_ops(observed, &rxevm_reset_ops())
}
const BBPLL_STARTS: &[u32] = &[0, 1, 2];
const RX_EVM_ENABLES: &[u32] = &[0, 1];
const RX_EVM_PARAMETERS: &[u32] = &[0, 0x46, 0x7f];

const LEAVES: &[Leaf] = &[
    expected(
        leaf(
            "phy_get_i2c_hostid_",
            "open_phy_i2c_trace_phy_get_i2c_hostid_",
            &[("block", Domain::Words(BLOCKS))],
            true,
        ),
        expect_host,
    ),
    expected(
        leaf(
            "phy_get_i2c_read_mask_",
            "open_phy_i2c_trace_phy_get_i2c_read_mask_",
            &[("block", Domain::Words(&BLOCK_BYTES))],
            true,
        ),
        expect_read_mask,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_chip_i2c_readReg",
                        "open_phy_i2c_trace_phy_chip_i2c_readReg",
                        &[
                            ("block", Domain::Words(BLOCKS)),
                            ("host_id", Domain::Words(HOSTS)),
                            ("reg_add", Domain::Words(REGISTERS)),
                        ],
                        true,
                    ),
                    sampled_abi,
                ),
                SAMPLES,
            ),
            polling,
        ),
        expect_read,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_chip_i2c_writeReg",
                        "open_phy_i2c_trace_phy_chip_i2c_writeReg",
                        &[
                            ("block", Domain::Words(BLOCKS)),
                            ("host_id", Domain::Words(HOSTS)),
                            ("reg_add", Domain::Words(REGISTERS)),
                            ("data", Domain::Words(BYTES)),
                        ],
                        false,
                    ),
                    written_abi,
                ),
                PORT_STATES,
            ),
            polling,
        ),
        expect_write,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_i2c_readReg_Mask",
                        "open_phy_i2c_trace_phy_i2c_readReg_Mask",
                        &[
                            ("block", Domain::Words(FIELD_BLOCKS)),
                            ("host_id", Domain::Words(HOST)),
                            ("reg_add", Domain::Words(REGISTERS)),
                            ("msb", Domain::Words(MOST_SIGNIFICANT)),
                            ("lsb", Domain::Words(LEAST_SIGNIFICANT)),
                        ],
                        true,
                    ),
                    sampled_abi,
                ),
                SAMPLES,
            ),
            polling,
        ),
        expect_read_field,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_i2c_writeReg_Mask",
                        "open_phy_i2c_trace_phy_i2c_writeReg_Mask",
                        &[
                            ("block", Domain::Words(FIELD_BLOCKS)),
                            ("host_id", Domain::Words(HOST)),
                            ("reg_add", Domain::Words(REGISTERS)),
                            ("msb", Domain::Words(MOST_SIGNIFICANT)),
                            ("lsb", Domain::Words(LEAST_SIGNIFICANT)),
                            ("data", Domain::Words(FIELD_VALUES)),
                        ],
                        false,
                    ),
                    sampled_abi,
                ),
                SAMPLES,
            ),
            polling,
        ),
        expect_write_field,
    ),
    expected(
        objects(
            leaf(
                "phy_i2c_paral_write",
                "open_phy_i2c_trace_phy_i2c_paral_write",
                &[
                    ("block0", Domain::Words(PARALLEL_FIRST_BLOCKS)),
                    ("reg0", Domain::Words(PARALLEL_REGISTERS)),
                    ("data0", Domain::Words(BYTES)),
                    ("block1", Domain::Words(PARALLEL_SECOND_BLOCKS)),
                    ("reg1", Domain::Words(PARALLEL_REGISTERS)),
                    ("data1", Domain::Words(BYTES)),
                    ("flag", Domain::Words(PARALLEL_FLAG)),
                ],
                false,
            ),
            parallel_abi,
        ),
        expect_parallel,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_i2c_init1",
                    "open_phy_i2c_trace_phy_i2c_init1",
                    &[("parameters", Domain::Words(PARAMETER_IMAGE))],
                    false,
                ),
                initialization_abi,
            ),
            PARAMETER_PROFILES,
        ),
        expect_initialization,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_open_i2c_xpd",
                    "open_phy_i2c_trace_phy_open_i2c_xpd",
                    NO_ARGUMENT,
                    false,
                ),
                pmu_abi,
            ),
            RF_PWC_STATES,
        ),
        expect_open_i2c_xpd,
    ),
    expected(
        leaf(
            "phy_i2c_clk_sel",
            "open_phy_i2c_trace_phy_i2c_clk_sel",
            &[("selection", Domain::Words(CLOCK_SELECTIONS))],
            false,
        ),
        expect_clk_sel,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_band_i2c_set",
                        "open_phy_i2c_trace_phy_band_i2c_set",
                        &[("band", Domain::Words(BANDS))],
                        false,
                    ),
                    band_abi,
                ),
                CONFIGURATION_STATES,
            ),
            polling,
        ),
        expect_band,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_xtal_reg_set",
                        "open_phy_i2c_trace_phy_xtal_reg_set",
                        NO_ARGUMENT,
                        false,
                    ),
                    crystal_abi,
                ),
                CONFIGURATION_STATES,
            ),
            polling,
        ),
        expect_crystal,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_bias_reg_set",
                        "open_phy_i2c_trace_phy_bias_reg_set",
                        NO_ARGUMENT,
                        false,
                    ),
                    bias_abi,
                ),
                CONFIGURATION_STATES,
            ),
            polling,
        ),
        expect_bias,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_dac_rate_set",
                        "open_phy_i2c_trace_phy_dac_rate_set",
                        &[("rate", Domain::Words(RATES))],
                        false,
                    ),
                    dac_abi,
                ),
                CONFIGURATION_STATES,
            ),
            polling,
        ),
        expect_dac,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_adc_rate_set",
                        "open_phy_i2c_trace_phy_adc_rate_set",
                        &[("rate", Domain::Words(RATES))],
                        false,
                    ),
                    adc_abi,
                ),
                CONFIGURATION_STATES,
            ),
            polling,
        ),
        expect_adc,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_i2c_bbpll_set",
                        "open_phy_i2c_trace_phy_i2c_bbpll_set",
                        NO_ARGUMENT,
                        false,
                    ),
                    bbpll_abi,
                ),
                CONFIGURATION_STATES,
            ),
            polling,
        ),
        expect_bbpll,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_open_fe_bb_clk",
                    "open_phy_i2c_trace_phy_open_fe_bb_clk",
                    NO_ARGUMENT,
                    false,
                ),
                register_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_open_fe_bb_clk,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_i2cmst_reg_init",
                    "open_phy_i2c_trace_phy_i2cmst_reg_init",
                    NO_ARGUMENT,
                    false,
                ),
                register_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_i2cmst_reg_init,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_pwdet_reg_init",
                    "open_phy_i2c_trace_phy_pwdet_reg_init",
                    NO_ARGUMENT,
                    false,
                ),
                register_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_pwdet_reg_init,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_dac_scale_set",
                    "open_phy_i2c_trace_phy_dac_scale_set",
                    &[("scale", Domain::Words(DAC_SCALES))],
                    false,
                ),
                register_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_dac_scale,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_iq_swap_set",
                    "open_phy_i2c_trace_phy_iq_swap_set",
                    &[("parameters", Domain::Words(PARAMETER_IMAGE))],
                    false,
                ),
                parameter_leaf_abi,
            ),
            SWAP_SCALE_STATES,
        ),
        expect_iq_swap,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_fe_reg_init",
                    "open_phy_i2c_trace_phy_fe_reg_init",
                    &[("parameters", Domain::Words(PARAMETER_IMAGE))],
                    false,
                ),
                parameter_leaf_abi,
            ),
            SWAP_SCALE_STATES,
        ),
        expect_fe_reg_init,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_rxiq_scale_set",
                    "open_phy_i2c_trace_phy_rxiq_scale_set",
                    &[("parameters", Domain::Words(PARAMETER_IMAGE))],
                    false,
                ),
                parameter_leaf_abi,
            ),
            SWAP_SCALE_STATES,
        ),
        expect_rxiq_scale,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_pwdet_sar2_init",
                    "open_phy_i2c_trace_phy_pwdet_sar2_init",
                    &[("parameters", Domain::Words(PARAMETER_IMAGE))],
                    false,
                ),
                parameter_leaf_abi,
            ),
            SWAP_SCALE_STATES,
        ),
        expect_pwdet_sar2_init,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_i2c_rc_cal_set",
                        "open_phy_i2c_trace_phy_i2c_rc_cal_set",
                        &[
                            ("first", Domain::Words(RC_FIRST)),
                            ("second", Domain::Words(RC_SECOND)),
                            ("third", Domain::Words(RC_THIRD)),
                        ],
                        false,
                    ),
                    rc_cal_abi,
                ),
                CONFIGURATION_STATES,
            ),
            polling,
        ),
        expect_rc_cal,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_i2c_pkdet_set",
                        "open_phy_i2c_trace_phy_i2c_pkdet_set",
                        NO_ARGUMENT,
                        false,
                    ),
                    pkdet_abi,
                ),
                CONFIGURATION_STATES,
            ),
            polling,
        ),
        expect_pkdet,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_i2c_sar2_init_code",
                        "open_phy_i2c_trace_phy_i2c_sar2_init_code",
                        &[("code", Domain::Words(SAR2_CODES))],
                        false,
                    ),
                    sar2_abi,
                ),
                CONFIGURATION_STATES,
            ),
            polling,
        ),
        expect_sar2,
    ),
    expected(
        ruled(
            stated(
                objects(
                    leaf(
                        "phy_filter_dcap_set",
                        "open_phy_i2c_trace_phy_filter_dcap_set",
                        &[("parameters", Domain::Words(PARAMETER_IMAGE))],
                        false,
                    ),
                    filter_dcap_abi,
                ),
                PARAMETER_PROFILES,
            ),
            long_polling,
        ),
        expect_filter_dcap,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_set_tsens_power",
                    "open_phy_i2c_trace_phy_set_tsens_power",
                    &[("on", Domain::Words(TSENS_ON))],
                    false,
                ),
                tsens_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_tsens_power,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_set_tsens_pwr",
                    "open_phy_i2c_trace_phy_set_tsens_pwr",
                    NO_ARGUMENT,
                    false,
                ),
                tsens_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_tsens_pwr,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_tsens_read_init",
                    "open_phy_i2c_trace_phy_tsens_read_init",
                    &[
                        ("mode", Domain::Words(TSENS_MODES)),
                        ("code", Domain::Words(BYTES)),
                    ],
                    false,
                ),
                tsens_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_tsens_read_init,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_bbpll_cal",
                    "open_phy_i2c_trace_phy_bbpll_cal",
                    &[("start", Domain::Words(BBPLL_STARTS))],
                    false,
                ),
                register_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_bbpll_cal,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_bbpll_recal",
                    "open_phy_i2c_trace_phy_bbpll_recal",
                    NO_ARGUMENT,
                    false,
                ),
                register_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_bbpll_recal,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_rxevm_init_cfg",
                    "open_phy_i2c_trace_phy_rxevm_init_cfg",
                    &[
                        ("enable", Domain::Words(RX_EVM_ENABLES)),
                        ("second", Domain::Words(RX_EVM_PARAMETERS)),
                        ("third", Domain::Words(RX_EVM_PARAMETERS)),
                    ],
                    false,
                ),
                register_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_rxevm_init,
    ),
    expected(
        stated(
            objects(
                leaf(
                    "phy_rxevm_reset_mem",
                    "open_phy_i2c_trace_phy_rxevm_reset_mem",
                    NO_ARGUMENT,
                    false,
                ),
                register_abi,
            ),
            OUTSIDE_STATES,
        ),
        expect_rxevm_reset,
    ),
];

/// Names no pinned input defines: ESP-IDF's `rtc_clk_xtal_freq_get` and the
/// ROM linker script's `_rom_eco_version`, which only `phy_init.o`'s
/// `phy_get_xtal_freq` and `phy_get_rom_ver` in the link closure reference.
/// They resolve to an unmapped address, so reaching one stops the case.
const ABSENT: &[&str] = &["rtc_clk_xtal_freq_get", "_rom_eco_version"];

/// The analog-register I2C transport suite over `libphy.a`.
pub const PHY_I2C: Suite = Suite {
    title: "ESP32-C5 PHY analog and RF-initialization leaf comparison",
    id: "phy-i2c",
    archives: &[LIBRARY],
    rom: ROM,
    firmware: None,
    leaves: LEAVES,
    absent: ABSENT,
    roots: &[],
    prepare: None,
    claims: &[],
};

#[cfg(test)]
mod tests {
    use super::*;
    use oer_vendor_scenario_engine::phy::layout::COMMAND_BUSY;

    #[test]
    fn the_expectations_follow_the_reviewed_transport_facts() {
        assert_eq!(host(0x66), 1);
        assert_eq!(host(0x61), 0);
        assert_eq!(read_mask(0x61), 0x100);
        assert_eq!(read_mask(0x62), 0);
        assert_eq!(read_mask(0x00), 0);
        assert_eq!(read_mask(0x70), 0);
        assert_eq!(selector(0x62, 0x07), 0x0765);
        assert_eq!(field_mask(7, 0), 0xff);
        assert_eq!(field_mask(4, 3), 0x3);
        // A busy command is never the completed command either side reads.
        assert_eq!(COMMAND_READ & COMMAND_BUSY, 0);
    }
}
