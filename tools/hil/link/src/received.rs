//! Device messages as the host receives them.

use std::collections::HashMap;
use std::sync::LazyLock;

use oer_hil_protocol::{Envelope, Frame, Key, Message, MessageInfo};

/// Every message this host knows, by key.
static REGISTRY: LazyLock<HashMap<Key, &'static MessageInfo>> = LazyLock::new(|| {
    oer_hil_protocol::registry()
        .map(|message| (message.key, message))
        .collect()
});

/// The registered message with `key`.
pub fn message_info(key: Key) -> Option<&'static MessageInfo> {
    REGISTRY.get(&key).copied()
}

/// One device message: its header, its key and its payload.
#[derive(Clone, Debug)]
pub struct Received {
    pub boot_id: u64,
    pub message_sequence: u32,
    pub session_id: u64,
    pub request_id: u32,
    pub key: Key,
    payload: Vec<u8>,
}

impl Received {
    /// `frame` as a message this host knows; a key it does not know is an
    /// error, since nothing could then decode or check the message.
    pub fn from_frame(frame: &Frame<'_>) -> Result<Self, String> {
        let Some(info) = message_info(frame.key) else {
            return Err(format!(
                "device sent message key {} that this host does not know",
                frame.key
            ));
        };
        if (info.decode_json)(frame.payload).is_none() {
            return Err(format!(
                "{}: payload does not decode as its type",
                info.path
            ));
        }
        Ok(Self {
            boot_id: frame.boot_id,
            message_sequence: frame.message_sequence,
            session_id: frame.session_id,
            request_id: frame.request_id,
            key: frame.key,
            payload: frame.payload.to_vec(),
        })
    }

    /// The registered path of this message.
    pub fn path(&self) -> &'static str {
        message_info(self.key).map_or("unknown", |info| info.path)
    }

    pub fn is<M: Message>(&self) -> bool {
        self.key == M::KEY
    }

    /// This message as `M`, when it is one.
    pub fn decode<M: Message>(&self) -> Option<M> {
        self.is::<M>()
            .then(|| postcard::from_bytes(&self.payload).ok())
            .flatten()
    }

    /// This message as an envelope of `body`.
    pub fn envelope<T>(&self, body: T) -> Envelope<T> {
        Envelope::new(
            self.boot_id,
            self.message_sequence,
            self.session_id,
            self.request_id,
            body,
        )
    }

    /// The readable record of this message.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "boot_id": self.boot_id,
            "message_sequence": self.message_sequence,
            "session_id": self.session_id,
            "request_id": self.request_id,
            "key": self.key.to_string(),
            "path": self.path(),
            "body": message_info(self.key).and_then(|info| (info.decode_json)(&self.payload)),
        })
    }
}
