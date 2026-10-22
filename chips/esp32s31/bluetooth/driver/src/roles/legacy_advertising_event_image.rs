//! Pure channel planning, timing and PDU address policy for advertising.
use oer_esp32s31_hal::bluetooth::engine::{
    AdvertisingEventImage, AdvertisingItemImage, LegacyAdvertisingOwnAddress,
    LegacyAdvertisingPduError, LegacyAdvertisingPrimaryChannel, SchedulerEventPriority,
};
const ADV_NONCONN_IND_TYPE: u8 = 0x02;
const TX_ADD_RANDOM: u8 = 1 << 6;
const RESERVED_HEADER_BITS: u8 = (1 << 4) | (1 << 5) | (1 << 7);
const DEVICE_ADDRESS_BYTES: usize = 6;

/// Canonical non-empty primary-channel plan for one hardware event.
///
/// The plan is semantic: it contains no frequency, whitening or SRAM image.
/// Selected channels are always ordered 37, 38, 39.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingPrimaryChannelPlan {
    channels: [LegacyAdvertisingPrimaryChannel; 3],
    len: u8,
}

impl LegacyAdvertisingPrimaryChannelPlan {
    pub const fn new(channel_37: bool, channel_38: bool, channel_39: bool) -> Option<Self> {
        let mut channels = [LegacyAdvertisingPrimaryChannel::Channel37; 3];
        let mut len = 0;
        if channel_37 {
            channels[len] = LegacyAdvertisingPrimaryChannel::Channel37;
            len += 1;
        }
        if channel_38 {
            channels[len] = LegacyAdvertisingPrimaryChannel::Channel38;
            len += 1;
        }
        if channel_39 {
            channels[len] = LegacyAdvertisingPrimaryChannel::Channel39;
            len += 1;
        }
        if len == 0 {
            None
        } else {
            Some(Self {
                channels,
                len: len as u8,
            })
        }
    }

    pub const fn channel_count(self) -> usize {
        self.len as usize
    }

    pub const fn channel(self, position: usize) -> Option<LegacyAdvertisingPrimaryChannel> {
        if position < self.channel_count() {
            Some(self.channels[position])
        } else {
            None
        }
    }
}

pub(super) fn own_address_from_pdu(
    pdu: &[u8],
) -> Result<LegacyAdvertisingOwnAddress, LegacyAdvertisingPduError> {
    if pdu.len() < 2 + DEVICE_ADDRESS_BYTES {
        return Err(LegacyAdvertisingPduError::MissingAdvertiserAddress);
    }
    let header = pdu[0];
    if header & 0x0f != ADV_NONCONN_IND_TYPE {
        return Err(LegacyAdvertisingPduError::UnsupportedPduType);
    }
    if header & RESERVED_HEADER_BITS != 0 {
        return Err(LegacyAdvertisingPduError::UnsupportedHeaderFlags);
    }

    if header & TX_ADD_RANDOM == 0 {
        Ok(LegacyAdvertisingOwnAddress::Public)
    } else {
        let mut address = [0; DEVICE_ADDRESS_BYTES];
        address.copy_from_slice(&pdu[2..2 + DEVICE_ADDRESS_BYTES]);
        Ok(LegacyAdvertisingOwnAddress::Random(address))
    }
}
impl LegacyAdvertisingPrimaryChannelPlan {
    pub fn image(self, start: u32, duration: u32, sequence_lead: u32) -> AdvertisingEventImage {
        let priority = SchedulerEventPriority::new(1).expect("reviewed priority fits");
        let first = AdvertisingItemImage {
            channel: self.channels[0],
            flexible_start: false,
            priority,
            priority_copy: priority,
            start,
            end: start.wrapping_add(duration),
            sequence_lead,
        };
        let mut items = [first; 3];
        for (index, item) in items.iter_mut().enumerate().take(self.channel_count()) {
            let raw_start = start.wrapping_add(duration.wrapping_mul(index as u32));
            item.channel = self.channels[index];
            item.flexible_start = index != 0;
            item.start = raw_start;
            item.end = raw_start.wrapping_add(duration);
        }
        AdvertisingEventImage::new(&items[..self.channel_count()])
            .expect("a channel plan has one to three entries")
    }
}
