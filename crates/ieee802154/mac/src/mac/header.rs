//! MAC header inspection ported from the public ESP-IDF IEEE 802.15.4 driver
//! (`components/ieee802154/driver/esp_ieee802154_frame.c`).
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/ieee802154/driver/esp_ieee802154_frame.c` (Copyright Espressif
//! Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!
//! [`PhrFrame`](crate::PhrFrame) is the PHY-level image `[PHR, PSDU...]`: byte zero is the PSDU
//! length and the MAC header starts at byte one. Offsets below keep that
//! origin so each computation reads like the vendor source. For a malformed
//! header the vendor may read past the image or add its `0xff` invalid
//! sentinel to an offset; this port reports the field as absent instead.
//! Well-formed headers produce identical results.

const PHR_SIZE: u8 = 1;
const FCF_SIZE: u8 = 2;
const FCS_SIZE: u8 = 2;
const DSN_SIZE: u8 = 1;
const PANID_SIZE: u8 = 2;
const SHORT_ADDR_SIZE: u8 = 2;
const EXT_ADDR_SIZE: u8 = 8;
const SECURITY_HEADER_SIZE: u8 = 1;
const FRAME_COUNTER_SIZE: u8 = 4;
const COMMAND_ID_LEN: u8 = 1;
const IE_HEADER_LEN: u8 = 2;

const COMMAND_DATA_REQUEST: u8 = 0x04;
const IE_HEADER_ID_MASK: u16 = 0x3f80;
const IE_TYPE_HT2: u16 = 0x3f80;
const IE_SUBFIELD_LEN_MASK: u16 = 0x007f;

/// MAC frame type (frame-control bits 0..2).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FrameType {
    /// Beacon.
    Beacon,
    /// Data.
    Data,
    /// Acknowledgement.
    Ack,
    /// MAC command.
    Command,
    /// Reserved type 4.
    Reserved,
    /// Multipurpose.
    Multipurpose,
    /// Fragment or FRAK.
    Fragment,
    /// Extended.
    Extended,
}

impl FrameType {
    const fn from_bits(bits: u8) -> Self {
        match bits & 0x07 {
            0 => Self::Beacon,
            1 => Self::Data,
            2 => Self::Ack,
            3 => Self::Command,
            4 => Self::Reserved,
            5 => Self::Multipurpose,
            6 => Self::Fragment,
            _ => Self::Extended,
        }
    }

    /// `ieee802154_is_supported_frame_type`: the MAC hardware handles beacon,
    /// data, ACK and command frames.
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Beacon | Self::Data | Self::Ack | Self::Command)
    }
}

/// MAC frame version (frame-control bits 12..13).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FrameVersion {
    /// IEEE 802.15.4-2003.
    V2003,
    /// IEEE 802.15.4-2006 and 2011.
    V2006,
    /// IEEE 802.15.4-2015.
    V2015,
    /// Reserved.
    Reserved,
}

impl FrameVersion {
    const fn from_bits(bits: u8) -> Self {
        match bits & 0x30 {
            0x00 => Self::V2003,
            0x10 => Self::V2006,
            0x20 => Self::V2015,
            _ => Self::Reserved,
        }
    }
}

/// Destination or source addressing mode.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AddressMode {
    /// No address.
    None,
    /// Reserved mode `0b01`.
    Reserved,
    /// Two-byte short address.
    Short,
    /// Eight-byte extended address.
    Extended,
}

impl AddressMode {
    const fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0 => Self::None,
            1 => Self::Reserved,
            2 => Self::Short,
            _ => Self::Extended,
        }
    }

    const fn is_valid(self) -> bool {
        !matches!(self, Self::Reserved)
    }

    const fn size(self) -> u8 {
        match self {
            Self::None | Self::Reserved => 0,
            Self::Short => SHORT_ADDR_SIZE,
            Self::Extended => EXT_ADDR_SIZE,
        }
    }
}

/// An address copied out of a frame header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FrameAddress {
    /// Short address in frame (little-endian) byte order.
    Short([u8; 2]),
    /// Extended address in frame (little-endian) byte order.
    Extended([u8; 8]),
}

/// A frame image `[PHR, PSDU...]` as the MAC DMA reads and writes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhrFrame<'image> {
    image: &'image [u8],
}

impl<'image> PhrFrame<'image> {
    /// Borrow one image; byte zero is the PHR length.
    pub const fn new(image: &'image [u8]) -> Self {
        Self { image }
    }

    const fn byte(self, offset: u8) -> Option<u8> {
        if (offset as usize) < self.image.len() {
            Some(self.image[offset as usize])
        } else {
            None
        }
    }

    const fn header_byte(self, offset: u8) -> u8 {
        match self.byte(offset) {
            Some(byte) => byte,
            None => 0,
        }
    }

    /// The PHR length byte.
    pub const fn length(self) -> u8 {
        self.header_byte(0)
    }

    /// `ieee802154_frame_get_type`.
    pub const fn frame_type(self) -> FrameType {
        FrameType::from_bits(self.header_byte(1))
    }

    /// `ieee802154_frame_get_version`.
    pub const fn version(self) -> FrameVersion {
        FrameVersion::from_bits(self.header_byte(2))
    }

    /// `ieee802154_frame_is_security_enabled`.
    pub const fn security_enabled(self) -> bool {
        self.header_byte(1) & 0x08 != 0
    }

    /// `ieee802154_frame_is_ack_required`: only a supported frame type
    /// requests an acknowledgement.
    pub const fn ack_required(self) -> bool {
        self.frame_type().is_supported() && self.header_byte(1) & 0x20 != 0
    }

    const fn panid_compression(self) -> bool {
        self.header_byte(1) & 0x40 != 0
    }

    const fn ie_present(self) -> bool {
        self.header_byte(2) & 0x02 != 0
    }

    const fn dsn_present(self) -> bool {
        !matches!(self.version(), FrameVersion::V2015) || self.header_byte(2) & 0x01 == 0
    }

    /// Destination addressing mode.
    pub const fn destination_mode(self) -> AddressMode {
        AddressMode::from_bits(self.header_byte(2) >> 2)
    }

    /// Source addressing mode.
    pub const fn source_mode(self) -> AddressMode {
        AddressMode::from_bits(self.header_byte(2) >> 6)
    }

    const fn valid_modes(self) -> bool {
        self.destination_mode().is_valid() && self.source_mode().is_valid()
    }

    const fn destination_panid_present(self) -> bool {
        if !self.valid_modes() {
            return false;
        }
        let dst = self.destination_mode();
        let src = self.source_mode();
        let compression = self.panid_compression();
        if matches!(self.version(), FrameVersion::V2015) {
            if !matches!(dst, AddressMode::None) {
                !((matches!(src, AddressMode::None) && compression)
                    || (matches!(dst, AddressMode::Extended)
                        && matches!(src, AddressMode::Extended)
                        && compression))
            } else {
                matches!(src, AddressMode::None) && compression
            }
        } else {
            !matches!(dst, AddressMode::None)
        }
    }

    const fn source_panid_present(self) -> bool {
        if !self.valid_modes() {
            return false;
        }
        let dst = self.destination_mode();
        let src = self.source_mode();
        let compression = self.panid_compression();
        if matches!(src, AddressMode::None) {
            return false;
        }
        if matches!(self.version(), FrameVersion::V2015)
            && matches!(dst, AddressMode::Extended)
            && matches!(src, AddressMode::Extended)
            && !compression
        {
            return false;
        }
        !compression
    }

    const fn address_offset(self) -> u8 {
        PHR_SIZE + FCF_SIZE + if self.dsn_present() { DSN_SIZE } else { 0 }
    }

    const fn address_size(self) -> Option<u8> {
        if !self.valid_modes() {
            return None;
        }
        let mut size = self.destination_mode().size() + self.source_mode().size();
        if self.destination_panid_present() {
            size += PANID_SIZE;
        }
        if self.source_panid_present() {
            size += PANID_SIZE;
        }
        Some(size)
    }

    /// The image offset of the auxiliary security header, whether or not
    /// security is enabled.
    pub const fn security_header_offset(self) -> Option<u8> {
        if !self.frame_type().is_supported() {
            return None;
        }
        match self.address_size() {
            Some(size) => Some(self.address_offset() + size),
            None => None,
        }
    }

    const fn security_field_len(self) -> Option<u8> {
        let Some(offset) = self.security_header_offset() else {
            return None;
        };
        let Some(security_header) = self.byte(offset) else {
            return None;
        };
        let mut length = SECURITY_HEADER_SIZE;
        if security_header & 0x20 == 0 {
            length += FRAME_COUNTER_SIZE;
        }
        length += match security_header & 0x18 {
            0x08 => 1,
            0x10 => 5,
            0x18 => 9,
            _ => 0,
        };
        Some(length)
    }

    const fn ie_header_offset(self) -> Option<u8> {
        match (self.security_header_offset(), self.security_field_len()) {
            (Some(offset), Some(length)) => Some(offset + length),
            _ => None,
        }
    }

    const fn mic_len(self) -> u8 {
        let Some(offset) = self.security_header_offset() else {
            return 0;
        };
        match self.header_byte(offset) & 0x07 {
            0x01 | 0x05 => 4,
            0x02 | 0x06 => 8,
            0x03 | 0x07 => 16,
            _ => 0,
        }
    }

    /// Header-IE length ending at the payload, `ieee802154_frame_get_ie_field_len`.
    const fn ie_field_len(self) -> Option<u8> {
        let Some(offset) = self.ie_header_offset() else {
            return None;
        };
        let footer = self.mic_len() + FCS_SIZE;
        let frame_len = self.length() as u16;
        let offset = offset as u16;
        let footer = footer as u16;
        let mut length: u16 = 0;
        while frame_len > offset + length + footer {
            if offset + length + 1 >= frame_len {
                break;
            }
            let (Some(low), Some(high)) = (
                self.byte((offset + length) as u8),
                self.byte((offset + length + 1) as u8),
            ) else {
                return None;
            };
            let header = (high as u16) << 8 | low as u16;
            if header & IE_HEADER_ID_MASK == IE_TYPE_HT2 {
                length += IE_HEADER_LEN as u16;
                break;
            }
            length += IE_HEADER_LEN as u16 + (header & IE_SUBFIELD_LEN_MASK);
        }
        if length > u8::MAX as u16 {
            None
        } else {
            Some(length as u8)
        }
    }

    /// The image offset of the descriptor of the first header IE with
    /// element `id`, as OpenThread's `Frame::GetHeaderIe` finds it: the
    /// header IEs of a 2015 frame that has them follow its auxiliary
    /// security header, up to a header termination IE or the payload.
    pub const fn header_ie(self, id: u8) -> Option<u8> {
        if !matches!(self.version(), FrameVersion::V2015) || !self.ie_present() {
            return None;
        }
        let Some(mut offset) = self.security_header_offset() else {
            return None;
        };
        if self.security_enabled() {
            match self.security_field_len() {
                Some(length) => offset += length,
                None => return None,
            }
        }
        let end = self.length() as u16;
        let mut index = offset as u16;
        while index + IE_HEADER_LEN as u16 + FCS_SIZE as u16 <= end {
            let (Some(low), Some(high)) = (self.byte(index as u8), self.byte(index as u8 + 1))
            else {
                return None;
            };
            let descriptor = (high as u16) << 8 | low as u16;
            let element = ((descriptor & IE_HEADER_ID_MASK) >> 7) as u8;
            if element == id {
                return Some(index as u8);
            }
            // Header termination IEs end the header IEs.
            if element == 0x7e || element == 0x7f {
                return None;
            }
            index += IE_HEADER_LEN as u16 + (descriptor & IE_SUBFIELD_LEN_MASK);
        }
        None
    }

    /// Offset of the byte after the auxiliary security header and header IEs.
    const fn mac_payload_offset(self) -> Option<u8> {
        let Some(mut offset) = self.security_header_offset() else {
            return None;
        };
        if self.security_enabled() {
            match self.security_field_len() {
                Some(length) => offset += length,
                None => return None,
            }
        }
        if matches!(self.version(), FrameVersion::V2015) && self.ie_present() {
            match self.ie_field_len() {
                Some(length) => match offset.checked_add(length) {
                    Some(end) => offset = end,
                    None => return None,
                },
                None => return None,
            }
        }
        Some(offset)
    }

    /// `ieee802154_frame_get_security_payload_offset`: the PSDU offset of the
    /// first secured payload byte, after a command identifier for 2003/2006
    /// command frames.
    pub const fn security_payload_offset(self) -> Option<u8> {
        let Some(mut offset) = self.mac_payload_offset() else {
            return None;
        };
        if matches!(self.version(), FrameVersion::V2003 | FrameVersion::V2006)
            && matches!(self.frame_type(), FrameType::Command)
        {
            match offset.checked_add(COMMAND_ID_LEN) {
                Some(end) => offset = end,
                None => return None,
            }
        }
        Some(offset - 1)
    }

    /// `ieee802154_is_data_request`: a command frame whose command identifier
    /// is Data Request.
    pub const fn is_data_request(self) -> bool {
        if !matches!(self.frame_type(), FrameType::Command) {
            return false;
        }
        match self.mac_payload_offset() {
            Some(offset) => matches!(self.byte(offset), Some(COMMAND_DATA_REQUEST)),
            None => false,
        }
    }

    const fn copy_address(self, offset: u8, mode: AddressMode) -> Option<FrameAddress> {
        match mode {
            AddressMode::Short => match (self.byte(offset), self.byte(offset + 1)) {
                (Some(low), Some(high)) => Some(FrameAddress::Short([low, high])),
                _ => None,
            },
            AddressMode::Extended => {
                let mut address = [0; 8];
                let mut index = 0;
                while index < 8 {
                    match self.byte(offset + index) {
                        Some(byte) => address[index as usize] = byte,
                        None => return None,
                    }
                    index += 1;
                }
                Some(FrameAddress::Extended(address))
            }
            AddressMode::None | AddressMode::Reserved => None,
        }
    }

    /// `ieee802154_frame_get_dst_addr`: `Err(())` for an unsupported frame
    /// type or a reserved mode, `Ok(None)` when no destination is present.
    #[allow(clippy::result_unit_err)]
    pub const fn destination_address(self) -> Result<Option<FrameAddress>, ()> {
        let mode = self.destination_mode();
        if !self.frame_type().is_supported() || !mode.is_valid() {
            return Err(());
        }
        let mut offset = self.address_offset();
        if self.destination_panid_present() {
            offset += PANID_SIZE;
        }
        Ok(self.copy_address(offset, mode))
    }

    /// `ieee802154_frame_get_src_addr`: `Err(())` for an unsupported frame
    /// type or a reserved mode, `Ok(None)` when no source is present.
    #[allow(clippy::result_unit_err)]
    pub const fn source_address(self) -> Result<Option<FrameAddress>, ()> {
        let mode = self.source_mode();
        if !self.frame_type().is_supported() || !self.valid_modes() {
            return Err(());
        }
        let mut offset = self.address_offset();
        if self.destination_panid_present() {
            offset += PANID_SIZE;
        }
        offset += self.destination_mode().size();
        if self.source_panid_present() {
            offset += PANID_SIZE;
        }
        Ok(self.copy_address(offset, mode))
    }

    /// `ieee802154_frame_get_dest_panid`.
    pub const fn destination_panid(self) -> Option<[u8; 2]> {
        if !self.destination_panid_present() {
            return None;
        }
        let offset = self.address_offset();
        match (self.byte(offset), self.byte(offset + 1)) {
            (Some(low), Some(high)) => Some([low, high]),
            _ => None,
        }
    }

    /// `ieee802154_frame_get_src_panid`.
    pub const fn source_panid(self) -> Option<[u8; 2]> {
        if !self.destination_mode().is_valid() || !self.source_panid_present() {
            return None;
        }
        let mut offset = self.address_offset();
        if self.destination_panid_present() {
            offset += PANID_SIZE;
        }
        offset += self.destination_mode().size();
        match (self.byte(offset), self.byte(offset + 1)) {
            (Some(low), Some(high)) => Some([low, high]),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
