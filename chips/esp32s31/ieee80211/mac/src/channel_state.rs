//! Rust-owned state for the ESP32-S31 channel manager.
//!
//! The layout and validation rules in this module come from the pinned
//! `wl_chm.o` archive:
//!
//! - home and current channel selectors are two-byte `(primary, secondary)`
//!   values;
//! - the 2.4 GHz table contains 14 records of 12 bytes;
//! - byte zero of every record is its one-based primary channel;
//! - the little-endian frequency in MHz starts at byte two.
//!
//! The remaining record bytes deliberately stay opaque. They are copied at
//! cold handoff so vendor leaves that still need a complete record can be
//! migrated without inventing meanings for fields that have not been proved.

use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};

pub const CHANNEL_COUNT: usize = 14;
pub const CHANNEL_INFO_BYTES: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelStateAdoptionError {
    StateUnavailable,
    OperationInProgress,
    InvalidHome,
    InvalidCurrent,
    InvalidRecord(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedChannel {
    pub frequency_mhz: u16,
    pub cbw: u8,
}

#[derive(Clone, Copy)]
#[repr(C, align(4))]
struct ChannelInfo {
    bytes: [u8; CHANNEL_INFO_BYTES],
}

impl ChannelInfo {
    const fn empty() -> Self {
        Self {
            bytes: [0; CHANNEL_INFO_BYTES],
        }
    }

    fn primary(&self) -> u8 {
        self.bytes[0]
    }

    fn frequency_mhz(&self) -> u16 {
        u16::from_le_bytes([self.bytes[2], self.bytes[3]])
    }
}

pub struct ChannelState {
    adopted: AtomicBool,
    home: AtomicU16,
    current: AtomicU16,
    channels: [ChannelInfo; CHANNEL_COUNT],
}

impl ChannelState {
    pub const fn new() -> Self {
        Self {
            adopted: AtomicBool::new(false),
            home: AtomicU16::new(0),
            current: AtomicU16::new(0),
            channels: [ChannelInfo::empty(); CHANNEL_COUNT],
        }
    }

    pub fn adopt(
        &mut self,
        home: [u8; 2],
        current: [u8; 2],
        records: [[u8; CHANNEL_INFO_BYTES]; CHANNEL_COUNT],
    ) -> Result<(), ChannelStateAdoptionError> {
        if !selector_valid(home) {
            return Err(ChannelStateAdoptionError::InvalidHome);
        }
        if !selector_valid(current) {
            return Err(ChannelStateAdoptionError::InvalidCurrent);
        }
        for (index, record) in records.iter().enumerate() {
            let expected_primary = index as u8 + 1;
            if record[0] != expected_primary
                || u16::from_le_bytes([record[2], record[3]])
                    != expected_frequency_mhz(expected_primary)
            {
                return Err(ChannelStateAdoptionError::InvalidRecord(expected_primary));
            }
        }

        for (destination, source) in self.channels.iter_mut().zip(records) {
            destination.bytes = source;
        }
        self.home.store(pack_selector(home), Ordering::Relaxed);
        self.current
            .store(pack_selector(current), Ordering::Relaxed);
        self.adopted.store(true, Ordering::Release);
        Ok(())
    }

    pub fn adopted(&self) -> bool {
        self.adopted.load(Ordering::Acquire)
    }

    pub fn home(&self) -> Option<[u8; 2]> {
        self.adopted()
            .then(|| unpack_selector(self.home.load(Ordering::Acquire)))
    }

    pub fn current(&self) -> Option<[u8; 2]> {
        self.adopted()
            .then(|| unpack_selector(self.current.load(Ordering::Acquire)))
    }

    pub fn set_current(&self, channel: [u8; 2]) -> Result<(), ChannelStateAdoptionError> {
        if !self.adopted() {
            return Err(ChannelStateAdoptionError::StateUnavailable);
        }
        if !selector_valid(channel) {
            return Err(ChannelStateAdoptionError::InvalidCurrent);
        }
        self.current
            .store(pack_selector(channel), Ordering::Release);
        Ok(())
    }

    pub fn promote_current_to_home(&self) -> Result<(), ChannelStateAdoptionError> {
        if !self.adopted() {
            return Err(ChannelStateAdoptionError::StateUnavailable);
        }
        self.home
            .store(self.current.load(Ordering::Acquire), Ordering::Release);
        Ok(())
    }

    pub fn prepare(&self, channel: [u8; 2]) -> Option<PreparedChannel> {
        if !self.adopted() || !selector_valid(channel) {
            return None;
        }
        let info = &self.channels[usize::from(channel[0] - 1)];
        if info.primary() != channel[0] {
            return None;
        }
        let mut frequency_mhz = info.frequency_mhz();
        let cbw = match channel[1] {
            0 => 0,
            1 if (1..=9).contains(&channel[0]) => {
                frequency_mhz = frequency_mhz.checked_add(10)?;
                2
            }
            2 if (5..=13).contains(&channel[0]) => {
                frequency_mhz = frequency_mhz.checked_sub(10)?;
                3
            }
            _ => return None,
        };
        Some(PreparedChannel { frequency_mhz, cbw })
    }
}

impl Default for ChannelState {
    fn default() -> Self {
        Self::new()
    }
}

fn selector_valid(channel: [u8; 2]) -> bool {
    (1..=CHANNEL_COUNT as u8).contains(&channel[0]) && channel[1] <= 2
}

const fn pack_selector(channel: [u8; 2]) -> u16 {
    u16::from_le_bytes(channel)
}

const fn unpack_selector(channel: u16) -> [u8; 2] {
    channel.to_le_bytes()
}

fn expected_frequency_mhz(primary: u8) -> u16 {
    if primary == 14 {
        2484
    } else {
        2407 + u16::from(primary) * 5
    }
}

#[cfg(test)]
mod tests;
