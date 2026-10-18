//! Concrete scan-port composition for cold and quiesced connected STA epochs.
//!
//! The chip-independent lifecycle service owns plan progress and retry policy;
//! [`StaScanBackend`](oer_ieee80211_sta_service::scan::StaScanBackend)
//! owns the
//! mandatory channel-visit transaction order. This module binds that transaction
//! to the returned PHY, cooperative register owner, RX ring and control-TX
//! descriptor without importing board fixtures, credentials or diagnostics.

use core::future::Future;
use oer_esp32s31_ieee80211_sta::connection_coex::ConnectionFrameCoex;

use crate::{
    datapath::rx::frontier::RxFrontierError,
    roles::scan::rx::{RunningScanRx, ScanFrameObserver, ScanObservationContext, ScanRxProgress},
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile};

use oer_esp32s31_ieee80211_mac::{rx::RxDma, tx::TxHardware};

use oer_esp32s31_ieee80211_sta::{
    control_tx::ControlTxError,
    scan::{ActiveProbeOutcome, StaScanPort},
    scan_tx::{RunningScanTx, ScanProbeReport, ScanProbeRequest},
};

use oer_ieee80211_mac::{
    scan::{ScanRecord, ScanTable, best_matching_ssid_and_security},
    security::StaSecurityPolicy,
    station::StaSequenceCounter,
};

use oer_ieee80211_sta::scan::StaScanChannelContext;

/// PHY channel-switch capability required by a running scan.
pub trait ScanPhyPort<H> {
    type Error;

    /// Switch to `channel` and return its dwell in milliseconds, which may
    /// differ from `requested_dwell_millis` while another radio shares the
    /// air.
    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut H,
        channel: u8,
        requested_dwell_millis: u16,
    ) -> impl Future<Output = Result<u16, Self::Error>> + 'a;

    /// The per-frame coexistence requests of the scan's Probe Requests.
    type Coex: ConnectionFrameCoex;

    fn connection_coex(&self) -> Self::Coex;
}

/// RX-ring capability retained across every finite running-scan channel.
pub trait ScanReceivePort<H> {
    type Error;

    fn prepare_initial(&mut self, hardware: &mut H) -> Result<(), Self::Error>;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;

    fn observe_management<O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<ScanRxProgress, Self::Error>
    where
        O: ScanFrameObserver;

    /// Close the logical channel epoch without stopping the physical walker.
    fn park(&mut self) -> Result<(), Self::Error>;

    fn prepare_next_channel(&mut self, hardware: &mut H) -> Result<(), Self::Error>;
}

/// Polling control-TX capability available only after connected IRQ teardown.
pub trait ScanTransmitPort<H> {
    type Error;

    fn begin_scan(&mut self);

    fn transmit_probe_request<'a, C: ConnectionFrameCoex + 'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: ScanProbeRequest<'a>,
        coex: &'a mut C,
    ) -> impl Future<Output = Result<ScanProbeReport, Self::Error>> + 'a;
}

/// Driver resources returned by one completely quiesced connected epoch.
pub struct ScanRadio<P, H, R, T> {
    phy: P,
    hardware: H,
    rx: R,
    tx: T,
}

impl<P, H, R, T> ScanRadio<P, H, R, T> {
    pub const fn new(phy: P, hardware: H, rx: R, tx: T) -> Self {
        Self {
            phy,
            hardware,
            rx,
            tx,
        }
    }
}

/// Borrowed allocation-free storage for one running scan.
pub struct ScanStorage<'resources, 'sequence, O, const RECORDS: usize> {
    table: &'resources mut ScanTable<RECORDS>,
    frame: &'resources mut [u8],
    observer: O,
    sequence: &'sequence mut StaSequenceCounter,
}

impl<'resources, 'sequence, O, const RECORDS: usize>
    ScanStorage<'resources, 'sequence, O, RECORDS>
{
    pub fn new(
        table: &'resources mut ScanTable<RECORDS>,
        frame: &'resources mut [u8],
        observer: O,
        sequence: &'sequence mut StaSequenceCounter,
    ) -> Self {
        Self {
            table,
            frame,
            observer,
            sequence,
        }
    }
}

/// Peer-independent station policy for active scan and candidate selection.
pub struct ScanStation<'ssid, 'rates> {
    station_address: [u8; 6],
    target_ssid: &'ssid [u8],
    supported_rates: &'rates [u8],
    descriptor_capacity: Option<u32>,
    select_candidate: bool,
    security: StaSecurityPolicy,
}

impl<'ssid, 'rates> ScanStation<'ssid, 'rates> {
    pub const fn new(
        station_address: [u8; 6],
        target_ssid: &'ssid [u8],
        supported_rates: &'rates [u8],
        security: StaSecurityPolicy,
    ) -> Self {
        Self {
            station_address,
            target_ssid,
            supported_rates,
            descriptor_capacity: None,
            select_candidate: true,
            security,
        }
    }

    pub const fn with_descriptor_capacity(mut self, capacity: u32) -> Self {
        self.descriptor_capacity = Some(capacity);
        self
    }

    pub const fn with_candidate_selection(mut self, enabled: bool) -> Self {
        self.select_candidate = enabled;
        self
    }
}

/// Bounded telemetry produced without retaining any DMA-backed frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanTelemetry {
    pub raw_frames: u32,
    pub ring_epochs: u32,
}

/// Exact primitive edge which failed inside the concrete running port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanPortError<P, R, T> {
    ChannelSwitch(P),
    Receive(R),
    Transmit(T),
}

/// Owners returned after the scan service has parked logical RX while keeping
/// the physical descriptor frontier live.
pub struct ScanPortParts<'resources, 'sequence, P, H, R, T, W, O, const RECORDS: usize> {
    pub phy: P,
    pub hardware: H,
    pub rx: R,
    pub tx: T,
    pub timer: W,
    pub observer: O,
    pub table: &'resources mut ScanTable<RECORDS>,
    pub frame: &'resources mut [u8],
    pub sequence: &'sequence mut StaSequenceCounter,
    pub telemetry: ScanTelemetry,
}

/// Complete production running-scan port.
///
/// Board code supplies only coherent driver owners, fixed storage and station
/// policy. No PAC singleton, static address or executor task is reconstructed
/// inside this value.
pub struct ScanPort<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, const RECORDS: usize> {
    radio: ScanRadio<P, H, R, T>,
    storage: ScanStorage<'resources, 'sequence, O, RECORDS>,
    station: ScanStation<'ssid, 'rates>,
    timer: W,
    telemetry: ScanTelemetry,
}

mod bindings;
mod owner;
mod service;

#[cfg(test)]
mod tests;
