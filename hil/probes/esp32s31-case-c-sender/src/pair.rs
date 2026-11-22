//! What both images of the radio pair answer besides their own steps: the
//! boot's two snapshots in pages and the receive path's counters
//! (`oer-hil-services-radio-pair`'s `Radio` endpoints).

use oer_chips_esp32s31_hal::{
    radio::domain::transaction::Operation,
    radio::ieee80211::dma::receive::{NextAt, Standing},
    system::owner::RadioAccess,
};
use oer_chips_esp32s31_radio_phy::{PhyCalibrationIdentity, PhyCalibrationSnapshot};
use oer_hil_services_radio_pair as service;
use service::{Item, PAGE_READINGS, Reading, Refusal, SnapshotPage, Value};

use crate::radio::Radio;
use oer_chips_esp32s31_hal::power::ChipClocks;
use oer_chips_esp32s31_hal::radio::phy::power_words::ModemBus;

/// The most readings a snapshot holds: the bring-up's registers, the
/// temperature code, the RF PLL's four words and every calibration result.
pub(crate) const READINGS: usize = 224;

/// A snapshot's readings, in the order the host prints them.
pub(crate) type Readings = heapless::Vec<Reading, READINGS>;

/// Page `page` of `readings`, or the refusal of a page past the last.
pub(crate) fn page(readings: &[Reading], page: u8) -> SnapshotPage {
    let length = readings.len();
    let pages = length / PAGE_READINGS + usize::from(!length.is_multiple_of(PAGE_READINGS));
    let start = usize::from(page).saturating_mul(PAGE_READINGS);
    let Some(rest) = readings.get(start..) else {
        return SnapshotPage::Refused(Refusal::NoSuchPage);
    };
    if usize::from(page) >= pages.max(1) {
        return SnapshotPage::Refused(Refusal::NoSuchPage);
    }
    SnapshotPage::Readings {
        page,
        pages: u8::try_from(pages.max(1)).unwrap_or(u8::MAX),
        readings: rest.iter().take(PAGE_READINGS).copied().collect(),
    }
}

/// The snapshot before the bring-up: the reset cause, and the radio's and
/// the platform clocks' power, clock and reset words as the previous run
/// left them.
pub(crate) fn cold(reset_cause: u32, radio: &Radio, clocks: &ChipClocks) -> Readings {
    let mut readings = Readings::new();
    push(&mut readings, Item::ResetCause, 0, Value::Word(reset_cause));
    power(&mut readings, radio, clocks);
    readings
}

/// The radio's power, clock and reset words ([`Radio::observe_power`]) and
/// the platform clocks' ([`ChipClocks::observe`]): a modem block's word
/// unreadable while its register bus is closed, the platform clocks' while
/// another context holds them.
fn power(readings: &mut Readings, radio: &Radio, clocks: &ChipClocks) {
    let (words, bus) = radio.observe_power();
    let platform = clocks.observe(&bus);
    let (pll, lpcon) = match platform {
        Ok(platform) => (
            Value::Word(platform.hp_sys_clkrst_ref_160m_ctrl0),
            platform
                .modem_lpcon_clk_conf
                .map_or(Value::Unreadable, Value::Word),
        ),
        Err(_) => (Value::Unreadable, Value::Unreadable),
    };
    push(readings, Item::HpSysClkrstRef160mCtrl0, 0, pll);
    push(readings, Item::ModemLpconClkConf, 0, lpcon);
    for (item, word) in [
        (Item::PmuHpActiveIcgModem, words.pmu_hp_active_icg_modem),
        (Item::PmuHpActiveHpCkPower, words.pmu_hp_active_hp_ck_power),
        (Item::PmuRfPwc, words.pmu_rf_pwc),
        (Item::PmuAnaPeriPwrCtrl, words.pmu_ana_peri_pwr_ctrl),
        (Item::HpSysClkrstModemCtrl0, words.hp_sys_clkrst_modem_ctrl0),
        (Item::HpSysClkrstModemConf, words.hp_sys_clkrst_modem_conf),
    ] {
        push(readings, item, 0, Value::Word(word));
    }
    let modem = match bus {
        ModemBus::Open { words, .. } => Some(words),
        ModemBus::Closed => None,
    };
    for (item, word) in [
        (Item::ModemSysconClkConf, modem.map(|m| m.syscon_clk_conf)),
        (Item::ModemSysconClkConf1, modem.map(|m| m.syscon_clk_conf1)),
        (
            Item::ModemSysconClkConfPowerSt,
            modem.map(|m| m.syscon_clk_conf_power_st),
        ),
        (
            Item::ModemSysconModemRstConf,
            modem.map(|m| m.syscon_modem_rst_conf),
        ),
        (
            Item::ModemSysconWifiBbCfg,
            modem.map(|m| m.syscon_wifi_bb_cfg),
        ),
        (
            Item::ModemLpconLpTimerConf,
            modem.map(|m| m.lpcon_lp_timer_conf),
        ),
        (
            Item::ModemLpconCoexLpClkConf,
            modem.map(|m| m.lpcon_coex_lp_clk_conf),
        ),
        (
            Item::ModemLpconWifiLpClkConf,
            modem.map(|m| m.lpcon_wifi_lp_clk_conf),
        ),
        (
            Item::ModemLpconModem32kClkConf,
            modem.map(|m| m.lpcon_modem_32k_clk_conf),
        ),
        (
            Item::ModemLpconClkConfPowerSt,
            modem.map(|m| m.lpcon_clk_conf_power_st),
        ),
        (Item::ModemLpconTickConf, modem.map(|m| m.lpcon_tick_conf)),
    ] {
        push(
            readings,
            item,
            0,
            word.map_or(Value::Unreadable, Value::Word),
        );
    }
}

/// The snapshot after the bring-up: the cold snapshot's words again, the
/// temperature code, the RF PLL's frequency block and the PHY's calibration
/// results, read in one PHY transaction over the radio in `slot`.
///
/// # Errors
/// [`Refusal::NotUp`] when no radio is in `slot`.
pub(crate) fn up(
    slot: &mut Option<Radio>,
    clocks: &ChipClocks,
    reset_cause: u32,
    identity: PhyCalibrationIdentity,
) -> Result<Readings, Refusal> {
    let mut readings = cold(reset_cause, slot.as_ref().ok_or(Refusal::NotUp)?, clocks);
    let radio = slot.take().ok_or(Refusal::NotUp)?;
    let calibration = radio
        .attachment()
        .calibration_cache(identity)
        .ok()
        .map(|cache| cache.into_snapshot());
    let mut transaction = radio.detach(Operation::Maintain);
    let hardware = transaction.hardware_mut();
    let temperature = oer_chips_esp32s31_hal::radio::phy::temperature::read_code(hardware);
    let frequency = oer_chips_esp32s31_hal::radio::phy::frequency::observe(hardware);
    *slot = Some(transaction.reunite());
    push(
        &mut readings,
        Item::TemperatureCode,
        0,
        Value::Word(u32::from(temperature)),
    );
    match frequency {
        Some(observed) => {
            push(
                &mut readings,
                Item::RfpllControl,
                0,
                Value::Word(observed.control_image),
            );
            push(
                &mut readings,
                Item::RfpllParameter,
                0,
                Value::Word(observed.parameter_image),
            );
            push(
                &mut readings,
                Item::RfpllResult,
                0,
                Value::Word(observed.result),
            );
            push(
                &mut readings,
                Item::RfpllReady,
                0,
                Value::Word(u32::from(observed.ready)),
            );
        }
        None => {
            for item in [
                Item::RfpllControl,
                Item::RfpllParameter,
                Item::RfpllResult,
                Item::RfpllReady,
            ] {
                push(&mut readings, item, 0, Value::Unreadable);
            }
        }
    }
    if let Some(snapshot) = calibration {
        calibration_readings(&mut readings, &snapshot);
    }
    Ok(readings)
}

fn push(readings: &mut Readings, item: Item, index: usize, value: Value) {
    let _full = readings.push(Reading {
        item,
        index: u8::try_from(index).unwrap_or(u8::MAX),
        value,
    });
}

fn signed(readings: &mut Readings, item: Item, values: impl IntoIterator<Item = i32>) {
    for (index, value) in values.into_iter().enumerate() {
        push(readings, item, index, Value::Signed(value));
    }
}

fn words(readings: &mut Readings, item: Item, values: impl IntoIterator<Item = u32>) {
    for (index, value) in values.into_iter().enumerate() {
        push(readings, item, index, Value::Word(value));
    }
}

/// The calibration results, field by field, arrays flattened in order.
fn calibration_readings(readings: &mut Readings, snapshot: &PhyCalibrationSnapshot) {
    let common = &snapshot.common;
    signed(
        readings,
        Item::CalibrationTemperature,
        [i32::from(common.temperature)],
    );
    signed(
        readings,
        Item::CalibrationRfpllReferenceTemperature,
        [i32::from(common.rfpll_reference_temperature)],
    );
    signed(
        readings,
        Item::CalibrationRxReferenceTemperature,
        [i32::from(common.rxcal_reference_temperature)],
    );
    signed(
        readings,
        Item::CalibrationTxReferenceTemperature,
        [i32::from(common.txcal_reference_temperature)],
    );
    signed(
        readings,
        Item::CalibrationPowerReferenceTemperature,
        [i32::from(common.power_reference_temperature)],
    );
    words(
        readings,
        Item::CalibrationSensorIndex,
        [u32::from(common.sensor_index)],
    );
    words(
        readings,
        Item::CalibrationCrystalSelector,
        [u32::from(common.crystal_selector)],
    );
    words(
        readings,
        Item::CalibrationRcResult,
        [u32::from(common.rc_result)],
    );
    words(
        readings,
        Item::CalibrationFilterDcap,
        common.filter_dcap.map(u32::from),
    );
    words(
        readings,
        Item::CalibrationRcCalibrated,
        [u32::from(common.rc_calibrated)],
    );
    words(
        readings,
        Item::CalibrationDcode,
        common.dcode.map(u32::from),
    );
    words(
        readings,
        Item::CalibrationI2cFrequencyParameter,
        [u32::from(common.i2c_frequency_parameter)],
    );
    words(
        readings,
        Item::CalibrationXtalDuty,
        common.xtal_duty.map(u32::from),
    );
    let wifi = &snapshot.wifi;
    signed(
        readings,
        Item::WifiTxGainAdjustment,
        [i32::from(wifi.tx_gain_adjustment)],
    );
    words(
        readings,
        Item::WifiCalibratedFlags,
        [
            wifi.baseband_calibrated,
            wifi.pwdet_calibrated,
            wifi.tx_power_calibrated,
            wifi.tx_iq_calibrated,
            wifi.rx_gain_dc_calibrated,
            wifi.rx_gain_tables_initialized,
            wifi.rx_saturation_detected,
        ]
        .map(u32::from),
    );
    words(
        readings,
        Item::WifiTxDco,
        wifi.tx_dco.iter().flatten().copied().map(u32::from),
    );
    signed(
        readings,
        Item::WifiTxReferenceCodes,
        wifi.tx_reference_codes.map(i32::from),
    );
    words(
        readings,
        Item::WifiTxCapacitance,
        wifi.tx_capacitance.map(u32::from),
    );
    signed(
        readings,
        Item::WifiTxPowerCurve,
        wifi.tx_power_curve.map(i32::from),
    );
    signed(
        readings,
        Item::WifiTxPowerCorrections,
        wifi.tx_power_corrections.map(i32::from),
    );
    signed(
        readings,
        Item::WifiTxPowerAdjustment,
        [i32::from(wifi.tx_power_adjustment)],
    );
    words(
        readings,
        Item::WifiCalibratedAttenuation,
        [u32::from(wifi.calibrated_attenuation)],
    );
    words(
        readings,
        Item::WifiTxIqConfig,
        [u32::from(wifi.tx_iq_config)],
    );
    words(
        readings,
        Item::WifiTxIqCoefficient,
        [u32::from(wifi.tx_iq_coefficient)],
    );
    words(
        readings,
        Item::WifiRxIqCoefficients,
        wifi.rx_iq_coefficients.map(u32::from),
    );
    words(
        readings,
        Item::WifiExternalDcode,
        wifi.external_dcode.map(u32::from),
    );
    signed(
        readings,
        Item::WifiCalibrationTemperature,
        [i32::from(wifi.calibration_temperature)],
    );
    words(
        readings,
        Item::WifiCalibrationChannel,
        [u32::from(wifi.calibration_channel)],
    );
    words(
        readings,
        Item::WifiRxTableLastIndex,
        [u32::from(wifi.wifi_rx_table_last_index)],
    );
    words(
        readings,
        Item::WifiSharedRxTableLastIndex,
        [u32::from(wifi.shared_rx_table_last_index)],
    );
    words(
        readings,
        Item::WifiIndexDc,
        wifi.wifi_index_dc.iter().flatten().copied().map(u32::from),
    );
    words(
        readings,
        Item::WifiFineDc,
        wifi.wifi_fine_dc.iter().flatten().copied().map(u32::from),
    );
    words(
        readings,
        Item::WifiSharedIndexDc,
        wifi.shared_index_dc
            .iter()
            .flatten()
            .copied()
            .map(u32::from),
    );
}

/// The receive path's counters now: the MAC's, as the hardware holds them,
/// and the ring's walker and descriptors.
pub(crate) fn counters(
    access: &RadioAccess,
    ring: Option<service::RingState>,
) -> service::Counters {
    let statistics = access.receive_statistics_snapshot();
    let primary = statistics.primary;
    let decode = statistics.decode_errors;
    let hang = statistics.hang;
    service::Counters::Counted {
        mac: service::MacCounters {
            mpdu: primary.mpdu_count,
            fcs_error: primary.fcs_error,
            abort: primary.abort,
            abort_fcs_pass: primary.abort_fcs_pass,
            power_drop_error: primary.power_drop_error,
            signal_field: primary.signal_field,
            end: primary.end,
            data_success: primary.data_success,
            other_unicast: primary.other_unicast,
            buffer_full: primary.buffer_full,
            fifo_overflow: primary.fifo_overflow,
            last_unmatched_error: primary.last_unmatched_error,
            ack_interrupt: primary.ack_interrupt,
        },
        decode: service::DecodeCounters {
            brx_agc: decode.brx_agc,
            brx: decode.brx,
            nrx: decode.nrx,
            nrx_abort: decode.nrx_abort,
            nrx_agc_exit: decode.nrx_agc_exit,
            nrx_baseband_off: decode.nrx_baseband_off,
            nrx_fdm_watchdog: decode.nrx_fdm_watchdog,
            nrx_restart: decode.nrx_restart,
            nrx_service: decode.nrx_service,
            nrx_tx_over: decode.nrx_tx_over,
            nrx_unsupported: decode.nrx_unsupported,
        },
        hang: service::HangCounters {
            rx: hang.rx,
            tx: hang.tx,
            rx_tx_hang: hang.rx_tx_hang,
            rx_tx_panic: hang.rx_tx_panic,
        },
        ring,
    }
}

/// The ring's state: where its walker stands and each descriptor's control
/// word, R's last.
pub(crate) fn ring_state<const N: usize>(
    standing: Standing,
    controls: (
        [oer_chips_esp32s31_hal::layouts::WifiRxDmaDescriptorControl; N],
        oer_chips_esp32s31_hal::layouts::WifiRxDmaDescriptorControl,
    ),
) -> service::RingState {
    let (ring, reserved) = controls;
    let all = || ring.iter().chain(core::iter::once(&reserved));
    service::RingState {
        standing: standing_of(standing),
        controls: all().map(|control| control.bits()).collect(),
        seats: all().map(|control| seat(*control)).collect(),
    }
}

/// A descriptor's control word as its fields read.
fn seat(control: oer_chips_esp32s31_hal::layouts::WifiRxDmaDescriptorControl) -> service::Seat {
    if control.rx_done() {
        service::Seat::Done {
            length: control.length(),
        }
    } else if control.owner() && control.length() == control.size() {
        service::Seat::Armed
    } else {
        service::Seat::Other
    }
}

/// The ring's standing in the service's words.
pub(crate) fn standing_of(standing: Standing) -> service::Standing {
    let index = |at: usize| u8::try_from(at).unwrap_or(u8::MAX);
    service::Standing {
        last: standing.last.map(index),
        next: match standing.next {
            NextAt::Zero => service::NextAt::Zero,
            NextAt::Ring(at) => service::NextAt::Ring(index(at)),
            NextAt::Elsewhere => service::NextAt::Elsewhere,
        },
        tail: index(standing.tail),
        tail_word: if standing.tail_done {
            service::TailWord::Completed
        } else {
            service::TailWord::Armed
        },
        cursor: if standing.unchanged {
            service::Cursor::Steady
        } else {
            service::Cursor::Moved
        },
    }
}
