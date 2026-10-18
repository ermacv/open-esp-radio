//! Coexistence priorities a scheduler item carries to the shared arbiter.
//!
//! The item word at `+0x24` holds four five-bit priority lanes in bits
//! 19:0; the connection link state holds a six-bit protection in bits 19:14
//! of `+0x30`. The values are a policy of the role: the memory only encodes
//! them.

/// One five-bit coexistence priority lane of a scheduler item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerItemCoexistencePriority(u8);

impl SchedulerItemCoexistencePriority {
    /// A lane from 0 to 31.
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 31 { Some(Self(value)) } else { None }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// The four lanes of an advertising channel item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingCoexistencePriorities {
    pub lanes: [SchedulerItemCoexistencePriority; 4],
}

/// The four lanes of a passive scan window item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanCoexistencePriorities {
    pub lanes: [SchedulerItemCoexistencePriority; 4],
}

/// The two lanes of a connection event item: the event's own priority and
/// the connection's base priority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionCoexistencePriorities {
    pub event: SchedulerItemCoexistencePriority,
    pub base: SchedulerItemCoexistencePriority,
}

/// Six-bit coexistence protection of a connection's link state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionCoexistenceProtection(u8);

impl PeripheralConnectionCoexistenceProtection {
    /// A protection from 0 to 63.
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 63 { Some(Self(value)) } else { None }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Lanes occupying bits 19:0 of the item word.
pub(crate) const LANES_MASK: u32 = 0x000f_ffff;

/// The low lanes of the item word, from lane zero up.
pub(crate) const fn lanes_image(lanes: &[SchedulerItemCoexistencePriority]) -> u32 {
    let mut image = 0;
    let mut index = 0;
    while index < lanes.len() {
        image |= (lanes[index].0 as u32) << (5 * index);
        index += 1;
    }
    image
}

#[cfg(test)]
mod tests;
