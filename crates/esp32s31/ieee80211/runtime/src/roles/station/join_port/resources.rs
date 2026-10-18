use {
    oer_ieee80211_mac::scan::ScanRecord, oer_ieee80211_mac::station::SelectedRsn,
    oer_ieee80211_mac::station::association::Preference,
};

use oer_ieee80211_sta::request::StationListenInterval;

/// Driver resources borrowed for one join runner lifetime.
pub struct StaJoinRadio<'hardware, 'transmit, H, R, T, C> {
    pub(super) hardware: &'hardware mut H,
    pub(super) receive: R,
    pub(super) transmit: &'transmit mut T,
    /// The per-frame coexistence requests of Authentication and
    /// Association Requests.
    pub(super) coex: C,
}

impl<'hardware, 'transmit, H, R, T, C> StaJoinRadio<'hardware, 'transmit, H, R, T, C> {
    pub const fn new(
        hardware: &'hardware mut H,
        receive: R,
        transmit: &'transmit mut T,
        coex: C,
    ) -> Self {
        Self {
            hardware,
            receive,
            transmit,
            coex,
        }
    }
}

/// Borrowed allocation-free parsing storage and diagnostic observer.
pub struct StaJoinStorage<'scratch, O> {
    pub(super) frame: &'scratch mut [u8],
    pub(super) observer: O,
}

impl<'scratch, O> StaJoinStorage<'scratch, O> {
    pub fn new(frame: &'scratch mut [u8], observer: O) -> Self {
        Self { frame, observer }
    }
}

/// Station and peer policy for one Authentication/Association epoch.
#[derive(Clone, Copy)]
pub struct StaJoinStation {
    pub(super) station_address: [u8; 6],
    pub(super) access_point: ScanRecord,
    pub(super) association_preference: Preference,
    pub(super) listen_interval: u16,
    pub(super) rsn: SelectedRsn,
}

impl StaJoinStation {
    pub const fn new(
        station_address: [u8; 6],
        access_point: ScanRecord,
        association_preference: Preference,
        rsn: SelectedRsn,
    ) -> Self {
        Self {
            station_address,
            access_point,
            association_preference,
            listen_interval: StationListenInterval::DEFAULT.get(),
            rsn,
        }
    }

    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    pub const fn with_listen_interval(mut self, listen_interval: StationListenInterval) -> Self {
        self.listen_interval = listen_interval.get();
        self
    }
}
