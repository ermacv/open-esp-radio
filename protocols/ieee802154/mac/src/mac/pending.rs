//! Frame-pending table and ACK pending-bit decision ported from the public
//! ESP-IDF IEEE 802.15.4 driver (`components/ieee802154/driver/esp_ieee802154_ack.c`).
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/ieee802154/driver/esp_ieee802154_ack.c` (Copyright Espressif
//! Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!

use super::header::{FrameAddress, FrameVersion, PhrFrame};

/// Automatic frame-pending policy (`ieee802154_ll_pending_mode_t`).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum AutoPendingMode {
    /// The pending bit is always set in the ACK to a data request.
    #[default]
    Disable,
    /// The pending bit is set in the ACK to a data request whose source
    /// address is in the pending table.
    Enable,
    /// The pending bit is set in every ACK whose source address is in the
    /// pending table.
    Enhanced,
    /// The pending bit is cleared only for a data request from a short source
    /// address in the pending table.
    Zigbee,
}

impl AutoPendingMode {
    /// Whether the mode needs the hardware's enhanced pending lookup
    /// (`ieee802154_ll_set_pending_mode(true)`).
    pub const fn selects_enhanced_lookup(self) -> bool {
        matches!(self, Self::Enhanced | Self::Zigbee)
    }
}

/// The pending table has no free entry for a new address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingTableFull;

/// Short and extended source addresses with pending data, `CAPACITY` of each.
///
/// Entries keep their slot until cleared; a cleared slot is reused by the
/// next insertion that scans past it first, as the vendor table does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingTable<const CAPACITY: usize> {
    short: [Option<[u8; 2]>; CAPACITY],
    extended: [Option<[u8; 8]>; CAPACITY],
}

impl<const CAPACITY: usize> Default for PendingTable<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAPACITY: usize> PendingTable<CAPACITY> {
    /// An empty table.
    pub const fn new() -> Self {
        Self {
            short: [None; CAPACITY],
            extended: [None; CAPACITY],
        }
    }

    fn insert<const N: usize>(
        slots: &mut [Option<[u8; N]>],
        address: [u8; N],
    ) -> Result<(), PendingTableFull> {
        if slots.contains(&Some(address)) {
            return Ok(());
        }
        let free = slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(PendingTableFull)?;
        *free = Some(address);
        Ok(())
    }

    fn remove<const N: usize>(slots: &mut [Option<[u8; N]>], address: [u8; N]) -> bool {
        match slots.iter_mut().find(|slot| **slot == Some(address)) {
            Some(slot) => {
                *slot = None;
                true
            }
            None => false,
        }
    }

    /// `ieee802154_add_pending_addr`: adding a present address succeeds
    /// without a second entry.
    pub fn add(&mut self, address: FrameAddress) -> Result<(), PendingTableFull> {
        match address {
            FrameAddress::Short(address) => Self::insert(&mut self.short, address),
            FrameAddress::Extended(address) => Self::insert(&mut self.extended, address),
        }
    }

    /// Whether [`Self::add`] of `address` would succeed: it is present or
    /// its half has a free slot.
    pub fn has_room(&self, address: FrameAddress) -> bool {
        match address {
            FrameAddress::Short(address) => {
                self.short.contains(&Some(address)) || self.short.contains(&None)
            }
            FrameAddress::Extended(address) => {
                self.extended.contains(&Some(address)) || self.extended.contains(&None)
            }
        }
    }

    /// `ieee802154_clear_pending_addr`: whether the address was present.
    pub fn clear(&mut self, address: FrameAddress) -> bool {
        match address {
            FrameAddress::Short(address) => Self::remove(&mut self.short, address),
            FrameAddress::Extended(address) => Self::remove(&mut self.extended, address),
        }
    }

    /// `ieee802154_reset_pending_table` of the short addresses.
    pub fn reset_short(&mut self) {
        self.short = [None; CAPACITY];
    }

    /// `ieee802154_reset_pending_table` of the extended addresses.
    pub fn reset_extended(&mut self) {
        self.extended = [None; CAPACITY];
    }

    /// `ieee802154_addr_in_pending_table`.
    pub fn contains(&self, address: FrameAddress) -> bool {
        match address {
            FrameAddress::Short(address) => self.short.contains(&Some(address)),
            FrameAddress::Extended(address) => self.extended.contains(&Some(address)),
        }
    }
}

/// Pending bit chosen for the ACK to `frame`, and whether the hardware ACK
/// carries it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AckPending {
    /// The frame-pending decision reported to the upper layer.
    pub pending: bool,
    /// Whether the decision is written to the hardware ACK: only 2003 and
    /// 2006 frames are acknowledged by the hardware.
    pub set_to_hardware: bool,
}

/// `ieee802154_ack_config_pending_bit` without its register accesses.
///
/// `enhanced_lookup` is the hardware pending-mode selector the driver read
/// first; when set, a data request starts as pending. A frame without a
/// parseable source keeps that initial decision.
pub fn ack_pending<const CAPACITY: usize>(
    frame: PhrFrame<'_>,
    enhanced_lookup: bool,
    mode: AutoPendingMode,
    table: &PendingTable<CAPACITY>,
) -> AckPending {
    let set_to_hardware = matches!(frame.version(), FrameVersion::V2003 | FrameVersion::V2006);
    let mut pending = enhanced_lookup && frame.is_data_request();
    if let Ok(source) = frame.source_address() {
        match mode {
            AutoPendingMode::Disable => pending = true,
            AutoPendingMode::Enable | AutoPendingMode::Enhanced => {
                if let Some(source) = source {
                    pending = table.contains(source);
                }
            }
            AutoPendingMode::Zigbee => {
                pending = !matches!(
                    source,
                    Some(address @ FrameAddress::Short(_)) if table.contains(address)
                );
            }
        }
    }
    AckPending {
        pending,
        set_to_hardware,
    }
}

#[cfg(test)]
mod tests;
