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

impl<'a> ImageKeySet<'a> {
    /// `keys` must come from [`sorted_keys`].
    pub const fn new(keys: &'a [Key]) -> Self {
        let mut index = 1;
        while index < keys.len() {
            assert!(
                keys[index - 1].to_u64() < keys[index].to_u64(),
                "capability keys must be sorted and distinct"
            );
            index += 1;
        }
        Self { keys }
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
        let start = usize::from(first).min(self.keys.len());
        let end = (start + IMAGE_KEYS_PER_PAGE).min(self.keys.len());
        ImageKeyPage {
            first,
            total: self.keys.len() as u16,
            keys: heapless::Vec::from_slice(&self.keys[start..end])
                .expect("a page holds at most its capacity"),
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
    let mut index = 1;
    while index < N {
        let mut at = index;
        while at > 0 && keys[at - 1].to_u64() > keys[at].to_u64() {
            let swap = keys[at - 1];
            keys[at - 1] = keys[at];
            keys[at] = swap;
            at -= 1;
        }
        index += 1;
    }
    keys
}
