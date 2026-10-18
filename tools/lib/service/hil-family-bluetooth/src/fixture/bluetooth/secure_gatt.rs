//! BlueZ owns SMP and the peer bond; this lease owns only its newly created DUT record.
mod agent;
use super::model::{Adapter, PeerAddress};
use crate::Result;
pub use agent::Prompt;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    net::{RecvFlags, recv},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
const AGENT: &str = "/org/open_radio/hil/agent";
const DEVICE: &str = "org.bluez.Device1";
const ADAPTER: &str = "org.bluez.Adapter1";
const CHARACTERISTIC: &str = "org.bluez.GattCharacteristic1";

fn bluez_owner(bus: &Connection) -> Result<String> {
    Ok(Proxy::new(
        bus,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )?
    .call("GetNameOwner", &("org.bluez",))?)
}

pub struct Owner {
    bus: Connection,
    sender: String,
    adapter: String,
    device: String,
    address: String,
    prompts: async_channel::Receiver<Prompt>,
    pending: Arc<Mutex<Option<async_channel::Sender<bool>>>>,
    registered: bool,
    discovery: bool,
    restored: bool,
}

impl Owner {
    /// Must precede any connection/discovery of this DUT. Existing records are
    /// never adopted, replaced or deleted, including stale records after a crash.
    pub fn acquire(adapter: Adapter, peer: PeerAddress) -> Result<Self> {
        let bus = zbus::blocking::connection::Builder::system()?
            .method_timeout(Duration::from_secs(35))
            .build()?;
        let sender = bluez_owner(&bus)?;
        let adapter = format!("/org/bluez/{adapter}");
        let address = peer.to_string();
        let device = format!("{adapter}/dev_{}", address.replace(':', "_"));
        let (tx, prompts) = async_channel::bounded(1);
        let pending = Arc::new(Mutex::new(None));
        let mut owner = Self {
            bus,
            sender,
            adapter,
            device,
            address,
            prompts,
            pending,
            registered: false,
            discovery: false,
            restored: true,
        };
        if owner.objects()?.keys().any(|p| p.as_str() == owner.device) {
            return Err(
                "DUT already has a BlueZ device record; refusing to touch existing state".into(),
            );
        }
        owner.bus.object_server().at(
            AGENT,
            agent::Agent {
                sender: owner.sender.clone(),
                device: owner.device.clone(),
                prompts: tx,
                pending: owner.pending.clone(),
            },
        )?;
        owner.manager()?.call::<_, _, ()>(
            "RegisterAgent",
            &(OwnedObjectPath::try_from(AGENT)?, "DisplayYesNo"),
        )?;
        owner.registered = true;
        owner.restored = false;
        Ok(owner)
    }
    fn check_owner(&self) -> Result<()> {
        if bluez_owner(&self.bus)? != self.sender {
            return Err("BlueZ restarted; ownership cannot be proved".into());
        }
        Ok(())
    }
    fn proxy<'a>(&'a self, path: &'a str, interface: &'a str) -> Result<Proxy<'a>> {
        Ok(Proxy::new(
            &self.bus,
            self.sender.as_str(),
            path,
            interface,
        )?)
    }
    fn manager(&self) -> Result<Proxy<'_>> {
        self.proxy("/org/bluez", "org.bluez.AgentManager1")
    }
    fn device(&self) -> Result<Proxy<'_>> {
        self.proxy(&self.device, DEVICE)
    }
    fn objects(&self) -> Result<Objects> {
        Ok(self
            .proxy("/", "org.freedesktop.DBus.ObjectManager")?
            .call("GetManagedObjects", &())?)
    }

    pub fn discover(&mut self) -> Result<()> {
        self.check_owner()?;
        let options = HashMap::from([
            ("Transport", Value::from("le")),
            ("Pattern", Value::from(self.address.as_str())),
        ]);
        self.proxy(&self.adapter, ADAPTER)?
            .call::<_, _, ()>("SetDiscoveryFilter", &(options,))?;
        // A timed-out call may have taken effect. Cleanup must attempt to stop
        // our discovery session even when its acknowledgement was lost.
        self.discovery = true;
        self.proxy(&self.adapter, ADAPTER)?
            .call::<_, _, ()>("StartDiscovery", &())?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if self.objects()?.keys().any(|p| p.as_str() == self.device) {
                break;
            }
            if Instant::now() >= deadline {
                return Err("BlueZ did not discover DUT".into());
            }
            oer_process::sleep(Duration::from_millis(100))?;
        }
        self.proxy(&self.adapter, ADAPTER)?
            .call::<_, _, ()>("StopDiscovery", &())?;
        self.discovery = false;
        if self.device()?.get_property::<String>("AddressType")? != "public" {
            return Err("DUT public identity required".into());
        }
        Ok(())
    }

    /// Same bus connection as RegisterAgent ensures BlueZ selects our agent.
    /// The caller must join before dropping this owner; calls have a 35 s bound.
    pub fn pair(&self) -> Result<()> {
        self.check_owner()?;
        self.device()?.call::<_, _, ()>("Pair", &())?;
        if !self.device()?.get_property::<bool>("Paired")? {
            return Err("Pair returned without a bond".into());
        }
        Ok(())
    }
    pub fn prompt(&self) -> Result<Option<Prompt>> {
        match self.prompts.try_recv() {
            Ok(prompt) => Ok(Some(prompt)),
            Err(async_channel::TryRecvError::Empty) => Ok(None),
            Err(_) => Err("BlueZ agent closed".into()),
        }
    }
    pub fn cancel_pairing(&self) -> Result<()> {
        if let Some(sender) = self.pending.lock().unwrap().as_ref() {
            let _ = sender.try_send(false);
        }
        // CancelPairing is only meaningful while Pair is in flight. Already
        // completed/failed Pair is handled by the joined result, not ignored here.
        self.device()?.call::<_, _, ()>("CancelPairing", &())?;
        Ok(())
    }
    pub fn disconnect(&self) -> Result<()> {
        if self.device()?.get_property::<bool>("Connected")? {
            self.device()?.call::<_, _, ()>("Disconnect", &())?;
        }
        Ok(())
    }
    pub fn connect(&self) -> Result<()> {
        self.device()?.call::<_, _, ()>("Connect", &())?;
        Ok(())
    }

    pub fn characteristic(&self) -> Result<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.device()?.get_property::<bool>("ServicesResolved")? {
                break;
            }
            if Instant::now() >= deadline {
                return Err("GATT services did not resolve".into());
            }
            oer_process::sleep(Duration::from_millis(50))?;
        }
        let objects = self.objects()?;
        let mut found = Vec::new();
        for (path, interfaces) in objects {
            if !path.as_str().starts_with(&format!("{}/", self.device))
                || !interfaces.contains_key(CHARACTERISTIC)
            {
                continue;
            }
            let characteristic = self.proxy(path.as_str(), CHARACTERISTIC)?;
            if characteristic.get_property::<String>("UUID")?
                != "0000fff1-0000-1000-8000-00805f9b34fb"
            {
                continue;
            }
            let service: OwnedObjectPath = characteristic.get_property("Service")?;
            if !service.as_str().starts_with(&format!("{}/", self.device))
                || self
                    .proxy(service.as_str(), "org.bluez.GattService1")?
                    .get_property::<String>("UUID")?
                    != "0000fff0-0000-1000-8000-00805f9b34fb"
            {
                return Err("characteristic belongs to another service".into());
            }
            found.push(path.to_string());
        }
        if found.len() != 1 {
            return Err("expected one secure GATT value".into());
        }
        Ok(found.remove(0))
    }
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        Ok(self
            .proxy(path, CHARACTERISTIC)?
            .call("ReadValue", &(HashMap::<&str, Value>::new(),))?)
    }
    pub fn write(&self, path: &str, value: u8) -> Result<()> {
        self.proxy(path, CHARACTERISTIC)?.call::<_, _, ()>(
            "WriteValue",
            &(
                [value].as_slice(),
                HashMap::from([("type", Value::from("request"))]),
            ),
        )?;
        Ok(())
    }
    pub fn notify(&self, path: &str) -> Result<Notification> {
        let (fd, _mtu): (zbus::zvariant::OwnedFd, u16) = self
            .proxy(path, CHARACTERISTIC)?
            .call("AcquireNotify", &(HashMap::<&str, Value>::new(),))?;
        Ok(Notification(fd))
    }

    pub fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        self.check_owner()?;
        if let Some(sender) = self.pending.lock().unwrap().as_ref() {
            let _ = sender.try_send(false);
        }
        if self.discovery {
            self.proxy(&self.adapter, ADAPTER)?
                .call::<_, _, ()>("StopDiscovery", &())?;
            self.discovery = false;
        }
        if self.objects()?.keys().any(|p| p.as_str() == self.device) {
            self.disconnect()?;
            self.proxy(&self.adapter, ADAPTER)?.call::<_, _, ()>(
                "RemoveDevice",
                &(OwnedObjectPath::try_from(self.device.as_str())?,),
            )?;
        }
        if self.objects()?.keys().any(|p| p.as_str() == self.device) {
            return Err("temporary DUT bond/device was not removed".into());
        }
        if self.registered {
            self.manager()?
                .call::<_, _, ()>("UnregisterAgent", &(OwnedObjectPath::try_from(AGENT)?,))?;
            self.registered = false;
        }
        self.restored = true;
        Ok(())
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if !self.restored {
            let _ = oer_process::cleanup(|| self.restore());
        }
    }
}

pub struct Notification(zbus::zvariant::OwnedFd);
impl Notification {
    pub fn receive(&self) -> Result<Vec<u8>> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            oer_process::check_cancelled()?;
            if Instant::now() >= deadline {
                return Err("notification deadline".into());
            }
            let timeout = Timespec::try_from(Duration::from_millis(100))?;
            let mut descriptors = [PollFd::new(&self.0, PollFlags::IN)];
            poll(&mut descriptors, Some(&timeout))?;
            let ready = descriptors[0].revents();
            if ready.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) {
                return Err("notification transport closed".into());
            }
            if ready.contains(PollFlags::IN) {
                let mut bytes = [0; 64];
                // MSG_TRUNC exposes an oversized datagram rather than a prefix.
                let (_, length) = recv(
                    &self.0,
                    &mut bytes[..],
                    RecvFlags::TRUNC | RecvFlags::DONTWAIT,
                )?;
                if length == 0 || length > bytes.len() {
                    return Err("invalid notification size".into());
                }
                return Ok(bytes[..length].to_vec());
            }
        }
    }
}
