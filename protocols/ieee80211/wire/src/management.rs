//! Allocation-free IEEE 802.11 management-frame construction.

use crate::status::{ReasonCode, StatusCode};
use oer_portable_wire::PrefixLength;
use oer_protocols_ieee802_wire::{Eui48, Eui48Kind};

use crate::sequence::SequenceNumber;

pub const MANAGEMENT_HEADER_LEN: usize = 24;
pub const MAX_SSID_LEN: usize = 32;
pub const MAX_SUPPORTED_RATES_LEN: usize = 24;

const PROBE_REQUEST_FRAME_CONTROL: crate::frame::FrameControlFields =
    crate::frame::FrameControlFields::from_bytes([0x40, 0x00]);
const SSID_ELEMENT_ID: u8 = 0;
const SUPPORTED_RATES_ELEMENT_ID: u8 = 1;
const EXTENDED_SUPPORTED_RATES_ELEMENT_ID: u8 = 50;
const SUPPORTED_RATES_ELEMENT_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeRequestError {
    SsidTooLong,
    NoSupportedRates,
    TooManySupportedRates,
    OutputTooSmall { required: usize },
}

/// A borrowed Probe Request description.
///
/// The lifetime ties `ssid` and `supported_rates` to their caller-owned
/// storage. Encoding copies the finished frame into caller-owned DMA-capable
/// memory, so this type needs neither allocation nor global buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeRequest<'a> {
    pub destination: Eui48,
    pub source: Eui48,
    pub bssid: Eui48,
    pub sequence_number: SequenceNumber,
    pub ssid: &'a [u8],
    pub supported_rates: &'a [u8],
}

impl ProbeRequest<'_> {
    /// Encode a wildcard or directed Probe Request into `output`.
    ///
    /// `sequence_number` is the 12-bit 802.11 sequence number; the fragment
    /// number is emitted as zero. Rate bytes use the standard 500 kbit/s
    /// representation, including any basic-rate bit supplied by the caller.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ProbeRequestError> {
        let ssid_len = PrefixLength::<MAX_SSID_LEN>::new(self.ssid.len())
            .ok_or(ProbeRequestError::SsidTooLong)?;
        if self.supported_rates.is_empty() {
            return Err(ProbeRequestError::NoSupportedRates);
        }
        if self.supported_rates.len() > MAX_SUPPORTED_RATES_LEN {
            return Err(ProbeRequestError::TooManySupportedRates);
        }

        let first_rates_len = self
            .supported_rates
            .len()
            .min(SUPPORTED_RATES_ELEMENT_CAPACITY);
        let mut reader = oer_portable_wire::Reader::new(self.supported_rates);
        let first_rates = reader
            .take(first_rates_len)
            .map_err(|_| ProbeRequestError::TooManySupportedRates)?;
        let extended_rates = reader.remaining();
        let first_rates_len =
            PrefixLength::<SUPPORTED_RATES_ELEMENT_CAPACITY>::new(first_rates.len())
                .ok_or(ProbeRequestError::TooManySupportedRates)?;
        let extended_rates_len = PrefixLength::<MAX_SUPPORTED_RATES_LEN>::new(extended_rates.len())
            .ok_or(ProbeRequestError::TooManySupportedRates)?;
        let mut header = [0; MANAGEMENT_HEADER_LEN];
        crate::encoding::write_array::<0, _, _>(
            &mut header,
            &PROBE_REQUEST_FRAME_CONTROL.to_bytes(),
        );
        crate::encoding::write_array::<4, _, _>(&mut header, &self.destination.octets());
        crate::encoding::write_array::<10, _, _>(&mut header, &self.source.octets());
        crate::encoding::write_array::<16, _, _>(&mut header, &self.bssid.octets());
        crate::encoding::write_array::<22, _, _>(
            &mut header,
            &self.sequence_number.sequence_control().to_bytes(),
        );
        let extended_header = [
            EXTENDED_SUPPORTED_RATES_ELEMENT_ID,
            extended_rates_len.to_u8(),
        ];
        crate::encoding::FrameParts([
            header.as_slice(),
            &[SSID_ELEMENT_ID, ssid_len.to_u8()],
            self.ssid,
            &[SUPPORTED_RATES_ELEMENT_ID, first_rates_len.to_u8()],
            first_rates,
            if extended_rates.is_empty() {
                &[]
            } else {
                &extended_header
            },
            extended_rates,
        ])
        .write(output)
        .map_err(|required| ProbeRequestError::OutputTooSmall { required })
    }
}

/// The MAC header of a management frame without HT Control.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.3.2, Figure 9-49 (Management frame
/// format: Frame Control, Duration, Address 1, Address 2, Address 3 and
/// Sequence Control).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagementHeader {
    pub frame_control: crate::frame::FrameControlFields,
    pub duration: crate::frame::DurationId,
    /// Address 1: the receiver.
    pub receiver: Eui48,
    /// Address 2: the transmitter.
    pub transmitter: Eui48,
    /// Address 3: the BSSID.
    pub bssid: Eui48,
    pub sequence_control: crate::sequence::SequenceControl,
}

impl ManagementHeader {
    /// The header at the start of `frame` and the body after it; `None` for
    /// a frame shorter than the header. The frame type is not checked.
    pub fn split(frame: &[u8]) -> Option<(Self, &[u8])> {
        let (header, body) = frame.split_first_chunk::<MANAGEMENT_HEADER_LEN>()?;
        Some((Self::from_bytes(header), body))
    }

    /// The header's fields as they appear on air.
    pub const fn from_bytes(bytes: &[u8; MANAGEMENT_HEADER_LEN]) -> Self {
        let [
            f0,
            f1,
            d0,
            d1,
            r0,
            r1,
            r2,
            r3,
            r4,
            r5,
            t0,
            t1,
            t2,
            t3,
            t4,
            t5,
            b0,
            b1,
            b2,
            b3,
            b4,
            b5,
            s0,
            s1,
        ] = *bytes;
        Self {
            frame_control: crate::frame::FrameControlFields::from_bytes([f0, f1]),
            duration: crate::frame::DurationId::from_bytes([d0, d1]),
            receiver: Eui48::new([r0, r1, r2, r3, r4, r5]),
            transmitter: Eui48::new([t0, t1, t2, t3, t4, t5]),
            bssid: Eui48::new([b0, b1, b2, b3, b4, b5]),
            sequence_control: crate::sequence::SequenceControl::from_bytes([s0, s1]),
        }
    }

    /// The header as it appears on air.
    pub const fn to_bytes(self) -> [u8; MANAGEMENT_HEADER_LEN] {
        let [f0, f1] = self.frame_control.to_bytes();
        let [d0, d1] = self.duration.to_bytes();
        let [r0, r1, r2, r3, r4, r5] = self.receiver.octets();
        let [t0, t1, t2, t3, t4, t5] = self.transmitter.octets();
        let [b0, b1, b2, b3, b4, b5] = self.bssid.octets();
        let [s0, s1] = self.sequence_control.to_bytes();
        [
            f0, f1, d0, d1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5, b0, b1, b2, b3, b4, b5,
            s0, s1,
        ]
    }

    /// Whether Address 1 is a group address.
    pub const fn group_receiver(&self) -> bool {
        matches!(self.receiver.kind(), Eui48Kind::Group(_))
    }
}

/// The identifier an access point gives an association, 1 to 2007.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.8 (AID field).
///
/// A number is no identifier until it is checked:
///
/// ```compile_fail,E0308
/// use oer_protocols_ieee80211_wire::management::AssociationId;
/// let association_id: AssociationId = 1;
/// ```
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct AssociationId(oer_portable_wire::U16<14>);

impl AssociationId {
    /// The largest identifier.
    pub const MAX: u16 = 2007;

    /// `value`, when it is an identifier.
    pub const fn new(value: u16) -> Option<Self> {
        match oer_portable_wire::U16::new(value) {
            Some(aid) if value != 0 && value <= Self::MAX => Some(Self(aid)),
            _ => None,
        }
    }

    /// The identifier `field` carries, whatever its marker bits.
    pub const fn from_field(field: AidField) -> Option<Self> {
        Self::new(field.aid().get())
    }

    /// The identifier.
    pub const fn value(self) -> u16 {
        self.0.get()
    }

    /// The AID field that carries it: both marker bits set.
    pub const fn field(self) -> AidField {
        AidField::from_bytes([0; 2])
            .with_aid(self.0)
            .with_marker(oer_portable_wire::U8::constant::<3>())
    }
}

/// A Dialog Token, which matches an action's request with its responses: any
/// octet. What zero means, and how a sequence of tokens goes, is the
/// action's.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.12 (Dialog Token field).
///
/// A number is no token until it is named one:
///
/// ```compile_fail,E0308
/// use oer_protocols_ieee80211_wire::management::DialogToken;
/// let token: DialogToken = 1;
/// ```
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct DialogToken(u8);

impl DialogToken {
    /// The token of all zero bits.
    pub const ZERO: Self = Self(0);

    /// The token `value`.
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Its octet.
    pub const fn value(self) -> u8 {
        self.0
    }

    /// The token after this one in a sequence that skips zero.
    pub const fn next_nonzero(self) -> Self {
        match self.0.wrapping_add(1) {
            0 => Self(1),
            next => Self(next),
        }
    }
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.4 (Capability Information field).
    pub struct CapabilityInformation[2] (little) {
        pub ess, with_ess: bool = 0..1;
        pub ibss, with_ibss: bool = 1..2;
        pub cf_pollable, with_cf_pollable: bool = 2..3;
        pub cf_poll_request, with_cf_poll_request: bool = 3..4;
        pub privacy, with_privacy: bool = 4..5;
        pub short_preamble, with_short_preamble: bool = 5..6;
        pub pbcc, with_pbcc: bool = 6..7;
        pub channel_agility, with_channel_agility: bool = 7..8;
        pub spectrum_management, with_spectrum_management: bool = 8..9;
        pub qos, with_qos: bool = 9..10;
        pub short_slot, with_short_slot: bool = 10..11;
        pub apsd, with_apsd: bool = 11..12;
        pub radio_measurement, with_radio_measurement: bool = 12..13;
        pub dsss_ofdm, with_dsss_ofdm: bool = 13..14;
        pub delayed_ba, with_delayed_ba: bool = 14..15;
        pub immediate_ba, with_immediate_ba: bool = 15..16;
    }
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.8 (AID field).
    pub struct AidField[2] (little) {
        pub aid, with_aid: (u16) = 0..14;
        pub marker, with_marker: (u8) = 14..16;
    }
}

oer_portable_wire::bitfield! {
    /// The fixed fields of an Authentication frame's body.
    /// SOURCE: IEEE Std 802.11-2020, 9.3.3.12 (Authentication frame format,
    /// Table 9-36): Authentication algorithm number (9.4.1.1),
    /// Authentication transaction sequence number (9.4.1.2), Status code
    /// (9.4.1.9).
    pub struct AuthenticationFields[6] (little) {
        algorithm_number, with_algorithm_number: {u16} = 0..16;
        transaction_number, with_transaction_number: {u16} = 16..32;
        status_number, with_status_number: {u16} = 32..48;
    }
}

impl AuthenticationFields {
    /// The fields of `algorithm`, `transaction` and `status_code`.
    pub const fn of(
        algorithm: AuthenticationAlgorithm,
        transaction: AuthenticationTransaction,
        status_code: StatusCode,
    ) -> Self {
        Self::from_bytes([0; 6])
            .with_algorithm_number(algorithm.value())
            .with_transaction_number(transaction.value())
            .with_status_number(status_code.value())
    }

    /// The authentication algorithm.
    pub const fn algorithm(self) -> AuthenticationAlgorithm {
        AuthenticationAlgorithm::new(self.algorithm_number())
    }

    /// The place of the frame in its exchange.
    pub const fn transaction(self) -> AuthenticationTransaction {
        AuthenticationTransaction::new(self.transaction_number())
    }

    /// The status.
    pub const fn status_code(self) -> StatusCode {
        StatusCode::new(self.status_number())
    }
}

/// An authentication algorithm: any number, some of them named.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.1 (Authentication Algorithm Number
/// field).
///
/// A number is no algorithm until it is named one:
///
/// ```compile_fail,E0308
/// use oer_protocols_ieee80211_wire::management::AuthenticationAlgorithm;
/// let algorithm: AuthenticationAlgorithm = 3;
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct AuthenticationAlgorithm(u16);

impl AuthenticationAlgorithm {
    /// Open System.
    pub const OPEN_SYSTEM: Self = Self(0);
    /// Simultaneous authentication of equals.
    pub const SAE: Self = Self(3);

    /// The algorithm `value`, a named one or not.
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Its number.
    pub const fn value(self) -> u16 {
        self.0
    }
}

/// The place of an Authentication frame in its exchange. What a place is
/// named, the algorithm says.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.2 (Authentication Transaction
/// Sequence Number field); Table 9-41 (Presence of fields and elements in
/// Authentication frames).
///
/// A number is no place until it is named one:
///
/// ```compile_fail,E0308
/// use oer_protocols_ieee80211_wire::management::AuthenticationTransaction;
/// let transaction: AuthenticationTransaction = 1;
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct AuthenticationTransaction(u16);

impl AuthenticationTransaction {
    /// Open System's request.
    pub const OPEN_REQUEST: Self = Self(1);
    /// Open System's response.
    pub const OPEN_RESPONSE: Self = Self(2);
    /// SAE's Commit.
    pub const SAE_COMMIT: Self = Self(1);
    /// SAE's Confirm.
    pub const SAE_CONFIRM: Self = Self(2);

    /// The place `value`, a named one or not.
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Its number.
    pub const fn value(self) -> u16 {
        self.0
    }
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.9 (Status Code field).
    pub struct StatusCodeField[2] (little) {
        number, with_number: {u16} = 0..16;
    }
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.7 (Reason Code field).
    pub struct ReasonCodeField[2] (little) {
        number, with_number: {u16} = 0..16;
    }
}

impl StatusCodeField {
    /// The field holding `code`.
    pub const fn of(code: StatusCode) -> Self {
        Self::from_bytes([0; 2]).with_number(code.value())
    }

    /// The code.
    pub const fn code(self) -> StatusCode {
        StatusCode::new(self.number())
    }
}

impl ReasonCodeField {
    /// The field holding `code`.
    pub const fn of(code: ReasonCode) -> Self {
        Self::from_bytes([0; 2]).with_number(code.value())
    }

    /// The code.
    pub const fn code(self) -> ReasonCode {
        ReasonCode::new(self.number())
    }
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.3 (Beacon Interval field), in
    /// time units.
    pub struct BeaconIntervalField[2] (little) {
        pub time_units, with_time_units: {u16} = 0..16;
    }
}

impl BeaconIntervalField {
    /// The field holding `time_units`.
    pub const fn of(time_units: u16) -> Self {
        Self::from_bytes([0; 2]).with_time_units(time_units)
    }
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.6 (Listen Interval field), in
    /// beacon intervals.
    pub struct ListenIntervalField[2] (little) {
        pub intervals, with_intervals: {u16} = 0..16;
    }
}

impl ListenIntervalField {
    /// The field holding `intervals`.
    pub const fn of(intervals: u16) -> Self {
        Self::from_bytes([0; 2]).with_intervals(intervals)
    }
}

oer_portable_wire::bitfield! {
    /// A Timeout Interval element's Timeout Interval Value.
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.49 (Timeout Interval element).
    pub struct TimeoutIntervalValue[4] (little) {
        pub value, with_value: {u32} = 0..32;
    }
}
oer_portable_wire::bitfield! {
    /// The PMKID Count of an RSN element's PMKID List.
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.24.1 (RSNE: general, Figure
    /// 9-288).
    pub struct PmkidCount[2] (little) {
        pub count, with_count: {u16} = 0..16;
    }
}
oer_portable_wire::bitfield! {
    /// An element's header: its Element ID and Length.
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.1 (Elements: general, Figure
    /// 9-144).
    pub struct ElementHeader[2] (little) {
        pub id, with_id: {u8} = 0..8;
        pub length, with_length: {u8} = 8..16;
    }
}

#[cfg(test)]
mod tests;
