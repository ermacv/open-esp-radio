//! TSF transactions serialized by the single Wi-Fi register owner.

#![forbid(unsafe_code)]

use crate::{WifiRadioRegisters, device_fence, svd};

/// One station TBTT schedule, as the vendor power manager programs it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTbttSchedule {
    /// Station TSF of the first TBTT.
    pub first_tbtt_tsf: u64,
    /// Interval between TBTT events, in microseconds.
    pub interval_micros: u32,
    /// How long before each TBTT the event fires, in microseconds.
    pub ahead_micros: u16,
    /// Light-sleep wake lead published beside the schedule, in microseconds.
    pub wake_ahead_micros: u16,
}

/// Snapshot either or both station TSF words using the complete ROM leaf's
/// conditional-output semantics.
#[inline(always)]
pub(crate) fn snapshot_station_tsf(
    registers: &svd::WifiMacTsfLoad,
    low: Option<&mut u32>,
    high: Option<&mut u32>,
) {
    registers
        .control()
        .modify(|_, writer| writer.snapshot_station_tsf().set_bit());
    if let Some(low) = low {
        *low = registers.snapshot_low().read().value().bits();
    }
    if let Some(high) = high {
        *high = registers.snapshot_high().read().value().bits();
    }
    registers
        .control()
        .modify(|_, writer| writer.snapshot_station_tsf().clear_bit());
}

impl WifiRadioRegisters {
    /// Return one coherent access-point TSF snapshot.
    ///
    /// The exclusive register owner covers select, both shared readback
    /// words and release. STA snapshots and TSF writes require the same
    /// owner, so no domain can change the selector within this transaction.
    /// No selected-register handle or callback escapes the method.
    ///
    /// SOURCE: complete pinned `libpp[hal_mac.o]::hal_get_tsf_time`, with
    /// interface one, selects SoftAP, reads high then low, and releases the
    /// selector using a fresh control read. The reviewed field is published
    /// as SNAPSHOT_SOFTAP_TSF; the words are shared with the station snapshot.
    pub fn access_point_tsf(&mut self) -> u64 {
        let registers = &self.peripherals.wifi_mac.wifi_mac_tsf_load;
        registers
            .control()
            .modify(|_, writer| writer.snapshot_softap_tsf().set_bit());
        let high = registers.snapshot_high().read().value().bits();
        let low = registers.snapshot_low().read().value().bits();
        registers
            .control()
            .modify(|_, writer| writer.snapshot_softap_tsf().clear_bit());
        u64::from(low) | (u64::from(high) << 32)
    }

    /// Return one coherent station TSF snapshot.
    ///
    /// SOURCE: complete rev0 ROM `rom[]::hal_get_sta_tsf` at `0x2f82c15c` sets
    /// CONTROL bit zero, reads the low word at `0x2010_d820`, reads the high
    /// word at `0x2010_d824`, then clears CONTROL bit zero. Both output
    /// pointers are present in this production specialization.
    pub fn station_tsf(&mut self) -> u64 {
        let mut low = 0;
        let mut high = 0;
        snapshot_station_tsf(
            &self.peripherals.wifi_mac.wifi_mac_tsf_load,
            Some(&mut low),
            Some(&mut high),
        );
        u64::from(low) | (u64::from(high) << 32)
    }

    /// Publish a station TSF value.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]`: `hal_set_sta_tsf`
    /// writes the low word, writes the high word and then asserts bit four at
    /// `0x2010_d814` through a fresh-read RMW.
    pub fn set_station_tsf(&mut self, value: u64) {
        let load = &self.peripherals.wifi_mac.wifi_mac_tsf_load;
        crate::generated::station_tsf_value_low(
            load,
            crate::generated::StationTsfLowWord::new(value as u32),
        );
        crate::generated::station_tsf_value_high(
            load,
            crate::generated::StationTsfHighWord::new((value >> 32) as u32),
        );
        load.control().modify(|_, w| w.load_station_tsf().set_bit());
        device_fence();
    }

    /// Enable the station TSF scheduler.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]`: `hal_enable_sta_tsf`
    /// performs two fresh-read RMWs at `0x2010_d858`: first it sets bits 27
    /// and 31, then it replaces bits 22:19 with one.
    pub fn enable_station_tsf(&mut self) {
        let control = self
            .peripherals
            .wifi_mac
            .wifi_mac_rtc_timer_update
            .sta_tsf_control();
        control.modify(|_, w| {
            w.sta_tsf_enable_low()
                .set_bit()
                .sta_tsf_enable_high()
                .set_bit()
        });
        control.modify(|_, w| w.sta_tsf_mode().enabled());
        device_fence();
    }

    /// Program the station TBTT schedule and enable its interrupt.
    ///
    /// SOURCE: complete pinned `libpp[pm.o]::pm_update_next_tbtt` disables
    /// and re-enables the station TBTT, programs the schedule with
    /// `hal_set_sta_tbtt`, then clears the SoC wakeup request and sets bit
    /// zero at `0x2010_d810`. Complete `hal_tsf.o::hal_disable_sta_tbtt`
    /// clears TBTT enable bit 26 at `0x2010_d858`; `hal_enable_sta_tbtt`
    /// sets it; `hal_set_sta_tbtt` writes the interval, the target, the
    /// target load strobe, the ahead time and the light-sleep wake lead, in
    /// that order. The station TBTT interrupt enable in the same leaves
    /// belongs to the interrupt epoch, which keeps it enabled; bit 26 gates
    /// the event.
    pub fn start_station_tbtt(&mut self, schedule: StaTbttSchedule) {
        self.stop_station_tbtt();
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        rtc.sta_tsf_control()
            .modify(|_, w| w.sta_tbtt_enable().set_bit());

        self.set_station_tbtt_interval(schedule.interval_micros);
        let target_bits_35_10 = ((schedule.first_tbtt_tsf >> 10) as u32)
            & crate::generated::StationTbttTargetBits35To10::MAX;
        crate::generated::publish_station_tbtt_target(
            &self.peripherals.wifi_mac.wifi_mac_sta_tbtt_target,
            crate::generated::StationTbttTargetBits35To10::new(target_bits_35_10)
                .expect("masked station-TBTT target is a reviewed 26-bit value"),
        );
        self.peripherals
            .wifi_mac
            .wifi_mac_tsf_load
            .control()
            .modify(|_, w| w.load_station_tbtt_target().set_bit());
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        rtc.sta_tsf_control()
            .modify(|_, w| w.sta_tbtt_ahead_time().set(schedule.ahead_micros));
        rtc.sta_light_sleep_wake_ahead()
            .modify(|_, w| w.time().set(schedule.wake_ahead_micros));

        rtc.soc_wakeup_clear()
            .modify(|_, w| w.clear_request().set_bit());
        self.peripherals
            .wifi_mac
            .wifi_mac_tsf_status
            .wakeup_signal_clear()
            .modify(|_, w| w.clear().set_bit());
        device_fence();
    }

    /// Stop the station TBTT.
    ///
    /// SOURCE: complete pinned `libpp[hal_tsf.o]::hal_disable_sta_tbtt`
    /// clears bit 26 at `0x2010_d858`.
    pub fn stop_station_tbtt(&mut self) {
        self.peripherals
            .wifi_mac
            .wifi_mac_rtc_timer_update
            .sta_tsf_control()
            .modify(|_, w| w.sta_tbtt_enable().clear_bit());
        device_fence();
    }

    /// Replace the station TBTT lead and the light-sleep wake lead beside it.
    ///
    /// SOURCE: complete pinned `libpp[pm.o]::pm_update_params` calls
    /// `hal_set_sta_tbtt_ahead_time`, which replaces bits 15:0 at
    /// `0x2010_d858`, then `hal_set_sta_light_sleep_wake_ahead_time`, which
    /// replaces bits 31:16 at `0x2010_d840`.
    pub fn set_station_tbtt_ahead(&mut self, ahead_micros: u16, wake_ahead_micros: u16) {
        let rtc = &self.peripherals.wifi_mac.wifi_mac_rtc_timer_update;
        rtc.sta_tsf_control()
            .modify(|_, w| w.sta_tbtt_ahead_time().set(ahead_micros));
        rtc.sta_light_sleep_wake_ahead()
            .modify(|_, w| w.time().set(wake_ahead_micros));
    }

    /// Replace the station TBTT interval, as the vendor does when the
    /// coexistence period changes.
    ///
    /// SOURCE: complete pinned `libpp[hal_tsf.o]::hal_set_sta_tbtt_interval` replaces bits 25:0 at `0x2010_d85c` with
    /// the interval shifted right by ten.
    pub fn set_station_tbtt_interval(&mut self, interval_micros: u32) {
        self.peripherals
            .wifi_mac
            .wifi_mac_rtc_timer_update
            .sta_tbtt_interval()
            .modify(|_, w| w.interval_tsf_bits_35_10().set(interval_micros >> 10));
    }
}
