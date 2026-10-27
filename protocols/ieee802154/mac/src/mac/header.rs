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
//! header, fields outside the PHR-declared MAC bytes are absent. FCS (or
//! RSSI/LQI on reception) is never parsed as a header.
//! Bounds follow ESP-IDF `4d80c1b2`, `esp_ieee802154_frame.c`.

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

const COMMAND_DATA_REQUEST: u8 = 0x04;
use oer_wire::Reader;

oer_wire::bitfield! {
    // SOURCE: IEEE Std 802.15.4-2020, 12.1.3 (PHR frame length).
    struct Phr[1] (little) { length, with_length: u8 = 0..7; }
}
oer_wire::bitfield! {
    // SOURCE: IEEE Std 802.15.4-2020, 7.2.2 (Frame Control field).
    struct Control[2] (little) {
        kind, with_kind: u8 = 0..3;
        security, with_security: bool = 3..4;
        ack, with_ack: bool = 5..6;
        compression, with_compression: bool = 6..7;
        suppress_dsn, with_suppress_dsn: bool = 8..9;
        ie, with_ie: bool = 9..10;
        destination, with_destination: u8 = 10..12;
        version, with_version: u8 = 12..14;
        source, with_source: u8 = 14..16;
    }
}
oer_wire::bitfield! {
    // SOURCE: IEEE Std 802.15.4-2020, 9.4.2 (Security Control field).
    struct SecurityControl[1] (little) {
        level, with_level: u8 = 0..3;
        key_mode, with_key_mode: u8 = 3..5;
        suppress_counter, with_suppress_counter: bool = 5..6;
    }
}
oer_wire::bitfield! {
    // SOURCE: IEEE Std 802.15.4-2020, 7.4.2.1 (Header IE descriptor).
    struct HeaderIe[2] (little) {
        length, with_length: u8 = 0..7;
        id, with_id: u8 = 7..15;
        // SOURCE: ESP-IDF 4d80c1b2 esp_ieee802154_frame.h
        // IEEE802154_FRAME_IE_HEAD_ID_MASK: payload scanning ignores bit 14.
        termination_id, with_termination_id: u8 = 7..14, view;
    }
}

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
        match bits {
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
        match bits {
            0 => Self::V2003,
            1 => Self::V2006,
            2 => Self::V2015,
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
        match bits {
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

    /// A cursor restricted to declared MAC data, excluding both footer bytes.
    const fn mac_bytes(self, offset: u8, count: u8) -> Option<&'image [u8]> {
        let Some(length) = self.length().checked_sub(FCS_SIZE) else {
            return None;
        };
        let Some(skip) = offset.checked_sub(PHR_SIZE) else {
            return None;
        };
        let mut image = Reader::new(self.image);
        if image.take(PHR_SIZE as usize).is_err() {
            return None;
        }
        let Ok(mac) = image.take(length as usize) else {
            return None;
        };
        let mut mac = Reader::new(mac);
        if mac.take(skip as usize).is_err() {
            return None;
        }
        match mac.take(count as usize) {
            Ok(bytes) => Some(bytes),
            Err(_) => None,
        }
    }

    const fn byte(self, offset: u8) -> Option<u8> {
        let Some(bytes) = self.mac_bytes(offset, 1) else {
            return None;
        };
        match Reader::new(bytes).u8() {
            Ok(byte) => Some(byte),
            Err(_) => None,
        }
    }

    const fn header_byte(self, offset: u8) -> u8 {
        match self.byte(offset) {
            Some(byte) => byte,
            None => 0,
        }
    }

    const fn control(self) -> Control {
        Control::from_bytes([self.header_byte(1), self.header_byte(2)])
    }

    /// The seven-bit PHR length.
    pub const fn length(self) -> u8 {
        match Reader::new(self.image).u8() {
            Ok(byte) => Phr::from_bytes([byte]).length(),
            Err(_) => 0,
        }
    }

    /// `ieee802154_frame_get_type`.
    pub const fn frame_type(self) -> FrameType {
        FrameType::from_bits(self.control().kind())
    }

    /// `ieee802154_frame_get_version`.
    pub const fn version(self) -> FrameVersion {
        FrameVersion::from_bits(self.control().version())
    }

    /// `ieee802154_frame_is_security_enabled`.
    pub const fn security_enabled(self) -> bool {
        self.control().security()
    }

    /// `ieee802154_frame_is_ack_required`: only a supported frame type
    /// requests an acknowledgement.
    pub const fn ack_required(self) -> bool {
        self.mac_bytes(PHR_SIZE, FCF_SIZE).is_some()
            && self.frame_type().is_supported()
            && self.control().ack()
    }

    const fn panid_compression(self) -> bool {
        self.control().compression()
    }

    const fn ie_present(self) -> bool {
        self.control().ie()
    }

    const fn dsn_present(self) -> bool {
        !matches!(self.version(), FrameVersion::V2015) || !self.control().suppress_dsn()
    }

    /// Destination addressing mode.
    pub const fn destination_mode(self) -> AddressMode {
        AddressMode::from_bits(self.control().destination())
    }

    /// Source addressing mode.
    pub const fn source_mode(self) -> AddressMode {
        AddressMode::from_bits(self.control().source())
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
        if self.mac_bytes(PHR_SIZE, FCF_SIZE).is_none() || !self.frame_type().is_supported() {
            return None;
        }
        match self.address_size() {
            Some(size) if self.mac_bytes(self.address_offset(), size).is_some() => {
                self.address_offset().checked_add(size)
            }
            _ => None,
        }
    }

    const fn security_field_len(self) -> Option<u8> {
        let Some(offset) = self.security_header_offset() else {
            return None;
        };
        let Some(security_header) = self.byte(offset) else {
            return None;
        };
        let security_header = SecurityControl::from_bytes([security_header]);
        let mut length = SECURITY_HEADER_SIZE;
        if !security_header.suppress_counter() {
            length += FRAME_COUNTER_SIZE;
        }
        length += match security_header.key_mode() {
            1 => 1,
            2 => 5,
            3 => 9,
            _ => 0,
        };
        if self.mac_bytes(offset, length).is_none() {
            return None;
        }
        Some(length)
    }

    const fn ie_header_offset(self) -> Option<u8> {
        let Some(offset) = self.security_header_offset() else {
            return None;
        };
        if !self.security_enabled() {
            return Some(offset);
        }
        match self.security_field_len() {
            Some(length) => offset.checked_add(length),
            None => None,
        }
    }

    const fn mic_len(self) -> u8 {
        if !self.security_enabled() {
            return 0;
        }
        let Some(offset) = self.security_header_offset() else {
            return 0;
        };
        match SecurityControl::from_bytes([self.header_byte(offset)]).level() {
            1 | 5 => 4,
            2 | 6 => 8,
            3 | 7 => 16,
            _ => 0,
        }
    }

    const fn ie_bytes(self, offset: u8) -> Option<&'image [u8]> {
        let Some(end) = self.length().checked_add(PHR_SIZE) else {
            return None;
        };
        let Some(end) = end.checked_sub(FCS_SIZE + self.mic_len()) else {
            return None;
        };
        let Some(length) = end.checked_sub(offset) else {
            return None;
        };
        self.mac_bytes(offset, length)
    }

    /// `ieee802154_frame_get_ie_field_len`: each complete descriptor and body
    /// must fit before the MIC/FCS boundary; HT2 starts the payload.
    const fn ie_field_len(self) -> Option<u8> {
        let Some(offset) = self.ie_header_offset() else {
            return None;
        };
        let Some(bytes) = self.ie_bytes(offset) else {
            return None;
        };
        let mut cursor = Reader::new(bytes);
        while !cursor.is_empty() {
            let Ok(bytes) = cursor.read() else {
                return None;
            };
            let descriptor = HeaderIe::from_bytes(bytes);
            if descriptor.termination_id() == 0x7f {
                break;
            }
            if cursor.take(descriptor.length() as usize).is_err() {
                return None;
            }
        }
        // The cursor covers at most the seven-bit PHR extent.
        Some((bytes.len().saturating_sub(cursor.remaining().len())) as u8)
    }

    /// The image offset of the descriptor of the first header IE with
    /// element `id`, as OpenThread's `Frame::GetHeaderIe` finds it: the
    /// header IEs of a 2015 frame that has them follow its auxiliary
    /// security header, up to a header termination IE or the payload.
    pub const fn header_ie(self, id: u8) -> Option<u8> {
        if !matches!(self.version(), FrameVersion::V2015) || !self.ie_present() {
            return None;
        }
        let Some(offset) = self.ie_header_offset() else {
            return None;
        };
        let Some(bytes) = self.ie_bytes(offset) else {
            return None;
        };
        let mut cursor = Reader::new(bytes);
        while !cursor.is_empty() {
            let index = (bytes.len().saturating_sub(cursor.remaining().len())) as u8;
            let Ok(bytes) = cursor.read() else {
                return None;
            };
            let descriptor = HeaderIe::from_bytes(bytes);
            if cursor.take(descriptor.length() as usize).is_err() {
                return None;
            }
            if descriptor.id() == id {
                return offset.checked_add(index);
            }
            if descriptor.id() == 0x7e || descriptor.id() == 0x7f {
                return None;
            }
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
            if self.mac_bytes(offset, COMMAND_ID_LEN).is_none() {
                return None;
            }
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
        if self.mac_bytes(PHR_SIZE, FCF_SIZE).is_none()
            || !self.frame_type().is_supported()
            || !mode.is_valid()
        {
            return Err(());
        }
        let mut offset = self.address_offset();
        if self.destination_panid_present() {
            offset += PANID_SIZE;
        }
        match (mode, self.copy_address(offset, mode)) {
            (AddressMode::None, _) => Ok(None),
            (_, Some(address)) => Ok(Some(address)),
            _ => Err(()),
        }
    }

    /// `ieee802154_frame_get_src_addr`: `Err(())` for an unsupported frame
    /// type or a reserved mode, `Ok(None)` when no source is present.
    #[allow(clippy::result_unit_err)]
    pub const fn source_address(self) -> Result<Option<FrameAddress>, ()> {
        let mode = self.source_mode();
        if self.mac_bytes(PHR_SIZE, FCF_SIZE).is_none()
            || !self.frame_type().is_supported()
            || !self.valid_modes()
        {
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
        match (mode, self.copy_address(offset, mode)) {
            (AddressMode::None, _) => Ok(None),
            (_, Some(address)) => Ok(Some(address)),
            _ => Err(()),
        }
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
