use super::*;

/// Exact primitive error reported by the concrete target port.
#[derive(Debug, Eq, PartialEq)]
pub enum StaAttemptTargetError<J, W> {
    State(StaAttemptStateError),
    Candidate(StaPeerPortError),
    Channel(ConcurrentWifiChannelError),
    Authentication(StaJoinError<StaJoinPortError<RxFrontierError, J>>),
    Association(StaJoinError<StaJoinPortError<RxFrontierError, J>>),
    Peer(StaPeerPortError),
    Security(StaSecurityError),
    /// No SAE commit could be formed from the password.
    SaeCommit(oer_ieee80211_rsn::sae::SaeError),
    Wpa2Handshake(
        RsnHandshakeError<Wpa2HandshakePortError<RxFrontierError, W>, SoftwareAesKeyUnwrapError>,
    ),
    RsnKeyInstall(RsnKeyInstallError<Wpa2KeyPortError<W>>),
}

/// Coherent owner consumed by
/// [`StaAttempt`](oer_ieee80211_sta_service::attempt::StaAttempt).
pub struct StaAttemptTargetOwner<
    'hardware,
    'transmit,
    'storage,
    'scratch,
    'security,
    H,
    C,
    D,
    T,
    J,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    pub(super) hardware: &'hardware mut H,
    pub(super) channel: C,
    pub(super) receive: Option<ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>>,
    pub(super) rx_storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub(super) transmit: &'transmit mut T,
    pub(super) timer: D,
    pub(super) frame: &'scratch mut [u8],
    pub(super) station: StaAttemptStation,
    pub(super) listen_interval: oer_ieee80211_sta::request::StationListenInterval,
    pub(super) security: StaAttemptSecurity<'security>,
    pub(super) prepared_peer: Option<PreparedStaPeer>,
    /// The security elements authentication selected, which association,
    /// peer programming and the four-way handshake then use unchanged.
    pub(super) selected_rsn: Option<SelectedRsn>,
    pub(super) association: Option<AssociationResponse>,
    pub(super) connected_peer: Option<ConnectedStaPeer>,
    pub(super) pending_keys: Option<RsnPendingKeyInstall>,
    pub(super) installed_security: Option<StaInstalledSecurity>,
    pub(super) report: StaAttemptReport,
    pub(super) _join_observer: PhantomData<fn() -> J>,
}

impl<
    'hardware,
    'transmit,
    'storage,
    'scratch,
    'security,
    H,
    C,
    D,
    T,
    J,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    StaAttemptTargetOwner<
        'hardware,
        'transmit,
        'storage,
        'scratch,
        'security,
        H,
        C,
        D,
        T,
        J,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub fn new(
        radio: StaAttemptRadio<
            'hardware,
            'transmit,
            'storage,
            H,
            C,
            D,
            T,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        storage: StaAttemptStorage<'scratch>,
        station: StaAttemptStation,
        listen_interval: oer_ieee80211_sta::request::StationListenInterval,
        security: StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            hardware: radio.hardware,
            channel: radio.channel,
            receive: Some(radio.receive),
            rx_storage: radio.rx_storage,
            transmit: radio.transmit,
            timer: radio.timer,
            frame: storage.frame,
            station,
            listen_interval,
            security,
            prepared_peer: None,
            selected_rsn: None,
            association: None,
            connected_peer: None,
            pending_keys: None,
            installed_security: None,
            report: StaAttemptReport {
                security: None,
                authentication: None,
                pmksa_resumed: false,
                association: None,
                peer: None,
                wpa2_handshake: None,
                wpa2: None,
                message4: None,
            },
            _join_observer: PhantomData,
        }
    }

    pub const fn report(&self) -> StaAttemptReport {
        self.report
    }

    pub fn take_connected_peer(&mut self) -> Option<ConnectedStaPeer> {
        self.connected_peer.take()
    }

    pub fn take_installed_security(&mut self) -> Option<StaInstalledSecurity> {
        self.installed_security.take()
    }

    pub fn into_parts(
        self,
    ) -> (
        StaAttemptRadio<
            'hardware,
            'transmit,
            'storage,
            H,
            C,
            D,
            T,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        StaAttemptStorage<'scratch>,
        StaAttemptStation,
        StaAttemptSecurity<'security>,
    ) {
        (
            StaAttemptRadio::new(
                self.hardware,
                self.channel,
                self.receive.expect("attempt owner retains RX"),
                self.rx_storage,
                self.transmit,
                self.timer,
            ),
            StaAttemptStorage::new(self.frame),
            self.station,
            self.security,
        )
    }
}
