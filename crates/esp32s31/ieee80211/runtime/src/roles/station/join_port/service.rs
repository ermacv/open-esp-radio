#![expect(
    clippy::manual_async_fn,
    reason = "join test services implement the same explicit borrowed Future contracts"
)]

use core::future::Future;

use oer_esp32s31_ieee80211_sta::{
    association::esp32s31_sta_association_profile,
    connection_coex::{ConnectionFrame, ConnectionFrameCoex},
    join::{StaJoinObserver, StaJoinPortError, StaJoinReceive, StaJoinTransmit},
};

use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_mac::station::{
    AssociationRequest, OpenAuthenticationRequest, SaeAuthenticationFrame,
};
use oer_ieee80211_sta::join::sae::StaSaeTransmission;

use oer_ieee80211_sta::join::{
    StaJoinBackend, StaJoinRxObserver, association::StaAssociationAttempt,
    authentication::StaAuthenticationAttempt,
};

use super::StaJoinPort;

impl<H, R, T, C, O> StaJoinBackend for StaJoinPort<'_, '_, '_, H, R, T, C, O>
where
    C: ConnectionFrameCoex,
    R: StaJoinReceive<H>,
    T: StaJoinTransmit<H>,
    O: StaJoinObserver,
{
    type Error = StaJoinPortError<R::Error, T::Error>;

    fn start_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.radio
                .receive
                .start(self.radio.hardware)
                .await
                .map_err(StaJoinPortError::Receive)
        }
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.radio
                .receive
                .stop(self.radio.hardware)
                .map_err(StaJoinPortError::Receive)
        }
    }

    fn transmit_open_authentication(
        &mut self,
        attempt: StaAuthenticationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            let reconnect = self
                .radio
                .coex
                .connection_frame(ConnectionFrame::Authentication)
                .await;
            let completion = self
                .radio
                .transmit
                .transmit_open_authentication(
                    self.radio.hardware,
                    OpenAuthenticationRequest {
                        source: self.station.station_address,
                        bssid: self.station.access_point.bssid,
                        sequence_number: attempt.sequence_number,
                    },
                    reconnect,
                )
                .await
                .map_err(StaJoinPortError::Transmit)?;
            self.storage.observer.authentication_transmitted(completion);
            Ok(())
        }
    }

    fn transmit_sae_authentication<'a>(
        &'a mut self,
        sequence_number: SequenceNumber,
        transmission: &'a StaSaeTransmission,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            let reconnect = self
                .radio
                .coex
                .connection_frame(ConnectionFrame::Authentication)
                .await;
            let completion = self
                .radio
                .transmit
                .transmit_sae_authentication(
                    self.radio.hardware,
                    SaeAuthenticationFrame {
                        source: self.station.station_address,
                        bssid: self.station.access_point.bssid,
                        sequence_number,
                        transaction: transmission.transaction,
                        status_code: transmission.status_code,
                        body: transmission.body(),
                    },
                    reconnect,
                )
                .await
                .map_err(StaJoinPortError::Transmit)?;
            self.storage.observer.authentication_transmitted(completion);
            Ok(())
        }
    }

    fn transmit_association(
        &mut self,
        attempt: StaAssociationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            let profile = esp32s31_sta_association_profile(
                &self.station.access_point,
                self.station.association_preference,
                self.radio.transmit.power_profile(),
            )
            .map_err(StaJoinPortError::AssociationProfile)?;
            self.storage.observer.association_profile_selected(profile);
            let reconnect = self
                .radio
                .coex
                .connection_frame(ConnectionFrame::Association)
                .await;
            let completion = self
                .radio
                .transmit
                .transmit_association(
                    self.radio.hardware,
                    AssociationRequest {
                        source: self.station.station_address,
                        access_point: &self.station.access_point,
                        sequence_number: attempt.sequence_number,
                        listen_interval: self.station.listen_interval,
                        phy: profile.phy,
                        security: &self.station.rsn,
                        power_capability: profile.power_capability,
                        he_ul_mu_power: profile.he_ul_mu_power,
                    },
                    reconnect,
                )
                .await
                .map_err(StaJoinPortError::Transmit)?;
            self.storage.observer.association_transmitted(completion);
            Ok(())
        }
    }

    fn service_receive<'a, V>(
        &'a mut self,
        observer: &'a mut V,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        V: StaJoinRxObserver + 'a,
    {
        async move {
            self.radio
                .receive
                .service_management(self.radio.hardware, self.storage.frame, observer)
                .map_err(StaJoinPortError::Receive)
        }
    }
}
