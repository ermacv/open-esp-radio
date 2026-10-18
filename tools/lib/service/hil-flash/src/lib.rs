//! The HIL flash operation, how a run writes a board's flash:
//!
//! 1. **lease**: the target is a leased board ([`oer_stand_board::LeasedBoard`]),
//!    which holds the board's device access: the arbiter's grant takes it
//!    for every leased board;
//! 2. **write**: the devices layer's one write operation
//!    (`oer_devices::image`) invalidates the board's receipt, writes the image
//!    bundle's segments, publishes the receipt of the whole bundle, starts
//!    the image as the chip profile's `[flash] start` says and confirms the
//!    start;
//! 3. **journal**: the board journal records the flash as history
//!    ([`Journal::record_flash`]); it never decides whether a board carries
//!    an image.
//!
//! [`flash_if_changed`] skips a board whose receipt says it runs the whole
//! bundle. The runner's flash of the device under test and of the reference
//! peers, and the calibration capture call [`flash`] or
//! [`flash_if_changed`]; [`catalog`] flashes an ESP-IDF catalog image.
#![forbid(unsafe_code)]

use std::path::Path;

use oer_device_lock::DeviceId;
use oer_devices::image::{Receipt, Store};
use oer_image_bundle::ImageBundle;
use oer_stand_board::Via;
use oer_stand_journal::{ImageIdentity, Journal};

pub mod catalog;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A board the operation writes: a [`oer_stand_board::LeasedBoard`], or a
/// test's fake.
pub trait Target {
    /// The board.
    fn mac(&self) -> &DeviceId;
    /// Write, start and confirm `bundle` as `image` through `via` for `by`;
    /// the receipt the write published into `store`.
    fn flash(
        &self,
        store: &Store,
        bundle: &ImageBundle,
        image: &str,
        via: Via,
        by: &str,
    ) -> Result<Receipt>;
    /// The receipt of `bundle` when the board runs it, by the receipts in
    /// `store`, read once.
    fn carries(&self, store: &Store, bundle: &ImageBundle) -> Result<Option<Receipt>>;
}

impl Target for oer_stand_board::LeasedBoard {
    fn mac(&self) -> &DeviceId {
        oer_stand_board::LeasedBoard::mac(self)
    }

    fn flash(
        &self,
        store: &Store,
        bundle: &ImageBundle,
        image: &str,
        via: Via,
        by: &str,
    ) -> Result<Receipt> {
        oer_stand_board::LeasedBoard::flash(self, store, bundle, image, via, by)
    }

    fn carries(&self, store: &Store, bundle: &ImageBundle) -> Result<Option<Receipt>> {
        oer_stand_board::LeasedBoard::carries(self, store, bundle)
    }
}

/// The source an image was built from: its commit and whether the tree
/// differed from it, when known.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Revision {
    pub commit: Option<String>,
    pub dirty: Option<bool>,
}

impl Revision {
    /// The checkout at `root`'s commit and whether `directory` differs from
    /// it.
    pub fn of_checkout(root: &Path, directory: &Path) -> Self {
        let git = |args: &[&str]| oer_process::git::text(root, args).ok();
        Self {
            commit: git(&["rev-parse", "HEAD"]),
            dirty: git(&["status", "--porcelain", "--", &directory.to_string_lossy()])
                .map(|status| !status.is_empty()),
        }
    }
}

/// What the operation writes and journals.
pub struct Image<'a> {
    pub bundle: &'a ImageBundle,
    /// The name the receipt and the journal know the image by.
    pub name: &'a str,
    pub revision: Revision,
    /// The run or command that writes it.
    pub origin: String,
}

/// What [`flash_if_changed`] did, with the receipt of what the board runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Flashed {
    Written(Receipt),
    /// The board's receipt says it already runs the whole bundle; nothing
    /// was written.
    Carried(Receipt),
}

impl Flashed {
    pub fn receipt(&self) -> &Receipt {
        match self {
            Self::Written(receipt) | Self::Carried(receipt) => receipt,
        }
    }
}

/// Write `image` into `target`, receipted in `store`, journal it for
/// `owner` in `journal`; the receipt.
pub fn flash(
    journal: &Journal,
    store: &Store,
    owner: &str,
    target: &dyn Target,
    image: &Image<'_>,
    via: Via,
) -> Result<Receipt> {
    let receipt = target.flash(store, image.bundle, image.name, via, &image.origin)?;
    let application = receipt
        .segment("application")
        .ok_or("the receipt names no application segment")?;
    journal.record_flash(
        owner.to_owned(),
        target.mac(),
        &ImageIdentity {
            name: image.name.to_owned(),
            sha256: application.sha256.clone(),
            commit: image.revision.commit.clone(),
            dirty: image.revision.dirty,
            origin: image.origin.clone(),
        },
    )?;
    Ok(receipt)
}

/// [`flash`], unless the board's receipt says it already runs the whole
/// bundle.
pub fn flash_if_changed(
    journal: &Journal,
    store: &Store,
    owner: &str,
    target: &dyn Target,
    image: &Image<'_>,
    via: Via,
) -> Result<Flashed> {
    if let Some(receipt) = target.carries(store, image.bundle)? {
        return Ok(Flashed::Carried(receipt));
    }
    flash(journal, store, owner, target, image, via).map(Flashed::Written)
}

#[cfg(test)]
mod tests;
