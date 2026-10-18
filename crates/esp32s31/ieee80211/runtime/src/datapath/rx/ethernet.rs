//! Role-neutral bounded storage for decoded Ethernet receive batches.
//!
//! An 802.11 A-MSDU may yield several Ethernet frames while the network queue
//! owns fewer free slots.  This module records complete frames in caller-owned
//! scratch so a role adapter can release the DMA unit and resume publication
//! later without dropping a suffix of the aggregate.

use oer_ieee80211_mac::data::EthernetFrameParts;

const RECORD_PREFIX_SIZE: usize = 2;
const ETHERNET_HEADER_SIZE: usize = 14;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackedEthernetError {
    FrameTooLong,
    StorageExhausted,
    CorruptRecord,
}

/// Temporary writer over one caller-owned batch arena.
pub(crate) struct PackedEthernetWriter<'storage> {
    storage: &'storage mut [u8],
    used: usize,
}

impl<'storage> PackedEthernetWriter<'storage> {
    pub(crate) fn new(storage: &'storage mut [u8]) -> Self {
        Self { storage, used: 0 }
    }

    pub(crate) fn resume(
        storage: &'storage mut [u8],
        used: usize,
    ) -> Result<Self, PackedEthernetError> {
        if used > storage.len() {
            return Err(PackedEthernetError::CorruptRecord);
        }
        Ok(Self { storage, used })
    }

    pub(crate) const fn used(&self) -> usize {
        self.used
    }

    pub(crate) fn push(
        &mut self,
        frame: EthernetFrameParts<'_>,
    ) -> Result<(), PackedEthernetError> {
        let frame_length = frame.length();
        let encoded_length =
            u16::try_from(frame_length).map_err(|_| PackedEthernetError::FrameTooLong)?;
        let record_length = frame_length
            .checked_add(RECORD_PREFIX_SIZE)
            .ok_or(PackedEthernetError::StorageExhausted)?;
        let end = self
            .used
            .checked_add(record_length)
            .ok_or(PackedEthernetError::StorageExhausted)?;
        let record = self
            .storage
            .get_mut(self.used..end)
            .ok_or(PackedEthernetError::StorageExhausted)?;
        record[..RECORD_PREFIX_SIZE].copy_from_slice(&encoded_length.to_be_bytes());
        frame
            .copy_to(&mut record[RECORD_PREFIX_SIZE..])
            .map_err(|_| PackedEthernetError::StorageExhausted)?;
        self.used = end;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PackedEthernetRecord<'storage> {
    pub(crate) frame: EthernetFrameParts<'storage>,
    pub(crate) next_offset: usize,
}

/// Decode one immutable record without moving the publication cursor.
pub(crate) fn record_at(
    storage: &[u8],
    used: usize,
    offset: usize,
) -> Result<Option<PackedEthernetRecord<'_>>, PackedEthernetError> {
    if offset == used {
        return Ok(None);
    }
    if offset > used || used > storage.len() {
        return Err(PackedEthernetError::CorruptRecord);
    }
    let prefix_end = offset
        .checked_add(RECORD_PREFIX_SIZE)
        .ok_or(PackedEthernetError::CorruptRecord)?;
    let prefix = storage
        .get(offset..prefix_end)
        .ok_or(PackedEthernetError::CorruptRecord)?;
    let length = usize::from(u16::from_be_bytes([prefix[0], prefix[1]]));
    if length < ETHERNET_HEADER_SIZE {
        return Err(PackedEthernetError::CorruptRecord);
    }
    let end = prefix_end
        .checked_add(length)
        .filter(|end| *end <= used)
        .ok_or(PackedEthernetError::CorruptRecord)?;
    let ethernet = &storage[prefix_end..end];
    Ok(Some(PackedEthernetRecord {
        frame: EthernetFrameParts {
            destination: ethernet[..6]
                .try_into()
                .expect("validated Ethernet destination width"),
            source: ethernet[6..12]
                .try_into()
                .expect("validated Ethernet source width"),
            ether_type: u16::from_be_bytes([ethernet[12], ethernet[13]]),
            payload: &ethernet[ETHERNET_HEADER_SIZE..],
        },
        next_offset: end,
    }))
}

#[cfg(test)]
mod tests;
