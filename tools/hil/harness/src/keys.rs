//! Whether the running image serves a scenario's services: its key pages
//! hold every key of each.

use std::{collections::BTreeSet, fmt, time::Duration};

use oer_hil_link::{Boot, CallError, Link};
use oer_hil_protocol::{
    Key,
    base::{GetImageKeys, ImageKeysCheck, ImageKeysError},
};

/// How long one key page may take.
const PAGE_TIMEOUT: Duration = Duration::from_secs(2);

/// Why the image's key set is unknown.
#[derive(Debug)]
pub(crate) enum KeysError {
    /// A page request failed.
    Call(CallError),
    /// The pages are not the set the image's Hello announced.
    Set(ImageKeysError),
}

impl fmt::Display for KeysError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Call(error) => write!(formatter, "{error}"),
            Self::Set(error) => write!(
                formatter,
                "the key pages are not the set the Hello announced: {error:?}"
            ),
        }
    }
}

/// Every key the image of `boot` serves, page by page, once the pages are
/// exactly the set its Hello announced.
pub(crate) fn served(link: &Link, boot: &Boot) -> Result<BTreeSet<Key>, KeysError> {
    let mut check = ImageKeysCheck::new(&boot.hello);
    let mut keys = Vec::new();
    while let Some(first) = check.next_first() {
        let page = link
            .call(boot, GetImageKeys { first }, PAGE_TIMEOUT)
            .map_err(KeysError::Call)?;
        check.accept(&page).map_err(KeysError::Set)?;
        keys.extend_from_slice(&page.keys);
    }
    check.finish().map_err(KeysError::Set)?;
    Ok(keys.into_iter().collect())
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
