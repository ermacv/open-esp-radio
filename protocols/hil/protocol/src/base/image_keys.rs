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
    /// The image's identity: its build's payload checksum (on a staged
    /// image, the stage-two payload CRC-32 the bootstrap checked). The host
    /// compares it with the image it wrote.
    pub image_crc32: u32,
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

    /// The keys, ascending.
    pub const fn keys(&self) -> &'a [Key] {
        self.keys
    }

    pub fn contains(&self, key: crate::Key) -> bool {
        self.keys.binary_search(&key).is_ok()
    }

    /// The Hello of an image of identity `image_crc32` serving this set.
    pub fn hello(&self, maximum_payload_bytes: u16, image_crc32: u32) -> Hello {
        Hello {
            keys: self.keys.len() as u16,
            keys_digest: digest(self.keys),
            maximum_payload_bytes,
            maximum_wire_frame_bytes: crate::MAX_WIRE_FRAME_BYTES as u16,
            image_crc32,
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
    keys.iter()
        .fold(DIGEST_BASIS, |state, key| digest_key(state, *key))
}

const DIGEST_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// `state` continued over `key`'s bytes.
fn digest_key(state: u64, key: Key) -> u64 {
    key.0.iter().fold(state, |state, byte| {
        (state ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The receiving side's check of an image's key pages against its
/// [`Hello`], one page at a time and without storing the keys: the pages
/// follow one another from key 0, none is empty or runs past the set, the
/// keys ascend strictly across pages and their digest is the Hello's. The
/// caller keeps a page's keys only after [`Self::accept`] took it and uses
/// them only after [`Self::finish`].
#[derive(Clone, Copy, Debug)]
pub struct ImageKeysCheck {
    total: u16,
    expected_digest: u64,
    next: u16,
    previous: Option<Key>,
    state: u64,
}

/// Why an image's key pages are not the set its Hello announced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageKeysError {
    /// The page starts elsewhere than where the previous one ended.
    First { expected: u16, first: u16 },
    /// The page's total is not the Hello's.
    Total { hello: u16, page: u16 },
    /// A page before the end of the set holds no key.
    Empty { first: u16 },
    /// The page holds more keys than the set has left.
    Overrun { first: u16, length: usize },
    /// A key does not follow its predecessor in strictly ascending order.
    Order { previous: Key, current: Key },
    /// Fewer keys arrived than the Hello announced.
    Incomplete { received: u16, total: u16 },
    /// The keys' digest is not the Hello's.
    Digest { hello: u64, keys: u64 },
}

impl ImageKeysCheck {
    pub const fn new(hello: &Hello) -> Self {
        Self {
            total: hello.keys,
            expected_digest: hello.keys_digest,
            next: 0,
            previous: None,
            state: DIGEST_BASIS,
        }
    }

    /// The first key of the page to ask for next; `None` once every key
    /// arrived.
    pub const fn next_first(&self) -> Option<u16> {
        if self.next < self.total {
            Some(self.next)
        } else {
            None
        }
    }

    /// Take `page` into the check.
    pub fn accept(&mut self, page: &ImageKeyPage) -> Result<(), ImageKeysError> {
        if page.first != self.next {
            return Err(ImageKeysError::First {
                expected: self.next,
                first: page.first,
            });
        }
        if page.total != self.total {
            return Err(ImageKeysError::Total {
                hello: self.total,
                page: page.total,
            });
        }
        let remaining = self.total.saturating_sub(self.next);
        if page.keys.is_empty() {
            return Err(ImageKeysError::Empty { first: page.first });
        }
        let length = match u16::try_from(page.keys.len()) {
            Ok(length) if length <= remaining => length,
            _ => {
                return Err(ImageKeysError::Overrun {
                    first: page.first,
                    length: page.keys.len(),
                });
            }
        };
        let mut previous = self.previous;
        let mut state = self.state;
        for current in page.keys.iter().copied() {
            if let Some(previous) = previous
                && previous.to_u64() >= current.to_u64()
            {
                return Err(ImageKeysError::Order { previous, current });
            }
            previous = Some(current);
            state = digest_key(state, current);
        }
        self.previous = previous;
        self.state = state;
        self.next = self.next.saturating_add(length);
        Ok(())
    }

    /// Whether the accepted pages are exactly the announced set.
    pub const fn finish(&self) -> Result<(), ImageKeysError> {
        if self.next < self.total {
            return Err(ImageKeysError::Incomplete {
                received: self.next,
                total: self.total,
            });
        }
        if self.state != self.expected_digest {
            return Err(ImageKeysError::Digest {
                hello: self.expected_digest,
                keys: self.state,
            });
        }
        Ok(())
    }
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

/// The total number of keys in `groups`; `None` when it overflows.
pub const fn keys_len(groups: &[&[Key]]) -> Option<usize> {
    let mut total = 0_usize;
    let mut remaining = groups;
    while let Some((group, rest)) = remaining.split_first() {
        let Some(sum) = total.checked_add(group.len()) else {
            return None;
        };
        total = sum;
        remaining = rest;
    }
    Some(total)
}

/// `groups` one after another, at compile time; `None` unless they hold
/// exactly `N` keys.
pub const fn concat_keys<const N: usize>(groups: &[&[Key]]) -> Option<[Key; N]> {
    let mut keys = [Key([0; 8]); N];
    let mut destination = keys.as_mut_slice();
    let mut remaining = groups;
    while let Some((group, rest)) = remaining.split_first() {
        let mut source = *group;
        while let Some((key, tail)) = source.split_first() {
            let Some((slot, free)) = destination.split_first_mut() else {
                return None;
            };
            *slot = *key;
            destination = free;
            source = tail;
        }
        remaining = rest;
    }
    if destination.is_empty() {
        Some(keys)
    } else {
        None
    }
}

/// How many distinct keys ascending `keys` holds: each run of equal keys
/// counts once.
pub const fn distinct_len(keys: &[Key]) -> usize {
    let mut count = 0_usize;
    let mut previous: Option<u64> = None;
    let mut remaining = keys;
    while let Some((key, rest)) = remaining.split_first() {
        let value = key.to_u64();
        let repeated = match previous {
            Some(last) => last == value,
            None => false,
        };
        if !repeated {
            count = count.saturating_add(1);
        }
        previous = Some(value);
        remaining = rest;
    }
    count
}

/// The distinct keys of ascending `keys`, at compile time: the union of
/// key groups that may share a key (two endpoints with one response).
/// `None` unless `keys` ascend and hold exactly `N` distinct keys.
pub const fn distinct_keys<const N: usize>(keys: &[Key]) -> Option<[Key; N]> {
    let mut distinct = [Key([0; 8]); N];
    let mut destination = distinct.as_mut_slice();
    let mut previous: Option<u64> = None;
    let mut remaining = keys;
    while let Some((key, rest)) = remaining.split_first() {
        let value = key.to_u64();
        match previous {
            Some(last) if last == value => {}
            Some(last) if last > value => return None,
            _ => {
                let Some((slot, free)) = destination.split_first_mut() else {
                    return None;
                };
                *slot = *key;
                destination = free;
            }
        }
        previous = Some(value);
        remaining = rest;
    }
    if destination.is_empty() {
        Some(distinct)
    } else {
        None
    }
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
    fn groups_concatenate_only_into_their_exact_length() {
        let groups: &[&[Key]] = &[&[Key([2; 8])], &[], &[Key([1; 8]), Key([2; 8])]];
        assert_eq!(keys_len(groups), Some(3));
        assert_eq!(
            concat_keys::<3>(groups),
            Some([Key([2; 8]), Key([1; 8]), Key([2; 8])])
        );
        assert_eq!(concat_keys::<2>(groups), None);
        assert_eq!(concat_keys::<4>(groups), None);
        assert_eq!(keys_len(&[]), Some(0));
    }

    #[test]
    fn shared_keys_merge_once_and_unsorted_input_is_refused() {
        let sorted = sorted_keys([Key([2; 8]), Key([1; 8]), Key([2; 8]), Key([3; 8])]);
        assert_eq!(distinct_len(&sorted), 3);
        assert_eq!(
            distinct_keys::<3>(&sorted),
            Some([Key([1; 8]), Key([2; 8]), Key([3; 8])])
        );
        assert_eq!(distinct_keys::<2>(&sorted), None);
        assert_eq!(distinct_keys::<4>(&sorted), None);
        assert_eq!(distinct_keys::<2>(&[Key([2; 8]), Key([1; 8])]), None);
        assert_eq!(distinct_len(&[]), 0);
    }

    /// Ascending keys `1..=count`.
    fn ascending(count: u64) -> heapless::Vec<Key, 128> {
        (1..=count).map(|value| Key(value.to_be_bytes())).collect()
    }

    fn page(first: u16, total: u16, keys: &[Key]) -> ImageKeyPage {
        ImageKeyPage {
            first,
            total,
            keys: heapless::Vec::from_slice(keys).unwrap(),
        }
    }

    /// Feed `pages` to the check of `hello`; the first refusal.
    fn check(hello: &Hello, pages: &[ImageKeyPage]) -> Result<(), ImageKeysError> {
        let mut check = ImageKeysCheck::new(hello);
        for page in pages {
            check.accept(page)?;
        }
        check.finish()
    }

    #[test]
    fn the_pages_of_a_set_pass_the_check_of_its_hello() {
        let keys = ascending(100);
        let set = ImageKeySet::new(&keys).unwrap();
        let hello = set.hello(0, 0);
        let mut check = ImageKeysCheck::new(&hello);
        let mut received = heapless::Vec::<Key, 128>::new();
        while let Some(first) = check.next_first() {
            let page = set.page(first);
            check.accept(&page).unwrap();
            received.extend_from_slice(&page.keys).unwrap();
        }
        assert_eq!(check.finish(), Ok(()));
        assert_eq!(received, keys);
        let empty = ImageKeySet::new(&[]).unwrap().hello(0, 0);
        assert_eq!(ImageKeysCheck::new(&empty).next_first(), None);
        assert_eq!(ImageKeysCheck::new(&empty).finish(), Ok(()));
    }

    #[test]
    fn a_set_other_than_the_announced_one_is_refused() {
        let keys = ascending(4);
        let hello = ImageKeySet::new(&keys).unwrap().hello(0, 0);
        let other = ascending(5);
        assert_eq!(
            check(&hello, &[page(0, 4, &other[1..])]),
            Err(ImageKeysError::Digest {
                hello: hello.keys_digest,
                keys: digest(&other[1..])
            })
        );
        let repeated = [keys[0], keys[1], keys[1], keys[3]];
        assert_eq!(
            check(
                &hello,
                &[page(0, 4, &repeated[..2]), page(2, 4, &repeated[2..])]
            ),
            Err(ImageKeysError::Order {
                previous: keys[1],
                current: keys[1]
            }),
            "a repeat across a page boundary"
        );
        let permuted = [keys[0], keys[2], keys[1], keys[3]];
        assert_eq!(
            check(&hello, &[page(0, 4, &permuted)]),
            Err(ImageKeysError::Order {
                previous: keys[2],
                current: keys[1]
            })
        );
    }

    #[test]
    fn pages_that_do_not_tile_the_set_are_refused() {
        let keys = ascending(4);
        let hello = ImageKeySet::new(&keys).unwrap().hello(0, 0);
        let five = ascending(5);
        assert_eq!(
            check(&hello, &[page(0, 4, &keys[..2]), page(2, 4, &five[2..])]),
            Err(ImageKeysError::Overrun {
                first: 2,
                length: 3
            }),
            "a last page longer than the rest of the set"
        );
        assert_eq!(
            check(&hello, &[page(0, 4, &keys[..2]), page(2, 4, &[])]),
            Err(ImageKeysError::Empty { first: 2 })
        );
        assert_eq!(
            check(&hello, &[page(0, 4, &keys[..2]), page(3, 4, &keys[3..])]),
            Err(ImageKeysError::First {
                expected: 2,
                first: 3
            })
        );
        assert_eq!(
            check(&hello, &[page(0, 5, &keys)]),
            Err(ImageKeysError::Total { hello: 4, page: 5 })
        );
        assert_eq!(
            check(&hello, &[page(0, 4, &keys[..3])]),
            Err(ImageKeysError::Incomplete {
                received: 3,
                total: 4
            })
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
