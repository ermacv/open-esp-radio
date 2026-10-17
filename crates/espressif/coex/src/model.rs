use oer_radio_coex::RadioClient;

use crate::CoexEventId;

pub const COEX_TIMER_COUNT: usize = 5;

// Complete `coex_core_timer_idx_get` switch image of esp-coex-lib
// c758e7b56e0fa22177a0539796e1df59978dc322 (`esp32s31/libcoexist.a` sha256
// 13b1e1d2a1550400ddb2622648933288aee6a285d3aad454978314c4af685147,
// `coexist_core.o` `.rodata.CSWTCH.31`). Element zero corresponds to event
// one; 0xff means that the event has no hardware timer. Event 48 selects
// timer 5, which the radio arbiter reserves for the PHY grant-protect request.
const REVIEWED_TIMER_MAP: [u8; 48] = [
    0x00, 0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0x04, 0xff, 0x02, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x03, 0x03, 0xff, 0x05,
];

// Complete `g_coex_param` initializer. `coex_core_event_duration_get` maps
// these entries to events 4, 7, 9, 45 and 46 respectively.
const REVIEWED_EVENT_DURATIONS: [u32; 5] = [25_000, 20_000, 5_000, 25_000, 50_000];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexError {
    InvalidEvent,
    InvalidPti,
    InvalidTimer,
    UnsupportedClock,
    Disabled,
    /// A failed timer mutation must be retired through release/disable first.
    RecoveryRequired,
    Hardware,
}

/// The policy timer a vendor core request for `event` programs, or `None`
/// for an event without a timer and for the arbiter's grant-protect event 48.
pub const fn timer_index(event: CoexEventId) -> Option<CoexTimerIndex> {
    let value = event.value();
    if value == 0 || value as usize > REVIEWED_TIMER_MAP.len() {
        return None;
    }
    match REVIEWED_TIMER_MAP[(value - 1) as usize] {
        0 => Some(CoexTimerIndex::Timer0),
        1 => Some(CoexTimerIndex::Timer1),
        2 => Some(CoexTimerIndex::Timer2),
        3 => Some(CoexTimerIndex::Timer3),
        4 => Some(CoexTimerIndex::Timer4),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexEventDurations([u32; 5]);

impl CoexEventDurations {
    pub const fn reviewed_vendor() -> Self {
        Self(REVIEWED_EVENT_DURATIONS)
    }

    pub const fn duration(self, event: CoexEventId) -> Option<u32> {
        let index = match event.value() {
            4 => 0,
            7 => 1,
            9 => 2,
            45 => 3,
            46 => 4,
            _ => return None,
        };
        Some(self.0[index])
    }

    pub const fn as_words(&self) -> &[u32; 5] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CoexTimerIndex {
    Timer0 = 0,
    Timer1 = 1,
    Timer2 = 2,
    Timer3 = 3,
    Timer4 = 4,
}

impl CoexTimerIndex {
    pub const ALL: [Self; COEX_TIMER_COUNT] = [
        Self::Timer0,
        Self::Timer1,
        Self::Timer2,
        Self::Timer3,
        Self::Timer4,
    ];

    pub const fn new(value: u8) -> Result<Self, CoexError> {
        match value {
            0 => Ok(Self::Timer0),
            1 => Ok(Self::Timer1),
            2 => Ok(Self::Timer2),
            3 => Ok(Self::Timer3),
            4 => Ok(Self::Timer4),
            _ => Err(CoexError::InvalidTimer),
        }
    }

    pub const fn value(self) -> u8 {
        self as u8
    }
}

/// The requesting radio; the discriminant is its `coex_core_request`
/// request kind (`coex_bt_request` passes 0, `coex_wifi_request` 1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CoexClient {
    Bluetooth = 0,
    Wifi = 1,
}

// Complete `coex_core_request` request-kind switch image of esp-coex-lib
// c758e7b56e0fa22177a0539796e1df59978dc322 (`esp32s31/libcoexist.a` sha256
// 13b1e1d2a1550400ddb2622648933288aee6a285d3aad454978314c4af685147,
// `coexist_core.o` `.rodata.CSWTCH.27`). Element `k` is the timer client
// field value `coex_hw_timer_set` receives for request kind `k`; request kind
// 2 is the PHY grant-protect request the radio arbiter programs itself.
const REVIEWED_TIMER_CLIENT_MAP: [u8; 5] = [0x02, 0x01, 0x00, 0x03, 0x04];

impl CoexClient {
    /// The timer client field value `coex_core_request` programs for this
    /// radio's requests.
    pub const fn timer_client_value(self) -> u8 {
        REVIEWED_TIMER_CLIENT_MAP[self as usize]
    }

    /// The request kind of a portable client, or `None` for IEEE 802.15.4,
    /// which publishes its priorities to its MAC and requests no timer.
    pub const fn from_radio(client: RadioClient) -> Option<Self> {
        match client {
            RadioClient::Wifi => Some(Self::Wifi),
            RadioClient::Bluetooth => Some(Self::Bluetooth),
            RadioClient::Ieee802154 => None,
        }
    }

    /// The portable client of this request kind.
    pub const fn radio(self) -> RadioClient {
        match self {
            Self::Wifi => RadioClient::Wifi,
            Self::Bluetooth => RadioClient::Bluetooth,
        }
    }
}

/// A portable client without a timer request kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NoRequestKind(pub RadioClient);

impl TryFrom<RadioClient> for CoexClient {
    type Error = NoRequestKind;

    fn try_from(client: RadioClient) -> Result<Self, Self::Error> {
        Self::from_radio(client).ok_or(NoRequestKind(client))
    }
}

impl From<CoexClient> for RadioClient {
    fn from(client: CoexClient) -> Self {
        client.radio()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexClientRequest {
    pub event: CoexEventId,
    /// Source latency parameter converted into the secondary timer target.
    /// Programming this value does not establish a guaranteed RF grant time.
    pub latency: u32,
    /// Source duration parameter converted into the primary timer target.
    /// This is not proof of a granted or non-preemptible RF interval.
    pub duration: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CoexRequest {
    pub(crate) client: CoexClient,
    pub(crate) request: CoexClientRequest,
}
