use std::cell::RefCell;

use oer_chip_profile::Start;

use super::*;

const MAC: &str = "xx:xx:xx:xx:25:64";

/// A leased board that records what the operation asks of it, keeps the
/// receipt its write would publish, and what the journal held when it was
/// asked to write.
struct Fake<'a> {
    mac: DeviceId,
    journal: &'a Journal,
    write: std::result::Result<(), &'static str>,
    running: RefCell<Option<Receipt>>,
    calls: RefCell<Vec<String>>,
}

impl<'a> Fake<'a> {
    fn new(journal: &'a Journal, write: std::result::Result<(), &'static str>) -> Self {
        Self {
            mac: DeviceId::parse(MAC).unwrap(),
            journal,
            write,
            running: RefCell::new(None),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Target for Fake<'_> {
    fn mac(&self) -> &DeviceId {
        &self.mac
    }

    fn flash(
        &self,
        _: &Store,
        bundle: &ImageBundle,
        image: &str,
        via: Via,
        by: &str,
    ) -> Result<Receipt> {
        let journaled = self.journal.latest_flash(&self.mac)?.is_some();
        self.calls.borrow_mut().push(format!(
            "flash {} {via:?} journaled={journaled}",
            bundle.chip
        ));
        self.write
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
        let receipt = Receipt::of(&bundle.snapshot()?, image, by);
        *self.running.borrow_mut() = Some(receipt.clone());
        Ok(receipt)
    }

    fn carries(&self, _: &Store, bundle: &ImageBundle) -> Result<Option<Receipt>> {
        let wanted = Receipt::of(&bundle.snapshot()?, "", "");
        Ok(self
            .running
            .borrow()
            .clone()
            .filter(|running| running.digest == wanted.digest))
    }
}

fn bundle(directory: &Path) -> ImageBundle {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    // A chip whose written image starts by power-on reset.
    let profile = oer_chip_profile::Profile::all(&root)
        .unwrap()
        .into_iter()
        .find(|profile| {
            profile
                .flash
                .as_ref()
                .is_some_and(|flash| flash.start == Start::PowerOn)
        })
        .unwrap();
    let output = directory.join("bundle");
    let staging = oer_image_bundle::staging_directory(&output).unwrap();
    std::fs::create_dir_all(&staging).unwrap();
    let bundle = ImageBundle::new(&staging, &profile, profile.flash.clone().unwrap());
    for segment in bundle.segments() {
        std::fs::write(&segment.file, segment.description).unwrap();
    }
    std::fs::write(bundle.application(), b"application").unwrap();
    bundle.publish(&output).unwrap()
}

fn image(bundle: &ImageBundle) -> Image<'_> {
    Image {
        bundle,
        name: "ieee802154-peer",
        revision: Revision {
            commit: Some("c0ffee".into()),
            dirty: Some(false),
        },
        origin: String::from("test"),
    }
}

#[test]
fn the_flash_writes_then_journals_as_history() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::at(directory.path().join("arbiter/board.jsonl"));
    let store = Store::at(directory.path().join("receipts"));
    let bundle = bundle(directory.path());
    let board = Fake::new(&journal, Ok(()));
    let receipt = flash(&journal, &store, "peer", &board, &image(&bundle), Via::Usb).unwrap();
    assert_eq!(
        *board.calls.borrow(),
        [format!("flash {} Usb journaled=false", bundle.chip)]
    );
    let flashed = journal.latest_flash(MAC).unwrap().unwrap();
    assert_eq!(flashed.owner, "peer");
    assert_eq!(
        receipt.segment("application").unwrap().sha256,
        oer_durable::sha256_bytes(b"application")
    );
}

#[test]
fn a_failed_write_is_not_journaled() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::at(directory.path().join("arbiter/board.jsonl"));
    let store = Store::at(directory.path().join("receipts"));
    let bundle = bundle(directory.path());
    let board = Fake::new(&journal, Err("Protocol error"));
    assert!(flash(&journal, &store, "peer", &board, &image(&bundle), Via::Usb).is_err());
    assert_eq!(board.calls.borrow().len(), 1);
    assert!(journal.latest_flash(MAC).unwrap().is_none());
}

#[test]
fn a_carried_bundle_is_answered_by_the_receipt_carries_read() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::at(directory.path().join("arbiter/board.jsonl"));
    let store = Store::at(directory.path().join("receipts"));
    let bundle = bundle(directory.path());
    let board = Fake::new(&journal, Ok(()));
    let written =
        flash_if_changed(&journal, &store, "peer", &board, &image(&bundle), Via::Usb).unwrap();
    assert!(matches!(written, Flashed::Written(_)));
    // The store itself holds nothing: only the receipt `carries` returned
    // answers.
    let carried =
        flash_if_changed(&journal, &store, "peer", &board, &image(&bundle), Via::Usb).unwrap();
    assert_eq!(carried, Flashed::Carried(written.receipt().clone()));
    assert_eq!(board.calls.borrow().len(), 1);
}
