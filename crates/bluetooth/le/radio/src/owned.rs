//! Requests that own their PDUs, for a backend that hands a request to a
//! driver instead of serving it in the caller's borrow.
//!
//! A [`RadioRequest`] borrows its PDUs from the caller. A backend whose
//! driver owns the radio state receives each request as a message, which
//! outlives no caller's borrow, so it copies the request into an
//! [`OwnedRadioRequest`] ([`RadioRequest::to_owned`]) and serves its
//! borrowed view ([`OwnedRadioRequest::as_request`]). A copy keeps every
//! value its check read, bytes and fields alike, so the checked values move
//! between the storages through [`Supported::map_storage`] and nothing is
//! checked again. Only this crate implements [`Restore`].

use crate::pdu::{ADVERTISER_ADDRESS_BYTES, ADVERTISING_PAYLOAD_MAX, DATA_PAYLOAD_MAX};
use crate::supported::TEST_PAYLOAD_MAX;
use crate::{
    AcceptListChange, AdvertisingConfiguration, AdvertisingEvent, AdvertisingPdu,
    AdvertisingReception, AdvertisingSetId, ConnectionConfiguration, ConnectionEvent, ConnectionId,
    DataPdu, DataPduKind, EventId, LeCapabilities, LePhy, LeWindow, RadioRequest, ScanWindow,
    ScannerConfiguration, ScannerId, Supported, TestChannel, TestPayloadType, TestPhy, TestReceive,
    TestTransmit, TxPower,
};

mod sealed {
    pub trait Sealed {}

    /// Only this crate names it, so only this crate calls a `Restore`.
    pub struct Token(pub(super) ());
}

/// A move of a value's bytes into another storage that keeps every field
/// and byte its capability check read, so a [`Supported`] value stays
/// checked ([`Supported::map_storage`]).
///
/// Sealed: only this crate implements it, for the request values and their
/// owned counterparts, and only this crate calls it, through
/// [`Supported::map_storage`] on a value its check passed. Another crate
/// can neither turn a checked value into an unchecked one nor copy an
/// unchecked one:
///
/// ```compile_fail,E0603
/// // The token a `restore` call takes is private to this crate.
/// use oer_bluetooth_radio::owned::sealed::Token;
/// ```
///
/// Its compiling twin moves a checked value into owned storage and back:
///
/// ```
/// use oer_bluetooth_radio::{DataPdu, LeCapabilities, OwnedDataPdu, Supported};
/// fn round_trip<K: LeCapabilities>(pdu: Supported<K, DataPdu<'_>>) -> bool {
///     let owned: Supported<K, OwnedDataPdu> = pdu.map_storage();
///     let back: Supported<K, DataPdu<'_>> = owned.map_storage();
///     back == pdu
/// }
/// ```
pub trait Restore<'s, U>: sealed::Sealed {
    /// The same value in the storage `U`; only [`Supported::map_storage`]
    /// calls it, on a checked value.
    #[doc(hidden)]
    fn restore(&'s self, token: sealed::Token) -> U;
}

impl<K: LeCapabilities, T> Supported<K, T> {
    /// The same checked value with its bytes in another storage: owned
    /// from borrowed, or borrowed from owned. Nothing is checked again,
    /// because a [`Restore`] keeps what the check read.
    pub fn map_storage<'s, U>(&'s self) -> Supported<K, U>
    where
        T: Restore<'s, U>,
    {
        Supported::restored(self.get().restore(sealed::Token(())))
    }
}

/// At most `N` bytes, owned.
#[derive(Clone, Copy)]
struct Bytes<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Bytes<N> {
    /// A copy of `from`, which its type's check bounds by `N`.
    fn copy(from: &[u8]) -> Self {
        let mut bytes = [0; N];
        bytes[..from.len()].copy_from_slice(from);
        Self {
            bytes,
            len: from.len(),
        }
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl<const N: usize> core::fmt::Debug for Bytes<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.as_slice().fmt(f)
    }
}

impl<const N: usize> PartialEq for Bytes<N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<const N: usize> Eq for Bytes<N> {}

const ADVERTISING_PDU_MAX: usize = 2 + ADVERTISING_PAYLOAD_MAX;
const _: () = assert!(ADVERTISING_PDU_MAX > 2 + ADVERTISER_ADDRESS_BYTES);

/// An owned copy of an [`AdvertisingPdu`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OwnedAdvertisingPdu(Bytes<ADVERTISING_PDU_MAX>);

impl OwnedAdvertisingPdu {
    fn copy(pdu: AdvertisingPdu<'_>) -> Self {
        Self(Bytes::copy(pdu.bytes()))
    }

    fn borrow(&self) -> AdvertisingPdu<'_> {
        AdvertisingPdu::new(self.0.as_slice()).expect("a copy keeps a checked PDU")
    }
}

/// An owned copy of an [`AdvertisingReception`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnedAdvertisingReception {
    None,
    ScanResponse(OwnedAdvertisingPdu),
    Report,
}

/// An owned copy of an [`AdvertisingConfiguration`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnedAdvertisingConfiguration {
    set: AdvertisingSetId,
    pdu: OwnedAdvertisingPdu,
    reception: OwnedAdvertisingReception,
    tx_power: TxPower,
    phy: LePhy,
}

impl sealed::Sealed for AdvertisingConfiguration<'_> {}

impl<'s> Restore<'s, OwnedAdvertisingConfiguration> for AdvertisingConfiguration<'_> {
    fn restore(&'s self, _: sealed::Token) -> OwnedAdvertisingConfiguration {
        OwnedAdvertisingConfiguration {
            set: self.set,
            pdu: OwnedAdvertisingPdu::copy(self.pdu),
            reception: match self.reception {
                AdvertisingReception::None => OwnedAdvertisingReception::None,
                AdvertisingReception::ScanResponse(pdu) => {
                    OwnedAdvertisingReception::ScanResponse(OwnedAdvertisingPdu::copy(pdu))
                }
                AdvertisingReception::Report => OwnedAdvertisingReception::Report,
            },
            tx_power: self.tx_power,
            phy: self.phy,
        }
    }
}

impl sealed::Sealed for OwnedAdvertisingConfiguration {}

impl<'s> Restore<'s, AdvertisingConfiguration<'s>> for OwnedAdvertisingConfiguration {
    fn restore(&'s self, _: sealed::Token) -> AdvertisingConfiguration<'s> {
        AdvertisingConfiguration {
            set: self.set,
            pdu: self.pdu.borrow(),
            reception: match &self.reception {
                OwnedAdvertisingReception::None => AdvertisingReception::None,
                OwnedAdvertisingReception::ScanResponse(pdu) => {
                    AdvertisingReception::ScanResponse(pdu.borrow())
                }
                OwnedAdvertisingReception::Report => AdvertisingReception::Report,
            },
            tx_power: self.tx_power,
            phy: self.phy,
        }
    }
}

/// An owned copy of a [`DataPdu`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnedDataPdu {
    kind: DataPduKind,
    payload: Bytes<DATA_PAYLOAD_MAX>,
}

impl sealed::Sealed for DataPdu<'_> {}

impl<'s> Restore<'s, OwnedDataPdu> for DataPdu<'_> {
    fn restore(&'s self, _: sealed::Token) -> OwnedDataPdu {
        OwnedDataPdu {
            kind: self.kind(),
            payload: Bytes::copy(self.payload()),
        }
    }
}

impl sealed::Sealed for OwnedDataPdu {}

impl<'s> Restore<'s, DataPdu<'s>> for OwnedDataPdu {
    fn restore(&'s self, _: sealed::Token) -> DataPdu<'s> {
        DataPdu::new(self.kind, self.payload.as_slice()).expect("a copy keeps a checked PDU")
    }
}

/// An owned copy of a [`TestTransmit`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnedTestTransmit {
    id: EventId,
    channel: TestChannel,
    phy: TestPhy,
    window: LeWindow,
    tx_power: TxPower,
    payload_type: TestPayloadType,
    payload: Bytes<TEST_PAYLOAD_MAX>,
}

impl sealed::Sealed for TestTransmit<'_> {}

impl<'s> Restore<'s, OwnedTestTransmit> for TestTransmit<'_> {
    fn restore(&'s self, _: sealed::Token) -> OwnedTestTransmit {
        OwnedTestTransmit {
            id: self.id,
            channel: self.channel,
            phy: self.phy,
            window: self.window,
            tx_power: self.tx_power,
            payload_type: self.payload_type,
            payload: Bytes::copy(self.payload),
        }
    }
}

impl sealed::Sealed for OwnedTestTransmit {}

impl<'s> Restore<'s, TestTransmit<'s>> for OwnedTestTransmit {
    fn restore(&'s self, _: sealed::Token) -> TestTransmit<'s> {
        TestTransmit {
            id: self.id,
            channel: self.channel,
            phy: self.phy,
            window: self.window,
            tx_power: self.tx_power,
            payload_type: self.payload_type,
            payload: self.payload.as_slice(),
        }
    }
}

/// The request values that hold no bytes are their own storage.
macro_rules! own_storage {
    ($($value:ty),* $(,)?) => {$(
        impl sealed::Sealed for $value {}

        impl<'s> Restore<'s, $value> for $value {
            fn restore(&'s self, _: sealed::Token) -> $value {
                *self
            }
        }
    )*};
}

own_storage!(
    AdvertisingEvent,
    ScannerConfiguration,
    ScanWindow,
    ConnectionConfiguration,
    ConnectionEvent,
    TestReceive,
    AcceptListChange,
);

/// A [`RadioRequest`] that owns its PDUs: every checked value moved into
/// owned storage ([`RadioRequest::to_owned`]).
#[derive(Debug, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "a request owns at most one PDU of its kind, inline"
)]
pub enum OwnedRadioRequest<K: LeCapabilities> {
    /// [`RadioRequest::ConfigureAdvertising`].
    ConfigureAdvertising(Supported<K, OwnedAdvertisingConfiguration>),
    /// [`RadioRequest::Advertise`].
    Advertise(Supported<K, AdvertisingEvent>),
    /// [`RadioRequest::RemoveAdvertising`].
    RemoveAdvertising(AdvertisingSetId),
    /// [`RadioRequest::ConfigureScanner`].
    ConfigureScanner(Supported<K, ScannerConfiguration>),
    /// [`RadioRequest::Scan`].
    Scan(Supported<K, ScanWindow>),
    /// [`RadioRequest::RemoveScanner`].
    RemoveScanner(ScannerId),
    /// [`RadioRequest::OpenConnection`].
    OpenConnection(Supported<K, ConnectionConfiguration>),
    /// [`RadioRequest::ConnectionEvent`].
    ConnectionEvent(Supported<K, ConnectionEvent>),
    /// [`RadioRequest::Transmit`].
    Transmit {
        /// The connection.
        connection: ConnectionId,
        /// The PDU.
        pdu: Supported<K, OwnedDataPdu>,
    },
    /// [`RadioRequest::CloseConnection`].
    CloseConnection(ConnectionId),
    /// [`RadioRequest::TestTransmit`].
    TestTransmit(Supported<K, OwnedTestTransmit>),
    /// [`RadioRequest::TestReceive`].
    TestReceive(Supported<K, TestReceive>),
    /// [`RadioRequest::EndTest`].
    EndTest,
    /// [`RadioRequest::FilterAcceptList`].
    FilterAcceptList(Supported<K, AcceptListChange>),
}

impl<K: LeCapabilities> RadioRequest<'_, K> {
    /// The same request, owning a copy of its PDUs.
    pub fn to_owned(&self) -> OwnedRadioRequest<K> {
        match self {
            Self::ConfigureAdvertising(value) => {
                OwnedRadioRequest::ConfigureAdvertising(value.map_storage())
            }
            Self::Advertise(value) => OwnedRadioRequest::Advertise(value.map_storage()),
            Self::RemoveAdvertising(set) => OwnedRadioRequest::RemoveAdvertising(*set),
            Self::ConfigureScanner(value) => {
                OwnedRadioRequest::ConfigureScanner(value.map_storage())
            }
            Self::Scan(value) => OwnedRadioRequest::Scan(value.map_storage()),
            Self::RemoveScanner(scanner) => OwnedRadioRequest::RemoveScanner(*scanner),
            Self::OpenConnection(value) => OwnedRadioRequest::OpenConnection(value.map_storage()),
            Self::ConnectionEvent(value) => OwnedRadioRequest::ConnectionEvent(value.map_storage()),
            Self::Transmit { connection, pdu } => OwnedRadioRequest::Transmit {
                connection: *connection,
                pdu: pdu.map_storage(),
            },
            Self::CloseConnection(connection) => OwnedRadioRequest::CloseConnection(*connection),
            Self::TestTransmit(value) => OwnedRadioRequest::TestTransmit(value.map_storage()),
            Self::TestReceive(value) => OwnedRadioRequest::TestReceive(value.map_storage()),
            Self::EndTest => OwnedRadioRequest::EndTest,
            Self::FilterAcceptList(value) => {
                OwnedRadioRequest::FilterAcceptList(value.map_storage())
            }
        }
    }
}

impl<K: LeCapabilities> OwnedRadioRequest<K> {
    /// The request, borrowing the owned PDUs.
    pub fn as_request(&self) -> RadioRequest<'_, K> {
        match self {
            Self::ConfigureAdvertising(value) => {
                RadioRequest::ConfigureAdvertising(value.map_storage())
            }
            Self::Advertise(value) => RadioRequest::Advertise(value.map_storage()),
            Self::RemoveAdvertising(set) => RadioRequest::RemoveAdvertising(*set),
            Self::ConfigureScanner(value) => RadioRequest::ConfigureScanner(value.map_storage()),
            Self::Scan(value) => RadioRequest::Scan(value.map_storage()),
            Self::RemoveScanner(scanner) => RadioRequest::RemoveScanner(*scanner),
            Self::OpenConnection(value) => RadioRequest::OpenConnection(value.map_storage()),
            Self::ConnectionEvent(value) => RadioRequest::ConnectionEvent(value.map_storage()),
            Self::Transmit { connection, pdu } => RadioRequest::Transmit {
                connection: *connection,
                pdu: pdu.map_storage(),
            },
            Self::CloseConnection(connection) => RadioRequest::CloseConnection(*connection),
            Self::TestTransmit(value) => RadioRequest::TestTransmit(value.map_storage()),
            Self::TestReceive(value) => RadioRequest::TestReceive(value.map_storage()),
            Self::EndTest => RadioRequest::EndTest,
            Self::FilterAcceptList(value) => RadioRequest::FilterAcceptList(value.map_storage()),
        }
    }
}

#[cfg(test)]
mod tests;
