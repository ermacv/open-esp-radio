//! Executor-independent WPA2-Personal station four-way-handshake runners.
//!
//! This module owns absolute response deadlines, key-publication rollback and
//! the ordering of finite RX, RX restart and EAPOL TX transactions. The ports
//! and values they exchange live in `oer_ieee80211_rsn::runner`; concrete
//! DMA, timer, key-slot and frame-transmit bindings remain in chip/runtime
//! adapters.

use oer_time::{Duration, Instant, Timer};

use oer_ieee80211_rsn::{
    EapolKeyMessage, RsnInterface,
    aes::AsyncRsnKeyUnwrap,
    frames::RsnTxFrame,
    keys::RsnKeyKind,
    runner::{
        RSN_HANDSHAKE_EAPOL_CAPACITY, RsnHandshakeBackend, RsnHandshakeConfig, RsnHandshakeError,
        RsnKeyInstallBackend, RsnKeyInstallError, RsnKeyInstallFailure, RsnKeyInstallMetadata,
        RsnKeyInstallRequestError, RsnTxSequence,
    },
    supplicant::{
        RsnConnectedSupplicant, RsnStaDeadlineEvent, RsnStaKeyInstallRequest, RsnStaReady,
        RsnStaResponseDeadline, RsnStaResponseWait, RsnStaSupplicant, RsnStaSupplicantError,
    },
};

use crate::supplicant::process_frame;

const EAPOL_CAPACITY: usize = RSN_HANDSHAKE_EAPOL_CAPACITY;

/// Protocol state and exact key-install ticket returned with RX stopped.
pub struct RsnPendingKeyInstall {
    supplicant: RsnStaSupplicant,
    request: RsnStaKeyInstallRequest,
    completed_frames: u32,
    message2_transmissions: u16,
}

pub struct RsnCompletedKeyInstall {
    pub message4: RsnTxFrame<EAPOL_CAPACITY>,
    pub connected: RsnConnectedSupplicant,
}

impl RsnPendingKeyInstall {
    pub const fn request(&self) -> &RsnStaKeyInstallRequest {
        &self.request
    }

    pub const fn completed_frames(&self) -> u32 {
        self.completed_frames
    }

    pub const fn message2_transmissions(&self) -> u16 {
        self.message2_transmissions
    }

    pub fn complete(
        self,
        installed: bool,
    ) -> Result<RsnCompletedKeyInstall, RsnStaSupplicantError> {
        let Self {
            mut supplicant,
            request,
            ..
        } = self;
        match supplicant.complete_key_install::<EAPOL_CAPACITY>(request, installed)? {
            RsnStaReady::Transmit(message4) => Ok(RsnCompletedKeyInstall {
                message4,
                connected: supplicant.into_connected()?,
            }),
            _ => Err(RsnStaSupplicantError::UnexpectedAction),
        }
    }
}

pub struct RsnEstablished<Keys> {
    keys: Keys,
    connected: RsnConnectedSupplicant,
    metadata: RsnKeyInstallMetadata,
}

impl<Keys> RsnEstablished<Keys> {
    pub const fn metadata(&self) -> RsnKeyInstallMetadata {
        self.metadata
    }

    pub fn into_parts(self) -> (Keys, RsnConnectedSupplicant) {
        (self.keys, self.connected)
    }
}

/// Owns the security-critical ordering between the supplicant ticket and the
/// chip-specific key/TX backend. It contains no credentials, diagnostics,
/// retry timer or connected-network policy.
pub struct RsnKeyInstallRunner<B> {
    backend: B,
}

impl<B: RsnKeyInstallBackend> RsnKeyInstallRunner<B> {
    pub const fn new(backend: B) -> Self {
        Self { backend }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    fn rollback(
        &mut self,
        keys: B::InstalledKeys,
        failure: RsnKeyInstallFailure<B::Error>,
    ) -> RsnKeyInstallError<B::Error> {
        match self.backend.rollback_keys(keys) {
            Ok(()) => RsnKeyInstallError::Failed(failure),
            Err(rollback) => RsnKeyInstallError::Rollback { failure, rollback },
        }
    }

    pub async fn run(
        &mut self,
        pending: RsnPendingKeyInstall,
    ) -> Result<RsnEstablished<B::InstalledKeys>, RsnKeyInstallError<B::Error>> {
        let request = pending.request();
        let pairwise = request.pairwise();
        let group = request.group();
        let request_error = if pairwise.interface() != RsnInterface::Station {
            Some(RsnKeyInstallRequestError::PairwiseInterface)
        } else if pairwise.kind() != RsnKeyKind::Pairwise {
            Some(RsnKeyInstallRequestError::PairwiseKind)
        } else if group.interface() != RsnInterface::Station {
            Some(RsnKeyInstallRequestError::GroupInterface)
        } else if !matches!(group.kind(), RsnKeyKind::Group { .. }) {
            Some(RsnKeyInstallRequestError::GroupKind)
        } else {
            None
        };
        if let Some(error) = request_error {
            let _ = pending.complete(false);
            return Err(RsnKeyInstallError::Request(error));
        }
        let RsnKeyKind::Group {
            key_id: group_key_id,
            transmit: group_transmit,
        } = group.kind()
        else {
            unreachable!("group kind was validated above")
        };
        let replay_counter = request.replay_counter();
        let mut metadata = RsnKeyInstallMetadata {
            replay_counter,
            encrypted_key_data: request.encrypted_key_data(),
            plain_key_data_len: request.plain_key_data_len(),
            group_key_id,
            group_transmit,
            completed_frames: pending.completed_frames(),
            message2_transmissions: pending.message2_transmissions(),
            message4_len: 0,
        };
        let mut keys = match self.backend.install_keys(request) {
            Ok(keys) => keys,
            Err(error) => {
                let _ = pending.complete(false);
                return Err(RsnKeyInstallError::Install(error));
            }
        };
        let completed = match pending.complete(true) {
            Ok(completed) => completed,
            Err(error) => {
                return Err(self.rollback(keys, RsnKeyInstallFailure::Complete(error)));
            }
        };
        if completed.message4.key_frame().message() != EapolKeyMessage::PairwiseMessage4
            || completed.message4.key_frame().replay_counter() != replay_counter
            || completed.message4.key_frame().protocol_version() != 1
        {
            return Err(self.rollback(keys, RsnKeyInstallFailure::InvalidMessage4));
        }
        metadata.message4_len = completed.message4.as_bytes().len();
        if let Err(error) = self
            .backend
            .transmit_message4(&completed.message4, &mut keys)
            .await
        {
            return Err(self.rollback(keys, RsnKeyInstallFailure::Transmit(error)));
        }
        Ok(RsnEstablished {
            keys,
            connected: completed.connected,
            metadata,
        })
    }
}

/// How often the runner services received frames while it waits for a
/// message. The backends poll RX; an event-driven wait replaces this cadence
/// when the roles run on the lower-MAC port (radio plan 8).
pub const RX_POLL_INTERVAL: Duration = Duration::from_millis(1);

pub struct RsnHandshakeRunner<B, T, U> {
    backend: B,
    timer: T,
    key_unwrap: U,
}

impl<B, T, U> RsnHandshakeRunner<B, T, U>
where
    B: RsnHandshakeBackend,
    T: Timer,
    U: AsyncRsnKeyUnwrap,
{
    pub const fn new(backend: B, timer: T, key_unwrap: U) -> Self {
        Self {
            backend,
            timer,
            key_unwrap,
        }
    }

    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub fn into_parts(self) -> (B, T, U) {
        (self.backend, self.timer, self.key_unwrap)
    }

    async fn stop_receive(&mut self) -> Result<(), RsnHandshakeError<B::Error, U::Error>> {
        self.backend
            .stop_receive()
            .await
            .map_err(RsnHandshakeError::Backend)
    }

    /// Wait for the next receive poll after `previous`, returning its time.
    async fn wait_next_poll(
        &mut self,
        previous: Instant,
    ) -> Result<Instant, RsnHandshakeError<B::Error, U::Error>> {
        let poll = previous
            .checked_add(RX_POLL_INTERVAL)
            .ok_or(RsnHandshakeError::ClockOverflow)?;
        self.timer.wait_until(poll).await;
        Ok(poll)
    }

    async fn transmit_message2(
        &mut self,
        message2: &RsnTxFrame<EAPOL_CAPACITY>,
        sequence: &mut impl RsnTxSequence,
    ) -> Result<(), RsnHandshakeError<B::Error, U::Error>> {
        if message2.key_frame().message() != EapolKeyMessage::PairwiseMessage2 {
            return Err(RsnHandshakeError::InvalidMessage2);
        }
        self.backend
            .transmit_message2(message2, sequence.take_sequence())
            .await
            .map_err(RsnHandshakeError::Backend)
    }

    /// Drive Message 1 and Message 3, returning the exact typed key ticket.
    ///
    /// RX is serviced at the absolute deadline before timeout wins. Message 2
    /// is never retransmitted from a local timer; only a repeated peer Message
    /// 1 can produce another Message 2, preserving the recovered vendor
    /// behavior.
    pub async fn run(
        &mut self,
        config: RsnHandshakeConfig<'_>,
        sequence: &mut impl RsnTxSequence,
    ) -> Result<RsnPendingKeyInstall, RsnHandshakeError<B::Error, U::Error>> {
        let mut supplicant = RsnStaSupplicant::try_new(
            config.local,
            config.authenticator,
            config.supplicant_nonce,
            config.association_security_ies,
            config.authenticator_rsn_ie,
            config.authenticator_rsnxe,
        )
        .map_err(RsnHandshakeError::Create)?;
        let mut completed_frames = 0_u32;
        let mut message2_transmissions = 0_u16;
        let mut poll = self.timer.now();
        let message1_deadline = RsnStaResponseDeadline::start(RsnStaResponseWait::Message1, poll)
            .ok_or(RsnHandshakeError::ClockOverflow)?;

        'message1: loop {
            poll = self.wait_next_poll(poll).await?;
            loop {
                let progress = match self.backend.service_receive().await {
                    Ok(progress) => progress,
                    Err(error) => {
                        self.stop_receive().await?;
                        return Err(RsnHandshakeError::Backend(error));
                    }
                };
                completed_frames = completed_frames.saturating_add(progress.completed_frames);
                if let Some(frame) = progress.eapol {
                    let replay_counter = frame.key_frame().replay_counter();
                    match process_frame(&mut supplicant, frame, config.pmk, &mut self.key_unwrap)
                        .await
                    {
                        Ok(RsnStaReady::Transmit(message2))
                            if message2.key_frame().replay_counter() == replay_counter =>
                        {
                            if let Err(error) = self.backend.restart_receive().await {
                                self.stop_receive().await?;
                                return Err(RsnHandshakeError::Backend(error));
                            }
                            if let Err(error) = self.transmit_message2(&message2, sequence).await {
                                self.stop_receive().await?;
                                return Err(error);
                            }
                            message2_transmissions = message2_transmissions.saturating_add(1);
                            break 'message1;
                        }
                        Ok(RsnStaReady::None) => {}
                        Err(error) if error.is_peer_input_rejection() => {}
                        Err(error) => {
                            self.stop_receive().await?;
                            return Err(RsnHandshakeError::Process(error));
                        }
                        Ok(_) => {
                            self.stop_receive().await?;
                            return Err(RsnHandshakeError::UnexpectedAction);
                        }
                    }
                }
                if !progress.more {
                    break;
                }
            }
            if let RsnStaDeadlineEvent::Expired { wait, elapsed } = message1_deadline.poll(poll) {
                self.stop_receive().await?;
                return Err(RsnHandshakeError::Timeout {
                    wait,
                    elapsed,
                    completed_frames,
                });
            }
        }

        let mut poll = self.timer.now();
        let message3_deadline = RsnStaResponseDeadline::start(RsnStaResponseWait::Message3, poll)
            .ok_or(RsnHandshakeError::ClockOverflow)?;
        loop {
            poll = self.wait_next_poll(poll).await?;
            loop {
                let progress = match self.backend.service_receive().await {
                    Ok(progress) => progress,
                    Err(error) => {
                        self.stop_receive().await?;
                        return Err(RsnHandshakeError::Backend(error));
                    }
                };
                completed_frames = completed_frames.saturating_add(progress.completed_frames);
                if let Some(frame) = progress.eapol {
                    let action = match process_frame(
                        &mut supplicant,
                        frame,
                        config.pmk,
                        &mut self.key_unwrap,
                    )
                    .await
                    {
                        Ok(action) => action,
                        Err(error) if error.is_peer_input_rejection() => RsnStaReady::None,
                        Err(error) => {
                            self.stop_receive().await?;
                            return Err(RsnHandshakeError::Process(error));
                        }
                    };
                    match action {
                        RsnStaReady::Transmit(message2) => {
                            if let Err(error) = self.transmit_message2(&message2, sequence).await {
                                self.stop_receive().await?;
                                return Err(error);
                            }
                            message2_transmissions = message2_transmissions.saturating_add(1);
                        }
                        RsnStaReady::InstallKeys(request) => {
                            self.stop_receive().await?;
                            return Ok(RsnPendingKeyInstall {
                                supplicant,
                                request,
                                completed_frames,
                                message2_transmissions,
                            });
                        }
                        RsnStaReady::None => {}
                        RsnStaReady::RetryInstall(_) | RsnStaReady::Deauthenticate => {
                            self.stop_receive().await?;
                            return Err(RsnHandshakeError::UnexpectedAction);
                        }
                    }
                }
                if !progress.more {
                    break;
                }
            }
            if let RsnStaDeadlineEvent::Expired { wait, elapsed } = message3_deadline.poll(poll) {
                self.stop_receive().await?;
                return Err(RsnHandshakeError::Timeout {
                    wait,
                    elapsed,
                    completed_frames,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
