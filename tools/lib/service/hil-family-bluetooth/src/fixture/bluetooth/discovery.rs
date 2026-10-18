//! BlueZ LE discovery on the powered adapter of an [`super::att::Owner`].
//!
//! BlueZ discovers LE devices by active scanning: it sends `SCAN_REQ` to
//! every scannable advertiser and merges the `SCAN_RSP` into the device's
//! properties. A discovery session belongs to the D-Bus client that started
//! it and ends when that client disconnects, so the fixture keeps one system
//! bus connection for its whole lifetime. It needs no privilege beyond the
//! system bus.
use super::model::{Adapter, PeerAddress};
use crate::Result;
use std::{collections::HashMap, time::Duration};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, Value},
};

const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";

/// One LE discovery session; stopping it is part of the owner's cleanup.
pub struct Discovery {
    bus: Connection,
    adapter: String,
    stopped: bool,
}

impl Discovery {
    /// Start LE-only discovery that reports every advertisement.
    pub fn start(adapter: Adapter) -> Result<Self> {
        let bus = zbus::blocking::connection::Builder::system()?
            .method_timeout(Duration::from_secs(5))
            .build()?;
        let discovery = Self {
            bus,
            adapter: format!("/org/bluez/{adapter}"),
            stopped: false,
        };
        let filter = HashMap::from([
            ("Transport", Value::from("le")),
            ("DuplicateData", Value::from(true)),
        ]);
        {
            let proxy = discovery.adapter()?;
            proxy.call::<_, _, ()>("SetDiscoveryFilter", &(filter,))?;
            proxy.call::<_, _, ()>("StartDiscovery", &())?;
        }
        Ok(discovery)
    }

    fn adapter(&self) -> Result<Proxy<'_>> {
        Ok(Proxy::new(
            &self.bus,
            "org.bluez",
            self.adapter.as_str(),
            ADAPTER,
        )?)
    }

    fn device_path(&self, peer: PeerAddress) -> String {
        format!(
            "{}/dev_{}",
            self.adapter,
            peer.to_string().replace(':', "_")
        )
    }

    fn device(&self, peer: PeerAddress) -> Result<Proxy<'static>> {
        Ok(Proxy::new(
            &self.bus,
            "org.bluez",
            self.device_path(peer),
            DEVICE,
        )?)
    }

    /// Whether BlueZ discovered `peer` at all.
    pub fn seen(&self, peer: PeerAddress) -> bool {
        self.device(peer)
            .and_then(|device| Ok(device.get_property::<String>("Address")?))
            .is_ok()
    }

    /// The name BlueZ holds for `peer`, if it discovered the device and
    /// learned one.
    pub fn name(&self, peer: PeerAddress) -> Result<Option<String>> {
        Ok(self
            .device(peer)
            .and_then(|device| Ok(device.get_property::<String>("Name")?))
            .ok())
    }

    /// Forget `peer`, so that a later run cannot see a cached name.
    pub fn forget(&self, peer: PeerAddress) -> Result<()> {
        if !self.seen(peer) {
            return Ok(());
        }
        let path = OwnedObjectPath::try_from(self.device_path(peer))?;
        self.adapter()?.call::<_, _, ()>("RemoveDevice", &(path,))?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        self.adapter()?.call::<_, _, ()>("StopDiscovery", &())?;
        Ok(())
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = oer_process::cleanup(|| self.stop());
    }
}
