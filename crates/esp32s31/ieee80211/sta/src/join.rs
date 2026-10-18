//! ESP32-S31 semantic ports for pre-connected STA join.
//!
//! Portable Authentication/Association retry and deadline policy lives in
//! `oer-ieee80211-sta::join`. These interfaces describe the narrower
//! chip operations needed to bind that policy to ESP32-S31. They deliberately
//! contain no DMA owner, executor timer, task primitive or board fixture.

use core::future::Future;

use crate::association::{StaAssociationProfile, StaAssociationProfileError};
use crate::connection_coex::ReconnectFramePriority;

use oer_esp32s31_ieee80211::tx::WifiTxPowerProfile;

use oer_esp32s31_ieee80211_mac::tx::TxCompletion;

use oer_ieee80211_mac::station::{
    AssociationRequest, OpenAuthenticationRequest, SaeAuthenticationFrame,
};

/// RX capability consumed by a concrete ESP32-S31 join adapter.
pub trait StaJoinReceive<H> {
    type Error;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;

    fn stop(&mut self, hardware: &mut H) -> Result<(), Self::Error>;

    fn service_management<O>(
        &mut self,
        hardware: &mut H,
        frame: &mut [u8],
        observer: &mut O,
    ) -> Result<(), Self::Error>
    where
        O: oer_ieee80211_sta::join::StaJoinRxObserver;
}

/// Control-TX capability consumed by a concrete ESP32-S31 join adapter.
pub trait StaJoinTransmit<H> {
    type Error;
    type PowerProfile: WifiTxPowerProfile;

    fn power_profile(&self) -> &Self::PowerProfile;

    fn transmit_open_authentication<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: OpenAuthenticationRequest,
        reconnect: Option<ReconnectFramePriority>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a;

    fn transmit_association<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: AssociationRequest<'a>,
        reconnect: Option<ReconnectFramePriority>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a;

    fn transmit_sae_authentication<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: SaeAuthenticationFrame<'a>,
        reconnect: Option<ReconnectFramePriority>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a;
}

/// Value-only observation points which do not participate in driver policy.
///
/// Callbacks run after the matching TX transaction has completed. Production
/// composition normally uses `()`; applications and HIL may retain bounded
/// evidence without wrapping the hardware operation.
pub trait StaJoinObserver {
    fn authentication_transmitted(&mut self, _completion: TxCompletion) {}

    fn association_profile_selected(&mut self, _profile: StaAssociationProfile) {}

    fn association_transmitted(&mut self, _completion: TxCompletion) {}
}

impl StaJoinObserver for () {}

/// Exact primitive edge which failed inside a concrete join adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaJoinPortError<R, T> {
    Receive(R),
    AssociationProfile(StaAssociationProfileError),
    Transmit(T),
}
