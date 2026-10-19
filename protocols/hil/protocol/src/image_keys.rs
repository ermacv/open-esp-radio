//! What a device's image serves: the keys of its endpoints, of the messages
//! it sends and of its properties, as the host assembles them from the
//! boot's [`Hello`](crate::base::Hello) and its image key pages.

extern crate alloc;

use alloc::{collections::BTreeSet, string::String, vec::Vec};

use crate::{
    Key, Message,
    base::{Hello, ImageKeyPage},
};

/// What the device's image serves: the keys of its endpoints, of the
/// messages it sends and of its properties.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct DeviceImageKeys {
    #[serde(serialize_with = "keys_as_paths")]
    keys: BTreeSet<Key>,
    pub maximum_payload_bytes: u16,
    pub maximum_wire_frame_bytes: u16,
}

fn keys_as_paths<S: serde::Serializer>(
    keys: &BTreeSet<Key>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(keys.iter().map(|key| {
        match crate::registry().find(|info| info.key == *key) {
            Some(info) => String::from(info.path),
            None => alloc::format!("{key}"),
        }
    }))
}

impl DeviceImageKeys {
    /// The set a Hello announces, from its pages.
    pub fn assemble(hello: Hello, pages: &[ImageKeyPage]) -> Result<Self, String> {
        let keys: Vec<Key> = pages
            .iter()
            .flat_map(|page| page.keys.iter().copied())
            .collect();
        if keys.len() != usize::from(hello.keys) {
            return Err(alloc::format!(
                "image key pages hold {} keys, the hello announced {}",
                keys.len(),
                hello.keys
            ));
        }
        if crate::base::digest(&keys) != hello.keys_digest {
            return Err(String::from(
                "image key pages do not match the hello's digest",
            ));
        }
        Ok(Self {
            keys: keys.into_iter().collect(),
            maximum_payload_bytes: hello.maximum_payload_bytes,
            maximum_wire_frame_bytes: hello.maximum_wire_frame_bytes,
        })
    }

    /// A set of `keys`, for image classes the host derives.
    pub fn of_keys(keys: impl IntoIterator<Item = Key>) -> Self {
        Self {
            keys: keys.into_iter().collect(),
            maximum_payload_bytes: 0,
            maximum_wire_frame_bytes: crate::MAX_WIRE_FRAME_BYTES as u16,
        }
    }

    /// Whether the image has message or property `M`, with this host's
    /// schema of it.
    pub fn has<M: Message>(&self) -> bool {
        self.keys.contains(&M::KEY)
    }

    /// The image has message or property `M`; otherwise the error names it.
    pub fn require<M: Message>(&self) -> Result<(), String> {
        if self.has::<M>() {
            Ok(())
        } else {
            Err(alloc::format!("the image does not serve `{}`", M::PATH))
        }
    }

    pub fn keys(&self) -> &BTreeSet<Key> {
        &self.keys
    }

    /// Whether the image reports the same keys as `other`.
    pub fn same_keys(&self, other: &Self) -> bool {
        self.keys == other.keys
    }
}
