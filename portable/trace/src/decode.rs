//! Host-side description of drained records with the domains' event types.

use core::fmt;

use crate::{Kind, Record};

/// A set of event types a host describes records with; declare one with
/// [`event_set!`](crate::event_set).
pub trait EventSet {
    /// Write `record` if it is one of this set's events, `None` otherwise.
    fn describe(record: &Record, f: &mut fmt::Formatter<'_>) -> Option<fmt::Result>;
}

/// The describe function of one [`EventSet`], for composing sets of several
/// domains: `&[StationTrace::describe, PhyTrace::describe]`.
pub type Describer = fn(&Record, &mut fmt::Formatter<'_>) -> Option<fmt::Result>;

/// `record` as the first of `sets` that knows it, or generically as its
/// domain, event id and words.
pub struct Described<'a> {
    pub record: &'a Record,
    pub sets: &'a [Describer],
}

impl fmt::Display for Described<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for describe in self.sets {
            if let Some(result) = describe(self.record, f) {
                return result;
            }
        }
        match self.record.kind() {
            Some(kind) => write!(f, "{kind}")?,
            None => write!(f, "kind {:#06x}", self.record.kind)?,
        }
        write!(
            f,
            " [{:#010x}, {:#010x}]",
            self.record.words[0], self.record.words[1]
        )
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let domain = match self.domain {
            crate::Domain::Platform => "platform",
            crate::Domain::Phy => "phy",
            crate::Domain::Ieee80211 => "ieee80211",
            crate::Domain::Bluetooth => "bluetooth",
            crate::Domain::Ieee802154 => "ieee802154",
        };
        write!(f, "{domain}.{}", self.event)
    }
}

/// Declare a unit type implementing [`EventSet`] over event types that
/// implement [`Event`](crate::Event) and `Display`:
///
/// ```ignore
/// oer_trace::event_set!(pub StationTrace: BeaconDispatch, ControlMailbox);
/// ```
#[macro_export]
macro_rules! event_set {
    ($vis:vis $name:ident: $($event:ty),+ $(,)?) => {
        /// The trace events a host describes drained records with.
        $vis struct $name;

        impl $crate::EventSet for $name {
            fn describe(
                record: &$crate::Record,
                f: &mut ::core::fmt::Formatter<'_>,
            ) -> ::core::option::Option<::core::fmt::Result> {
                $(
                    if let ::core::option::Option::Some(event) = record.decode::<$event>() {
                        return ::core::option::Option::Some(::core::fmt::Display::fmt(&event, f));
                    }
                )+
                ::core::option::Option::None
            }
        }
    };
}
