//! One typed value for every event of this vocabulary, for collectors.

use oer_trace::Event;

use crate::{
    ArpAddresses, ArpReceived, ArpTransmitted, DataMsduLost, EspNowPeerTableChanged,
    EspNowPeersCleared, EspNowReplay, EspNowTxPacketNumber, GroupKeyInstalled, PortEventsLost,
    RsnInputDropped, RxFrameDropped, RxMsduRefused, ScanBssObserved, TokenLost, TxMsduTooLong,
    TxUnsent,
};

macro_rules! observations {
    ($($event:ident),+ $(,)?) => {
        /// An event of the IEEE 802.11 cores and runtime.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum Observation {
            $($event($event),)+
        }

        impl Observation {
            /// The observation `event` is, or `None` for an event of another
            /// vocabulary. A collector's `Observer::event` matches on it.
            pub fn of<E: Event>(event: &E) -> Option<Self> {
                $(
                    if E::KIND == <$event as Event>::KIND {
                        return <$event as Event>::decode(event.encode()).map(Self::$event);
                    }
                )+
                None
            }
        }

        oer_trace::event_set!(pub StationEvents: $($event),+);
    };
}

observations!(
    TokenLost,
    DataMsduLost,
    RsnInputDropped,
    ScanBssObserved,
    PortEventsLost,
    RxFrameDropped,
    RxMsduRefused,
    TxUnsent,
    TxMsduTooLong,
    GroupKeyInstalled,
    ArpAddresses,
    ArpTransmitted,
    ArpReceived,
    EspNowPeerTableChanged,
    EspNowPeersCleared,
    EspNowTxPacketNumber,
    EspNowReplay,
);
