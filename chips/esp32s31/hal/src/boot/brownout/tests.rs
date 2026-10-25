//! The brownout policy over host memory from LP_ANA_PERI's base and a fake
//! analog bus, against ESP-IDF 4d59230d `lp_analog_peri_reg.h`
//! (`BOD_MODE0_CNTL` 0x0: CLOSE_FLASH_ENA 6, PD_RF_ENA 7, INTR_WAIT 17:8,
//! RESET_WAIT 27:18, CNT_CLR 28, INTR_ENA 29, RESET_SEL 30, RESET_ENA 31;
//! `BOD_MODE1_CNTL` 0x4: RESET_ENA 31; `FIB_ENABLE` 0x1c) and
//! `brownout_ll.h:22` (the mode-1 FIB bit 1).

use core::cell::RefCell;
use std::{vec, vec::Vec};

use oer_esp32s31_pac::{analog_i2c, instances};
use oer_espressif_analog::{AnalogRegisterBus, Busy};
use oer_espressif_boot::wait::{BootBudget, BootDeadline, WaitFailure};
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::{BrownoutFailure, BrownoutRegisters, THRESHOLD_WAIT, apply_idf_policy};

/// Virtual time that moves a microsecond at every read.
struct Ticking<'a>(&'a VirtualClock);

impl Clock for Ticking<'_> {
    fn now(&self) -> Instant {
        self.0
            .advance(Duration::from_micros(1))
            .expect("the test's virtual time does not overflow")
    }
}

fn budget() -> BootBudget {
    BootBudget::below(Duration::from_millis(10), Duration::from_secs(9)).expect("below")
}

/// An analog bus over one byte per address, each command done at once, or
/// never when `stuck`; it records every command.
struct Bus {
    byte: u8,
    stuck: bool,
    log: RefCell<Vec<&'static str>>,
}

impl AnalogRegisterBus for Bus {
    type Address = analog_i2c::Address;

    fn try_start_read(&mut self, address: Self::Address) -> Result<(), Busy> {
        assert_eq!(address, analog_i2c::ulp_bod::i2c_bod_threshold::ADDRESS);
        self.log.borrow_mut().push("read");
        Ok(())
    }

    fn try_finish_read(&self, _: Self::Address) -> Result<u8, Busy> {
        if self.stuck { Err(Busy) } else { Ok(self.byte) }
    }

    fn try_start_write(&mut self, address: Self::Address, value: u8) -> Result<(), Busy> {
        assert_eq!(address, analog_i2c::ulp_bod::i2c_bod_threshold::ADDRESS);
        self.log.borrow_mut().push("write");
        self.byte = value;
        Ok(())
    }

    fn try_finish_write(&self, _: Self::Address) -> Result<(), Busy> {
        if self.stuck { Err(Busy) } else { Ok(()) }
    }
}

fn memory() -> Vec<u32> {
    let mut words = vec![0; instances::lp_ana_peri::SIZE / 4];
    // The bootloader's mode-1 reset: FIB all ones, mode 1 armed.
    words[0x1c / 4] = u32::MAX;
    words[1] = 1 << 31; // BOD_MODE1_CNTL at 0x4
    words
}

#[test]
fn the_policy_arms_mode_0_writes_the_threshold_and_reads_back() {
    let mut words = memory();
    let mut bus = Bus {
        byte: 0b1111_0000,
        stuck: false,
        log: RefCell::new(Vec::new()),
    };
    let virtual_clock = VirtualClock::new();
    let clock = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&clock, budget());
    let mut registers = BrownoutRegisters::host(&mut words).expect("memory holds the block");
    assert_eq!(
        apply_idf_policy(&mut registers, &mut bus, &deadline),
        Ok(())
    );
    drop(registers);
    assert_eq!(words[0x1c / 4], !(1 << 1), "mode 1 under software");
    assert_eq!(words[1], 0, "mode 1's reset disarmed");
    assert_eq!(
        words[0],
        (1 << 6) | (1 << 7) | (2 << 8) | (0x3ff << 18) | (1 << 29) | (1 << 30) | (1 << 31),
        "mode 0: flash and RF actions, waits, event and system reset, the count cleared again"
    );
    assert_eq!(
        bus.byte, 0b1111_0111,
        "threshold 7 in bits 2:0, the byte kept"
    );
    assert_eq!(*bus.log.borrow(), ["read", "write"]);
}

#[test]
fn a_threshold_the_bus_never_finishes_is_a_named_timeout_before_the_event_is_armed() {
    let mut words = memory();
    let mut bus = Bus {
        byte: 0,
        stuck: true,
        log: RefCell::new(Vec::new()),
    };
    let virtual_clock = VirtualClock::new();
    let clock = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&clock, budget());
    let mut registers = BrownoutRegisters::host(&mut words).expect("memory holds the block");
    assert_eq!(
        apply_idf_policy(&mut registers, &mut bus, &deadline),
        Err(BrownoutFailure::Threshold(WaitFailure::TimedOut(
            THRESHOLD_WAIT.name()
        )))
    );
    drop(registers);
    assert_eq!(words[0] & (1 << 29), 0, "the event is not armed");
}
