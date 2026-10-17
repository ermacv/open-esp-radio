//! The coexistence time-slice schedule, as `coexist_scheme.o` implements it.
//!
//! Every radio publishes status bits. The status words select one recovered
//! [`CoexScheme`]; its phases divide a period between the radios, and each
//! phase notifies the radios that own it. The schedule itself programs no
//! priority: a notified radio requests its own coexistence events for the
//! slice, and the hardware arbitrates by priority.
//!
//! This module is the executor-neutral state of `coex_schm_env` of esp-coex-lib
//! c758e7b56e0fa22177a0539796e1df59978dc322 (`esp32s31/libcoexist.a` sha256
//! 13b1e1d2a1550400ddb2622648933288aee6a285d3aad454978314c4af685147). A
//! runtime owner arms the phase timer each [`CoexPhaseStep`] requests and
//! delivers its notifications; the vendor does both from its timer task and
//! under its schedule lock.

mod schemes;

pub use schemes::CoexSchemeId;

/// One phase of a scheme: its share of the period and the radios it
/// notifies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPhase {
    share_percent: u8,
    wifi: u8,
    bluetooth: [u8; 2],
}

impl CoexPhase {
    pub(crate) const fn new(share_percent: u8, wifi: u8, bluetooth: [u8; 2]) -> Self {
        Self {
            share_percent,
            wifi,
            bluetooth,
        }
    }

    /// The phase's share of the period, in percent.
    pub const fn share_percent(self) -> u8 {
        self.share_percent
    }

    /// The Wi-Fi flags: nonzero notifies Wi-Fi. Wi-Fi's own phase handler
    /// interprets the bits.
    pub const fn wifi(self) -> u8 {
        self.wifi
    }

    /// The two Bluetooth flag bytes: either nonzero notifies Bluetooth. The
    /// Bluetooth controller interprets them.
    pub const fn bluetooth(self) -> [u8; 2] {
        self.bluetooth
    }

    const fn notifies_wifi(self) -> bool {
        self.wifi != 0
    }

    const fn notifies_bluetooth(self) -> bool {
        self.bluetooth[0] != 0 || self.bluetooth[1] != 0
    }
}

/// One recovered scheme: its period multiplier and its phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexScheme {
    period: u8,
    phases: &'static [CoexPhase],
}

impl CoexScheme {
    pub(crate) const fn new(period: u8, phases: &'static [CoexPhase]) -> Self {
        Self { period, phases }
    }

    /// The period multiplier of the schedule interval.
    pub const fn period(&self) -> u8 {
        self.period
    }

    pub const fn phases(&self) -> &'static [CoexPhase] {
        self.phases
    }

    const fn last_phase(&self) -> u8 {
        self.phases.len() as u8 - 1
    }
}

/// The radio a status word belongs to, as `coex_schm_st_type_t`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexStatusType {
    Wifi,
    Ble,
    Bt,
    ExternalCoex,
    Ieee802154,
}

impl CoexStatusType {
    /// Every radio, in the bit order of `coex_schm_status_bitmap_get`.
    pub const ALL: [Self; 5] = [
        Self::Wifi,
        Self::Ble,
        Self::Bt,
        Self::ExternalCoex,
        Self::Ieee802154,
    ];

    /// The status word a portable client publishes: Bluetooth is the LE
    /// word, as no supported client runs Bluetooth Classic.
    pub const fn from_radio(client: oer_radio_coex::RadioClient) -> Self {
        match client {
            oer_radio_coex::RadioClient::Wifi => Self::Wifi,
            oer_radio_coex::RadioClient::Bluetooth => Self::Ble,
            oer_radio_coex::RadioClient::Ieee802154 => Self::Ieee802154,
        }
    }
}

impl From<oer_radio_coex::RadioClient> for CoexStatusType {
    fn from(client: oer_radio_coex::RadioClient) -> Self {
        Self::from_radio(client)
    }
}

/// Wi-Fi status bits as the vendor Wi-Fi library publishes them. Only the
/// bits the scheme selection and the phase gating test are named.
pub mod wifi_status {
    /// Scanning.
    pub const SCAN: u16 = 0x01;
    /// Authenticating, associating or completing the key handshake.
    pub const CONNECTING: u16 = 0x02;
    /// Connected.
    pub const CONNECTED: u16 = 0x04;
    /// Only a connectionless wake window is registered.
    pub const CONNECTIONLESS: u16 = 0x40;
}

/// The external-coexistence status bits `ic_set_extern_coex` sets and
/// `ic_stop_extern_coex` clears (`coex_schm_status_bit_set(3, 1 | 2)`), on
/// [`CoexStatusType::ExternalCoex`].
pub const EXTERNAL_COEX_STATUS: u16 = 0x01 | 0x02;

/// The IEEE 802.15.4 status bit of the pinned libcoexist
/// `esp_coex_ieee802154_status_enable` and `_disable`
/// (`coex_schm_status_bit_set/clear(4, 1)`), on
/// [`CoexStatusType::Ieee802154`].
pub const IEEE802154_COEX_STATUS: u16 = 0x01;

/// The five status words of `coex_schm_env`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CoexStatusWords {
    pub wifi: u16,
    pub ble: u16,
    pub bt: u16,
    pub external_coex: u16,
    pub ieee802154: u16,
}

impl CoexStatusWords {
    const fn word(&self, kind: CoexStatusType) -> u16 {
        match kind {
            CoexStatusType::Wifi => self.wifi,
            CoexStatusType::Ble => self.ble,
            CoexStatusType::Bt => self.bt,
            CoexStatusType::ExternalCoex => self.external_coex,
            CoexStatusType::Ieee802154 => self.ieee802154,
        }
    }

    fn word_mut(&mut self, kind: CoexStatusType) -> &mut u16 {
        match kind {
            CoexStatusType::Wifi => &mut self.wifi,
            CoexStatusType::Ble => &mut self.ble,
            CoexStatusType::Bt => &mut self.bt,
            CoexStatusType::ExternalCoex => &mut self.external_coex,
            CoexStatusType::Ieee802154 => &mut self.ieee802154,
        }
    }

    /// Whether a radio other than `radio` publishes status: the
    /// `coex_schm_status_bitmap_get` test of `coex_status_get` with the
    /// radio's own bit masked out.
    pub const fn others_publish(&self, radio: CoexStatusType) -> bool {
        let mut index = 0;
        while index < CoexStatusType::ALL.len() {
            let other = CoexStatusType::ALL[index];
            if other as u8 != radio as u8 && self.word(other) != 0 {
                return true;
            }
            index += 1;
        }
        false
    }

    /// `coex_schm_is_loop_allowed`: the phases loop only while at least two
    /// of Wi-Fi, Bluetooth (BLE or classic), external coexistence and
    /// IEEE 802.15.4 publish status.
    pub const fn loop_allowed(&self) -> bool {
        let groups = (self.wifi != 0) as u8
            + (self.ble != 0 || self.bt != 0) as u8
            + (self.external_coex != 0) as u8
            + (self.ieee802154 != 0) as u8;
        groups >= 2
    }

    /// Whether Wi-Fi scans, connects or only keeps a connectionless window:
    /// the states whose phases loop on the schedule's own timer. A connected
    /// Wi-Fi restarts the phases itself at each beacon.
    const fn wifi_loops_on_timer(&self) -> bool {
        matches!(
            self.wifi,
            wifi_status::SCAN | wifi_status::CONNECTING | wifi_status::CONNECTIONLESS
        )
    }

    /// `coex_schm_status_change`: the scheme these status words select.
    pub const fn select(&self) -> CoexSchemeId {
        use CoexSchemeId as S;
        if self.external_coex & 0x01 != 0 {
            return if self.wifi & wifi_status::SCAN != 0 {
                S::ExternalCoexWifiScan
            } else if self.wifi & wifi_status::CONNECTING != 0 {
                S::ExternalCoexWifiConnecting
            } else if self.external_coex & 0x02 != 0 {
                S::ExternalCoexWifiDefaultRxonly
            } else {
                S::ExternalCoexWifiDefault
            };
        }
        let Some(wifi) = WifiState::of(self.wifi) else {
            return S::AllDefault;
        };
        if self.ble != 0 {
            if self.ble & 0x08 != 0 {
                return mesh(Mesh::Config, wifi, self.bt);
            }
            if self.ble & 0x10 != 0 || self.ble & 0x3a == 0x02 {
                return mesh(Mesh::Traffic, wifi, self.bt);
            }
            if self.ble & 0x20 != 0 {
                return mesh(Mesh::Standby, wifi, self.bt);
            }
            return ble_default(wifi, self.bt);
        }
        if self.ieee802154 != 0 {
            return ble_default(wifi, self.bt);
        }
        bt_only(wifi, self.bt)
    }
}

/// The Wi-Fi state the selection distinguishes, in its test order.
#[derive(Clone, Copy)]
enum WifiState {
    Scan,
    Connecting,
    Connected,
}

impl WifiState {
    const fn of(wifi: u16) -> Option<Self> {
        if wifi & wifi_status::SCAN != 0 {
            Some(Self::Scan)
        } else if wifi & wifi_status::CONNECTING != 0 {
            Some(Self::Connecting)
        } else if wifi & wifi_status::CONNECTED != 0 {
            Some(Self::Connected)
        } else {
            None
        }
    }

    const fn pick(
        self,
        scan: CoexSchemeId,
        connecting: CoexSchemeId,
        connected: CoexSchemeId,
    ) -> CoexSchemeId {
        match self {
            Self::Scan => scan,
            Self::Connecting => connecting,
            Self::Connected => connected,
        }
    }
}

#[derive(Clone, Copy)]
enum Mesh {
    Config,
    Traffic,
    Standby,
}

/// Classic Bluetooth states, in the order the BLE mesh selections test them.
#[derive(Clone, Copy)]
enum MeshBt {
    None,
    A2dpPaused,
    A2dp,
    SniffSco,
    Connected,
    PageInquiryScan,
    Default,
}

impl MeshBt {
    const fn of(bt: u16) -> Self {
        if bt == 0 {
            Self::None
        } else if bt & 0x20 != 0 {
            Self::A2dpPaused
        } else if bt & 0x10 != 0 {
            Self::A2dp
        } else if bt & 0x88 != 0 {
            Self::SniffSco
        } else if bt & 0x04 != 0 {
            Self::Connected
        } else if bt & 0x01 != 0 {
            Self::PageInquiryScan
        } else {
            Self::Default
        }
    }
}

const fn mesh(kind: Mesh, wifi: WifiState, bt: u16) -> CoexSchemeId {
    use CoexSchemeId as S;
    use MeshBt as B;
    macro_rules! pick {
        ($($bt:ident => [$scan:ident, $connecting:ident, $connected:ident]),* $(,)?) => {
            match MeshBt::of(bt) {
                $(B::$bt => wifi.pick(S::$scan, S::$connecting, S::$connected),)*
            }
        };
    }
    match kind {
        Mesh::Config => pick! {
            None => [BleMeshConfigWifiScan, BleMeshConfigWifiConnecting, BleMeshConfigWifiConn],
            A2dpPaused => [BleMeshConfigBtA2dpPausedWifiScan, BleMeshConfigBtA2dpPausedWifiConnecting, BleMeshConfigBtA2dpPausedWifiConn],
            A2dp => [BleMeshConfigBtA2dpWifiScan, BleMeshConfigBtA2dpWifiConnecting, BleMeshConfigBtA2dpWifiConn],
            SniffSco => [BleMeshConfigBtSniffScoWifiScan, BleMeshConfigBtSniffScoWifiConnecting, BleMeshConfigBtSniffScoWifiConn],
            Connected => [BleMeshConfigBtConnWifiScan, BleMeshConfigBtConnWifiConnecting, BleMeshConfigBtConnWifiConn],
            PageInquiryScan => [BleMeshConfigBtPiscanWifiScan, BleMeshConfigBtPiscanWifiConnecting, BleMeshConfigBtPiscanWifiConn],
            Default => [BleMeshConfigBtDefaultWifiScan, BleMeshConfigBtDefaultWifiConnecting, BleMeshConfigBtDefaultWifiConn],
        },
        Mesh::Traffic => pick! {
            None => [BleMeshTrafficWifiScan, BleMeshTrafficWifiConnecting, BleMeshTrafficWifiConn],
            A2dpPaused => [BleMeshTrafficBtA2dpPausedWifiScan, BleMeshTrafficBtA2dpPausedWifiConnecting, BleMeshTrafficBtA2dpPausedWifiConn],
            A2dp => [BleMeshTrafficBtA2dpWifiScan, BleMeshTrafficBtA2dpWifiConnecting, BleMeshTrafficBtA2dpWifiConn],
            SniffSco => [BleMeshTrafficBtSniffScoWifiScan, BleMeshTrafficBtSniffScoWifiConnecting, BleMeshTrafficBtSniffScoWifiConn],
            Connected => [BleMeshTrafficBtConnWifiScan, BleMeshTrafficBtConnWifiConnecting, BleMeshTrafficBtConnWifiConn],
            PageInquiryScan => [BleMeshTrafficBtPiscanWifiScan, BleMeshTrafficBtPiscanWifiConnecting, BleMeshTrafficBtPiscanWifiConn],
            Default => [BleMeshTrafficBtDefaultWifiScan, BleMeshTrafficBtDefaultWifiConnecting, BleMeshTrafficBtDefaultWifiConn],
        },
        Mesh::Standby => pick! {
            None => [BleMeshStandbyWifiScan, BleMeshStandbyWifiConnecting, BleMeshStandbyWifiConn],
            A2dpPaused => [BleMeshStandbyBtA2dpPausedWifiScan, BleMeshStandbyBtA2dpPausedWifiConnecting, BleMeshStandbyBtA2dpPausedWifiConn],
            A2dp => [BleMeshStandbyBtA2dpWifiScan, BleMeshStandbyBtA2dpWifiConnecting, BleMeshStandbyBtA2dpWifiConn],
            SniffSco => [BleMeshStandbyBtSniffScoWifiScan, BleMeshStandbyBtSniffScoWifiConnecting, BleMeshStandbyBtSniffScoWifiConn],
            Connected => [BleMeshStandbyBtConnWifiScan, BleMeshStandbyBtConnWifiConnecting, BleMeshStandbyBtConnWifiConn],
            PageInquiryScan => [BleMeshStandbyBtPiscanWifiScan, BleMeshStandbyBtPiscanWifiConnecting, BleMeshStandbyBtPiscanWifiConn],
            Default => [BleMeshStandbyBtDefaultWifiScan, BleMeshStandbyBtDefaultWifiConnecting, BleMeshStandbyBtDefaultWifiConn],
        },
    }
}

/// BLE without a mesh state, or IEEE 802.15.4, beside classic Bluetooth.
const fn ble_default(wifi: WifiState, bt: u16) -> CoexSchemeId {
    use CoexSchemeId as S;
    if bt == 0 {
        wifi.pick(
            S::BleDefaultBtIdleWifiScan,
            S::BleDefaultBtIdleWifiConnecting,
            S::BleDefaultBtIdleWifiConn,
        )
    } else if bt & 0x10 != 0 {
        wifi.pick(
            S::BleDefaultBtA2dpWifiScan,
            S::BleDefaultBtA2dpWifiConnecting,
            S::BleDefaultBtA2dpWifiConn,
        )
    } else {
        wifi.pick(
            S::BleDefaultBtDefaultWifiScan,
            S::BleDefaultBtDefaultWifiConnecting,
            S::BleDefaultBtDefaultWifiConn,
        )
    }
}

/// Classic Bluetooth alone beside Wi-Fi. A connecting Wi-Fi tests the A2DP
/// states before paging and inquiry; scanning and connected Wi-Fi test them
/// after.
const fn bt_only(wifi: WifiState, bt: u16) -> CoexSchemeId {
    use CoexSchemeId as S;
    macro_rules! pick {
        ($scan:ident, $connecting:ident, $connected:ident) => {
            wifi.pick(S::$scan, S::$connecting, S::$connected)
        };
    }
    if bt == 0 {
        return pick!(BtIdleWifiScan, BtIdleWifiConnecting, BtIdleWifiConn);
    }
    let a2dp_first = matches!(wifi, WifiState::Connecting);
    if a2dp_first && bt & 0x20 != 0 {
        pick!(
            BtA2dpPausedWifiScan,
            BtA2dpPausedWifiConnecting,
            BtA2dpPausedWifiConn
        )
    } else if a2dp_first && bt & 0x10 != 0 {
        pick!(BtA2dpWifiScan, BtA2dpWifiConnecting, BtA2dpWifiConn)
    } else if bt & 0x40 != 0 {
        pick!(BtPageWifiScan, BtPageWifiConnecting, BtPageWifiConn)
    } else if bt & 0x02 != 0 {
        pick!(BtInqWifiScan, BtInqWifiConnecting, BtInqWifiConn)
    } else if bt & 0x20 != 0 {
        pick!(
            BtA2dpPausedWifiScan,
            BtA2dpPausedWifiConnecting,
            BtA2dpPausedWifiConn
        )
    } else if bt & 0x10 != 0 {
        pick!(BtA2dpWifiScan, BtA2dpWifiConnecting, BtA2dpWifiConn)
    } else if bt & 0x88 != 0 {
        pick!(
            BtSniffScoWifiScan,
            BtSniffScoWifiConnecting,
            BtSniffScoWifiConn
        )
    } else if bt & 0x04 != 0 {
        pick!(BtConnWifiScan, BtConnWifiConnecting, BtConnWifiConn)
    } else if bt & 0x01 != 0 {
        pick!(BtPiscanWifiScan, BtPiscanWifiConnecting, BtPiscanWifiConn)
    } else {
        pick!(
            BtDefaultWifiScan,
            BtDefaultWifiConnecting,
            BtDefaultWifiConn
        )
    }
}

/// What one phase change requires of the runtime owner, which the vendor
/// performs under its schedule lock and then after it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPhaseStep {
    /// The phase now current.
    pub phase: CoexPhase,
    /// Re-arm the phase timer for this many microseconds; `None` leaves it
    /// disarmed until the next restart. The timer was disarmed in every case.
    pub timer_micros: Option<u32>,
    /// Notify Wi-Fi's phase handler.
    pub notify_wifi: bool,
    /// Notify the Bluetooth phase handler, after Wi-Fi.
    pub notify_bluetooth: bool,
}

/// Why a status change or a timeout took no phase step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexScheduleIdle {
    /// The last phase stays current: the phases do not loop in this state.
    LastPhase,
}

/// The state of `coex_schm_env` that the schedule owns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexSchedule {
    status: CoexStatusWords,
    scheme: CoexSchemeId,
    phase_index: u8,
    interval: u32,
    flexible_period: u8,
}

impl CoexSchedule {
    /// `coex_schm_init`: no status, the all-default scheme, phase 0.
    pub const fn new() -> Self {
        Self {
            status: CoexStatusWords {
                wifi: 0,
                ble: 0,
                bt: 0,
                external_coex: 0,
                ieee802154: 0,
            },
            scheme: CoexSchemeId::AllDefault,
            phase_index: 0,
            interval: 0,
            flexible_period: 0,
        }
    }

    /// A schedule in an arbitrary state, for comparison with the vendor
    /// state it mirrors. Production reaches every state through the
    /// transitions only.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn for_validation(
        status: CoexStatusWords,
        scheme: CoexSchemeId,
        phase_index: u8,
        interval: u32,
        flexible_period: u8,
    ) -> Self {
        Self {
            status,
            scheme,
            phase_index,
            interval,
            flexible_period,
        }
    }

    pub const fn status(&self) -> CoexStatusWords {
        self.status
    }

    /// `coex_schm_status_get` of one radio.
    pub const fn status_of(&self, kind: CoexStatusType) -> u16 {
        self.status.word(kind)
    }

    pub const fn scheme(&self) -> CoexSchemeId {
        self.scheme
    }

    /// `coex_schm_curr_phase_idx_get`.
    pub const fn phase_index(&self) -> u8 {
        self.phase_index
    }

    /// `coex_schm_curr_phase_get`: `None` when the index lies beyond a
    /// newly selected scheme.
    pub fn current_phase(&self) -> Option<CoexPhase> {
        self.scheme
            .scheme()
            .phases
            .get(usize::from(self.phase_index))
            .copied()
    }

    /// `coex_schm_get_phase_by_idx`.
    pub fn phase_by_index(&self, index: u8) -> Option<CoexPhase> {
        self.scheme.scheme().phases.get(usize::from(index)).copied()
    }

    /// `coex_schm_curr_period_get`.
    pub const fn current_period(&self) -> u8 {
        self.scheme.scheme().period
    }

    /// `coex_schm_interval_get`; Wi-Fi sets it in units of 100 microseconds
    /// of its beacon interval.
    pub const fn interval(&self) -> u32 {
        self.interval
    }

    /// `coex_schm_interval_set`. It takes effect at the next phase change.
    pub fn set_interval(&mut self, interval: u32) {
        self.interval = interval;
    }

    /// `coex_schm_flexible_period_get`.
    pub const fn flexible_period(&self) -> u8 {
        self.flexible_period
    }

    /// `coex_schm_flexible_period_set`. The schedule keeps the value for
    /// its readers; no phase duration uses it.
    pub fn set_flexible_period(&mut self, period: u8) {
        self.flexible_period = period;
    }

    /// `coex_schm_status_bit_set`: publish status bits and reselect the
    /// scheme. When this change first lets the phases loop while Wi-Fi scans,
    /// connects or keeps only a connectionless window, the phases restart;
    /// the returned step is that restart.
    pub fn set_status_bits(&mut self, kind: CoexStatusType, bits: u16) -> Option<CoexPhaseStep> {
        let looped = self.status.loop_allowed();
        let word = self.status.word_mut(kind);
        if !*word & bits == 0 {
            return None;
        }
        *word |= bits;
        self.scheme = self.status.select();
        if !looped && self.status.loop_allowed() && self.status.wifi_loops_on_timer() {
            return self.change_phase(true).ok();
        }
        None
    }

    /// `coex_schm_status_bit_clear`: withdraw status bits and reselect the
    /// scheme. The phase index and the timer stay as they are.
    pub fn clear_status_bits(&mut self, kind: CoexStatusType, bits: u16) {
        let word = self.status.word_mut(kind);
        if *word & bits == 0 {
            return;
        }
        *word &= !bits;
        self.scheme = self.status.select();
    }

    /// `coex_schm_process_restart`: begin the phases again at phase 0.
    ///
    /// # Errors
    ///
    /// Never: a restart always takes a step. The result type matches
    /// [`Self::timeout`].
    pub fn restart(&mut self) -> Result<CoexPhaseStep, CoexScheduleIdle> {
        self.change_phase(true)
    }

    /// `coex_schm_timeout_process`: the phase timer expired.
    ///
    /// # Errors
    ///
    /// The last phase stays current because the phases do not loop in this
    /// state; the timer stays disarmed and nobody is notified.
    pub fn timeout(&mut self) -> Result<CoexPhaseStep, CoexScheduleIdle> {
        self.change_phase(false)
    }

    /// `coex_schm_change_phase`.
    fn change_phase(&mut self, restart: bool) -> Result<CoexPhaseStep, CoexScheduleIdle> {
        let scheme = self.scheme.scheme();
        let last = scheme.last_phase();
        let mut restart = restart;
        if !restart && self.phase_index >= last {
            if !self.status.wifi_loops_on_timer() || !self.status.loop_allowed() {
                return Err(CoexScheduleIdle::LastPhase);
            }
            restart = true;
        }
        let index = if restart { 0 } else { self.phase_index + 1 };
        self.phase_index = index;
        let phase = scheme.phases[usize::from(index)];
        let arm =
            index != last || (self.status.wifi_loops_on_timer() && self.status.loop_allowed());
        let timer_micros = arm.then(|| {
            u32::from(scheme.period)
                .wrapping_mul(self.interval)
                .wrapping_mul(u32::from(phase.share_percent))
        });
        Ok(CoexPhaseStep {
            phase,
            timer_micros,
            notify_wifi: phase.notifies_wifi(),
            notify_bluetooth: phase.notifies_bluetooth(),
        })
    }
}

impl Default for CoexSchedule {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
