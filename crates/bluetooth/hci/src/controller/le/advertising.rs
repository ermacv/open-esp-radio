//! Semantic HCI boundary for the supported legacy advertising command subset.
//!
//! This module decodes the standard `bt-hci` command types and retains one
//! reset-scoped Host configuration initialized to the standard legacy defaults.
//! Enable freezes that state into a typed nonconnectable or connectable request.
//! It does not start a Link Layer event or claim scheduler, SRAM, radio, or
//! on-air progress.

use bt_hci::{
    FromHciBytes, PacketKind,
    cmd::{
        Cmd, Opcode,
        le::{
            LeSetAdvData, LeSetAdvDataParams, LeSetAdvEnable, LeSetAdvParams, LeSetAdvParamsParams,
            LeSetScanResponseData, LeSetScanResponseDataParams,
        },
    },
    param::{AddrKind, AdvFilterPolicy, AdvKind, BdAddr, Error as HciError, Status},
};

use crate::{BluetoothPublicDeviceAddress, HciCommandPacket, HciControllerResponse};

/// Largest legacy advertising data value accepted by the HCI command.
pub const LE_LEGACY_ADVERTISING_DATA_CAPACITY: usize = 31;
/// Complete Command Complete event size for this command family.
pub const LE_LEGACY_ADVERTISING_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 6;

const LEGACY_ADVERTISING_INTERVAL_MIN: u16 = 0x0020;
const LEGACY_ADVERTISING_INTERVAL_MAX: u16 = 0x4000;
const LEGACY_ADVERTISING_INTERVAL_DEFAULT: u16 = 0x0800;

/// Closed identity of the supported standard legacy advertising commands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingCommandKind {
    SetParameters,
    SetData,
    SetScanResponseData,
    SetEnable,
}

impl LeLegacyAdvertisingCommandKind {
    /// Classify an opcode without decoding its parameters.
    pub const fn from_opcode(opcode: Opcode) -> Option<Self> {
        let raw = opcode.to_raw();
        if raw == LeSetAdvParams::OPCODE.to_raw() {
            Some(Self::SetParameters)
        } else if raw == LeSetAdvData::OPCODE.to_raw() {
            Some(Self::SetData)
        } else if raw == LeSetScanResponseData::OPCODE.to_raw() {
            Some(Self::SetScanResponseData)
        } else if raw == LeSetAdvEnable::OPCODE.to_raw() {
            Some(Self::SetEnable)
        } else {
            None
        }
    }

    /// Exact standard HCI opcode.
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::SetParameters => LeSetAdvParams::OPCODE,
            Self::SetData => LeSetAdvData::OPCODE,
            Self::SetScanResponseData => LeSetScanResponseData::OPCODE,
            Self::SetEnable => LeSetAdvEnable::OPCODE,
        }
    }
}

/// Address source selected for the supported advertising role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingOwnAddressKind {
    Public,
    Random,
}

/// Supported legacy advertising behavior selected by Set Advertising Parameters.
///
/// This role is semantic rather than a response-capability flag. It is refined
/// into distinct Enable request and deferred-start types before a chip runner
/// can observe the command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingRole {
    /// Transmit `ADV_NONCONN_IND` without receiving a response.
    Nonconnectable,
    /// Transmit `ADV_SCAN_IND` and answer `SCAN_REQ` with `SCAN_RSP`.
    Scannable,
    /// Transmit `ADV_IND` and permit `SCAN_REQ` or `CONNECT_IND` responses.
    Connectable,
    /// Transmit `ADV_DIRECT_IND` at most 3.75 ms apart for 1.28 s and
    /// accept `CONNECT_IND` from the peer only.
    DirectedHighDuty,
    /// Transmit `ADV_DIRECT_IND` at the advertising interval and accept
    /// `CONNECT_IND` from the peer only.
    DirectedLowDuty,
}

/// Non-empty HCI primary-channel selection with reserved bits rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingPrimaryChannels {
    channel_37: bool,
    channel_38: bool,
    channel_39: bool,
}

impl LeLegacyAdvertisingPrimaryChannels {
    /// Whether primary advertising channel 37 is selected.
    pub const fn channel_37(self) -> bool {
        self.channel_37
    }

    /// Whether primary advertising channel 38 is selected.
    pub const fn channel_38(self) -> bool {
        self.channel_38
    }

    /// Whether primary advertising channel 39 is selected.
    pub const fn channel_39(self) -> bool {
        self.channel_39
    }
}

/// Validated interval range supplied by LE Set Advertising Parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingIntervalRange {
    minimum_units_625_us: u16,
    maximum_units_625_us: u16,
}

impl LeLegacyAdvertisingIntervalRange {
    /// Minimum requested interval in 0.625 ms units.
    pub const fn minimum_units_625_us(self) -> u16 {
        self.minimum_units_625_us
    }

    /// Maximum requested interval in 0.625 ms units.
    pub const fn maximum_units_625_us(self) -> u16 {
        self.maximum_units_625_us
    }
}

/// Validated parameters for one supported legacy advertising role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingParameters {
    role: LeLegacyAdvertisingRole,
    interval: LeLegacyAdvertisingIntervalRange,
    own_address_kind: LeLegacyAdvertisingOwnAddressKind,
    channels: LeLegacyAdvertisingPrimaryChannels,
    peer: LeLegacyAdvertisingPeer,
}

/// The peer a directed set addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingPeer {
    random: bool,
    address: [u8; 6],
}

impl LeLegacyAdvertisingPeer {
    /// Whether the peer address is random rather than public.
    pub const fn is_random(self) -> bool {
        self.random
    }

    /// The peer's device address.
    pub fn address(self) -> BdAddr {
        BdAddr::new(self.address)
    }
}

impl LeLegacyAdvertisingParameters {
    /// Exact Link Layer role selected by the Host.
    pub const fn role(self) -> LeLegacyAdvertisingRole {
        self.role
    }

    /// Controller-selectable advertising interval range.
    pub const fn interval(self) -> LeLegacyAdvertisingIntervalRange {
        self.interval
    }

    /// Source of the advertiser address.
    pub const fn own_address_kind(self) -> LeLegacyAdvertisingOwnAddressKind {
        self.own_address_kind
    }

    /// Complete non-empty primary-channel selection.
    pub const fn channels(self) -> LeLegacyAdvertisingPrimaryChannels {
        self.channels
    }

    /// The peer of a directed role; other roles ignore it.
    pub const fn peer(self) -> LeLegacyAdvertisingPeer {
        self.peer
    }
}

/// Owned, length-checked advertising data from LE Set Advertising Data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingData {
    bytes: [u8; LE_LEGACY_ADVERTISING_DATA_CAPACITY],
    length: u8,
}

/// Owned, length-checked scan-response data from LE Set Scan Response Data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanResponseData {
    bytes: [u8; LE_LEGACY_ADVERTISING_DATA_CAPACITY],
    length: u8,
}

impl LeLegacyScanResponseData {
    /// Borrow only the Host-declared scan-response prefix.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// Number of meaningful scan-response octets.
    pub const fn len(&self) -> usize {
        self.length as usize
    }

    /// Whether the Host selected an empty scan response.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }
}

impl LeLegacyAdvertisingData {
    /// No advertising data.
    pub const EMPTY: Self = Self {
        bytes: [0; LE_LEGACY_ADVERTISING_DATA_CAPACITY],
        length: 0,
    };

    /// Borrow only the Host-declared advertising-data prefix.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// Number of meaningful advertising-data octets.
    pub const fn len(&self) -> usize {
        self.length as usize
    }

    /// Whether the Host selected an empty advertising-data value.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }
}

/// One fully decoded standard command awaiting Controller policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingCommand {
    SetParameters(LeLegacyAdvertisingParameters),
    SetData(LeLegacyAdvertisingData),
    SetScanResponseData(LeLegacyScanResponseData),
    SetEnable(bool),
}

/// Configuration-only subset which can complete without starting hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingConfigurationCommand {
    SetParameters(LeLegacyAdvertisingParameters),
    SetData(LeLegacyAdvertisingData),
    SetScanResponseData(LeLegacyScanResponseData),
}

/// One decoded Set Advertising Enable command awaiting lifecycle policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingEnableCommand {
    enable: bool,
}

impl LeLegacyAdvertisingEnableCommand {
    /// Separate Enable from the full command family without losing its value.
    pub fn from_command(
        command: LeLegacyAdvertisingCommand,
    ) -> Result<Self, LeLegacyAdvertisingCommand> {
        match command {
            LeLegacyAdvertisingCommand::SetEnable(enable) => Ok(Self { enable }),
            command => Err(command),
        }
    }

    /// Whether the Host requested entry into the enabled lifecycle.
    pub const fn enable(self) -> bool {
        self.enable
    }
}

/// Resolved advertiser address retained by an accepted Enable transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingAddress {
    /// Controller public identity, retained in canonical EUI-48 order.
    Public(BluetoothPublicDeviceAddress),
    /// Host-configured random identity, retained in HCI/Link-Layer wire order.
    Random(BdAddr),
}

impl LeLegacyAdvertisingAddress {
    /// Produce the typed HCI/Link-Layer address without erasing its source.
    pub fn wire_address(self) -> BdAddr {
        match self {
            Self::Public(address) => BdAddr::new(address.hci_wire_bytes()),
            Self::Random(address) => address,
        }
    }
}

/// Immutable nonconnectable Host snapshot retained from Enable until radio start.
///
/// This value owns all software inputs needed to create a portable Link Layer
/// set. It grants no scheduler, SRAM, radio, or response-completion authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyNonconnectableAdvertisingEnableRequest {
    parameters: LeLegacyAdvertisingParameters,
    data: LeLegacyAdvertisingData,
    advertiser: LeLegacyAdvertisingAddress,
}

impl LeLegacyNonconnectableAdvertisingEnableRequest {
    /// Exact accepted nonconnectable parameter snapshot.
    pub const fn parameters(self) -> LeLegacyAdvertisingParameters {
        self.parameters
    }

    /// Exact accepted advertising-data snapshot.
    pub const fn data(self) -> LeLegacyAdvertisingData {
        self.data
    }

    /// Address resolved at the Enable ordering boundary.
    pub const fn advertiser(self) -> LeLegacyAdvertisingAddress {
        self.advertiser
    }
}

/// Immutable scannable or connectable Host snapshot retained from Enable until
/// radio start.
///
/// Scan-response data is present only on this response-capable request. The
/// value grants no scheduler, SRAM, radio, or response-completion authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyResponseCapableAdvertisingEnableRequest {
    parameters: LeLegacyAdvertisingParameters,
    data: LeLegacyAdvertisingData,
    scan_response_data: LeLegacyScanResponseData,
    advertiser: LeLegacyAdvertisingAddress,
}

impl LeLegacyResponseCapableAdvertisingEnableRequest {
    /// Exact accepted parameter snapshot.
    pub const fn parameters(self) -> LeLegacyAdvertisingParameters {
        self.parameters
    }

    /// Exact accepted advertising-data snapshot.
    pub const fn data(self) -> LeLegacyAdvertisingData {
        self.data
    }

    /// Exact accepted scan-response-data snapshot.
    pub const fn scan_response_data(self) -> LeLegacyScanResponseData {
        self.scan_response_data
    }

    /// Address resolved at the Enable ordering boundary.
    pub const fn advertiser(self) -> LeLegacyAdvertisingAddress {
        self.advertiser
    }
}

/// Immutable directed Host snapshot retained from Enable until radio start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyDirectedAdvertisingEnableRequest {
    parameters: LeLegacyAdvertisingParameters,
    advertiser: LeLegacyAdvertisingAddress,
}

impl LeLegacyDirectedAdvertisingEnableRequest {
    /// Exact accepted parameter snapshot, including the peer.
    pub const fn parameters(self) -> LeLegacyAdvertisingParameters {
        self.parameters
    }

    /// Whether the set advertises at high duty cycle for 1.28 s.
    pub fn is_high_duty(self) -> bool {
        self.parameters.role == LeLegacyAdvertisingRole::DirectedHighDuty
    }

    /// Address resolved at the Enable ordering boundary.
    pub const fn advertiser(self) -> LeLegacyAdvertisingAddress {
        self.advertiser
    }
}

/// Role-specific snapshot of the configuration taken by Enable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingEnableRequest {
    /// Non-connectable, non-scannable undirected advertising.
    Nonconnectable(LeLegacyNonconnectableAdvertisingEnableRequest),
    /// Scannable non-connectable undirected advertising with its scan
    /// response.
    Scannable(LeLegacyResponseCapableAdvertisingEnableRequest),
    /// Connectable undirected advertising with its scan response.
    Connectable(LeLegacyResponseCapableAdvertisingEnableRequest),
    /// Connectable directed advertising.
    Directed(LeLegacyDirectedAdvertisingEnableRequest),
}

/// Enable asked for a random own address that the Host never set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingRandomAddressMissing;

impl LeLegacyAdvertisingConfigurationCommand {
    /// Extract the software-only configuration commands from the full family.
    pub fn from_command(
        command: LeLegacyAdvertisingCommand,
    ) -> Result<Self, LeLegacyAdvertisingCommand> {
        match command {
            LeLegacyAdvertisingCommand::SetParameters(parameters) => {
                Ok(Self::SetParameters(parameters))
            }
            LeLegacyAdvertisingCommand::SetData(data) => Ok(Self::SetData(data)),
            LeLegacyAdvertisingCommand::SetScanResponseData(data) => {
                Ok(Self::SetScanResponseData(data))
            }
            command @ LeLegacyAdvertisingCommand::SetEnable(_) => Err(command),
        }
    }

    /// Exact standard HCI command identity.
    pub const fn kind(&self) -> LeLegacyAdvertisingCommandKind {
        match self {
            Self::SetParameters(_) => LeLegacyAdvertisingCommandKind::SetParameters,
            Self::SetData(_) => LeLegacyAdvertisingCommandKind::SetData,
            Self::SetScanResponseData(_) => LeLegacyAdvertisingCommandKind::SetScanResponseData,
        }
    }
}

/// Reset-scoped software configuration for one legacy advertising set.
///
/// A new value holds the standard connectable-undirected, public-address,
/// all-primary-channels, unfiltered parameter defaults. It contains only Host
/// intent and has no enabled state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingConfiguration {
    parameters: LeLegacyAdvertisingParameters,
    data: LeLegacyAdvertisingData,
    scan_response_data: LeLegacyScanResponseData,
}

impl LeLegacyAdvertisingConfiguration {
    /// The standard defaults.
    pub const fn new() -> Self {
        Self {
            parameters: LeLegacyAdvertisingParameters {
                role: LeLegacyAdvertisingRole::Connectable,
                interval: LeLegacyAdvertisingIntervalRange {
                    minimum_units_625_us: LEGACY_ADVERTISING_INTERVAL_DEFAULT,
                    maximum_units_625_us: LEGACY_ADVERTISING_INTERVAL_DEFAULT,
                },
                own_address_kind: LeLegacyAdvertisingOwnAddressKind::Public,
                channels: LeLegacyAdvertisingPrimaryChannels {
                    channel_37: true,
                    channel_38: true,
                    channel_39: true,
                },
                peer: LeLegacyAdvertisingPeer {
                    random: false,
                    address: [0; 6],
                },
            },
            data: LeLegacyAdvertisingData {
                bytes: [0; LE_LEGACY_ADVERTISING_DATA_CAPACITY],
                length: 0,
            },
            scan_response_data: LeLegacyScanResponseData {
                bytes: [0; LE_LEGACY_ADVERTISING_DATA_CAPACITY],
                length: 0,
            },
        }
    }

    /// Apply one configuration command.
    pub fn configure(&mut self, command: LeLegacyAdvertisingConfigurationCommand) {
        match command {
            LeLegacyAdvertisingConfigurationCommand::SetParameters(parameters) => {
                self.parameters = parameters;
            }
            LeLegacyAdvertisingConfigurationCommand::SetData(data) => {
                self.data = data;
            }
            LeLegacyAdvertisingConfigurationCommand::SetScanResponseData(data) => {
                self.scan_response_data = data;
            }
        }
    }

    /// Snapshot the configuration for Enable, resolving the own address.
    pub fn enable_request(
        &self,
        public_address: BluetoothPublicDeviceAddress,
        random_address: Option<BdAddr>,
    ) -> Result<LeLegacyAdvertisingEnableRequest, LeLegacyAdvertisingRandomAddressMissing> {
        let parameters = self.parameters;
        let advertiser = match parameters.own_address_kind() {
            LeLegacyAdvertisingOwnAddressKind::Public => {
                LeLegacyAdvertisingAddress::Public(public_address)
            }
            LeLegacyAdvertisingOwnAddressKind::Random => LeLegacyAdvertisingAddress::Random(
                random_address.ok_or(LeLegacyAdvertisingRandomAddressMissing)?,
            ),
        };
        Ok(match parameters.role() {
            LeLegacyAdvertisingRole::Nonconnectable => {
                LeLegacyAdvertisingEnableRequest::Nonconnectable(
                    LeLegacyNonconnectableAdvertisingEnableRequest {
                        parameters,
                        data: self.data,
                        advertiser,
                    },
                )
            }
            LeLegacyAdvertisingRole::Scannable => {
                LeLegacyAdvertisingEnableRequest::Scannable(self.response_capable(advertiser))
            }
            LeLegacyAdvertisingRole::Connectable => {
                LeLegacyAdvertisingEnableRequest::Connectable(self.response_capable(advertiser))
            }
            LeLegacyAdvertisingRole::DirectedHighDuty
            | LeLegacyAdvertisingRole::DirectedLowDuty => {
                LeLegacyAdvertisingEnableRequest::Directed(
                    LeLegacyDirectedAdvertisingEnableRequest {
                        parameters,
                        advertiser,
                    },
                )
            }
        })
    }

    fn response_capable(
        &self,
        advertiser: LeLegacyAdvertisingAddress,
    ) -> LeLegacyResponseCapableAdvertisingEnableRequest {
        LeLegacyResponseCapableAdvertisingEnableRequest {
            parameters: self.parameters,
            data: self.data,
            scan_response_data: self.scan_response_data,
            advertiser,
        }
    }

    /// Current parameters.
    pub const fn parameters(&self) -> LeLegacyAdvertisingParameters {
        self.parameters
    }

    /// Current advertising data.
    pub const fn data(&self) -> LeLegacyAdvertisingData {
        self.data
    }

    /// Current scan-response data.
    pub const fn scan_response_data(&self) -> LeLegacyScanResponseData {
        self.scan_response_data
    }
}

impl Default for LeLegacyAdvertisingConfiguration {
    fn default() -> Self {
        Self::new()
    }
}

impl LeLegacyAdvertisingCommand {
    /// Decode one command without mutating HCI, Link Layer, or hardware state.
    pub fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeLegacyAdvertisingDecodeError> {
        let Some(kind) = LeLegacyAdvertisingCommandKind::from_opcode(command.opcode()) else {
            return Err(LeLegacyAdvertisingDecodeError::UnsupportedOpcode {
                opcode: command.opcode(),
            });
        };

        match kind {
            LeLegacyAdvertisingCommandKind::SetParameters => {
                let parameters =
                    LeSetAdvParamsParams::from_hci_bytes_complete(command.parameters()).map_err(
                        |_| LeLegacyAdvertisingDecodeError::MalformedParameters { command: kind },
                    )?;
                Self::decode_parameters(parameters)
            }
            LeLegacyAdvertisingCommandKind::SetData => {
                let parameters = LeSetAdvDataParams::from_hci_bytes_complete(command.parameters())
                    .map_err(|_| LeLegacyAdvertisingDecodeError::MalformedParameters {
                        command: kind,
                    })?;
                if usize::from(parameters.data_len) > LE_LEGACY_ADVERTISING_DATA_CAPACITY {
                    return Err(LeLegacyAdvertisingDecodeError::InvalidParameters {
                        command: kind,
                    });
                }
                Ok(Self::SetData(LeLegacyAdvertisingData {
                    bytes: parameters.data,
                    length: parameters.data_len,
                }))
            }
            LeLegacyAdvertisingCommandKind::SetScanResponseData => {
                let parameters =
                    LeSetScanResponseDataParams::from_hci_bytes_complete(command.parameters())
                        .map_err(|_| LeLegacyAdvertisingDecodeError::MalformedParameters {
                            command: kind,
                        })?;
                if usize::from(parameters.data_len) > LE_LEGACY_ADVERTISING_DATA_CAPACITY {
                    return Err(LeLegacyAdvertisingDecodeError::InvalidParameters {
                        command: kind,
                    });
                }
                Ok(Self::SetScanResponseData(LeLegacyScanResponseData {
                    bytes: parameters.data,
                    length: parameters.data_len,
                }))
            }
            LeLegacyAdvertisingCommandKind::SetEnable => {
                let enable = bool::from_hci_bytes_complete(command.parameters()).map_err(|_| {
                    LeLegacyAdvertisingDecodeError::MalformedParameters { command: kind }
                })?;
                Ok(Self::SetEnable(enable))
            }
        }
    }

    fn decode_parameters(
        parameters: LeSetAdvParamsParams,
    ) -> Result<Self, LeLegacyAdvertisingDecodeError> {
        let command = LeLegacyAdvertisingCommandKind::SetParameters;
        let LeSetAdvParamsParams {
            adv_interval_min,
            adv_interval_max,
            adv_kind,
            own_addr_kind,
            peer_addr_kind,
            peer_addr,
            adv_channel_map,
            adv_filter_policy,
        } = parameters;

        let role = if adv_kind == AdvKind::AdvNonconnInd {
            LeLegacyAdvertisingRole::Nonconnectable
        } else if adv_kind == AdvKind::AdvScanInd {
            LeLegacyAdvertisingRole::Scannable
        } else if adv_kind == AdvKind::AdvInd {
            LeLegacyAdvertisingRole::Connectable
        } else if adv_kind == AdvKind::AdvDirectIndHigh {
            LeLegacyAdvertisingRole::DirectedHighDuty
        } else if adv_kind == AdvKind::AdvDirectIndLow {
            LeLegacyAdvertisingRole::DirectedLowDuty
        } else {
            return Err(LeLegacyAdvertisingDecodeError::UnsupportedFeature { command });
        };

        if adv_filter_policy != AdvFilterPolicy::Unfiltered {
            return Err(LeLegacyAdvertisingDecodeError::UnsupportedFeature { command });
        }

        let own_address_kind = if own_addr_kind == AddrKind::PUBLIC {
            LeLegacyAdvertisingOwnAddressKind::Public
        } else if own_addr_kind == AddrKind::RANDOM {
            LeLegacyAdvertisingOwnAddressKind::Random
        } else {
            return Err(LeLegacyAdvertisingDecodeError::UnsupportedFeature { command });
        };

        let peer = if peer_addr_kind == AddrKind::PUBLIC {
            LeLegacyAdvertisingPeer {
                random: false,
                address: peer_addr
                    .raw()
                    .try_into()
                    .expect("a device address has six octets"),
            }
        } else if peer_addr_kind == AddrKind::RANDOM {
            LeLegacyAdvertisingPeer {
                random: true,
                address: peer_addr
                    .raw()
                    .try_into()
                    .expect("a device address has six octets"),
            }
        } else if matches!(
            role,
            LeLegacyAdvertisingRole::DirectedHighDuty | LeLegacyAdvertisingRole::DirectedLowDuty
        ) {
            return Err(LeLegacyAdvertisingDecodeError::InvalidParameters { command });
        } else {
            LeLegacyAdvertisingPeer {
                random: false,
                address: peer_addr
                    .raw()
                    .try_into()
                    .expect("a device address has six octets"),
            }
        };

        // High duty cycle directed advertising ignores the interval.
        let (minimum_units_625_us, maximum_units_625_us) =
            if role == LeLegacyAdvertisingRole::DirectedHighDuty {
                (
                    LEGACY_ADVERTISING_INTERVAL_DEFAULT,
                    LEGACY_ADVERTISING_INTERVAL_DEFAULT,
                )
            } else {
                (adv_interval_min.as_u16(), adv_interval_max.as_u16())
            };
        if minimum_units_625_us < LEGACY_ADVERTISING_INTERVAL_MIN
            || maximum_units_625_us > LEGACY_ADVERTISING_INTERVAL_MAX
            || minimum_units_625_us > maximum_units_625_us
        {
            return Err(LeLegacyAdvertisingDecodeError::InvalidParameters { command });
        }

        let channel_bits = adv_channel_map.into_inner();
        if channel_bits == 0 || channel_bits & !0x07 != 0 {
            return Err(LeLegacyAdvertisingDecodeError::InvalidParameters { command });
        }

        Ok(Self::SetParameters(LeLegacyAdvertisingParameters {
            role,
            interval: LeLegacyAdvertisingIntervalRange {
                minimum_units_625_us,
                maximum_units_625_us,
            },
            own_address_kind,
            channels: LeLegacyAdvertisingPrimaryChannels {
                channel_37: adv_channel_map.is_channel_37_enabled(),
                channel_38: adv_channel_map.is_channel_38_enabled(),
                channel_39: adv_channel_map.is_channel_39_enabled(),
            },
            peer,
        }))
    }

    /// Exact command identity retained by this semantic token.
    pub const fn kind(&self) -> LeLegacyAdvertisingCommandKind {
        match self {
            Self::SetParameters(_) => LeLegacyAdvertisingCommandKind::SetParameters,
            Self::SetData(_) => LeLegacyAdvertisingCommandKind::SetData,
            Self::SetScanResponseData(_) => LeLegacyAdvertisingCommandKind::SetScanResponseData,
            Self::SetEnable(_) => LeLegacyAdvertisingCommandKind::SetEnable,
        }
    }
}

/// Why a packet could not become an owned semantic advertising command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingDecodeError {
    UnsupportedOpcode {
        opcode: Opcode,
    },
    MalformedParameters {
        command: LeLegacyAdvertisingCommandKind,
    },
    InvalidParameters {
        command: LeLegacyAdvertisingCommandKind,
    },
    UnsupportedFeature {
        command: LeLegacyAdvertisingCommandKind,
    },
}

impl LeLegacyAdvertisingDecodeError {
    /// Convert a rejection for a known opcode into the required completion.
    ///
    /// An unclaimed opcode remains available to the next command family.
    pub fn into_command_complete(self) -> Result<LeLegacyAdvertisingCommandCompleteEvent, Self> {
        let (command, status) = match self {
            Self::UnsupportedOpcode { .. } => return Err(self),
            Self::MalformedParameters { command } | Self::InvalidParameters { command } => {
                (command, HciError::INVALID_HCI_PARAMETERS.to_status())
            }
            Self::UnsupportedFeature { command } => (command, HciError::UNSUPPORTED.to_status()),
        };
        Ok(LeLegacyAdvertisingCommandCompleteEvent::new(
            command.opcode(),
            status,
        ))
    }
}

/// Owned Command Complete for one legacy advertising command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingCommandCompleteEvent {
    bytes: [u8; LE_LEGACY_ADVERTISING_COMMAND_COMPLETE_EVENT_CAPACITY],
    opcode: Opcode,
    status: Status,
}

impl LeLegacyAdvertisingCommandCompleteEvent {
    /// Command Complete for `opcode` with `status` and no return parameters.
    pub fn new(opcode: Opcode, status: Status) -> Self {
        let opcode_bytes = opcode.to_raw().to_le_bytes();
        Self {
            bytes: [
                0x0e,
                0x04,
                0x01,
                opcode_bytes[0],
                opcode_bytes[1],
                status.into_inner(),
            ],
            opcode,
            status,
        }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Exact consumed command opcode.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Result selected by the semantic/controller boundary.
    pub const fn status(&self) -> Status {
        self.status
    }
}

impl HciControllerResponse for LeLegacyAdvertisingCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(test)]
mod tests;
