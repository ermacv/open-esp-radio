//! Pure channel planning, timing and PDU address policy for advertising.
use oer_esp32s31_hal::bluetooth::engine::{
    AdvertisingEventImage, AdvertisingItemImage, LegacyAdvertisingOwnAddress,
    LegacyAdvertisingPduError, LegacyAdvertisingPrimaryChannel, SchedulerEventPriority,
};
use oer_wire::Reader;

oer_wire::bitfield! {
    /// SOURCE: Bluetooth Core 6.0, Vol 6, Part B, sections 2.3.1 and 2.3.1.3.
    struct NonconnectableHeader[1] (little) {
        /// PDU Type, section 2.3.1.
        pdu_type, with_pdu_type: u8 = 0..4;
        /// RFU and ChSel, unused in ADV_NONCONN_IND, section 2.3.1 (RFU/ChSel).
        unused, with_unused: u8 = 4..6;
        /// Advertiser address class, section 2.3.1.3.
        random, with_random: bool = 6..7;
        /// RxAdd, unused in ADV_NONCONN_IND, sections 2.3.1/2.3.1.3.
        rx_add, with_rx_add: bool = 7..8;
    }
}

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
    // Read the address before classifying the header to preserve error precedence.
    let mut reader = Reader::new(pdu);
    let [header, _length] = reader
        .read::<2>()
        .map_err(|_| LegacyAdvertisingPduError::MissingAdvertiserAddress)?;
    let address = reader
        .read::<6>()
        .map_err(|_| LegacyAdvertisingPduError::MissingAdvertiserAddress)?;
    let header = NonconnectableHeader::from_bytes([header]);
    if header.pdu_type() != 2 {
        return Err(LegacyAdvertisingPduError::UnsupportedPduType);
    }
    if header.unused() != 0 || header.rx_add() {
        return Err(LegacyAdvertisingPduError::UnsupportedHeaderFlags);
    }
    if header.random() {
        Ok(LegacyAdvertisingOwnAddress::Random(address))
    } else {
        Ok(LegacyAdvertisingOwnAddress::Public)
    }
}

impl LegacyAdvertisingPrimaryChannelPlan {
    pub fn image(self, start: u32, duration: u32, sequence_lead: u32) -> AdvertisingEventImage {
        let priority = const { SchedulerEventPriority::new(1).expect("reviewed priority fits") };
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
