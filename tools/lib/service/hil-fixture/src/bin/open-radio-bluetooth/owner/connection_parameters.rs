//! Kernel ATT defaults, not an HCI user-channel connection. MGMT 0x004b/0x004c:
//! <https://github.com/bluez/bluez/blob/master/doc/mgmt-protocol.rst>
//!
//! The root-owned journal precedes every mutation. EOF, cancellation and expiry
//! restore the snapshot; SIGKILL leaves the journal and blocks further use until
//! explicit recovery. No bond database or other adapter is modified.

use super::{Adapter, File, OpenOptions, Owner, PathBuf, Result, fs, powered};
use std::{
    io::{Read as _, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    time::{Duration, Instant},
};

const TYPES: [u16; 4] = [0x17, 0x18, 0x19, 0x1a];
// Intervals in 1.25 ms, latency in events, supervision in 10 ms.
const SELECTED: [u16; 4] = [6, 6, 0, 200];

pub(super) fn journal_path(adapter: Adapter) -> PathBuf {
    PathBuf::from(format!(
        "/run/open-radio-bluetooth/{adapter}.att-parameters.json"
    ))
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    schema: u8,
    adapter: u16,
    address: Vec<u8>,
    rfkill_identity: PathBuf,
    rfkill_index: u32,
    blocked: bool,
    original: [u16; 4],
}

fn parameters(mut bytes: &[u8]) -> Result<[u16; 4]> {
    let mut values = [None; 4];
    let mut seen = std::collections::BTreeSet::new();
    while !bytes.is_empty() {
        if bytes.len() < 3 {
            return Err("truncated MGMT parameter header".into());
        }
        let kind = u16::from_le_bytes([bytes[0], bytes[1]]);
        let length = usize::from(bytes[2]);
        if !seen.insert(kind) {
            return Err("duplicate MGMT parameter".into());
        }
        let value = bytes
            .get(3..3 + length)
            .ok_or("truncated MGMT parameter value")?;
        if let Some(index) = TYPES.iter().position(|candidate| *candidate == kind) {
            values[index] = Some(u16::from_le_bytes(
                value
                    .try_into()
                    .map_err(|_| "invalid connection parameter width")?,
            ));
        }
        bytes = &bytes[3 + length..];
    }
    let [Some(min), Some(max), Some(latency), Some(timeout)] = values else {
        return Err("kernel does not expose every required ATT connection default".into());
    };
    Ok([min, max, latency, timeout])
}

fn encode(values: [u16; 4]) -> Vec<u8> {
    TYPES
        .into_iter()
        .zip(values)
        .flat_map(|(kind, value)| {
            let [a, b] = kind.to_le_bytes();
            let [c, d] = value.to_le_bytes();
            [a, b, 2, c, d]
        })
        .collect()
}

fn read(owner: &Owner) -> Result<[u16; 4]> {
    parameters(&owner.management.management(owner.adapter.0, 0x4b, &[])?)
}

fn set(owner: &Owner, values: [u16; 4]) -> Result<()> {
    owner
        .management
        .management(owner.adapter.0, 0x4c, &encode(values))?;
    if read(owner)? != values {
        return Err("connection defaults readback mismatch".into());
    }
    Ok(())
}

fn restore(owner: &mut Owner, snapshot: &Snapshot) -> Result<()> {
    let info = owner.manage_retry(4, &[])?;
    if snapshot.schema != 1
        || snapshot.adapter != owner.adapter.0
        || info.get(..6) != Some(snapshot.address.as_slice())
        || owner.rfkill.identity != snapshot.rfkill_identity
        || owner.rfkill.index != snapshot.rfkill_index
    {
        return Err("ATT recovery identity mismatch; journal retained".into());
    }
    // This lease admitted only an initially powered-off, dedicated adapter.
    // Parent death may leave it on; stop that adapter before restoring defaults.
    if powered(&info)? {
        owner.manage_retry(5, &[0])?;
    }
    let parameters = set(owner, snapshot.original);
    let rfkill = owner.rfkill.set(snapshot.blocked);
    let errors: Vec<_> = [parameters, rfkill]
        .into_iter()
        .filter_map(|result| result.err().map(|error| error.to_string()))
        .collect();
    if !errors.is_empty() {
        return Err(errors.join("; ").into());
    }
    if powered(&owner.manage_retry(4, &[])?)? {
        return Err("ATT recovery power readback mismatch".into());
    }
    fs::remove_file(journal_path(owner.adapter))?;
    File::open("/run/open-radio-bluetooth")?.sync_all()?;
    Ok(())
}

// The caller has already durably journaled the original state. Restoration is
// unconditional, including an unsuccessful Set/readback or a broken ready pipe.
fn guarded<T>(
    owner: &mut T,
    operation: impl FnOnce(&mut T) -> Result<()>,
    cleanup: impl FnOnce(&mut T) -> Result<()>,
) -> Result<()> {
    let operation = operation(owner);
    let cleanup = oer_process::cleanup(|| cleanup(owner));
    match (operation, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(format!("{error}; cleanup: {cleanup}; journal retained").into())
        }
    }
}

fn hold() -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        oer_process::check_cancelled()?;
        if Instant::now() >= deadline {
            return Err("ATT parameter lease expired".into());
        }
        let stdin = std::io::stdin();
        let ready = match rustix::event::poll(
            &mut [rustix::event::PollFd::new(
                &stdin,
                rustix::event::PollFlags::IN,
            )],
            Some(&rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            }),
        ) {
            Ok(ready) => ready,
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        };
        if ready == 0 {
            continue;
        }
        let mut byte = [0];
        if std::io::stdin().read(&mut byte)? == 0 {
            return Ok(());
        }
        return Err("ATT lease accepts EOF only, not caller-supplied parameters".into());
    }
}

pub(crate) fn run(adapter: Adapter, recovery: bool) -> Result<()> {
    let mut owner = Owner::snapshot_for_parameters(adapter, recovery)?;
    let path = journal_path(adapter);
    if recovery {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let snapshot: Snapshot = serde_json::from_reader(file.take(4096))?;
        return oer_process::cleanup(|| restore(&mut owner, &snapshot));
    }
    if owner.powered {
        return Err("ATT parameters require an initially powered-off adapter".into());
    }
    let snapshot = Snapshot {
        schema: 1,
        adapter: adapter.0,
        address: owner.identity.clone(),
        rfkill_identity: owner.rfkill.identity.clone(),
        rfkill_index: owner.rfkill.index,
        blocked: owner.rfkill.blocked,
        original: read(&owner)?,
    };
    // create_new also rejects an existing/symlink leaf. An incomplete write is
    // a quarantine marker, never permission to take another snapshot over it.
    let mut journal = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    serde_json::to_writer(&mut journal, &snapshot)?;
    journal.sync_all()?;
    File::open("/run/open-radio-bluetooth")?.sync_all()?;
    guarded(
        &mut owner,
        |owner| -> Result<()> {
            set(owner, SELECTED)?;
            eprintln!(
                "ATT defaults: original={:?}, selected={SELECTED:?}",
                snapshot.original
            );
            writeln!(std::io::stdout(), "ATT_PARAMETERS_READY_V1")?;
            std::io::stdout().flush()?;
            hold()
        },
        |owner| restore(owner, &snapshot),
    )?;
    writeln!(std::io::stdout(), "ATT_PARAMETERS_RESTORED_V1")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_failure_cancellation_and_cleanup_failure_cannot_skip_restoration() {
        for operation_error in [
            None,
            Some("Set/readback failed"),
            Some("cancelled"),
            Some("EOF pipe failed"),
        ] {
            for cleanup_error in [None, Some("restore readback failed")] {
                let mut calls = Vec::new();
                let result = guarded(
                    &mut calls,
                    |calls| {
                        calls.push("operation");
                        operation_error.map_or(Ok(()), |error| Err(error.into()))
                    },
                    |calls| {
                        calls.push("restore");
                        cleanup_error.map_or(Ok(()), |error| Err(error.into()))
                    },
                );
                assert_eq!(calls, ["operation", "restore"]);
                assert_eq!(
                    result.is_ok(),
                    operation_error.is_none() && cleanup_error.is_none()
                );
                if let Err(error) = result {
                    for expected in [operation_error, cleanup_error].into_iter().flatten() {
                        assert!(error.to_string().contains(expected));
                    }
                }
            }
        }
    }
    #[test]
    fn complete_roundtrip_and_unrelated_defaults() {
        let original = [24, 40, 0, 500];
        let mut bytes = vec![0x80, 0, 3, 1, 2, 3];
        bytes.extend(encode(original));
        assert_eq!(parameters(&bytes).unwrap(), original);
        assert_eq!(parameters(&encode(SELECTED)).unwrap(), SELECTED);
    }
    #[test]
    fn missing_duplicate_truncated_and_wrong_width_fail_closed() {
        let bytes = encode(SELECTED);
        for length in 0..bytes.len() {
            assert!(parameters(&bytes[..length]).is_err());
        }
        let mut duplicate = bytes.clone();
        duplicate.extend(&bytes[..5]);
        assert!(parameters(&duplicate).is_err());
        let mut width = bytes;
        width[2] = 1;
        assert!(parameters(&width).is_err());
    }
}
