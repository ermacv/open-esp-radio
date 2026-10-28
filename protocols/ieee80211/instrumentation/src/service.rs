//! Events of the station service: what it dropped, refused or could not
//! send, and the group keys it installed.

use oer_trace::{Domain, Event, Kind};

use crate::SERVICE;
use crate::event::{coded, coded_event, unit_event};

unit_event!(
    /// The port reported that it lost events; the indication carries no
    /// frame count.
    PortEventsLost = 16 on SERVICE,
    "service.port-events-lost"
);

coded!(
    /// Why receive admission dropped a frame. The codes are the order of
    /// the upper MAC's receive drop reasons.
    pub enum FrameDrop {
        Header = 0,
        FrameKind = 1,
        Address = 2,
        Fragmented = 3,
        Protection = 4,
        Integrity = 5,
        Replay = 6,
        Payload = 7,
        NotConnected = 8,
    }
);

coded!(
    /// Why the connected RSN protocol dropped a frame.
    pub enum SecurityDrop {
        /// The EAPOL-Key MIC did not verify.
        InvalidMic = 0,
        /// The EAPOL-Key replay counter was stale.
        StaleReplay = 1,
        /// Another refusal of the supplicant or of key unwrap.
        Other = 2,
    }
);

/// Why the service dropped a received frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDrop {
    /// Receive admission refused it.
    Frame(FrameDrop),
    /// The authenticated RSN protocol refused it.
    Security(SecurityDrop),
}

/// The service dropped one received frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxFrameDropped {
    pub reason: RxDrop,
}

impl Event for RxFrameDropped {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 17);
    const CHANNEL: oer_trace::Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        match self.reason {
            RxDrop::Frame(reason) => [0, reason.code()],
            RxDrop::Security(reason) => [1, reason.code()],
        }
    }

    fn decode([class, code]: [u32; 2]) -> Option<Self> {
        let reason = match class {
            0 => RxDrop::Frame(FrameDrop::of_code(code)?),
            1 => RxDrop::Security(SecurityDrop::of_code(code)?),
            _ => return None,
        };
        Some(Self { reason })
    }
}

impl core::fmt::Display for RxFrameDropped {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "service.rx-dropped {:?}", self.reason)
    }
}

coded!(
    /// Why the service did not deliver a received MSDU.
    pub enum RxMsduRefusal {
        /// It exceeded the receive queue's frame length.
        TooLong = 0,
        /// The receive queue to the application was full.
        QueueFull = 1,
    }
);

coded_event!(
    /// The service did not deliver one received MSDU.
    RxMsduRefused { reason: RxMsduRefusal } = 18 on SERVICE,
    "service.rx-msdu-refused"
);

unit_event!(
    /// A station frame left the service unsent; the application heard its
    /// refusal.
    TxUnsent = 19 on SERVICE,
    "service.tx-unsent"
);

unit_event!(
    /// An application MSDU exceeded the data machine's frame length and came
    /// back to the application.
    TxMsduTooLong = 20 on SERVICE,
    "service.tx-msdu-too-long"
);

/// The service installed a group key: the station's first or a replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupKeyInstalled {
    /// The key ID, 0..=3.
    pub key_id: u8,
    /// Whether it replaced an installed group key.
    pub replaced: bool,
}

impl Event for GroupKeyInstalled {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 21);
    const CHANNEL: oer_trace::Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [u32::from(self.key_id), u32::from(self.replaced)]
    }

    fn decode([key_id, replaced]: [u32; 2]) -> Option<Self> {
        Some(Self {
            key_id: u8::try_from(key_id).ok().filter(|key_id| *key_id <= 3)?,
            replaced: match replaced {
                0 => false,
                1 => true,
                _ => return None,
            },
        })
    }
}

impl core::fmt::Display for GroupKeyInstalled {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "service.group-key key_id={} replaced={}",
            self.key_id, self.replaced
        )
    }
}
