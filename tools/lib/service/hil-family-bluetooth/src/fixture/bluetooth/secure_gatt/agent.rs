//! Application-specific BlueZ agent. No default-agent replacement or auto-accept.
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::{message::Header, zvariant::OwnedObjectPath};
#[cfg(test)]
mod tests;

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
pub(super) enum Error {
    Rejected(String),
    Canceled(String),
}

pub struct Prompt {
    pub number: u32,
    reply: async_channel::Sender<bool>,
}
impl Prompt {
    pub fn answer(self, accept: bool) -> crate::Result<()> {
        self.reply
            .try_send(accept)
            .map_err(|_| "expired BlueZ confirmation".into())
    }
}
impl Drop for Prompt {
    fn drop(&mut self) {
        // A missing UI consumer is never acceptance; a queued answer is not
        // overwritten because this single-response channel has capacity one.
        let _ = self.reply.try_send(false);
    }
}

pub(super) struct Agent {
    pub(super) sender: String,
    pub(super) device: String,
    pub(super) prompts: async_channel::Sender<Prompt>,
    pub(super) pending: Arc<Mutex<Option<async_channel::Sender<bool>>>>,
}
impl Agent {
    fn check(&self, header: &Header<'_>, device: &OwnedObjectPath) -> Result<(), Error> {
        if header.sender().map(|s| s.as_str()) != Some(self.sender.as_str())
            || device.as_str() != self.device
        {
            return Err(Error::Rejected("foreign sender or device".into()));
        }
        Ok(())
    }
    fn cancel_pending(&self, header: &Header<'_>) -> Result<(), Error> {
        if header.sender().map(|s| s.as_str()) != Some(self.sender.as_str()) {
            return Err(Error::Rejected("foreign sender".into()));
        }
        if let Some(sender) = self.pending.lock().unwrap().as_ref() {
            let _ = sender.try_send(false);
        }
        Ok(())
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    async fn request_confirmation(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), Error> {
        self.check(&header, &device)?;
        if passkey > 999_999 {
            return Err(Error::Rejected("invalid number".into()));
        }
        let (reply, receive) = async_channel::bounded(1);
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.is_some() {
                return Err(Error::Rejected("overlapping confirmation".into()));
            }
            *pending = Some(reply.clone());
        }
        if self
            .prompts
            .try_send(Prompt {
                number: passkey,
                reply,
            })
            .is_err()
        {
            self.pending.lock().unwrap().take();
            return Err(Error::Rejected("confirmation consumer unavailable".into()));
        }
        let accepted =
            futures_lite::future::race(async { receive.recv().await.unwrap_or(false) }, async {
                async_io::Timer::after(Duration::from_secs(25)).await;
                false
            })
            .await;
        self.pending.lock().unwrap().take();
        if accepted {
            Ok(())
        } else {
            Err(Error::Canceled("comparison declined or expired".into()))
        }
    }
    fn cancel(&self, #[zbus(header)] header: Header<'_>) -> Result<(), Error> {
        self.cancel_pending(&header)
    }
    fn release(&self, #[zbus(header)] header: Header<'_>) -> Result<(), Error> {
        self.cancel_pending(&header)
    }
    fn request_authorization(&self, _device: OwnedObjectPath) -> Result<(), Error> {
        Err(Error::Rejected("Just Works forbidden".into()))
    }
    fn request_pin_code(&self, _device: OwnedObjectPath) -> Result<String, Error> {
        Err(Error::Rejected("legacy pairing forbidden".into()))
    }
    fn request_passkey(&self, _device: OwnedObjectPath) -> Result<u32, Error> {
        Err(Error::Rejected("passkey entry forbidden".into()))
    }
    fn display_passkey(
        &self,
        _device: OwnedObjectPath,
        _passkey: u32,
        _entered: u16,
    ) -> Result<(), Error> {
        Err(Error::Rejected("passkey display forbidden".into()))
    }
    fn display_pin_code(&self, _device: OwnedObjectPath, _pincode: String) -> Result<(), Error> {
        Err(Error::Rejected("legacy pairing forbidden".into()))
    }
    fn authorize_service(&self, _device: OwnedObjectPath, _uuid: String) -> Result<(), Error> {
        Err(Error::Rejected("incoming service forbidden".into()))
    }
}
