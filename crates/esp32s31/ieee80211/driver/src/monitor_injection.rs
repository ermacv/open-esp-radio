//! Fail-closed ESP32-S31 standalone-monitor injection frontier.
//!
//! The ordinary TX owner proves descriptor/FCS/IRQ/deadline behavior, but its
//! reviewed interface selector is currently assigned only to Station and
//! AccessPoint. Reviewed Context2/Context3 values intentionally have no
//! protocol-role semantics. Selecting any of them for a standalone monitor
//! would therefore be a hardware guess.

use crate::ordinary_tx::{TX_FCS_SIZE, TX_METADATA_SIZE};

use oer_esp32s31_ieee80211_mac::tx::LegacyRate;

use oer_ieee80211_softmac::{
    MonitorInjectionChannelBinding, MonitorInjectionRate, MonitorInjectionRequest, MonitorSink,
    WifiStandaloneMonitorPlan,
};

/// First unproven hardware edge after portable request and buffer validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionUnsupported {
    /// No reviewed [`crate::ordinary_tx::OrdinaryTxInterface`] value denotes
    /// an interface-free standalone monitor.
    UnassignedMacInterface,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionAdmissionError {
    /// The sink is observation-only or has not begun a physical-channel epoch.
    MissingTaskDwellBinding,
    ChannelBindingMismatch {
        requested: MonitorInjectionChannelBinding,
        active: MonitorInjectionChannelBinding,
    },
    BufferGeometry {
        required: usize,
        capacity: usize,
    },
    Unsupported(MonitorInjectionUnsupported),
}

/// Opaque future success token. There is intentionally no public constructor;
/// current reviewed hardware evidence cannot produce this value.
pub struct MonitorInjectionAdmission {
    _private: (),
}

/// Pure mapping for the standard legacy rate subset exposed by the portable
/// request. Raw/LR/HT/HE codes cannot enter this function.
pub const fn esp32s31_monitor_injection_rate(rate: MonitorInjectionRate) -> LegacyRate {
    match rate {
        MonitorInjectionRate::Dsss1MLong => LegacyRate::Dsss1MLong,
        MonitorInjectionRate::Dsss2MLong => LegacyRate::Dsss2MLong,
        MonitorInjectionRate::Cck5M5Long => LegacyRate::Cck5M5Long,
        MonitorInjectionRate::Cck11MLong => LegacyRate::Cck11MLong,
        MonitorInjectionRate::Dsss2MShort => LegacyRate::Dsss2MShort,
        MonitorInjectionRate::Cck5M5Short => LegacyRate::Cck5M5Short,
        MonitorInjectionRate::Cck11MShort => LegacyRate::Cck11MShort,
        MonitorInjectionRate::Ofdm6M => LegacyRate::Ofdm6M,
        MonitorInjectionRate::Ofdm9M => LegacyRate::Ofdm9M,
        MonitorInjectionRate::Ofdm12M => LegacyRate::Ofdm12M,
        MonitorInjectionRate::Ofdm18M => LegacyRate::Ofdm18M,
        MonitorInjectionRate::Ofdm24M => LegacyRate::Ofdm24M,
        MonitorInjectionRate::Ofdm36M => LegacyRate::Ofdm36M,
        MonitorInjectionRate::Ofdm48M => LegacyRate::Ofdm48M,
        MonitorInjectionRate::Ofdm54M => LegacyRate::Ofdm54M,
    }
}

/// Validate every source-owned edge, then stop at the first unproven MMIO
/// selector before sequence, DMA, queue, IRQ or deadline state is mutated.
///
/// `sink` is the actual monitor task's retained capture owner. Its binding is
/// created by `begin_channel_epoch`, not copied from the application request.
/// Passing a checked [`WifiStandaloneMonitorPlan`] excludes connected and
/// concurrent monitor topologies at the API boundary.
pub fn admit_esp32s31_monitor_injection<Rate, S: MonitorSink<Rate>, const TX_BUFFER_SIZE: usize>(
    _plan: WifiStandaloneMonitorPlan,
    sink: &S,
    request: MonitorInjectionRequest<'_>,
) -> Result<MonitorInjectionAdmission, MonitorInjectionAdmissionError> {
    let active = sink
        .injection_channel_binding()
        .ok_or(MonitorInjectionAdmissionError::MissingTaskDwellBinding)?;
    if request.binding() != active {
        return Err(MonitorInjectionAdmissionError::ChannelBindingMismatch {
            requested: request.binding(),
            active,
        });
    }

    let transfer_length = TX_METADATA_SIZE
        .checked_add(request.mpdu().len())
        .and_then(|length| length.checked_add(TX_FCS_SIZE));
    let descriptor_capacity = transfer_length
        .and_then(|length| length.checked_add(3))
        .map(|length| length & !3);
    let Some(required) = descriptor_capacity else {
        return Err(MonitorInjectionAdmissionError::BufferGeometry {
            required: usize::MAX,
            capacity: TX_BUFFER_SIZE,
        });
    };
    if required > TX_BUFFER_SIZE {
        return Err(MonitorInjectionAdmissionError::BufferGeometry {
            required,
            capacity: TX_BUFFER_SIZE,
        });
    }

    let _standard_legacy_rate = esp32s31_monitor_injection_rate(request.rate());

    Err(MonitorInjectionAdmissionError::Unsupported(
        MonitorInjectionUnsupported::UnassignedMacInterface,
    ))
}
