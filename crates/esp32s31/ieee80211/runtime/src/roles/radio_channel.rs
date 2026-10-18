//! Wi-Fi channel transactions leased from the shared radio.
//!
//! Every retune takes the arbiter lease for exactly one transaction, as the
//! vendor `phy_lock` scope does, so another radio client can use the shared
//! domain between the channels of one scan or hop sequence. A completed
//! retune records the channel for coexistence, as the vendor
//! `ieee80211_update_channel` calls `coex_wifi_channel_set`.

use core::sync::atomic::{AtomicBool, Ordering};

use oer_esp32s31_coex::{CoexClientRequest, CoexEventDurations, CoexEventId, CoexStatusType};
use oer_esp32s31_hal::{
    ieee80211::arena::RadioAccess, owner::RadioRuntimeOwner, shared_radio::PlatformClockProvider,
};
use oer_esp32s31_ieee80211::coex::{WifiCoexActivity, WifiCoexChannel, shared_scan_dwell_millis};
use oer_esp32s31_ieee80211_sta::connection_coex::{
    ConnectionFrame, ConnectionFrameCoex, ReconnectFramePriority,
};
use oer_esp32s31_phy::{ConcurrentWifiChannelError, PhyTargetObserver};
use oer_esp32s31_radio_runtime::{CoexWifiChannel, RadioGuard, RadioSystem};

use oer_esp32s31_ieee80211_sta::hardware::channel::ScanPhy;

/// A Wi-Fi role's channel authority on the shared radio.
/// `D` is the shared radio's timer, which the retunes wait on.
pub struct RadioChannel<'radio, P, C, O, D> {
    radio: &'radio RadioSystem<P, C, D>,
    phy: ScanPhy<O, &'radio D>,
}

impl<'radio, P, C, O, D> RadioChannel<'radio, P, C, O, D>
where
    C: PlatformClockProvider,
    O: PhyTargetObserver,
    D: oer_time::Timer,
{
    pub const fn new(radio: &'radio RadioSystem<P, C, D>, observer: O) -> Self {
        Self {
            radio,
            phy: ScanPhy::new(observer, radio.timer()),
        }
    }

    /// Retune while the caller still owns a cold, stopped MAC.
    pub async fn select_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        owner: &mut RadioRuntimeOwner,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        self.phy
            .select_channel(lease, platform, channel_or_frequency, cbw, owner)
            .await?;
        record_coex_channel(&mut guard, channel_or_frequency, cbw);
        Ok(())
    }

    /// Stop the MAC, retune and restore the qualified REGDMA link.
    // CAPABILITY: channel-selection-switch
    pub async fn switch_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        owner: &mut RadioRuntimeOwner,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        self.phy
            .switch_channel(lease, platform, channel_or_frequency, cbw, owner)
            .await?;
        record_coex_channel(&mut guard, channel_or_frequency, cbw);
        Ok(())
    }

    /// Switch through the arena's serialized channel-only capability.
    pub async fn switch_published_channel(
        &mut self,
        channel_or_frequency: u16,
        cbw: u8,
        access: RadioAccess<'_>,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        self.phy
            .switch_published_channel(lease, platform, channel_or_frequency, cbw, access)
            .await?;
        record_coex_channel(&mut guard, channel_or_frequency, cbw);
        Ok(())
    }

    /// Publish Wi-Fi's activity to the coexistence schedule, as the vendor
    /// `pm_on_coex_schm_status_config` does, and restart the phases after it
    /// while another radio shares the schedule, as its callers do.
    pub async fn publish_coex_activity(&self, activity: WifiCoexActivity) {
        publish_coex_activity(&mut self.radio.lock().await, activity);
    }

    /// Enter one scan channel for coexistence, as the vendor
    /// `scan_next_channel` does before it dwells: publish the scanning
    /// activity and, while another radio shares the schedule, replace the
    /// requested dwell with the schedule's scan dwell.
    pub async fn enter_scan_channel(&self, requested_dwell_millis: u16) -> u16 {
        let mut guard = self.radio.lock().await;
        publish_coex_activity(&mut guard, WifiCoexActivity::Scanning);
        if !guard.coex_active_for(CoexStatusType::Wifi) {
            return requested_dwell_millis;
        }
        let dwell = shared_scan_dwell_millis(
            u32::from(requested_dwell_millis),
            guard.coex_schedule().current_period(),
        );
        u16::try_from(dwell).unwrap_or(u16::MAX)
    }

    /// The per-frame coexistence requests of this Wi-Fi's connection frames.
    pub const fn connection_coex(&self) -> RadioConnectionCoex<'radio, P, C, D> {
        RadioConnectionCoex { radio: self.radio }
    }

    /// Return the observer once the role no longer retunes.
    pub fn into_observer(self) -> O {
        self.phy.into_observer()
    }
}

/// Record a completed retune for coexistence. Only a channel the PHY
/// accepted reaches this point, so an unrepresentable request is a caller
/// defect and records nothing.
fn record_coex_channel<P, C: PlatformClockProvider, T: oer_time::Timer>(
    guard: &mut RadioGuard<'_, P, C, T>,
    channel_or_frequency: u16,
    cbw: u8,
) {
    if let Some(channel) = WifiCoexChannel::from_phy_request(channel_or_frequency, cbw) {
        guard.set_coex_wifi_channel(CoexWifiChannel {
            primary: channel.primary,
            secondary: channel.secondary,
        });
    }
}

/// Publish one Wi-Fi activity under an already-held arbiter lease, and
/// restart the phases when its caller in the vendor library does.
pub(crate) fn publish_coex_activity<P, C: PlatformClockProvider, T: oer_time::Timer>(
    guard: &mut RadioGuard<'_, P, C, T>,
    activity: WifiCoexActivity,
) {
    let restart = apply_coex_status(guard, activity);
    if restart && guard.coex_active_for(CoexStatusType::Wifi) {
        guard.restart_coex_phases();
    }
}

/// Replace Wi-Fi's status and set the schedule interval, as the vendor
/// `pm_on_coex_schm_status_config` does, and report whether its callers
/// restart the phases after it.
pub(crate) fn apply_coex_status<P, C: PlatformClockProvider, T: oer_time::Timer>(
    guard: &mut RadioGuard<'_, P, C, T>,
    activity: WifiCoexActivity,
) -> bool {
    let update = activity.status_update();
    guard.clear_coex_status_bits(CoexStatusType::Wifi, u16::MAX);
    guard.set_coex_status_bits(CoexStatusType::Wifi, update.status);
    guard.set_coex_interval(update.interval);
    update.restart_when_shared
}

/// The vendor coexistence reconnect policy of Wi-Fi (`g_pm` byte 1069,
/// inverted): on after a station lost its association while another radio
/// shared the air, off again when the next association starts power
/// management.
///
/// SOURCE: complete pinned `libpp.a[pm_coex.o]::pm_coex_reconnect_policy`
/// and `pm_coex_set_reconnect_policy`; `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state` calls the latter when the station leaves its
/// connected state, and `libpp.a[pm.o]::pm_start` turns the policy off.
pub struct WifiReconnectPolicy {
    active: AtomicBool,
}

static WIFI_RECONNECT_POLICY: WifiReconnectPolicy = WifiReconnectPolicy {
    active: AtomicBool::new(false),
};

impl WifiReconnectPolicy {
    /// Wi-Fi's one policy, as the vendor keeps it in its power manager.
    pub fn get() -> &'static Self {
        &WIFI_RECONNECT_POLICY
    }

    pub fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// A station lost its association: turn the policy on while another
    /// radio shares the air.
    pub async fn association_lost<P, C: PlatformClockProvider, T: oer_time::Timer>(
        &self,
        radio: &RadioSystem<P, C, T>,
    ) {
        if radio.lock().await.coex_active_for(CoexStatusType::Wifi) {
            self.active.store(true, Ordering::Release);
        }
    }

    /// An association started power management.
    pub fn association_started(&self) {
        self.active.store(false, Ordering::Release);
    }
}

/// The per-frame coexistence requests of Wi-Fi's connection frames.
pub struct RadioConnectionCoex<'radio, P, C, T> {
    radio: &'radio RadioSystem<P, C, T>,
}

impl<'radio, P, C, T> RadioConnectionCoex<'radio, P, C, T> {
    /// The connection-frame requests of Wi-Fi on `radio`.
    pub const fn new(radio: &'radio RadioSystem<P, C, T>) -> Self {
        Self { radio }
    }
}

impl<P, C, T> Clone for RadioConnectionCoex<'_, P, C, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P, C, T> Copy for RadioConnectionCoex<'_, P, C, T> {}

impl<P, C: PlatformClockProvider, T: oer_time::Timer> ConnectionFrameCoex
    for RadioConnectionCoex<'_, P, C, T>
{
    /// `pp_coex_tx_request` for one connection frame. The vendor also
    /// requests event 45 for every Probe Request on the 5 GHz band, which
    /// this 2.4 GHz radio has no channel of. A rejected request changes
    /// nothing, as the vendor ignores its result.
    async fn connection_frame(&mut self, frame: ConnectionFrame) -> Option<ReconnectFramePriority> {
        const PROBE_EVENT: u8 = 45;
        const CONNECTION_EVENT: u8 = 46;
        const SLICE_EVENT: u8 = 1;
        if !WifiReconnectPolicy::get().active() {
            return None;
        }
        let event = CoexEventId::new(match frame {
            ConnectionFrame::ProbeRequest => PROBE_EVENT,
            ConnectionFrame::Authentication
            | ConnectionFrame::Association
            | ConnectionFrame::Eapol => CONNECTION_EVENT,
        })
        .expect("the reconnect events are vendor events");
        let duration = CoexEventDurations::reviewed_vendor()
            .duration(event)
            .expect("the reconnect events have reviewed durations");
        let mut guard = self.radio.lock().await;
        let _ = guard.request_wifi_coex(CoexClientRequest {
            event,
            latency: 0,
            duration,
        });
        if frame == ConnectionFrame::ProbeRequest {
            return None;
        }
        let packet = guard.lease().coex_pti(event).value();
        let slice = guard
            .lease()
            .coex_pti(CoexEventId::new(SLICE_EVENT).expect("the slice event is a vendor event"))
            .value();
        Some(ReconnectFramePriority {
            packet,
            scheduler: packet.min(slice),
        })
    }
}
