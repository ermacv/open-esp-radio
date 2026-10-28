//! Portable, non-retaining receive-monitor contract.
//!
//! A monitor sink observes a borrowed frame synchronously. It may copy the
//! frame into its own bounded storage, but it cannot retain the radio/DMA
//! borrow. Queue saturation is an ordinary observation loss and must never
//! backpressure the primary radio owner. Which frames a monitor keeps is
//! the runtime's policy; the injected MPDU's geometry is checked by
//! `oer_ieee80211_wire::monitor_injection`.

use core::{fmt, num::NonZeroU32};

use oer_ieee80211_wire::{channel::WifiChannel, monitor_injection::MonitorInjectionMpdu};

use crate::MacRxMetadata;

/// Stable identity of one hardware channel context within a radio owner.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct ChannelContextId(u8);

impl ChannelContextId {
    pub const PRIMARY: Self = Self(0);

    pub const fn new(index: u8) -> Self {
        Self(index)
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Observation point for a passive monitor consumer.
///
/// Monitor is a tap rather than a protocol VIF: a slow observer must not own
/// the normal RX path or force STA/AP protocol state into a synthetic role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorTapPoint {
    /// Raw DMA frame and chip RX status before normalization.
    Raw,
    /// Frame and portable metadata after backend normalization.
    Normalized,
    /// Frame after HMAC validation, decryption and reorder.
    ProtocolValidated,
}

/// Exact physical-channel dwell which may admit one monitor injection.
///
/// `generation` is the surrounding radio-service generation and
/// `channel_epoch` is the monotonically increasing dwell identity published
/// by the monitor capture owner.  Carrying both values prevents a retained
/// capture from authorizing transmission after either a hop or a complete
/// monitor restart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorInjectionChannelBinding {
    generation: u32,
    channel_epoch: NonZeroU32,
    channel: WifiChannel,
}

impl MonitorInjectionChannelBinding {
    /// Describe the dwell expected by an application request.
    ///
    /// Constructing this copyable value grants no authority. Admission
    /// compares it with the independently published binding owned by the live
    /// monitor task/sink.
    pub const fn new(
        generation: u32,
        channel_epoch: u32,
        channel: WifiChannel,
    ) -> Result<Self, MonitorInjectionBindingError> {
        match NonZeroU32::new(channel_epoch) {
            Some(channel_epoch) => Ok(Self {
                generation,
                channel_epoch,
                channel,
            }),
            None => Err(MonitorInjectionBindingError::ZeroChannelEpoch),
        }
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }

    pub const fn channel_epoch(self) -> u32 {
        self.channel_epoch.get()
    }

    pub const fn channel(self) -> WifiChannel {
        self.channel
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionBindingError {
    ZeroChannelEpoch,
}

impl fmt::Display for MonitorInjectionBindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a monitor injection channel epoch must be nonzero")
    }
}

impl core::error::Error for MonitorInjectionBindingError {}

/// Standard legacy rate accepted by the portable monitor-injection boundary.
///
/// There is deliberately no raw rate code, HT/HE or long-range variant.  A
/// backend must add a separately reviewed typed profile before any of those
/// PHY geometries can reach its queue-vector or PLCP owner.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MonitorInjectionRate {
    #[default]
    Dsss1MLong,
    Dsss2MLong,
    Cck5M5Long,
    Cck11MLong,
    Dsss2MShort,
    Cck5M5Short,
    Cck11MShort,
    Ofdm6M,
    Ofdm9M,
    Ofdm12M,
    Ofdm18M,
    Ofdm24M,
    Ofdm36M,
    Ofdm48M,
    Ofdm54M,
}

/// Complete borrowed request for one exact standalone-monitor dwell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorInjectionRequest<'frame> {
    binding: MonitorInjectionChannelBinding,
    rate: MonitorInjectionRate,
    mpdu: MonitorInjectionMpdu<'frame>,
}

impl<'frame> MonitorInjectionRequest<'frame> {
    pub const fn new(
        binding: MonitorInjectionChannelBinding,
        rate: MonitorInjectionRate,
        mpdu: MonitorInjectionMpdu<'frame>,
    ) -> Self {
        Self {
            binding,
            rate,
            mpdu,
        }
    }

    pub const fn binding(self) -> MonitorInjectionChannelBinding {
        self.binding
    }

    pub const fn rate(self) -> MonitorInjectionRate {
        self.rate
    }

    pub const fn mpdu(self) -> MonitorInjectionMpdu<'frame> {
        self.mpdu
    }
}

/// One frame observed at an explicitly identified receive-pipeline boundary.
#[derive(Clone, Copy, Debug)]
pub struct MonitorFrame<'frame, Rate> {
    pub tap: MonitorTapPoint,
    pub channel_context: ChannelContextId,
    /// Bytes visible at this tap. They never include a hardware-stripped FCS.
    pub bytes: &'frame [u8],
    pub metadata: MacRxMetadata<Rate>,
    /// Complete logical MPDU length inferred from the receive status.
    ///
    /// `bytes` may be shorter when hardware consumed an authenticated cipher
    /// trailer before publishing the DMA view. The distinction prevents a
    /// capture consumer from treating such a frame as ordinary queue loss.
    pub logical_length: usize,
}

impl<Rate> MonitorFrame<'_, Rate> {
    pub const fn is_complete(&self) -> bool {
        self.bytes.len() == self.logical_length
    }
}

/// Why a best-effort monitor sink did not retain an observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorDropReason {
    /// Every caller-owned capture slot was occupied.
    Full,
    /// The frame exceeds the sink's configured capture capacity.
    TooLong,
    /// Caller policy intentionally excluded this observation.
    Filtered,
}

/// Immediate result of a non-blocking monitor publication attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorPublishOutcome {
    Published,
    Dropped(MonitorDropReason),
}

/// Best-effort observation endpoint for one receive tap.
///
/// Implementations must return without waiting for capacity. An async adapter
/// can wake a consumer after copying into bounded storage, but the radio owner
/// never awaits that consumer and continues recycling its RX descriptor.
pub trait MonitorSink<Rate> {
    /// Start a fresh physical-channel capture epoch.
    ///
    /// The default preserves source-only sinks which do not retain channel
    /// metadata. Queue-backed product sinks override this edge and stamp the
    /// exact channel into every independently retained frame.
    fn begin_channel_epoch(&mut self, _channel: WifiChannel) {}

    /// Revoke dwell-bound injection authority before stop or retune begins.
    fn end_channel_epoch(&mut self) {}

    /// Exact authority for injection during the currently live dwell.
    ///
    /// Legacy and observation-only sinks return `None`. A production sink
    /// overrides this only after its task owner has bound the supervisor
    /// generation, tuned channel and monotonically increasing capture epoch.
    fn injection_channel_binding(&self) -> Option<MonitorInjectionChannelBinding> {
        None
    }

    fn try_publish(&mut self, frame: MonitorFrame<'_, Rate>) -> MonitorPublishOutcome;
}

#[cfg(test)]
mod tests;

