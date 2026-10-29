//! Whether the running image serves a scenario's services: its key pages
//! hold every key of each.

use std::{collections::BTreeSet, time::Duration};

use oer_hil_link::{Boot, CallError, Link};
use oer_hil_protocol::{Key, base::GetImageKeys};

/// How long one key page may take.
const PAGE_TIMEOUT: Duration = Duration::from_secs(2);

/// Every key the image of `boot` serves, page by page.
pub(crate) fn served(link: &Link, boot: &Boot) -> Result<BTreeSet<Key>, CallError> {
    let mut keys = BTreeSet::new();
    let mut first = 0_u16;
    while first < boot.hello.keys {
        let page = link.call(boot, GetImageKeys { first }, PAGE_TIMEOUT)?;
        let (Ok(length), true) = (u16::try_from(page.keys.len()), page.first == first) else {
            return Err(CallError::Payload {
                path: "base/image-keys",
            });
        };
        if length == 0 || page.total != boot.hello.keys {
            return Err(CallError::Payload {
                path: "base/image-keys",
            });
        }
        keys.extend(page.keys.iter().copied());
        first = first.saturating_add(length);
    }
    Ok(keys)
}

/// The keys of `required` that `served` lacks.
pub(crate) fn missing(required: &[&[Key]], served: &BTreeSet<Key>) -> Vec<Key> {
    required
        .iter()
        .copied()
        .flatten()
        .filter(|key| !served.contains(key))
        .copied()
        .collect()
}
