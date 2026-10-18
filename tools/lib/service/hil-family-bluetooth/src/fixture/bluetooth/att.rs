//! Kernel/BlueZ fixed ATT fixture. The runner holds the ordinary adapter lease.
//! Requires an initially powered-off adapter; restores its power and rfkill state.
use super::model::{Adapter, PeerAddress};
use crate::Result;
use oer_hil_fixture::linux_socket::{L2capAddress, set_bluetooth_security_low};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::{
        AddressFamily, RecvFlags, SendFlags, SocketFlags, SocketType, bind, connect, recv, send,
        socket_with, sockopt::socket_error,
    },
};
use std::{
    fs,
    io::Write,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn bus(adapter: Adapter, args: &[&str]) -> Result<String> {
    let mut command = oer_process::command("busctl");
    command
        .args([
            "--system",
            "--timeout=5",
            args[0],
            "org.bluez",
            &format!("/org/bluez/{adapter}"),
            "org.bluez.Adapter1",
        ])
        .args(&args[1..]);
    let output = oer_process::output(&mut command, Some(Duration::from_secs(6)))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
fn power(adapter: Adapter, powered: bool) -> Result<()> {
    // BlueZ can still be re-registering the controller just after rfkill clears.
    // Retry the idempotent request, retaining a finite setup/cleanup deadline.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = bus(
            adapter,
            &[
                "set-property",
                "Powered",
                "b",
                if powered { "true" } else { "false" },
            ],
        );
        match result {
            Ok(_) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => oer_process::sleep(Duration::from_millis(100))?,
        }
    }
}
pub fn preflight(adapter: Adapter) -> Result<()> {
    super::att_parameters::require_clean(adapter)?;
    if bus(adapter, &["get-property", "Powered"])? != "b false" {
        return Err("ATT calibration fixture requires an initially powered-off adapter".into());
    }
    Ok(())
}
pub struct Owner {
    adapter: Adapter,
    address: PeerAddress,
    rfkill: PathBuf,
    identity: PathBuf,
    index: u32,
    blocked: bool,
    restored: bool,
    parameters: Option<super::att_parameters::Lease>,
}
impl Owner {
    pub fn acquire(adapter: Adapter, output: &Path) -> Result<Self> {
        Self::acquire_profile(adapter, output, false)
    }
    pub fn acquire_calibration(adapter: Adapter, output: &Path) -> Result<Self> {
        Self::acquire_profile(adapter, output, true)
    }
    fn acquire_profile(adapter: Adapter, output: &Path, calibration: bool) -> Result<Self> {
        preflight(adapter)?;
        let address = bus(adapter, &["get-property", "Address"])?;
        let address: PeerAddress = address
            .strip_prefix("s \"")
            .and_then(|s| s.strip_suffix('"'))
            .ok_or("invalid BlueZ address")?
            .parse()?;
        let rfkill = fs::read_dir("/sys/class/rfkill")?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                fs::read_to_string(p.join("name")).is_ok_and(|s| s.trim() == adapter.to_string())
            })
            .ok_or("missing Bluetooth rfkill")?;
        if fs::read_to_string(rfkill.join("hard"))?.trim() != "0" {
            return Err("Bluetooth adapter hardware blocked".into());
        }
        let mut owner = Self {
            adapter,
            address,
            identity: fs::canonicalize(&rfkill)?,
            index: fs::read_to_string(rfkill.join("index"))?.trim().parse()?,
            blocked: fs::read_to_string(rfkill.join("soft"))?.trim() == "1",
            rfkill,
            restored: false,
            parameters: None,
        };
        oer_durable::atomic_json(
            &output.join("adapter-before.json"),
            &serde_json::json!({"adapter": adapter.to_string(), "address": address.to_string(), "powered": false, "soft_blocked": owner.blocked}),
        )?;
        if calibration {
            owner.parameters = Some(super::att_parameters::Lease::acquire(adapter, output)?);
        }
        owner.block(false)?;
        power(adapter, true)?;
        if bus(adapter, &["get-property", "Powered"])? != "b true" {
            return Err("BlueZ did not power adapter".into());
        }
        Ok(owner)
    }
    fn block(&self, blocked: bool) -> Result<()> {
        if fs::canonicalize(&self.rfkill)? != self.identity {
            return Err("rfkill identity changed".into());
        }
        let mut event = [0u8; 8];
        event[..4].copy_from_slice(&self.index.to_ne_bytes());
        event[4..].copy_from_slice(&[2, 2, u8::from(blocked), 0]);
        fs::OpenOptions::new()
            .write(true)
            .open("/dev/rfkill")?
            .write_all(&event)?;
        if (fs::read_to_string(self.rfkill.join("soft"))?.trim() == "1") != blocked {
            return Err("rfkill restore mismatch".into());
        }
        Ok(())
    }
    pub fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        let power = power(self.adapter, false);
        let block = self.block(self.blocked);
        // Even a failed power/rfkill request must release the helper. It checks
        // adapter identity and independently restores the original snapshot.
        let parameters = self
            .parameters
            .as_mut()
            .map_or(Ok(()), |lease| lease.restore());
        let errors: Vec<_> = [power, block, parameters]
            .into_iter()
            .filter_map(|result| result.err().map(|error| error.to_string()))
            .collect();
        if !errors.is_empty() {
            return Err(errors.join("; ").into());
        }
        if bus(self.adapter, &["get-property", "Powered"])? != "b false" {
            return Err("adapter power restore mismatch".into());
        }
        self.restored = true;
        Ok(())
    }
    /// The adapter's public address.
    pub fn address(&self) -> PeerAddress {
        self.address
    }
    pub fn connect(&self, peer: PeerAddress) -> Result<Att> {
        Att::connect(self.address, peer)
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if !self.restored {
            let _ = oer_process::cleanup(|| self.restore());
        }
    }
}

pub struct Att(OwnedFd);
impl Att {
    fn connect(local: PeerAddress, peer: PeerAddress) -> Result<Self> {
        let socket = Self(socket_with(
            AddressFamily::BLUETOOTH,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
            None,
        )?);
        set_bluetooth_security_low(&socket.0)?;
        bind(&socket.0, &L2capAddress::att(local.0))?;
        match connect(&socket.0, &L2capAddress::att(peer.0)) {
            Ok(()) | Err(Errno::INPROGRESS) => {}
            Err(error) => return Err(error.into()),
        }
        socket.wait(PollFlags::OUT, Instant::now() + Duration::from_secs(10))?;
        socket_error(&socket.0)??;
        Ok(socket)
    }
    fn wait(&self, events: PollFlags, deadline: Instant) -> Result<()> {
        loop {
            if oer_process::cancellation_requested() {
                return Err("ATT fixture cancelled".into());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("ATT fixture deadline exceeded".into());
            }
            let timeout = Timespec::try_from(left.min(Duration::from_millis(100)))?;
            let mut descriptors = [PollFd::new(&self.0, events)];
            match poll(&mut descriptors, Some(&timeout)) {
                Ok(0) | Err(Errno::INTR) => continue,
                Ok(_) => {}
                Err(error) => return Err(error.into()),
            }
            let ready = descriptors[0].revents();
            if ready.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) {
                return Err("ATT socket disconnected".into());
            }
            if ready.intersects(events) {
                return Ok(());
            }
        }
    }
    pub fn send(&self, bytes: &[u8]) -> Result<()> {
        self.wait(PollFlags::OUT, Instant::now() + Duration::from_secs(2))?;
        if send(&self.0, bytes, SendFlags::NOSIGNAL)? != bytes.len() {
            return Err("incomplete ATT send".into());
        }
        Ok(())
    }
    pub fn receive(&self) -> Result<Vec<u8>> {
        self.wait(PollFlags::IN, Instant::now() + Duration::from_secs(2))?;
        let mut bytes = [0u8; 256];
        let (_, length) = recv(&self.0, &mut bytes[..], RecvFlags::TRUNC)?;
        if length == 0 || length > bytes.len() {
            return Err("invalid ATT packet size".into());
        }
        Ok(bytes[..length].to_vec())
    }
}
