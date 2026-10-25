//! TSF transactions serialized by the single Wi-Fi register owner.

use oer_esp32s31_pac::modem_pwr::{WifiMacTsfLoad, sta_tsf_control::StaTsfMode};
use oer_mmio::Placement;

use super::{MacPlacement, WifiRadioRegisters, bits};

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
pub(crate) fn snapshot_station_tsf<P: Placement>(
    registers: &mut WifiMacTsfLoad<P>,
    low: Option<&mut u32>,
    high: Option<&mut u32>,
) {
    registers.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(true);
    });
    if let Some(low) = low {
        *low = registers.read_snapshot_low().value();
    }
    if let Some(high) = high {
        *high = registers.read_snapshot_high().value();
    }
    registers.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(false);
    });
}

/// The halves of a 64-bit TSF, low word first.
const fn tsf_words(value: u64) -> (u32, u32) {
    (value as u32, (value >> 32) as u32)
}

/// TSF bits 35:10 of a station TSF, the station TBTT target's field value.
const fn tsf_bits_35_10(value: u64) -> u32 {
    ((value >> 10) as u32) & 0x03ff_ffff
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
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
        let registers = &mut self.mac.wifi_mac_tsf_load;
        registers.modify_tsf_load_control(|w| {
            w.set_snapshot_softap_tsf(true);
        });
        let high = registers.read_snapshot_high().value();
        let low = registers.read_snapshot_low().value();
        registers.modify_tsf_load_control(|w| {
            w.set_snapshot_softap_tsf(false);
        });
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
            &mut self.mac.wifi_mac_tsf_load,
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
        let (low, high) = tsf_words(value);
        let load = &mut self.mac.wifi_mac_tsf_load;
        load.write_value_low(|w| {
            w.set_value(low);
        });
        load.write_value_high(|w| {
            w.set_value(high);
        });
        load.modify_tsf_load_control(|w| {
            w.set_load_station_tsf(true);
        });
        oer_mmio::fence();
    }

    /// Enable the station TSF scheduler.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]`: `hal_enable_sta_tsf`
    /// performs two fresh-read RMWs at `0x2010_d858`: first it sets bits 27
    /// and 31, then it replaces bits 22:19 with one.
    pub fn enable_station_tsf(&mut self) {
        let control = &mut self.mac.wifi_mac_rtc_timer_update;
        control.modify_sta_tsf_control(|w| {
            w.set_sta_tsf_enable_low(true).set_sta_tsf_enable_high(true);
        });
        control.modify_sta_tsf_control(|w| {
            w.set_sta_tsf_mode(StaTsfMode::Enabled);
        });
        oer_mmio::fence();
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
        self.mac
            .wifi_mac_rtc_timer_update
            .modify_sta_tsf_control(|w| {
                w.set_sta_tbtt_enable(true);
            });

        self.set_station_tbtt_interval(schedule.interval_micros);
        // SOURCE: the masked `publish_station_tbtt_target` transaction keeps
        // bits 31:26 and replaces TSF bits 35:10 (BLOB_LIBPP_HAL_TSF).
        let target = tsf_bits_35_10(schedule.first_tbtt_tsf);
        self.mac.wifi_mac_sta_tbtt_target.modify_target(|w| {
            w.set_tsf_bits_35_10(bits(target));
        });
        self.mac.wifi_mac_tsf_load.modify_tsf_load_control(|w| {
            w.set_load_station_tbtt_target(true);
        });
        let rtc = &mut self.mac.wifi_mac_rtc_timer_update;
        rtc.modify_sta_tsf_control(|w| {
            w.set_sta_tbtt_ahead_time(schedule.ahead_micros);
        });
        rtc.modify_sta_light_sleep_wake_ahead(|w| {
            w.set_time(schedule.wake_ahead_micros);
        });

        rtc.modify_soc_wakeup_clear(|w| {
            w.set_clear_request(true);
        });
        self.mac
            .wifi_mac_tsf_status
            .modify_wakeup_signal_clear(|w| {
                w.set_clear(true);
            });
        oer_mmio::fence();
    }

    /// Stop the station TBTT.
    ///
    /// SOURCE: complete pinned `libpp[hal_tsf.o]::hal_disable_sta_tbtt`
    /// clears bit 26 at `0x2010_d858`.
    pub fn stop_station_tbtt(&mut self) {
        self.mac
            .wifi_mac_rtc_timer_update
            .modify_sta_tsf_control(|w| {
                w.set_sta_tbtt_enable(false);
            });
        oer_mmio::fence();
    }

    /// Replace the station TBTT lead and the light-sleep wake lead beside it.
    ///
    /// SOURCE: complete pinned `libpp[pm.o]::pm_update_params` calls
    /// `hal_set_sta_tbtt_ahead_time`, which replaces bits 15:0 at
    /// `0x2010_d858`, then `hal_set_sta_light_sleep_wake_ahead_time`, which
    /// replaces bits 31:16 at `0x2010_d840`.
    pub fn set_station_tbtt_ahead(&mut self, ahead_micros: u16, wake_ahead_micros: u16) {
        let rtc = &mut self.mac.wifi_mac_rtc_timer_update;
        rtc.modify_sta_tsf_control(|w| {
            w.set_sta_tbtt_ahead_time(ahead_micros);
        });
        rtc.modify_sta_light_sleep_wake_ahead(|w| {
            w.set_time(wake_ahead_micros);
        });
    }

    /// Replace the station TBTT interval, as the vendor does when the
    /// coexistence period changes.
    ///
    /// SOURCE: complete pinned `libpp[hal_tsf.o]::hal_set_sta_tbtt_interval`
    /// replaces bits 25:0 at `0x2010_d85c` with the interval shifted right by
    /// ten.
    pub fn set_station_tbtt_interval(&mut self, interval_micros: u32) {
        self.mac
            .wifi_mac_rtc_timer_update
            .modify_sta_tbtt_interval(|w| {
                w.set_interval_tsf_bits_35_10(bits(interval_micros >> 10));
            });
    }
}

#[cfg(test)]
mod tests;
