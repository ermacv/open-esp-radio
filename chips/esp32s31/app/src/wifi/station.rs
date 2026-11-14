//! The Wi-Fi station on the S31: its lifecycle and datapath tasks, its
//! control, and the network device it hands the group.

use super::*;

/// The station's start: what its runner needs from the boot.
pub(crate) struct StationStart {
    pub(crate) runner: wifi::StationRunner<binding::Binding>,
    config: wifi::Config,
    address: [u8; 6],
    nonces: &'static mut wifi::NonceSource,
    /// The accelerator the station unwraps its EAPOL-Key data with.
    aes: oer_chips_esp32s31_platform_runtime::crypto::AesEngine,
    /// Where the lifecycle's half of an installation that did not retire
    /// goes, or the owners still in transit to it after a retire.
    home: EndSender<StationLifecycleLeft>,
}

/// The station lifecycle's half of an installation that did not retire.
pub type StationLifecycleEnd = wifi::UnfinishedLifecycle<binding::Binding>;

/// What the station lifecycle's task leaves: the lifecycle's half of an
/// installation that did not retire, or, after a completed retire, its
/// ends of the queues to the actor when an owner was still in transit in
/// them.
pub(crate) struct StationLifecycleLeft {
    lifecycle: Option<StationLifecycleEnd>,
    in_transit: Option<wifi::LifecycleQueues<binding::Binding>>,
}

/// One half of how the station's installation ended, as it arrived
/// ([`Station::installation_half`]).
#[must_use = "an unfinished installation's half holds its owners"]
pub enum StationEndHalf {
    /// The datapath actor's half: a retire, no installation, or the
    /// installation that did not retire, whole.
    Datapath(wifi::DatapathEnd<binding::Binding>),
    /// The station lifecycle's half.
    Lifecycle {
        /// Its retired lifecycle, when the installation did not retire.
        lifecycle: Option<StationLifecycleEnd>,
        /// After a completed retire, the lifecycle's ends of the queues to
        /// the actor when an owner was still in transit in them.
        in_transit: Option<wifi::LifecycleQueues<binding::Binding>>,
    },
}

/// How the station's installation ended, both halves, as the application
/// put them together from [`Station::installation_half`]: the datapath
/// actor's (a retire, no installation, or the installation that did not
/// retire, whole) and the station lifecycle's (its retired lifecycle when
/// the installation did not retire).
#[must_use = "an unfinished installation holds the port and its owners"]
pub struct StationInstallationEnd {
    /// The datapath actor's half.
    pub datapath: wifi::DatapathEnd<binding::Binding>,
    /// The lifecycle's half, when the installation did not retire.
    pub lifecycle: Option<StationLifecycleEnd>,
    /// After a completed retire, the lifecycle's ends of the queues to the
    /// actor when an owner was still in transit in them.
    pub in_transit: Option<wifi::LifecycleQueues<binding::Binding>>,
}

/// What finishing a late retire hands back: the finished lifecycle half,
/// and the datapath half's ends of the queues (when they held owners) and
/// to the network stack.
#[must_use = "a finished late retire returns the owners still in transit"]
pub struct LateRetireFinished {
    /// The lifecycle's half, finished.
    pub lifecycle: wifi::FinishedLifecycle<binding::Binding>,
    /// The actor's ends of the queues to the lifecycle, when they held
    /// owners.
    pub in_transit: Option<wifi::ActorQueues<binding::Binding>>,
    /// The actor's ends to the network stack.
    pub network: wifi::NetworkSide<binding::Binding>,
}

impl StationInstallationEnd {
    /// Finish an installation whose datapath half retired after its
    /// lifecycle stopped waiting for the release (the stop's drain passed
    /// first): the lifecycle's half ends with the retired installation and
    /// no release runs again. The actual end of the actor's task, beside
    /// the exit the station's task returned, the owners still in transit
    /// and the datapath's network ends come back. Any other end, and a
    /// proof the half refused, comes back unchanged.
    #[allow(
        clippy::result_large_err,
        reason = "an end this does not finish comes back whole, by value"
    )]
    pub fn finish_late_retire(self) -> Result<LateRetireFinished, Self> {
        match self {
            Self {
                datapath:
                    wifi::DatapathEnd::Retired(wifi::RetiredDatapath {
                        proof,
                        in_transit,
                        network,
                    }),
                lifecycle: Some(half),
                // A half that went out holds the lifecycle's queues: none
                // are in transit beside it.
                in_transit: None,
            } => match half.finish(proof) {
                Ok(lifecycle) => Ok(LateRetireFinished {
                    lifecycle,
                    in_transit,
                    network,
                }),
                Err(foreign) => Err(Self {
                    datapath: wifi::DatapathEnd::Retired(wifi::RetiredDatapath {
                        proof: foreign.proof,
                        in_transit,
                        network,
                    }),
                    lifecycle: Some(foreign.half),
                    in_transit: None,
                }),
            },
            other => Err(other),
        }
    }
}

static STATION_END: StaticCell<EndHome<wifi::DatapathEnd<binding::Binding>>> = StaticCell::new();

static STATION_LIFECYCLE_END: StaticCell<EndHome<StationLifecycleLeft>> = StaticCell::new();

/// The datapath actor, which owns the radio, on the station's executor,
/// and the home its end goes to.
pub(crate) struct Datapath {
    datapath: wifi::Datapath<binding::Binding>,
    home: EndSender<wifi::DatapathEnd<binding::Binding>>,
}

/// The datapath actor's task, on the station's executor. The station's
/// task hears its exit and decides; how the installation ended, an
/// unfinished one whole, goes to the application's [`Station`]
/// ([`Station::installation_half`]).
pub(crate) async fn datapath(datapath: Datapath) {
    let Datapath { datapath, home } = datapath;
    let end = observed!(
        Datapath,
        datapath.run(StationObserver::default(), STATION_SINK)
    )
    .await;
    leave(home, end).await;
}

/// The station lifecycle's task: a stop ends it; a restart of the station
/// stays inside the facade. Every other end keeps its typed cause with the
/// control ([`Station::stop`]) and resets nothing: an installation that
/// did not retire comes back whole, the lifecycle's half from here and
/// the actor's from the datapath task ([`Station::installation_half`]);
/// the transmit tokens (tickets and buffers) of one whose retirement was
/// asked ended before it, and come back with neither half.
pub(crate) async fn serve_station(start: StationStart) {
    let StationStart {
        runner,
        config,
        address,
        nonces,
        aes,
        home,
    } = start;
    let profile = wifi::Profile::new(&config, address, Callbacks(nonces), aes, STATION_SINK)
        .unwrap_or_else(|error| failure(error));
    let end = observed!(Station, runner.run(profile)).await;
    leave(
        home,
        StationLifecycleLeft {
            lifecycle: end.lifecycle,
            in_transit: end.in_transit,
        },
    )
    .await;
}

/// A network to join: its SSID and credentials, validated, secrets erased
/// on drop.
pub struct Profile(wifi::StationSettings);

/// Why a profile was refused.
pub use wifi::CredentialsError as ProfileError;

impl Profile {
    /// An open network.
    ///
    /// # Errors
    /// The SSID is empty or longer than 32 bytes.
    pub fn open(ssid: &[u8]) -> Result<Self, ProfileError> {
        wifi::StationSettings::open(ssid).map(Self)
    }

    /// A WPA2-Personal network and its passphrase (8 to 63 characters).
    ///
    /// # Errors
    /// The SSID or the passphrase has the wrong length.
    pub fn wpa2(ssid: &[u8], passphrase: &[u8]) -> Result<Self, ProfileError> {
        wifi::StationSettings::wpa2_passphrase(ssid, passphrase).map(Self)
    }

    /// Experimental, off unless asked: the connect asks an access point
    /// that admits HE20 for HE20 (one spatial stream, HE-MCS 0 to 9, no
    /// optional HE capability, no answer to Trigger frames); another access
    /// point gets the HT request. [`Connected::phy`] names the mode the
    /// station associated in; an access point that answers the HE request
    /// without HE ends the connect with `ConnectError::Failed(HeNotGranted)`.
    pub fn with_he20(self) -> Self {
        Self(self.0.with_he20())
    }
}

/// The network a connect joined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Connected {
    pub bssid: [u8; 6],
    pub channel: u8,
    /// The PHY mode the station associated in.
    pub phy: LinkPhy,
}

/// Why a connect did not join its network. Every cause is the station's own
/// observation; none guesses at the credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectError {
    /// The station refused the request in its state (already connected or
    /// connecting).
    Refused(CommandRefusal),
    /// The attempt ended with this cause: `NotFound`, `AuthenticationFailed`,
    /// `AssociationFailed`, `HeNotGranted` (an HE20 request the access point
    /// did not set up), `HandshakeFailed` (the four-way handshake failed
    /// or timed out), a peer's reason code and the others.
    Failed(DisconnectReason),
    /// The station's command queue is full.
    Busy,
    /// The station stopped.
    Stopped,
    /// The station never started, for this typed cause.
    NotStarted(wifi::StartError),
    /// The profile's credentials could not be prepared.
    Profile(ProfileError),
}

/// A connect's cause as a diagnostic path, which `oer::println!`
/// prints whole: `Failed(Sae(Protocol(Malformed)))`.
impl oer_portable_diagnostics::DiagnosticPath for ConnectError {
    fn push_path(&self, path: &mut oer_portable_diagnostics::DiagPath) {
        match self {
            Self::Refused(refusal) => {
                path.name("Refused");
                oer_portable_diagnostics::DiagnosticPath::push_path(refusal, path);
            }
            Self::Failed(reason) => {
                path.name("Failed");
                oer_portable_diagnostics::DiagnosticPath::push_path(reason, path);
            }
            Self::Busy => path.name("Busy"),
            Self::Stopped => path.name("Stopped"),
            Self::NotStarted(error) => {
                path.name("NotStarted");
                path.name(match error {
                    wifi::StartError::Unavailable(_) => "Unavailable",
                    wifi::StartError::RunnerLost => "RunnerLost",
                });
            }
            Self::Profile(error) => {
                path.name("Profile");
                oer_portable_diagnostics::DiagnosticPath::push_path(error, path);
            }
        }
    }
}

/// A started Wi-Fi station: its control.
pub struct Station {
    control: wifi::Control<'static, Notifies>,
    /// Where the datapath task and the lifecycle's task leave their
    /// halves of how the installation ended, each going out as it comes.
    ends: wifi::EndHalves<'static, wifi::DatapathEnd<binding::Binding>, StationLifecycleLeft>,
    started: bool,
    address: [u8; 6],
    /// How long a stop gives the network to return its receive buffers.
    drain: oer_portable_time::Duration,
}

/// Why a stop did not end with the station stopped: each end of the
/// station its own variant with the value its exit held. An installation
/// that did not retire comes back whole ([`Station::installation_half`]).
pub use wifi::StopError;

impl Station {
    /// Start the station on `wifi`: the radio executor, the actor, the MAC
    /// driver and the station runner. Its tasks run once the executor does.
    /// It unwraps its EAPOL-Key data in its own step on `aes`, the
    /// accelerator, which it owns from here as its synchronous AES engine.
    ///
    /// # Errors
    /// [`Error::InterruptRouted`] when the table routes the AES interrupt;
    /// the station started already, or a binding or storage was refused.
    pub fn start(
        wifi: WifiPeripheral,
        aes: AesPeripheral,
        config: Config,
        boot: &mut Boot,
    ) -> Result<Self, Error> {
        let aes = aes.engine(boot)?;
        let (home, end) = end_home(&STATION_END)?;
        let (lifecycle_home, lifecycle_end) = end_home(&STATION_LIFECYCLE_END)?;
        let started = start_radio(wifi, boot)?;
        let address = started.resources.address;
        let nonces = started.nonces;
        let seed = started.seed;
        let (control, network, transmit, stopper, runners) =
            wifi::station(started.resources, config.chip);
        boot.pending.datapath = Some(Datapath {
            datapath: runners.datapath,
            home,
        });
        boot.pending.station = Some(StationStart {
            runner: runners.station,
            config: config.chip,
            address,
            nonces,
            aes,
            home: lifecycle_home,
        });
        boot.device = Some(Device {
            network: Some((network, transmit, stopper)),
            address,
            seed,
        });
        Ok(Self {
            control,
            ends: wifi::EndHalves::new(end, lifecycle_end),
            started: false,
            address,
            drain: config.drain,
        })
    }

    /// Join the network `profile` names; the future ends with the result of
    /// this connection attempt. Once linked, the station keeps the network
    /// and rejoins it after a lost association; an attempt that ends without
    /// a link leaves the station idle for the next connect.
    ///
    /// # Errors
    /// The station refused the request or the attempt failed, with its
    /// cause.
    pub async fn connect(&mut self, profile: Profile) -> Result<Connected, ConnectError> {
        if !self.started {
            self.control
                .wait_started()
                .await
                .map_err(ConnectError::NotStarted)?;
            self.started = true;
        }
        let credentials = profile.0.prepare().await.map_err(ConnectError::Profile)?;
        let operation = self.control.connect(credentials).await.map_err(control)?;
        match operation.await {
            wifi::OperationEnd::Completed(CommandOutcome::Connected {
                bssid,
                channel,
                phy,
            }) => Ok(Connected {
                bssid,
                channel,
                phy,
            }),
            wifi::OperationEnd::Completed(
                CommandOutcome::ConnectFailed(reason) | CommandOutcome::Disconnected(reason),
            ) => Err(ConnectError::Failed(reason)),
            wifi::OperationEnd::Completed(CommandOutcome::BackgroundScan { .. })
            | wifi::OperationEnd::ServiceEnded => Err(ConnectError::Stopped),
            wifi::OperationEnd::Refused(refusal) => Err(ConnectError::Refused(refusal)),
        }
    }

    /// Leave the network; the station stays idle until the next connect.
    ///
    /// # Errors
    /// As [`Self::connect`].
    pub async fn disconnect(&mut self) -> Result<(), ConnectError> {
        let operation = self.control.disconnect().await.map_err(control)?;
        match operation.await {
            wifi::OperationEnd::Completed(_) => Ok(()),
            wifi::OperationEnd::Refused(refusal) => Err(ConnectError::Refused(refusal)),
            wifi::OperationEnd::ServiceEnded => Err(ConnectError::Stopped),
        }
    }

    /// Leave the network and stop the station for good: the network
    /// stops and returns every receive buffer it holds, then the radio
    /// retires. The configuration's drain ([`Config::with_drain`]) bounds
    /// the radio's stop; a network that returns late past it is no failure,
    /// and the future ends once that return came. The future ends once the station stopped; a later connect
    /// answers [`ConnectError::Stopped`], and the network's waits end
    /// with its stop. A second stop ends with the first one's result.
    ///
    /// # Errors
    /// [`StopError::Failed`]: the station ended by that failure instead;
    /// [`StopError::Unavailable`]: it never started;
    /// [`StopError::RunnerLost`]: its runner ended without an exit;
    /// [`StopError::ReleaseFailed`]: it stopped and its radio's release
    /// failed, nothing reset; [`StopError::NetworkLost`]: the network's stop
    /// was never confirmed.
    pub async fn stop(&mut self) -> Result<(), StopError> {
        let deadline = {
            use oer_portable_time::Clock as _;
            EmbassyClock.now().saturating_add(self.drain)
        };
        self.control.stop(deadline);
        wifi::stop_result(self.control.wait_stopped().await)
    }

    /// The next half of how the station's installation ended, as it
    /// arrives, the lifecycle's first when both are there: the datapath
    /// actor's half (a retire, no installation, or the installation that
    /// did not retire, whole: [`wifi::DatapathEnd::Unfinished`], the actor
    /// with its port, the item a port call could not keep, an attempt
    /// returned unaccepted, the cleanup's receipt, the cause and the
    /// phase) and the lifecycle's half of an installation that did not
    /// retire (its station, places, arbiter, issuer and contexts), which
    /// the application holds from here. `None` once both went out.
    ///
    /// Neither half waits for the other: the datapath's task may end only
    /// after the application served the lifecycle's half (one whose places
    /// for installations' news are full,
    /// [`wifi::LateRead::Full`]). A wait that is dropped takes nothing.
    pub async fn installation_half(&mut self) -> Option<StationEndHalf> {
        Some(match self.ends.next().await? {
            wifi::EndHalf::Datapath(datapath) => StationEndHalf::Datapath(datapath),
            wifi::EndHalf::Lifecycle(StationLifecycleLeft {
                lifecycle,
                in_transit,
            }) => StationEndHalf::Lifecycle {
                lifecycle,
                in_transit,
            },
        })
    }

    /// How the station ended, once it did, read without waiting: `None`
    /// while it runs, `Some(Ok(()))` after a stop that ended with it
    /// stopped, otherwise its end as [`Self::stop`] would answer it.
    pub fn ended(&self) -> Option<Result<(), StopError>> {
        self.control.exit().map(wifi::stop_result)
    }

    /// The link's state and epoch now.
    pub fn link(&self) -> LinkSnapshot {
        self.control.link()
    }

    /// How often the station restarted without a reset, and why it last did.
    pub fn restarts(&self) -> wifi::Restarts {
        self.control.restarts()
    }

    /// The network the station is linked to now: its link is up and its last
    /// notice was the connection.
    pub fn linked(&self) -> Option<Connected> {
        let status = self.control.status();
        match (status.link.state, status.notice) {
            (
                LinkState::Up,
                Some(Notice::Connected {
                    bssid,
                    channel,
                    phy,
                }),
            ) => Some(Connected {
                bssid,
                channel,
                phy,
            }),
            _ => None,
        }
    }

    /// The station's own MAC address.
    pub fn address(&self) -> [u8; 6] {
        self.address
    }
}

fn control(error: wifi::ControlError) -> ConnectError {
    match error {
        wifi::ControlError::QueueFull => ConnectError::Busy,
        wifi::ControlError::Stopped => ConnectError::Stopped,
    }
}
