//! Executor-independent ESP32-S31 ports for the WPA2 four-way handshake.
//!
//! The portable WPA2 crate owns replay protection, cryptography, deadlines and
//! rollback order. This module binds those transactions to ESP32-S31 frame TX
//! and hardware CCMP slots without choosing an executor, DMA allocation or
//! board fixture.

use core::future::Future;
use oer_ieee80211_mac::sequence::SequenceNumber;

use crate::connected::management_protection::StationManagementProtection;
use crate::connected_rx::{StaCcmpRxReplayEpoch, StaCcmpRxReplayError};
use crate::connection_coex::{ConnectionFrame, ConnectionFrameCoex, ReconnectFramePriority};

use oer_esp32s31_ieee80211_mac::{
    crypto::{
        CcmpKeyHardware, CcmpTxPacketNumberError, CryptoKeyError, StaGroupCcmpKeyMaterial,
        StaGroupCcmpSlot, StaPairwiseCcmpSlot, install_sta_group_ccmp, install_sta_pairwise_ccmp,
    },
    tx::{LegacyTxQueue, TxCompletion},
};

use oer_ieee80211_mac::station::{StaDataFrame, StaProtectedDataFrame, StaTxSequenceCounters};

use oer_ieee80211_rsn::{
    DEFAULT_EAPOL_FRAME_CAPACITY, OwnedEapolFrame, RsnInterface,
    frames::RsnTxFrame,
    keys::RsnKeyKind,
    runner::{RsnHandshakeBackend, RsnKeyInstallBackend, RsnRxProgress},
    supplicant::RsnStaKeyInstallRequest,
};

const LLC_SNAP_EAPOL: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e];

/// RX capability consumed by the concrete WPA2 handshake port.
pub trait Wpa2Receive<H> {
    type Error;

    fn service(&mut self, hardware: &mut H, frame: &mut [u8])
    -> Result<RsnRxProgress, Self::Error>;

    fn restart<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;

    fn stop(&mut self, hardware: &mut H) -> Result<(), Self::Error>;
}

/// Ordinary EAPOL TX capability shared by handshake and key-install ports.
pub trait HandshakeTransmit<H> {
    type Error;

    fn transmit_unprotected<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: StaDataFrame<'a>,
        reconnect: Option<ReconnectFramePriority>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a;

    fn transmit_protected<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: StaProtectedDataFrame<'a>,
        queue: LegacyTxQueue,
        hardware_key_selector: u8,
        reconnect: Option<ReconnectFramePriority>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a;
}

/// Stable local/peer identity shared by both WPA2 ports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
// CAPABILITY: wpa2
pub struct Wpa2Station {
    station_address: [u8; 6],
    bssid: [u8; 6],
}

impl Wpa2Station {
    pub const fn new(station_address: [u8; 6], bssid: [u8; 6]) -> Self {
        Self {
            station_address,
            bssid,
        }
    }
}

/// Copy an EAPOL packet only when it belongs to the selected station link.
pub fn copy_station_eapol(
    frame: &[u8],
    mpdu_length: usize,
    payload_offset: usize,
    station: Wpa2Station,
) -> Option<OwnedEapolFrame<DEFAULT_EAPOL_FRAME_CAPACITY>> {
    if mpdu_length < 24
        || frame.get(4..10) != Some(&station.station_address)
        || frame.get(10..16) != Some(&station.bssid)
    {
        return None;
    }
    let eapol_offset = payload_offset.checked_add(LLC_SNAP_EAPOL.len())?;
    if frame.get(payload_offset..eapol_offset) != Some(&LLC_SNAP_EAPOL) {
        return None;
    }
    OwnedEapolFrame::try_copy(
        RsnInterface::Station,
        station.bssid,
        frame.get(eapol_offset..mpdu_length)?,
    )
    .ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2HandshakePortError<R, T> {
    Receive(R),
    Transmit(T),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Wpa2HandshakeTelemetry {
    pub message2_transmissions: u16,
}

/// PAC/RX/TX owners borrowed for one handshake runner lifetime.
pub struct Wpa2HandshakeRadio<'hardware, 'transmit, H, R, T, C> {
    hardware: &'hardware mut H,
    receive: R,
    transmit: &'transmit mut T,
    coex: C,
}

impl<'hardware, 'transmit, H, R, T, C> Wpa2HandshakeRadio<'hardware, 'transmit, H, R, T, C> {
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

/// Borrowed allocation-free MPDU/EAPOL scratch storage.
pub struct Wpa2HandshakeStorage<'scratch> {
    frame: &'scratch mut [u8],
}

impl<'scratch> Wpa2HandshakeStorage<'scratch> {
    pub fn new(frame: &'scratch mut [u8]) -> Self {
        Self { frame }
    }
}

/// Complete production port for WPA2 Message 1/2/3 exchange.
pub struct Wpa2HandshakePort<'hardware, 'transmit, 'scratch, H, R, T, C> {
    radio: Wpa2HandshakeRadio<'hardware, 'transmit, H, R, T, C>,
    storage: Wpa2HandshakeStorage<'scratch>,
    station: Wpa2Station,
    telemetry: Wpa2HandshakeTelemetry,
}

impl<'hardware, 'transmit, 'scratch, H, R, T, C>
    Wpa2HandshakePort<'hardware, 'transmit, 'scratch, H, R, T, C>
{
    pub fn new(
        radio: Wpa2HandshakeRadio<'hardware, 'transmit, H, R, T, C>,
        storage: Wpa2HandshakeStorage<'scratch>,
        station: Wpa2Station,
    ) -> Self {
        Self {
            radio,
            storage,
            station,
            telemetry: Wpa2HandshakeTelemetry::default(),
        }
    }

    pub const fn telemetry(&self) -> Wpa2HandshakeTelemetry {
        self.telemetry
    }

    pub fn into_receive(self) -> R {
        self.radio.receive
    }
}

impl<H, R, T, C> RsnHandshakeBackend for Wpa2HandshakePort<'_, '_, '_, H, R, T, C>
where
    C: ConnectionFrameCoex,
    R: Wpa2Receive<H>,
    T: HandshakeTransmit<H>,
{
    type Error = Wpa2HandshakePortError<R::Error, T::Error>;

    async fn service_receive(&mut self) -> Result<RsnRxProgress, Self::Error> {
        self.radio
            .receive
            .service(self.radio.hardware, self.storage.frame)
            .map_err(Wpa2HandshakePortError::Receive)
    }

    async fn restart_receive(&mut self) -> Result<(), Self::Error> {
        self.radio
            .receive
            .restart(self.radio.hardware)
            .await
            .map_err(Wpa2HandshakePortError::Receive)
    }

    async fn stop_receive(&mut self) -> Result<(), Self::Error> {
        self.radio
            .receive
            .stop(self.radio.hardware)
            .map_err(Wpa2HandshakePortError::Receive)
    }

    async fn transmit_message2<'a>(
        &'a mut self,
        frame: &'a RsnTxFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        sequence_number: SequenceNumber,
    ) -> Result<(), Self::Error> {
        let reconnect = self
            .radio
            .coex
            .connection_frame(ConnectionFrame::Eapol)
            .await;
        self.radio
            .transmit
            .transmit_unprotected(
                self.radio.hardware,
                StaDataFrame {
                    source: self.station.station_address,
                    bssid: self.station.bssid,
                    destination: self.station.bssid,
                    sequence_number,
                    ether_type: 0x888e,
                    payload: frame.as_bytes(),
                },
                reconnect,
            )
            .await
            .map_err(Wpa2HandshakePortError::Transmit)?;
        self.telemetry.message2_transmissions =
            self.telemetry.message2_transmissions.saturating_add(1);
        Ok(())
    }
}

pub use oer_ieee80211_sta::attempt::Wpa2Message4Protection;

/// Installed hardware authorities returned to the connected data path.
pub struct InstalledWpa2Keys {
    pairwise: StaPairwiseCcmpSlot,
    group: StaGroupCcmpSlot,
    group_material: StaGroupCcmpKeyMaterial,
    replay: StaCcmpRxReplayEpoch,
    management: Option<StationManagementProtection>,
}

/// The owners of [`InstalledWpa2Keys`], returned to the connected station.
pub struct InstalledWpa2KeyParts {
    pub pairwise: StaPairwiseCcmpSlot,
    pub group: StaGroupCcmpSlot,
    pub group_material: StaGroupCcmpKeyMaterial,
    pub replay: StaCcmpRxReplayEpoch,
    /// Present when the association protects its management frames.
    pub management: Option<StationManagementProtection>,
}

impl InstalledWpa2Keys {
    pub fn into_parts(self) -> InstalledWpa2KeyParts {
        InstalledWpa2KeyParts {
            pairwise: self.pairwise,
            group: self.group,
            group_material: self.group_material,
            replay: self.replay,
            management: self.management,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2KeyPortError<T> {
    InvalidGroupKind,
    InvalidReceiveSequence(StaCcmpRxReplayError),
    Install(CryptoKeyError),
    PacketNumber(CcmpTxPacketNumberError),
    Transmit(T),
    TxStatus(u8),
}

/// Owners recovered after a key-install runner has completed.
pub struct Wpa2KeyPortParts<'hardware, 'transmit, 'sequence, H, T> {
    pub hardware: &'hardware mut H,
    pub transmit: &'transmit mut T,
    pub sequences: &'sequence mut StaTxSequenceCounters,
    pub completion: Option<TxCompletion>,
}

/// Hardware and ordinary-TX owners borrowed by key publication.
pub struct Wpa2KeyRadio<'hardware, 'transmit, H, T, C> {
    hardware: &'hardware mut H,
    transmit: &'transmit mut T,
    coex: C,
}

impl<'hardware, 'transmit, H, T, C> Wpa2KeyRadio<'hardware, 'transmit, H, T, C> {
    pub const fn new(hardware: &'hardware mut H, transmit: &'transmit mut T, coex: C) -> Self {
        Self {
            hardware,
            transmit,
            coex,
        }
    }
}

/// Link policy and sequence ownership retained through Message 4.
pub struct Wpa2KeySession<'sequence> {
    station: Wpa2Station,
    peer_qos: bool,
    sequences: &'sequence mut StaTxSequenceCounters,
    message4_protection: Wpa2Message4Protection,
}

impl<'sequence> Wpa2KeySession<'sequence> {
    pub const fn new(
        station: Wpa2Station,
        peer_qos: bool,
        sequences: &'sequence mut StaTxSequenceCounters,
        message4_protection: Wpa2Message4Protection,
    ) -> Self {
        Self {
            station,
            peer_qos,
            sequences,
            message4_protection,
        }
    }
}

/// Complete production port for atomic PTK/GTK publication and Message 4.
pub struct Wpa2KeyPort<'hardware, 'transmit, 'sequence, H, T, C> {
    radio: Wpa2KeyRadio<'hardware, 'transmit, H, T, C>,
    session: Wpa2KeySession<'sequence>,
    completion: Option<TxCompletion>,
}

impl<'hardware, 'transmit, 'sequence, H, T, C>
    Wpa2KeyPort<'hardware, 'transmit, 'sequence, H, T, C>
{
    pub const fn new(
        radio: Wpa2KeyRadio<'hardware, 'transmit, H, T, C>,
        session: Wpa2KeySession<'sequence>,
    ) -> Self {
        Self {
            radio,
            session,
            completion: None,
        }
    }

    pub const fn completion(&self) -> Option<TxCompletion> {
        self.completion
    }

    pub fn into_parts(self) -> Wpa2KeyPortParts<'hardware, 'transmit, 'sequence, H, T> {
        Wpa2KeyPortParts {
            hardware: self.radio.hardware,
            transmit: self.radio.transmit,
            sequences: self.session.sequences,
            completion: self.completion,
        }
    }
}

impl<H, T, C> RsnKeyInstallBackend for Wpa2KeyPort<'_, '_, '_, H, T, C>
where
    C: ConnectionFrameCoex,
    H: CcmpKeyHardware,
    T: HandshakeTransmit<H>,
{
    type Error = Wpa2KeyPortError<T::Error>;
    type InstalledKeys = InstalledWpa2Keys;

    fn install_keys(
        &mut self,
        request: &RsnStaKeyInstallRequest,
    ) -> Result<Self::InstalledKeys, Self::Error> {
        let pairwise = request.pairwise();
        let group = request.group();
        let RsnKeyKind::Group { key_id, .. } = group.kind() else {
            return Err(Wpa2KeyPortError::InvalidGroupKind);
        };
        let replay = StaCcmpRxReplayEpoch::new(
            *pairwise.receive_sequence(),
            key_id,
            *group.receive_sequence(),
        )
        .map_err(Wpa2KeyPortError::InvalidReceiveSequence)?;
        let group_material = StaGroupCcmpKeyMaterial::new(key_id, *group.key().as_bytes())
            .map_err(Wpa2KeyPortError::Install)?;
        let pairwise = install_sta_pairwise_ccmp(
            self.radio.hardware,
            *pairwise.peer(),
            pairwise.key().as_bytes(),
        )
        .map_err(Wpa2KeyPortError::Install)?;
        let group =
            match install_sta_group_ccmp(self.radio.hardware, key_id, group.key().as_bytes()) {
                Ok(group) => group,
                Err(error) => {
                    pairwise.clear(self.radio.hardware);
                    return Err(Wpa2KeyPortError::Install(error));
                }
            };
        // With management frame protection the station keeps its temporal
        // key to open protected management frames, as the vendor does.
        let management = request.igtk().map(|igtk| {
            StationManagementProtection::new(*request.pairwise().key().as_bytes(), igtk)
        });
        oer_trace::emit(&oer_ieee80211_trace::LinkControlTrace {
            event: oer_ieee80211_trace::LinkEvent::KeysInstalled {
                group_key_id: key_id,
            },
        });
        Ok(InstalledWpa2Keys {
            pairwise,
            group,
            group_material,
            replay,
            management,
        })
    }

    fn rollback_keys(&mut self, keys: Self::InstalledKeys) -> Result<(), Self::Error> {
        keys.group.clear(self.radio.hardware);
        keys.pairwise.clear(self.radio.hardware);
        Ok(())
    }

    async fn transmit_message4<'a>(
        &'a mut self,
        frame: &'a RsnTxFrame<DEFAULT_EAPOL_FRAME_CAPACITY>,
        keys: &'a mut Self::InstalledKeys,
    ) -> Result<(), Self::Error> {
        let reconnect = self
            .radio
            .coex
            .connection_frame(ConnectionFrame::Eapol)
            .await;
        let completion = match self.session.message4_protection {
            Wpa2Message4Protection::Unprotected => {
                self.radio
                    .transmit
                    .transmit_unprotected(
                        self.radio.hardware,
                        StaDataFrame {
                            source: self.session.station.station_address,
                            bssid: self.session.station.bssid,
                            destination: self.session.station.bssid,
                            sequence_number: self.session.sequences.take_non_qos(),
                            ether_type: 0x888e,
                            payload: frame.as_bytes(),
                        },
                        reconnect,
                    )
                    .await
            }
            Wpa2Message4Protection::PairwiseCcmp => {
                let ccmp_header = keys
                    .pairwise
                    .next_tx_ccmp_header()
                    .map_err(Wpa2KeyPortError::PacketNumber)?;
                self.radio
                    .transmit
                    .transmit_protected(
                        self.radio.hardware,
                        StaProtectedDataFrame {
                            source: self.session.station.station_address,
                            bssid: self.session.station.bssid,
                            destination: self.session.station.bssid,
                            sequence_number: self
                                .session
                                .sequences
                                .take_data(self.session.peer_qos.then_some(0))
                                .expect("selected EAPOL sequence-number owner exists"),
                            user_priority: 7,
                            peer_qos: self.session.peer_qos,
                            ccmp_header,
                            ether_type: 0x888e,
                            payload: frame.as_bytes(),
                        },
                        LegacyTxQueue::Voice,
                        keys.pairwise.hardware_index(),
                        reconnect,
                    )
                    .await
            }
        }
        .map_err(Wpa2KeyPortError::Transmit)?;
        self.completion = Some(completion);
        if completion.status() == 0 {
            Ok(())
        } else {
            Err(Wpa2KeyPortError::TxStatus(completion.status()))
        }
    }
}

#[cfg(test)]
mod tests;
