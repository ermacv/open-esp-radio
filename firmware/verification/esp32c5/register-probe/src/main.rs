//! Board observation of the ESP32-C5 IEEE 802.15.4 MAC register model.
//!
//! Opens the IEEE 802.15.4 MAC clocks as esp-radio does, then writes all ones
//! to each probed register through the generated raw PAC and reads the word
//! back. The implemented bits of a read-write register are the ones that
//! stay set, so every line checks the published field widths against the
//! silicon. The MAC is idle and no command is issued.
//!
//! It then drives every modem clock device and the IEEE 802.15.4 MAC reset
//! through the production PAC owner and checks each transition, and that the
//! modem clock words keep every bit outside the transitioned fields.
//!
//! Finally it reads analog registers of the powered IEEE blocks through the
//! production analog I2C owner and compares each byte with the ROM leaf
//! `phy_chip_i2c_readReg_org`, called with the same block, read mask and
//! host. The leaf is identical to the libphy one and reads no RAM table.
//! It repeats the BBPLL reads as polled field reads over the HAL analog bus,
//! and writes each of the first four ULP_CAL registers with its own value
//! through a polled field write, checking that the byte reads back unchanged.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::main;
use esp_println::println;
use oer_esp32c5_hal::analog::AnalogI2c;
use oer_esp32c5_pac::{
    ModemClockDevice, ModemClockRegisters, PhyI2cAddress, PhyI2cBlock, PhyI2cHost, PhyI2cRegisters,
    RadioPartitions,
};
use oer_esp32c5_pac_raw::Ieee802154Mac;
use oer_radio_analog::{AnalogField, FieldRead, FieldWrite, Step};

esp_bootloader_esp_idf::esp_app_desc!();

/// One read-write word: its name and the implemented bits the model declares.
struct Probe {
    name: &'static str,
    expected: u32,
    write_read: fn(&oer_esp32c5_pac_raw::ieee802154_mac::RegisterBlock) -> u32,
}

const PROBES: &[Probe] = &[
    Probe {
        // Bit 7 is implemented although the struct declares it reserved;
        // the first board run (2026-09-27) observed it.
        name: "CHANNEL (freq 6:0, unclassified 7)",
        expected: 0x0000_00ff,
        write_read: |mac| {
            // SAFETY: the idle MAC has no user; the probe restores nothing.
            mac.channel().modify(|_, w| unsafe { w.bits(u32::MAX) });
            mac.channel().read().bits()
        },
    },
    Probe {
        name: "TX_POWER (power 4:0)",
        expected: 0x0000_001f,
        write_read: |mac| {
            // SAFETY: as above.
            mac.tx_power().modify(|_, w| unsafe { w.bits(u32::MAX) });
            mac.tx_power().read().bits()
        },
    },
    Probe {
        name: "EVENT_ENABLE (events 12:0)",
        expected: 0x0000_1fff,
        write_read: |mac| {
            // SAFETY: as above; no event source is running.
            mac.event_enable()
                .modify(|_, w| unsafe { w.bits(u32::MAX) });
            mac.event_enable().read().bits()
        },
    },
    Probe {
        name: "COEX_PTI (pti 3:0, ack 7:4, close_rf_sel 8)",
        expected: 0x0000_01ff,
        write_read: |mac| {
            // SAFETY: as above.
            mac.coex_pti().modify(|_, w| unsafe { w.bits(u32::MAX) });
            mac.coex_pti().read().bits()
        },
    },
];

const DEVICES: [ModemClockDevice; 5] = [
    ModemClockDevice::Coexistence,
    ModemClockDevice::Etm,
    ModemClockDevice::BluetoothApb,
    ModemClockDevice::BluetoothIeee802154CommonBaseband,
    ModemClockDevice::Ieee802154Mac,
];

/// Bits of MODEM_SYSCON CLK_CONF, MODEM_RST_CONF and CLK_CONF1 and of
/// MODEM_LPCON CLK_CONF that the published fields own.
const OWNED: [u32; 4] = [0x11c0_0000, 0x0180_0000, 0x0003_0000, 0x0000_0002];

/// The four modem clock words, read through esp-pacs so the owner under test
/// is not also the observer.
fn modem_clock_words() -> [u32; 4] {
    // SAFETY: shared read-only views; reads have no side effects.
    let syscon = unsafe { &*esp32c5::MODEM_SYSCON::ptr() };
    // SAFETY: as above.
    let lpcon = unsafe { &*esp32c5::MODEM_LPCON::ptr() };
    [
        syscon.clk_conf().read().bits(),
        syscon.modem_rst_conf().read().bits(),
        syscon.clk_conf1().read().bits(),
        lpcon.clk_conf().read().bits(),
    ]
}

fn check(failures: &mut u32, name: &str, ok: bool) {
    let verdict = if ok {
        "MATCH"
    } else {
        *failures += 1;
        "DIFF"
    };
    println!("PROBE {verdict} {name}");
}

/// Toggle every device off and on, pulse the MAC reset, and leave every
/// device enabled for the MAC probes.
fn probe_modem_clock(clock: &mut ModemClockRegisters, failures: &mut u32) {
    let before = modem_clock_words();
    for device in DEVICES {
        clock.configure_modem_clock_device(device, false);
        let off = !clock.modem_clock_device_enabled(device);
        clock.configure_modem_clock_device(device, true);
        let on = clock.modem_clock_device_enabled(device);
        println!("PROBE {:?}", device);
        check(failures, "  disable then enable", off && on);
    }
    clock.pulse_ieee802154_mac_reset();
    let reset = clock.ieee802154_reset_observation();
    check(
        failures,
        "IEEE 802.15.4 MAC reset pulse releases both lines",
        reset.mac_released && reset.apb_released,
    );
    let after = modem_clock_words();
    for (index, owned) in OWNED.iter().enumerate() {
        println!(
            "PROBE word {index}: before {:#010x} after {:#010x}",
            before[index], after[index]
        );
        check(
            failures,
            "  bits outside the published fields preserved",
            before[index] & !owned == after[index] & !owned,
        );
    }
}

/// ESP32-C5 rev 1.0 ROM `phy_chip_i2c_readReg_org(block, mask, host, reg)`.
const ROM_PHY_CHIP_I2C_READREG_ORG: usize = 0x4000_754a;

/// Blocks read: BBPLL, BIAS, DIG_REG and ULP_CAL of ESP-IDF's `regi2c`.
const I2C_BLOCKS: [u8; 4] = [0x66, 0x6a, 0x6d, 0x61];
/// Registers read in each block.
const I2C_REGISTERS: u8 = 16;
/// Busy polls before a read is reported incomplete.
const I2C_POLLS: u32 = 100_000;

fn pac_read(i2c: &mut PhyI2cRegisters, address: PhyI2cAddress) -> Option<u8> {
    for _ in 0..I2C_POLLS {
        if i2c.try_start_read(address).is_ok() {
            for _ in 0..I2C_POLLS {
                if let Ok(value) = i2c.try_finish_read(address) {
                    return Some(value);
                }
            }
            return None;
        }
    }
    None
}

fn rom_read(address: PhyI2cAddress) -> u8 {
    let block = address.block();
    let mask = !block.read_mask_complement_low();
    let host = match block.host() {
        PhyI2cHost::Host0 => 0,
        PhyI2cHost::Host1 => 1,
    };
    // SAFETY: the address is the ESP32-C5 rev 1.0 ROM leaf of this ABI; it
    // touches only the analog I2C master, which the probe owns, and the PAC
    // read just completed on the same host, so its busy poll terminates.
    let leaf: extern "C" fn(u32, u32, u32, u32) -> u32 =
        unsafe { core::mem::transmute(ROM_PHY_CHIP_I2C_READREG_ORG) };
    leaf(
        u32::from(block.code()),
        mask,
        host,
        u32::from(address.register()),
    ) as u8
}

fn probe_phy_i2c(i2c: &mut PhyI2cRegisters, failures: &mut u32) {
    for code in I2C_BLOCKS {
        let block = PhyI2cBlock::from_vendor_abi(code).expect("reviewed block");
        let mut differences = 0;
        let mut incomplete = false;
        for register in 0..I2C_REGISTERS {
            let address = PhyI2cAddress::new(block, register);
            let Some(ours) = pac_read(i2c, address) else {
                incomplete = true;
                break;
            };
            let vendor = rom_read(address);
            if ours != vendor {
                differences += 1;
                println!(
                    "PROBE   block {code:#04x} reg {register}: ours {ours:#04x} rom {vendor:#04x}"
                );
            }
        }
        let verdict = if incomplete {
            *failures += 1;
            "INCOMPLETE"
        } else if differences == 0 {
            "MATCH"
        } else {
            *failures += 1;
            "DIFF"
        };
        println!("PROBE {verdict} analog I2C block {code:#04x} registers 0..{I2C_REGISTERS}");
    }
}

fn drive<T>(mut poll: impl FnMut() -> Step<T>) -> Option<T> {
    for _ in 0..I2C_POLLS {
        if let Step::Ready(value) = poll() {
            return Some(value);
        }
    }
    None
}

fn probe_analog_bus(bus: &mut AnalogI2c, failures: &mut u32) {
    let bbpll = PhyI2cBlock::from_vendor_abi(0x66).expect("BBPLL");
    let mut differences = 0;
    let mut incomplete = false;
    for register in 0..I2C_REGISTERS {
        let address = PhyI2cAddress::new(bbpll, register);
        let field = AnalogField::new(address, 7, 0).expect("whole byte");
        let mut read = FieldRead::new(field);
        let Some(ours) = drive(|| read.poll(bus)) else {
            incomplete = true;
            break;
        };
        if ours != rom_read(address) {
            differences += 1;
        }
    }
    let verdict = match (incomplete, differences) {
        (true, _) => "INCOMPLETE",
        (false, 0) => "MATCH",
        _ => "DIFF",
    };
    if verdict != "MATCH" {
        *failures += 1;
    }
    println!("PROBE {verdict} HAL field reads of block 0x66 against the ROM leaf");

    let ulp_cal = PhyI2cBlock::from_vendor_abi(0x61).expect("ULP_CAL");
    let mut unchanged = true;
    for register in 0..4 {
        let address = PhyI2cAddress::new(ulp_cal, register);
        let field = AnalogField::new(address, 7, 0).expect("whole byte");
        let before = rom_read(address);
        let mut write = FieldWrite::new(field, before).expect("byte fits");
        if drive(|| write.poll(bus)).is_none() {
            unchanged = false;
            break;
        }
        unchanged &= rom_read(address) == before;
    }
    check(
        failures,
        "HAL field write of each ULP_CAL register's own value leaves it unchanged",
        unchanged,
    );
}

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    let mut failures = 0;
    let RadioPartitions {
        ieee802154: _ieee802154,
        mut modem_clock,
        mut phy_i2c,
        ..
    } = RadioPartitions::take().expect("first radio partition acquisition");
    probe_modem_clock(&mut modem_clock, &mut failures);
    check(
        &mut failures,
        "analog I2C master clock enabled by esp-hal",
        modem_clock.analog_i2c_master_clock_enabled(),
    );
    probe_phy_i2c(&mut phy_i2c, &mut failures);
    let mut bus = AnalogI2c::new(phy_i2c);
    probe_analog_bus(&mut bus, &mut failures);

    // SAFETY: the probe is the sole user of the IEEE 802.15.4 MAC.
    let mac = unsafe { Ieee802154Mac::steal() };
    for probe in PROBES {
        let observed = (probe.write_read)(&mac);
        let verdict = if observed == probe.expected {
            "MATCH"
        } else {
            failures += 1;
            "DIFF"
        };
        println!(
            "PROBE {verdict} {}: observed {observed:#010x} expected {:#010x}",
            probe.name, probe.expected
        );
    }
    println!("PROBE-DONE failures={failures}");
    let delay = esp_hal::delay::Delay::new();
    loop {
        delay.delay_millis(1000);
    }
}
