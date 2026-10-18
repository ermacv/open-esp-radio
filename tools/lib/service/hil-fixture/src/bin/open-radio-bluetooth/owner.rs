//! Acquire the exclusive Linux user channel and restore power/rfkill on release.

use super::{
    Result,
    hci::{ManagementError, Socket},
    model::{Adapter, Check},
};
use bt_hci::cmd::{
    controller_baseband::Reset,
    info::{ReadBdAddr, ReadLocalVersionInformation},
};
use oer_hil_fixture::linux_socket::HciAddress;
use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _},
    path::PathBuf,
    time::{Duration, Instant},
};

const PEER_RF_LOSS_MILLIS: u16 = 2_500;

pub(super) mod connection_parameters;

struct Rfkill {
    index: u32,
    path: PathBuf,
    identity: PathBuf,
    blocked: bool,
}

impl Rfkill {
    fn read(adapter: Adapter) -> Result<Self> {
        for entry in fs::read_dir("/sys/class/rfkill")? {
            let path = entry?.path();
            if fs::read_to_string(path.join("type"))?.trim() != "bluetooth"
                || fs::read_to_string(path.join("name"))?.trim() != adapter.to_string()
            {
                continue;
            }
            if fs::read_to_string(path.join("hard"))?.trim() != "0" {
                return Err("Bluetooth adapter is hardware blocked".into());
            }
            return Ok(Self {
                index: fs::read_to_string(path.join("index"))?.trim().parse()?,
                blocked: fs::read_to_string(path.join("soft"))?.trim() == "1",
                identity: fs::canonicalize(&path)?,
                path,
            });
        }
        Err(format!("no rfkill identity for {adapter}").into())
    }
    fn set(&self, blocked: bool) -> Result<()> {
        if fs::canonicalize(&self.path)? != self.identity
            || fs::read_to_string(self.path.join("index"))?
                .trim()
                .parse::<u32>()?
                != self.index
        {
            return Err("rfkill identity changed".into());
        }
        let mut event = [0; 8];
        event[..4].copy_from_slice(&self.index.to_ne_bytes());
        event[4..].copy_from_slice(&[2, 2, u8::from(blocked), 0]);
        OpenOptions::new()
            .write(true)
            .open("/dev/rfkill")?
            .write_all(&event)?;
        let actual = fs::read_to_string(self.path.join("soft"))?;
        if (actual.trim() == "1") != blocked {
            return Err("rfkill state did not change".into());
        }
        Ok(())
    }
}

struct Owner {
    adapter: Adapter,
    management: Socket,
    user: Option<Socket>,
    rfkill: Rfkill,
    powered: bool,
    identity: Vec<u8>,
    restore_needed: bool,
    _lock: oer_process::lock::FileLock,
}

impl Owner {
    fn snapshot(adapter: Adapter) -> Result<Self> {
        Self::snapshot_for_parameters(adapter, false)
    }

    fn snapshot_for_parameters(adapter: Adapter, recovery: bool) -> Result<Self> {
        if !rustix::process::geteuid().is_root() {
            return Err("DTM requires the installed privileged Bluetooth helper".into());
        }
        match fs::DirBuilder::new()
            .mode(0o755)
            .create("/run/open-radio-bluetooth")
        {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = fs::symlink_metadata("/run/open-radio-bluetooth")?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err("Bluetooth lock directory must be owned and writable only by root".into());
        }
        // The lock directory is created by this helper, never under a
        // caller-writable directory. O_NOFOLLOW rejects a substituted leaf.
        let path = format!("/run/open-radio-bluetooth/{adapter}.lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        let lock = oer_process::lock::FileLock::try_lock(
            lock,
            std::path::Path::new(&path),
            oer_process::lock::Mode::Exclusive,
        )?
        .ok_or_else(|| format!("another helper owns adapter {adapter}"))?;
        if !recovery && connection_parameters::journal_path(adapter).try_exists()? {
            return Err("Bluetooth connection parameters require explicit helper recovery".into());
        }
        let management = Socket::open(HciAddress::new(u16::MAX, HciAddress::CONTROL_CHANNEL))?;
        let info = management.management(adapter.0, 4, &[])?;
        let powered = powered(&info)?;
        if powered {
            let connections = management.management(adapter.0, 0x15, &[])?;
            if connections != [0, 0] {
                return Err(
                    "Bluetooth adapter has active connections; use a dedicated fixture adapter"
                        .into(),
                );
            }
        }
        let rfkill = Rfkill::read(adapter)?;
        Ok(Self {
            adapter,
            management,
            user: None,
            rfkill,
            powered,
            identity: info[..6].to_vec(),
            restore_needed: false,
            _lock: lock,
        })
    }

    fn acquire(&mut self) -> Result<()> {
        self.restore_needed = true;
        self.rfkill.set(false)?;
        self.manage_retry(5, &[0])?;
        self.user = Some(Socket::open(HciAddress::new(
            self.adapter.0,
            HciAddress::USER_CHANNEL,
        ))?);
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        if !self.restore_needed {
            return Ok(());
        }
        // Closing the user channel re-registers the controller with the kernel.
        // Reset is attempted first even when a DTM start timed out.
        let mut errors = Vec::new();
        if let Some(user) = self.user.take() {
            if let Err(error) = user.command(Reset::new()) {
                errors.push(format!("controller reset: {error}"));
            }
            drop(user);
        }
        if let Err(error) = self.rfkill.set(false) {
            errors.push(format!("temporary rfkill release: {error}"));
        }
        let info = self.manage_retry(4, &[]);
        let mut same_adapter = true;
        match info {
            Ok(info) if info.get(..6) == Some(&self.identity[..]) => {
                if let Err(error) = self.manage_retry(5, &[u8::from(self.powered)]) {
                    errors.push(format!("power restore: {error}"));
                }
            }
            Ok(_) => {
                same_adapter = false;
                errors.push("adapter identity changed during restoration".into());
            }
            Err(error) => errors.push(format!("adapter re-registration: {error}")),
        }
        // A management failure must not skip rfkill recovery. The original
        // sysfs identity is checked independently before writing its index.
        if same_adapter {
            if let Err(error) = self.rfkill.set(self.rfkill.blocked) {
                errors.push(format!("rfkill restore: {error}"));
            }
            match self
                .management
                .management(self.adapter.0, 4, &[])
                .and_then(|info| powered(&info))
            {
                Ok(actual) if actual == self.powered => {}
                Ok(_) => errors.push("restored power differs from snapshot".into()),
                Err(error) => errors.push(format!("power verification: {error}")),
            }
        }
        self.restore_needed = false;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; ").into())
        }
    }

    fn manage_retry(&self, opcode: u16, payload: &[u8]) -> Result<Vec<u8>> {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match self.management.management(self.adapter.0, opcode, payload) {
                Err(error)
                    if Instant::now() < deadline
                        && error
                            .downcast_ref::<ManagementError>()
                            .is_some_and(|error| matches!(error.status, 0x0a | 0x11)) =>
                {
                    oer_process::sleep(Duration::from_millis(50))?;
                }
                result => return result,
            }
        }
    }

    fn rfkill_for_loss(
        &mut self,
        hold_ms: u16,
        connected: Instant,
        report: &mut super::model::ConnectionReset,
    ) -> Result<()> {
        oer_process::sleep(Duration::from_millis(u64::from(hold_ms)))?;
        let user = self.user.take().ok_or("missing exclusive HCI channel")?;
        drop(user);
        self.rfkill.set(true)?;
        report.termination_after_connection_micros = Some(connected.elapsed().as_micros() as u64);
        report.peer_rfkill_blocked = true;
        let blocked = Instant::now();
        oer_process::sleep(Duration::from_millis(u64::from(PEER_RF_LOSS_MILLIS)))?;
        if fs::read_to_string(self.rfkill.path.join("soft"))?.trim() != "1" {
            return Err("Bluetooth peer rfkill cleared during the RF-loss hold".into());
        }
        report.peer_rfkill_micros =
            Some(blocked.elapsed().as_micros().try_into().unwrap_or(u64::MAX));
        Ok(())
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        if self.restore_needed
            && let Err(error) = oer_process::cleanup(|| self.restore())
        {
            eprintln!("Bluetooth restoration failed: {error}");
        }
    }
}

fn powered(info: &[u8]) -> Result<bool> {
    if info.len() != 280 {
        return Err("invalid Read Controller Information response".into());
    }
    Ok(u32::from_le_bytes(info[13..17].try_into()?) & 1 != 0)
}

// Keep completion and restoration evidence separate: successful RX/TX cannot
// hide cleanup failure, and a preparation error must still run restoration.
fn checked_lifetime<T>(
    owner: &mut T,
    report: &mut Check,
    run: impl FnOnce(&mut T, &mut Check) -> Result<()>,
    restore: impl FnOnce(&mut T) -> Result<()>,
) {
    if let Err(error) = run(owner, report) {
        report.errors.push(error.to_string());
    }
    match oer_process::cleanup(|| restore(owner)) {
        Ok(()) => report.restored = true,
        Err(error) => report.errors.push(format!("restore: {error}")),
    }
}

pub(super) fn check(adapter: Adapter, report: &mut Check) -> Result<()> {
    let mut owner = Owner::snapshot(adapter)?;
    report.initial_powered = Some(owner.powered);
    report.initial_soft_blocked = Some(owner.rfkill.blocked);
    checked_lifetime(
        &mut owner,
        report,
        |owner, report| {
            owner.acquire()?;
            let user = owner.user.as_ref().ok_or("missing exclusive HCI channel")?;
            user.command(Reset::new())?;
            report.address = Some(format!("{:?}", user.command(ReadBdAddr::new())?));
            report.version = Some(format!(
                "{:?}",
                user.command(ReadLocalVersionInformation::new())?
            ));
            super::dtm::check(user, report)
        },
        Owner::restore,
    );
    Ok(())
}

pub(super) fn connect_reset(
    adapter: Adapter,
    peer: super::model::PeerAddress,
    hold_ms: u16,
    termination: oer_hil_protocol::bluetooth::BluetoothPeripheralTermination,
    report: &mut super::model::ConnectionReset,
) -> Result<()> {
    if hold_ms > 5_000 {
        return Err("connection hold must be at most 5000 ms".into());
    }
    let mut owner = Owner::snapshot(adapter)?;
    report.initial_powered = Some(owner.powered);
    report.initial_soft_blocked = Some(owner.rfkill.blocked);
    let result = (|| {
        owner.acquire()?;
        let user = owner.user.as_ref().ok_or("missing exclusive HCI channel")?;
        user.command(Reset::new())?;
        let outcome = super::connection_reset::run(user, peer, hold_ms, termination, report)?;
        match outcome {
            super::connection_reset::ConnectionRunOutcome::Complete => Ok(()),
            super::connection_reset::ConnectionRunOutcome::PeerRfkill { connected } => {
                owner.rfkill_for_loss(hold_ms, connected, report)
            }
        }
    })();
    if let Err(error) = result {
        report.errors.push(error.to_string());
    }
    match oer_process::cleanup(|| owner.restore()) {
        Ok(()) => report.restored = true,
        Err(error) => report.errors.push(format!("restore: {error}")),
    }
    Ok(())
}

pub(super) fn security_failure(
    adapter: Adapter,
    peer: super::model::PeerAddress,
    report: &mut super::model::security_failure::Report,
) -> Result<()> {
    let mut owner = Owner::snapshot(adapter)?;
    report.initial_powered = Some(owner.powered);
    report.initial_soft_blocked = Some(owner.rfkill.blocked);
    let result = (|| {
        owner.acquire()?;
        let user = owner.user.as_ref().ok_or("missing exclusive HCI channel")?;
        user.command(Reset::new())?;
        super::security_failure::run(user, peer, report)
    })();
    if let Err(error) = result {
        report.errors.push(error.to_string());
    }
    match oer_process::cleanup(|| owner.restore()) {
        Ok(()) => report.restored = true,
        Err(error) => report.errors.push(format!("restore: {error}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_acquisition_failure_restores_the_mutated_owner() {
        let mut unblocked = false;
        let mut report = Check::new(
            Adapter(0),
            super::super::model::DtmVersion::V2,
            super::super::model::DtmProfile::ReceiveTransmit,
        );
        checked_lifetime(
            &mut unblocked,
            &mut report,
            |state, _| {
                *state = true;
                Err("exclusive channel busy".into())
            },
            |state| {
                assert!(*state);
                *state = false;
                Ok(())
            },
        );
        assert!(!unblocked);
        assert!(report.restored);
        assert_eq!(report.errors, ["exclusive channel busy"]);
        assert!(!report.passed(
            Adapter(0),
            super::super::model::DtmVersion::V2,
            super::super::model::DtmProfile::ReceiveTransmit,
        ));
    }
    #[test]
    fn restoration_failure_is_retained_alongside_original_error() {
        let mut report = Check::new(
            Adapter(0),
            super::super::model::DtmVersion::V2,
            super::super::model::DtmProfile::ReceiveTransmit,
        );
        checked_lifetime(
            &mut (),
            &mut report,
            |_, _| Err("RX rejected".into()),
            |_| Err("power restore failed".into()),
        );
        assert!(!report.restored);
        assert_eq!(
            report.errors,
            ["RX rejected", "restore: power restore failed"]
        );
    }
}
