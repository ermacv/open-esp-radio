//! What a board's flash holds, as the devices layer knows it, and the one
//! operation that changes it.
//!
//! [`write()`] is the only writer of a board's flash on the host: `cargo fw`
//! (through `oer-devices`) and the stand's boards (through
//! `oer-stand-board`, which HIL flashes with) both call it. Under the
//! board's [`DeviceAccess`] it
//!
//! 1. reads the bundle's flash contents **once** into owned bytes (its
//!    [`Snapshot`], verified against the manifest) and computes the
//!    [`Receipt`] of the **whole** snapshot: the SHA-256 of every segment
//!    (bootloader, partition table, application, OTA selection) at its
//!    offset, and one digest over them all;
//! 2. invalidates the board's known image ([`State::Writing`]) **before**
//!    any byte of flash changes, so an interrupted or failed write leaves no
//!    stale claim, and takes the board's next write **generation**;
//! 3. writes the same snapshot (espflash over USB, or OpenOCD over JTAG
//!    from a private directory the snapshot is materialized in);
//! 4. publishes [`State::Written`] with the receipt and its generation.
//!
//! The image then still has to start; the caller starts it as its
//! [`Written::pending_start`] says and confirms it with [`Written::started`],
//! which publishes [`State::Started`]. "Written" and "started successfully"
//! are distinct states.
//!
//! [`DeviceAccess`] is cloneable, so two writes in one process may
//! interleave; the generation orders them. Every write takes the next
//! generation of its board in the store, and a commit (the `Written` after
//! the flash, the `Started` of [`Written::started`]) succeeds only while
//! the board's state is still the one its own write left, with its
//! generation: a confirmation that a later write overtook fails as a stale
//! write. Each read-and-replace of a state holds the process's commit lock;
//! across processes the board's device lock serializes them.
//!
//! Whether a board already carries a bundle, so a flash may be skipped, is
//! decided by [`carries`] from this store alone (a board journal is history
//! and never decides it), in one read: only a board whose state is
//! `Started` with the bundle's digest carries it, and that receipt is the
//! answer. No identity is read back from the board:
//! reading the flash's checksums needs the ROM's download mode, which
//! resets the image a skip would keep; the receipt is the best knowledge
//! short of that. Any write that bypasses this crate, or any process
//! writing the flash without the board's device lock, voids the receipt.
//!
//! The receipts are one JSON file per board in the XDG data directory
//! (`devices/<MAC>.image.json`), replaced atomically and read strictly: a
//! file that does not parse is an error, never "unknown, so carried". A
//! receipt of another schema carries nothing, and the next write replaces
//! it.

use std::path::{Path, PathBuf};

use crate::Result;

use crate::flash::After;
pub use oer_chip_profile::Start;
pub use oer_device_lock::{DeviceAccess, DeviceId};
use oer_image_bundle::{ImageBundle, Snapshot};
use serde::{Deserialize, Serialize};

/// Format of a receipt file.
pub const SCHEMA: u32 = 2;

/// One segment a write put into flash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentDigest {
    pub offset: u32,
    /// What it is: bootloader, partition table, application, OTA selection.
    pub description: String,
    pub sha256: String,
    pub bytes: u64,
}

/// What a write put into a board: the whole bundle, by digest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// The chip the bundle is for.
    pub chip: String,
    /// The name its writer knows it by: a HIL image class, a catalog image,
    /// an example, a bundle directory.
    pub image: String,
    /// Every segment in write order.
    pub segments: Vec<SegmentDigest>,
    /// The SHA-256 over the segments' offsets and digests: the bundle's
    /// identity on the board.
    pub digest: String,
    /// The command that wrote it.
    pub by: String,
    pub unix_millis: u64,
}

impl Receipt {
    /// The receipt a write of `snapshot` as `image` by `by` publishes: the
    /// digests of exactly the bytes the write puts into flash.
    pub fn of(snapshot: &Snapshot, image: &str, by: &str) -> Self {
        let segments = snapshot
            .segments
            .iter()
            .map(|segment| SegmentDigest {
                offset: segment.offset,
                description: segment.description.to_owned(),
                sha256: segment.sha256.clone(),
                bytes: segment.data.len() as u64,
            })
            .collect::<Vec<_>>();
        Self {
            chip: snapshot.chip.clone(),
            image: image.to_owned(),
            digest: digest_of(&snapshot.chip, &segments),
            segments,
            by: by.to_owned(),
            unix_millis: oer_durable::unix_millis(),
        }
    }

    /// The digest of the segment `description` (`application`, ...).
    pub fn segment(&self, description: &str) -> Option<&SegmentDigest> {
        self.segments
            .iter()
            .find(|segment| segment.description == description)
    }
}

/// The digest of a bundle of `chip` with `segments`: what [`carries`]
/// compares.
fn digest_of(chip: &str, segments: &[SegmentDigest]) -> String {
    let mut text = format!("{chip}\n");
    for segment in segments {
        text.push_str(&format!(
            "{:#x} {} {}\n",
            segment.offset, segment.bytes, segment.sha256
        ));
    }
    oer_durable::sha256_bytes(text.as_bytes())
}

/// What the devices layer knows a board's flash holds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
pub enum State {
    /// A write began and did not finish: the flash holds anything.
    Writing { by: String, unix_millis: u64 },
    /// The bundle is in flash; whether it started is not known.
    Written { receipt: Receipt },
    /// The bundle is in flash and started.
    Started { receipt: Receipt },
}

// No `deny_unknown_fields`: serde does not combine it with `flatten`.
#[derive(Deserialize, Serialize)]
struct File {
    schema: u32,
    device: DeviceId,
    /// The generation of the write that left this state: each write takes
    /// the next one.
    generation: u64,
    #[serde(flatten)]
    state: State,
}

/// Every read-and-replace of a board's state in this process, so that a
/// commit compares and replaces in one step; the board's device lock
/// serializes the processes.
static COMMIT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The receipts of every board, in one directory.
#[derive(Clone, Debug)]
pub struct Store {
    directory: PathBuf,
}

impl Store {
    /// The store in `directory`: a test's own.
    pub fn at(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// The user's store, `devices/` in the XDG data directory.
    pub fn open() -> Result<Self> {
        Ok(Self::at(oer_durable::xdg::path(
            oer_durable::xdg::Base::Data,
            "devices",
        )?))
    }

    fn path(&self, id: &DeviceId) -> PathBuf {
        self.directory.join(format!("{}.image.json", id.compact()))
    }

    /// What board `id`'s flash holds; `None` when nothing was ever written
    /// through this store. Read strictly: a file that does not parse is an
    /// error.
    pub fn state(&self, id: &DeviceId) -> Result<Option<State>> {
        Ok(self.file(id)?.map(|file| file.state))
    }

    fn file(&self, id: &DeviceId) -> Result<Option<File>> {
        let path = self.path(id);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let file: File = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: unreadable image receipt: {error}", path.display()))?;
        if file.schema != SCHEMA || file.device != *id {
            return Err(format!(
                "{}: a receipt of schema {} for board {}, not schema {SCHEMA} for {id}",
                path.display(),
                file.schema,
                file.device
            )
            .into());
        }
        Ok(Some(file))
    }

    /// Whether board `id`'s receipt is a JSON object of a schema other than
    /// [`SCHEMA`].
    fn of_another_schema(&self, id: &DeviceId) -> Result<bool> {
        let bytes = match std::fs::read(self.path(id)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        Ok(serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|value| value.get("schema").and_then(serde_json::Value::as_u64))
            .is_some_and(|schema| schema != u64::from(SCHEMA)))
    }

    /// The receipt of the image board `id` runs: its state is `Started`.
    pub fn started(&self, id: &DeviceId) -> Result<Option<Receipt>> {
        Ok(match self.state(id)? {
            Some(State::Started { receipt }) => Some(receipt),
            _ => None,
        })
    }

    /// Begin a write of the board of `access`: publish [`State::Writing`]
    /// with the board's next generation, which the write commits with.
    fn begin(&self, access: &DeviceAccess, by: &str) -> Result<u64> {
        let _commit = COMMIT
            .lock()
            .map_err(|_| "the receipt commit lock is poisoned")?;
        // A receipt this store cannot read (another schema, a damaged file)
        // was written by no write of this process, which writes only this
        // schema: the write that now invalidates it starts the generations.
        let current = match self.file(access.id()) {
            Ok(file) => file.map_or(0, |file| file.generation),
            Err(error) => {
                eprintln!("devices: replacing an unreadable image receipt: {error}");
                0
            }
        };
        let generation = current
            .checked_add(1)
            .ok_or("the board's write generation overflowed")?;
        self.replace(
            access,
            generation,
            State::Writing {
                by: by.to_owned(),
                unix_millis: oer_durable::unix_millis(),
            },
        )?;
        Ok(generation)
    }

    /// Replace the state of the board of `access` by `state` of the write
    /// `generation`, while it is still `current(state)` of that generation:
    /// a write that a later one overtook commits nothing.
    fn commit(
        &self,
        access: &DeviceAccess,
        generation: u64,
        current: impl FnOnce(&State) -> bool,
        state: State,
    ) -> Result<()> {
        let _commit = COMMIT
            .lock()
            .map_err(|_| "the receipt commit lock is poisoned")?;
        match self.file(access.id())? {
            Some(file) if file.generation == generation && current(&file.state) => {
                self.replace(access, generation, state)
            }
            _ => Err(format!(
                "stale write of board {}: a later write replaced write {generation}",
                access.id()
            )
            .into()),
        }
    }

    fn replace(&self, access: &DeviceAccess, generation: u64, state: State) -> Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        oer_durable::atomic_json(
            &self.path(access.id()),
            &File {
                schema: SCHEMA,
                device: access.id().clone(),
                generation,
                state,
            },
        )?;
        Ok(())
    }
}

/// How a write reaches the board's flash.
#[derive(Clone, Copy, Debug)]
pub enum Transport<'a> {
    /// espflash over the board's USB Serial/JTAG `port`, for the chip
    /// espflash names `espflash_chip`.
    Usb {
        port: &'a Path,
        espflash_chip: &'a str,
    },
    /// OpenOCD's program through the chip `chip`'s debug module, which works
    /// over any running image and resets into the written one.
    Jtag { chip: &'a str },
}

/// A bundle written into a board, its start pending.
#[must_use = "a written image is started and confirmed with `started`"]
#[derive(Debug)]
pub struct Written<'a> {
    store: &'a Store,
    access: &'a DeviceAccess,
    receipt: Receipt,
    generation: u64,
    pending: Start,
    _operation: oer_device_lock::DeviceOperation,
}

impl Written<'_> {
    pub fn receipt(&self) -> &Receipt {
        &self.receipt
    }

    /// The start the image still needs: [`Start::Reset`] when the writer's
    /// own reset started it, [`Start::PowerOn`] when the caller must reset
    /// the board (by power where it has it, else an RTS reset).
    pub fn pending_start(&self) -> Start {
        self.pending
    }

    /// The board's write generation this write took.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Confirm that the image started: publishes [`State::Started`] while
    /// the board's state is still this write's `Written`; a later write's
    /// state makes it a stale write, an error that publishes nothing.
    pub fn started(self) -> Result<Receipt> {
        let receipt = &self.receipt;
        self.store.commit(
            self.access,
            self.generation,
            |state| matches!(state, State::Written { receipt: written } if written == receipt),
            State::Started {
                receipt: self.receipt.clone(),
            },
        )?;
        Ok(self.receipt)
    }
}

/// Write `bundle`, known as `image`, into the board of `access` through
/// `transport` for the command `by`: snapshot, invalidate, write the
/// snapshot, publish the receipt. The one write of a board's flash.
pub fn write<'a>(
    store: &'a Store,
    access: &'a DeviceAccess,
    bundle: &ImageBundle,
    image: &str,
    transport: Transport<'_>,
    by: &str,
) -> Result<Written<'a>> {
    let _operation = access.operation()?;
    // The bytes the receipt names are the bytes written: one read.
    let snapshot = bundle.snapshot()?;
    let receipt = Receipt::of(&snapshot, image, by);
    // Before the flash changes: from here until the receipt, the board
    // carries nothing anyone may rely on.
    let generation = store.begin(access, by)?;
    let pending = match transport {
        Transport::Usb {
            port,
            espflash_chip,
        } => {
            crate::flash::write(port, espflash_chip, &snapshot, After::of(snapshot.start))?;
            snapshot.start
        }
        Transport::Jtag { chip } => {
            // OpenOCD programs files: the snapshot's, in a directory only
            // this write uses.
            let staging = tempfile::tempdir()?;
            let files = snapshot.materialize(staging.path())?;
            crate::openocd::Openocd::locate()?.program(
                chip,
                access.id(),
                &files,
                _operation.lifetime(),
            )?;
            Start::Reset
        }
    };
    store.commit(
        access,
        generation,
        |state| matches!(state, State::Writing { .. }),
        State::Written {
            receipt: receipt.clone(),
        },
    )?;
    Ok(Written {
        store,
        access,
        receipt,
        generation,
        pending,
        _operation,
    })
}

/// The receipt of the board of `access` when it runs `bundle`: its state is
/// `Started` with the digest of the whole bundle, read once. The only basis
/// for skipping a flash.
pub fn carries(
    store: &Store,
    access: &DeviceAccess,
    bundle: &ImageBundle,
) -> Result<Option<Receipt>> {
    let _operation = access.operation()?;
    let wanted = Receipt::of(&bundle.snapshot()?, "", "");
    // A receipt of another schema (an older tool's) claims nothing this
    // store can compare: the board carries no known image, and the next
    // write replaces it. A damaged receipt stays an error.
    if store.of_another_schema(access.id())? {
        return Ok(None);
    }
    Ok(match store.state(access.id())? {
        Some(State::Started { receipt })
            if receipt.chip == wanted.chip && receipt.digest == wanted.digest =>
        {
            Some(receipt)
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests;
