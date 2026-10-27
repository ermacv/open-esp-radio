//! P-EST/P-FREQ sheets. Preparation and sampling live here, outside production.
#![cfg(target_arch = "riscv32")]
#![deny(unsafe_code)]
use core::sync::atomic::{AtomicU32, Ordering};
use oer_esp32s31_hal::{
    owner::SharedPhyAccess,
    phy::{clock, delay::RomShortDelay, frequency as freq, iq_estimator as iq, pbus, rx_dco},
};

oer_memory::zeroed_static! {
    static EST_FREQ_OUTPUT: [AtomicU32; 2050] = zeroed in ".critical.bss.est_freq";
    static EST_FREQ_REQUEST: AtomicU32 = zeroed in ".critical.bss.est_freq_request";
}

oer_probe_macros::probe! {
    /// SRAM address for debugger access while halted: 0 journal, 1 request.
    /// These are addresses of retained statics, not independently owned hardware.
    pub fn open_phy_est_freq_buffer_address(buffer: u32) -> u32 {
        match buffer {
            0 => core::ptr::addr_of!(EST_FREQ_OUTPUT) as usize as u32,
            1 => core::ptr::addr_of!(EST_FREQ_REQUEST) as usize as u32,
            _ => 0,
        }
    }
}

pub(crate) fn take_request() -> u32 {
    EST_FREQ_REQUEST.swap(u32::MAX, Ordering::AcqRel)
}

pub(crate) fn status(value: u32) {
    if let Some(word) = EST_FREQ_OUTPUT.first() {
        word.store(value, Ordering::Release);
    }
}
pub(crate) fn clear() {
    for word in &EST_FREQ_OUTPUT {
        word.store(0, Ordering::Relaxed);
    }
}
pub(crate) fn emit(tag: u32, a: u32, b: u32, c: u32) {
    let Some(count) = EST_FREQ_OUTPUT.get(1) else {
        return;
    };
    let at = 2usize.saturating_add((count.load(Ordering::Relaxed) as usize).saturating_mul(4));
    if let Some(row) = EST_FREQ_OUTPUT.get(at..at.saturating_add(4)) {
        for (slot, value) in row.iter().zip([tag, a, b, c]) {
            slot.store(value, Ordering::Relaxed);
        }
        count.fetch_add(1, Ordering::Release);
    } else {
        status(0x1950_bad1);
    }
}

oer_probe_macros::probe! {
    /// 151: P-EST's two identical windows; 152: P-FREQ's read/reset sequence.
    /// This entry borrows the calibrated session. Unknown selections issue no MMIO.
    pub fn open_phy_est_freq_sheet(selection: u32) -> u32 {
        if !matches!(selection, 151 | 152) { return 1; }
        clear();
        status(0x1950_0002);
        let result = crate::session::with(|session| {
            if !session.calibrated { return Err(13); }
            if !session.measurement_clocks(true) { return Err(2); }
            let result = {
                let mut phy = session.radio.phy_hal();
                if selection == 151 { estimator(&mut phy) } else { frequency(&mut phy) }
            };
            if !session.measurement_clocks(false) { return Err(3); }
            result
        });
        let code = match result { Some(Ok(())) => 0, Some(Err(reason)) => reason, None => 4 };
        emit(selection, code, oer_riscv::cycle32(), 0);
        status(if code == 0 { 0x1950_0003 } else { 0x1950_bad0 });
        code
    }
}

fn settle() -> Result<(), u32> {
    if RomShortDelay::settle_micros(1) {
        Ok(())
    } else {
        Err(5)
    }
}
fn force(phy: &mut impl SharedPhyAccess, selector: u8, path: u8, value: u16) -> Result<(), u32> {
    pbus::start_force_test(phy, selector, path, value);
    for _ in 0..20 {
        if pbus::try_finish_force_test(phy).is_ok() {
            return Ok(());
        }
        settle()?;
    }
    pbus::try_finish_force_test(phy).map_err(|_| 6)
}
fn pairs(phy: &mut impl SharedPhyAccess, tag: u32) {
    for _ in 0..100 {
        let before = oer_riscv::cycle32();
        let value = iq::sample_indications(phy);
        emit(
            tag,
            before,
            u32::from(value.ready),
            u32::from(value.opaque_pair),
        );
    }
}
fn estimator(phy: &mut impl oer_esp32s31_hal::owner::SharedPhyContext) -> Result<(), u32> {
    let saved = iq::capture_configuration(phy);
    let original = saved.fields();
    emit(
        0x1550,
        u32::from(original.configuration_code),
        u32::from(original.control_code),
        u32::from(original.window),
    );
    emit(
        0x1551,
        u32::from(original.start_enabled),
        u32::from(original.measurement_enabled),
        0,
    );
    if saved.enabled() {
        return Err(7);
    }
    let calibration_clock = clock::calibration_clock_enabled(phy);
    phy.set_phy_calibration_clock(true);
    // SOURCE: phy/analog/crystal_duty.rs::XtalDutyPrepareTransition. The complete
    // production registration already calibrated RX-DCO and settled RFPLL; this
    // leaf reopens its internal stimulus, clocks and forced paths in that epoch.
    phy.configure_calibration_tone(true, 0x80, 0);
    pbus::configure_rx_clock(phy, true);
    pbus::configure_tx_clock(phy, true);
    pbus::configure_debug_mode(phy);
    let prepared = (|| {
        for (selector, path, value) in [
            (4, 1, 0),
            (4, 2, 1),
            (5, 1, 0),
            (0, 1, 0x40),
            (0, 2, 0xbf),
            (1, 1, 0x189),
            (1, 2, 0xf0),
            (0, 1, 0x43),
            (1, 1, 0x38),
            (1, 1, 0x189),
        ] {
            force(phy, selector, path, value)?;
        }
        rx_dco::prepare_control_restore(phy).map_err(|_| 8_u32)
    })();
    let bus_usable = prepared != Err(6);
    let result = if prepared.is_ok() {
        let measured = (|| {
            for repeat in 0..2 {
                iq::configure(phy, 1000);
                iq::set_start_enabled(phy, true);
                settle()?;
                iq::set_measurement_enabled(phy, true);
                pairs(phy, 0x1510 + repeat * 2);
                emit(
                    0x1540 + repeat * 2,
                    iq::read_total_power(phy) as u32,
                    oer_riscv::cycle32(),
                    1,
                );
                iq::set_measurement_enabled(phy, false);
                settle()?;
                iq::set_start_enabled(phy, false);
                pairs(phy, 0x1511 + repeat * 2);
                emit(
                    0x1541 + repeat * 2,
                    iq::read_total_power(phy) as u32,
                    oer_riscv::cycle32(),
                    0,
                );
            }
            Ok(())
        })();
        let restored = rx_dco::restore_control(phy).map_err(|_| 9_u32);
        measured.and(restored)
    } else {
        prepared
    };
    iq::set_measurement_enabled(phy, false);
    let delay_result = settle();
    iq::set_start_enabled(phy, false);
    iq::restore_configuration(phy, saved);
    let restored = iq::capture_configuration(phy).fields();
    emit(
        0x1552,
        u32::from(restored.configuration_code),
        u32::from(restored.control_code),
        u32::from(restored.window),
    );
    emit(
        0x1553,
        u32::from(restored.start_enabled),
        u32::from(restored.measurement_enabled),
        0,
    );
    phy.configure_calibration_tone(false, 0x80, 0x28);
    pbus::configure_rx_clock(phy, false);
    pbus::configure_tx_clock(phy, false);
    let mut cleanup = Ok(());
    for (selector, path) in [(0, 1), (1, 1), (1, 2)] {
        if !bus_usable || cleanup.is_err() {
            break;
        }
        cleanup = force(phy, selector, path, 0);
    }
    if bus_usable && cleanup.is_ok() && pbus::configure_work_mode(phy) {
        cleanup = cleanup.and(settle());
        oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(phy);
        cleanup = cleanup.and(settle());
        oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(phy);
    }
    phy.set_phy_calibration_clock(calibration_clock);
    result
        .and(delay_result)
        .and(cleanup)
        .and(if restored == original {
            Ok(())
        } else {
            Err(14)
        })
}
fn observation(phy: &mut impl SharedPhyAccess, tag: u32) -> Result<(), u32> {
    let Some(value) = freq::observe(phy) else {
        return Err(10);
    };
    emit(
        tag,
        value.control_image,
        value.parameter_image,
        value.result,
    );
    emit(
        tag + 1,
        u32::from(value.high_address_or_reset),
        u32::from(value.ready),
        oer_riscv::cycle32(),
    );
    Ok(())
}
fn frequency(phy: &mut impl SharedPhyAccess) -> Result<(), u32> {
    let Some(saved) = freq::capture_read_configuration(phy) else {
        return Err(11);
    };
    let original = saved.fields();
    emit(
        0x1532,
        u32::from(original.address_low),
        u32::from(original.high_address_or_reset),
        u32::from(original.read_mode),
    );
    emit(0x1533, u32::from(original.hardware_disabled), 0, 0);
    let result = (|| {
        observation(phy, 0x1520)?;
        freq::set_hardware_control(phy, false);
        if !RomShortDelay::settle_micros(2) {
            return Err(5);
        }
        observation(phy, 0x1522)?;
        // SOURCE: PhyFrequencyI2cTransition's final descriptor initializes
        // address 0 with the BBTOP03 command in a complete cold registration.
        for (address, tag) in [(0, 0x1524), (0x400, 0x1526), (0, 0x1528)] {
            let value = freq::read_memory(phy, address, 0);
            emit(tag + 0x100, u32::from(address), value, oer_riscv::cycle32());
            observation(phy, tag)?;
            if tag == 0x1524 {
                use oer_esp32s31_hal::phy::i2c::analog_i2c;
                let expected = analog_i2c::bbtop::shared_rx_gain_calibration_enable::ADDRESS;
                // SOURCE: hw19-20261027-est-freq-01, freq/freq-repeat, tag1624;
                // oer-notes/research/hw-est-freq.md. Mode0 returned 0x67 from
                // the initialized low entry. It happened to match expected.block();
                // the guard adopts that observed byte, without establishing its
                // decoding or a general memory-bank geometry. Run03 matched again.
                let reference = u32::from(expected.block());
                emit(0x1530, value, reference, u32::from(value == reference));
                if value != reference {
                    return Err(12);
                }
            }
        }
        freq::reset_module(phy);
        observation(phy, 0x152a)?;
        let value = freq::read_memory(phy, 0, 0);
        emit(0x162c, 0, value, oer_riscv::cycle32());
        observation(phy, 0x152c)
    })();
    freq::restore_read_configuration(phy, saved);
    let restored = freq::capture_read_configuration(phy)
        .ok_or(11_u32)?
        .fields();
    emit(
        0x1534,
        u32::from(restored.address_low),
        u32::from(restored.high_address_or_reset),
        u32::from(restored.read_mode),
    );
    emit(0x1535, u32::from(restored.hardware_disabled), 0, 0);
    result
        .and(observation(phy, 0x152e))
        .and(if restored == original {
            Ok(())
        } else {
            Err(14)
        })
}
