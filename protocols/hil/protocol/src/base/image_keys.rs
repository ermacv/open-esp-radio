//! What an image serves: the keys of its endpoints, of the topics it
//! publishes and of its properties.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

use crate::Key;

/// Keys one [`ImageKeyPage`] carries.
pub const IMAGE_KEYS_PER_PAGE: usize = 48;

/// The first frame of every boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Hello {
    /// Keys the image serves.
    pub keys: u16,
    /// FNV-1a 64 over the sorted keys: equal digests mean equal capability
    /// sets, so a host may reuse the set it paged in before.
    pub keys_digest: u64,
    pub maximum_payload_bytes: u16,
    pub maximum_wire_frame_bytes: u16,
}

/// Keys `first..` of the image's sorted capability set.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ImageKeyPage {
    pub first: u16,
    pub total: u16,
    pub keys: heapless::Vec<Key, IMAGE_KEYS_PER_PAGE>,
}

/// An image's capability set: its keys in ascending order, so the same image
/// always pages out the same bytes.
#[derive(Clone, Copy, Debug)]
pub struct ImageKeySet<'a> {
    keys: &'a [Key],
}

/// Invalid capability-set input. No keys are discarded or reordered by validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageKeySetError {
    /// The wire count cannot represent this set.
    Length { length: usize },
    /// Adjacent keys are not strictly ascending.
    Order { previous: Key, current: Key },
}

impl<'a> From<&'a crate::feature_keys::ImageKeys> for ImageKeySet<'a> {
    fn from(keys: &'a crate::feature_keys::ImageKeys) -> Self {
        Self {
            keys: keys.as_slice(),
        }
    }
}

impl<'a> ImageKeySet<'a> {
    /// Validates the strictly ascending keys and their representable wire count.
    pub const fn new(keys: &'a [Key]) -> Result<Self, ImageKeySetError> {
        if keys.len() > u16::MAX as usize {
            return Err(ImageKeySetError::Length { length: keys.len() });
        }
        let mut remaining = keys;
        while let Some((previous, rest)) = remaining.split_first() {
            if let Some((current, _)) = rest.split_first()
                && previous.to_u64() >= current.to_u64()
            {
                return Err(ImageKeySetError::Order {
                    previous: *previous,
                    current: *current,
                });
            }
            remaining = rest;
        }
        Ok(Self { keys })
    }

    pub fn contains(&self, key: crate::Key) -> bool {
        self.keys.binary_search(&key).is_ok()
    }

    pub fn hello(&self, maximum_payload_bytes: u16) -> Hello {
        Hello {
            keys: self.keys.len() as u16,
            keys_digest: digest(self.keys),
            maximum_payload_bytes,
            maximum_wire_frame_bytes: crate::MAX_WIRE_FRAME_BYTES as u16,
        }
    }

    pub fn page(&self, first: u16) -> ImageKeyPage {
        let length = self
            .keys
            .len()
            .saturating_sub(usize::from(first))
            .min(IMAGE_KEYS_PER_PAGE);
        let mut keys = heapless::Vec::from_array([Key([0; 8]); IMAGE_KEYS_PER_PAGE]);
        for (slot, key) in keys
            .iter_mut()
            .zip(self.keys.iter().skip(usize::from(first)))
        {
            *slot = *key;
        }
        keys.truncate(length);
        ImageKeyPage {
            first,
            total: self.keys.len() as u16,
            keys,
        }
    }
}

/// FNV-1a 64 over the keys in order.
pub fn digest(keys: &[Key]) -> u64 {
    let mut state = 0xcbf2_9ce4_8422_2325_u64;
    for byte in keys.iter().flat_map(|key| key.0) {
        state ^= u64::from(byte);
        state = state.wrapping_mul(0x0000_0100_0000_01b3);
    }
    state
}

/// `keys` in the ascending order [`ImageKeySet`] requires, at compile time.
pub const fn sorted_keys<const N: usize>(mut keys: [Key; N]) -> [Key; N] {
    let mut remaining = keys.as_mut_slice();
    while let Some((first, rest)) = remaining.split_first_mut() {
        let mut candidates = &mut *rest;
        while let Some((candidate, tail)) = candidates.split_first_mut() {
            if first.to_u64() > candidate.to_u64() {
                core::mem::swap(first, candidate);
            }
            candidates = tail;
        }
        remaining = rest;
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsorted_and_duplicate_keys_without_losing_observations() {
        for keys in [[Key([2; 8]), Key([1; 8])], [Key([1; 8]); 2]] {
            assert_eq!(
                ImageKeySet::new(&keys).unwrap_err(),
                ImageKeySetError::Order {
                    previous: keys[0],
                    current: keys[1]
                }
            );
        }
    }

    #[test]
    fn rejects_counts_that_the_wire_cannot_represent() {
        let keys = [Key([0; 8]); u16::MAX as usize + 1];
        assert_eq!(
            ImageKeySet::new(&keys).unwrap_err(),
            ImageKeySetError::Length { length: keys.len() }
        );
    }

    #[test]
    fn sorting_retains_duplicate_keys_and_empty_and_singleton_inputs() {
        assert_eq!(sorted_keys([]), []);
        assert_eq!(sorted_keys([Key([1; 8])]), [Key([1; 8])]);
        assert_eq!(
            sorted_keys([Key([3; 8]), Key([1; 8]), Key([3; 8]), Key([2; 8])]),
            [Key([1; 8]), Key([2; 8]), Key([3; 8]), Key([3; 8])]
        );
    }

    #[test]
    fn page_beyond_the_set_keeps_its_requested_first_and_is_empty() {
        let keys = sorted_keys([Key([2; 8]), Key([1; 8])]);
        let set = ImageKeySet::new(&keys).unwrap();
        for first in [2, 3, u16::MAX] {
            let page = set.page(first);
            assert_eq!((page.first, page.total), (first, 2));
            assert!(page.keys.is_empty());
        }
        assert!(ImageKeySet::new(&[]).unwrap().page(0).keys.is_empty());
    }
}
