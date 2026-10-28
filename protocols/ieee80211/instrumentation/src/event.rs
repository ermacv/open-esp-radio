//! The channels of this vocabulary and the macros that declare its events.
//!
//! The IEEE 802.11 domain owns event ids 1..=99 and channels 0..=18 here;
//! the ESP32-S31 driver's lower-MAC trace uses ids from 100 and channel 19.

use oer_trace::{Channel, Domain};

/// The `index`th channel of the IEEE 802.11 domain, checked at compile time.
macro_rules! channel {
    ($index:literal) => {
        const {
            match Channel::new(Domain::Ieee80211, $index) {
                Ok(channel) => channel,
                Err(_) => panic!("an IEEE 802.11 channel lies in the domain's range"),
            }
        }
    };
}

/// Station core events: tokens, data losses, RSN input drops, scan
/// observations.
pub const STATION: Channel = channel!(0);
/// Station service events: receive drops and refusals, lost port events,
/// unsent frames and group key changes.
pub const SERVICE: Channel = channel!(1);
/// ARP correlation of the protected link, apart from the service's channel
/// because it decodes every ARP MSDU.
pub const ARP: Channel = channel!(2);
/// ESP-NOW encrypted peer table events.
pub const ESP_NOW: Channel = channel!(3);

/// An enum of an event field with a fixed code per variant: the code is its
/// part of the event's encoding.
macro_rules! coded {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident = $code:literal,)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            /// This value's code in an event's encoding.
            pub const fn code(self) -> u32 {
                match self {
                    $(Self::$variant => $code,)+
                }
            }

            /// The value of `code`, or `None` for a code this type never
            /// encodes.
            pub const fn of_code(code: u32) -> Option<Self> {
                match code {
                    $($code => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

/// An event without fields.
macro_rules! unit_event {
    ($(#[$meta:meta])* $name:ident = $id:literal on $channel:path, $text:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct $name;

        impl oer_trace::Event for $name {
            const KIND: oer_trace::Kind = oer_trace::Kind::new(oer_trace::Domain::Ieee80211, $id);
            const CHANNEL: oer_trace::Channel = $channel;

            fn encode(&self) -> [u32; 2] {
                [0, 0]
            }

            fn decode(words: [u32; 2]) -> Option<Self> {
                (words == [0, 0]).then_some(Self)
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str($text)
            }
        }
    };
}

/// An event with one coded field.
macro_rules! coded_event {
    (
        $(#[$meta:meta])*
        $name:ident { $field:ident: $type:ty } = $id:literal on $channel:path, $text:literal
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct $name {
            pub $field: $type,
        }

        impl oer_trace::Event for $name {
            const KIND: oer_trace::Kind = oer_trace::Kind::new(oer_trace::Domain::Ieee80211, $id);
            const CHANNEL: oer_trace::Channel = $channel;

            fn encode(&self) -> [u32; 2] {
                [self.$field.code(), 0]
            }

            fn decode([code, zero]: [u32; 2]) -> Option<Self> {
                if zero != 0 {
                    return None;
                }
                Some(Self {
                    $field: <$type>::of_code(code)?,
                })
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!($text, " {:?}"), self.$field)
            }
        }
    };
}

pub(crate) use {coded, coded_event, unit_event};
