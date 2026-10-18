use oer_hil_protocol::base::RejectReason;
use oer_hil_protocol::base::{
    GetHello, GetImageKeys, GetLinkHealth, GetPostMortemCheckpoints, Hello, ImageKeyPage,
    PostMortemCheckpoints, Rejected,
};
use oer_hil_protocol::{Endpoint, Message};

use super::*;
use oer_hil_protocol::DeviceImageKeys;

#[derive(serde::Serialize)]
pub struct Observation {
    boot_id: u64,
    capabilities: DeviceImageKeys,
    operation: OperationStatus,
    link: LinkHealth,
}

impl SerialCapture {
    pub fn observe(&self, timeout: Duration) -> Result<Observation> {
        let capabilities = self.discover(timeout)?;
        let boot_id = self
            .latest_boot_id()
            .ok_or("discovery omitted the target boot identity")?;
        let oer_hil_protocol::network::Status(operation) =
            self.request(0, oer_hil_protocol::network::GetStatus, timeout)?;
        let link = self.query_link_health(timeout)?;
        Ok(Observation {
            boot_id,
            capabilities,
            operation,
            link,
        })
    }

    /// The capabilities of whatever boot runs, found without knowing it.
    pub fn discover(&self, timeout: Duration) -> Result<DeviceImageKeys> {
        let reply = self.exchange(0, 0, GetHello, timeout)?;
        let Some(hello) = reply.decode::<Hello>().filter(|_| reply.boot_id != 0) else {
            return Err(format!(
                "device answered read-only discovery with {}; firmware must support boot discovery",
                reply.path()
            )
            .into());
        };
        self.image_keys_of(reply.boot_id, hello, timeout)
    }

    /// Ask a boot whose Hello the link lost for it again.
    ///
    /// The USB Serial/JTAG can drop the first bytes of a frame, so the
    /// boot's unsolicited Hello never decodes while the boot runs and
    /// answers. Only a capture that began at a reset, whose console already
    /// shows a boot starting and which has seen no boot yet, may begin that
    /// boot with the answer, and only while the answer is among the boot's
    /// first messages; the capture then records the solicited Hello.
    /// Otherwise the missing Hello stands as the failure.
    fn solicit_lost_hello(&self, timeout: Duration) -> Result<DeviceImageKeys> {
        const MISSING: &str = "device did not publish a HIL protocol hello";
        {
            let booted = console_shows_boot(
                &self
                    .bytes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            let mut state = self
                .protocol
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.health.origin != CaptureOrigin::Boot
                || state.health.boot_id.is_some()
                || !booted
            {
                return Err(MISSING.into());
            }
            state.health.accept_solicited_hello = true;
        }
        let answer = self.exchange(0, 0, GetHello, timeout);
        let solicited = {
            let mut state = self
                .protocol
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.health.accept_solicited_hello = false;
            state.health.solicited_hello
        };
        let answer =
            answer.map_err(|error| format!("{MISSING}, nor answered a hello request: {error}"))?;
        match (answer.decode::<Hello>(), solicited) {
            (Some(hello), Some(solicited)) => {
                eprintln!(
                    "hil: the link lost the boot's hello; its answer to the hello request at \
                     target message {} began the boot",
                    solicited.message_sequence
                );
                self.image_keys_of(answer.boot_id, hello, timeout)
            }
            _ => Err(format!("{MISSING}, and its hello answer began no fresh boot").into()),
        }
    }

    /// The capability set `hello` of boot `boot_id` announces, page by page.
    fn image_keys_of(
        &self,
        boot_id: u64,
        hello: Hello,
        timeout: Duration,
    ) -> Result<DeviceImageKeys> {
        let mut pages = Vec::new();
        let mut first = 0_u16;
        while first < hello.keys {
            let page: ImageKeyPage =
                self.request_boot(boot_id, 0, GetImageKeys { first }, timeout)?;
            if page.first != first || page.keys.is_empty() {
                return Err(format!("invalid capability page {first}").into());
            }
            first += page.keys.len() as u16;
            pages.push(page);
        }
        Ok(DeviceImageKeys::assemble(hello, &pages)?)
    }

    /// The current image's capabilities, from its boot's Hello.
    pub fn request_image_keys(&self, timeout: Duration) -> Result<DeviceImageKeys> {
        let Some(hello) =
            self.wait_for_message_after(0, timeout, |message| message.is::<Hello>())?
        else {
            return self.solicit_lost_hello(timeout);
        };
        let announced = hello.decode::<Hello>().ok_or("undecodable hello")?;
        self.image_keys_of(hello.boot_id, announced, timeout)
    }

    /// Sends `body` to the current boot and returns its response, or the
    /// reason the device refused it. Every request of every family goes
    /// through this exchange (or [`Self::request`]); the link keeps no
    /// per-message wrappers.
    pub fn call<E: Endpoint>(
        &self,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<std::result::Result<E::Response, RejectReason>> {
        Ok(self.call_answered(session_id, body, timeout)?.1)
    }

    /// [`Self::call`] addressed to boot `boot_id`, a boot the caller
    /// observed: a later boot never answers it.
    pub fn call_boot<E: Endpoint>(
        &self,
        boot_id: u64,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<std::result::Result<E::Response, RejectReason>> {
        if boot_id == 0 {
            return Err(format!("{} requires an observed boot identity", E::PATH).into());
        }
        Ok(self.call_identified(boot_id, session_id, body, timeout)?.1)
    }

    /// [`Self::call`], with the reply's header: its request identifier is
    /// the one the device's later messages about this request carry.
    pub fn call_answered<E: Endpoint>(
        &self,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<(Received, std::result::Result<E::Response, RejectReason>)> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("HIL protocol hello disappeared before request")?;
        self.call_identified(boot_id, session_id, body, timeout)
    }

    /// The exchange of `body` with boot `boot_id`, with the reply's header.
    fn call_identified<E: Endpoint>(
        &self,
        boot_id: u64,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<(Received, std::result::Result<E::Response, RejectReason>)> {
        let reply = self.exchange(boot_id, session_id, body, timeout)?;
        if let Some(response) = reply.decode::<E::Response>() {
            return Ok((reply, Ok(response)));
        }
        match reply.decode::<Rejected>() {
            Some(Rejected(reason)) => Ok((reply, Err(reason))),
            None => Err(format!("device answered {} with {}", E::PATH, reply.path()).into()),
        }
    }

    /// Sends `body` to the current boot and returns its response; a refusal
    /// is an error naming the request.
    pub fn request<E: Endpoint>(
        &self,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<E::Response> {
        self.call(session_id, body, timeout)?
            .map_err(|reason| format!("device rejected {}: {reason:?}", E::PATH).into())
    }

    /// [`Self::request`] addressed to boot `boot_id`; see [`Self::call_boot`].
    pub fn request_boot<E: Endpoint>(
        &self,
        boot_id: u64,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<E::Response> {
        self.call_boot(boot_id, session_id, body, timeout)?
            .map_err(|reason| format!("device rejected {}: {reason:?}", E::PATH).into())
    }

    /// Sends `body` to boot `boot_id` (0: whichever boot runs) and returns
    /// the device's reply to it.
    fn exchange<E: Endpoint>(
        &self,
        boot_id: u64,
        session_id: u64,
        body: E,
        timeout: Duration,
    ) -> Result<Received> {
        self.check_link()?;
        let request_id = self.next_host_sequence.fetch_add(1, Ordering::Relaxed);
        let event_count = self.event_cursor();
        let command = Envelope::new(boot_id, request_id, session_id, request_id, body);
        let mut encoder = FrameEncoder::new();
        let frame = encoder
            .encode(&command)
            .map_err(|error| format!("cannot encode HIL command: {error}"))?
            .to_vec();
        self.outbound
            .send(Zeroizing::new(frame))
            .map_err(|_| LinkError::transport("serial worker stopped before HIL command"))?;
        self.worker_wake.wake()?;
        self.protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sent
            .push(super::SentCommand {
                request_id,
                session_id,
                path: E::PATH,
                host_sent_unix_micros: super::host_unix_micros(),
            });
        // Other messages about this request, such as the evidence a replay
        // publishes before its terminal response, are not its reply.
        self.wait_for_message_after(event_count, timeout, |message| {
            command_response_matches(
                message,
                if boot_id == 0 {
                    message.boot_id
                } else {
                    boot_id
                },
                session_id,
                request_id,
            ) && (message.is::<E::Response>() || message.is::<Rejected>())
        })?
        .ok_or_else(|| format!("device did not answer {}", E::PATH).into())
    }

    /// Sends command `body`, which the device accepts at once and completes
    /// later with messages correlated to its request; see
    /// [`Self::wait_command`].
    pub fn command<E: Endpoint<Response = oer_hil_protocol::base::Accepted>>(
        &self,
        body: E,
        timeout: Duration,
    ) -> Result<CommandHandle> {
        let first_event = self.event_cursor();
        let (reply, outcome) = self.call_answered(0, body, timeout)?;
        outcome.map_err(|reason| format!("device rejected {}: {reason:?}", E::PATH))?;
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let accepted_offset = state.messages[first_event..]
            .iter()
            .position(|message| {
                message.boot_id == reply.boot_id
                    && message.session_id == 0
                    && message.request_id == reply.request_id
                    && message.message_sequence == reply.message_sequence
            })
            .ok_or("an accepted command disappeared from the capture")?;
        Ok(CommandHandle {
            path: E::PATH,
            boot_id: reply.boot_id,
            request_id: reply.request_id,
            first_event: first_event + accepted_offset + 1,
        })
    }

    /// The `M` that completes the command of `handle`; the command's typed
    /// failure (`network::Failed`) is an error.
    pub fn wait_command<M: Message>(&self, handle: CommandHandle, timeout: Duration) -> Result<M> {
        let message = self
            .wait_for_message_after(handle.first_event, timeout, |message| {
                handle.correlates(message)
                    && (message.is::<M>() || message.is::<oer_hil_protocol::network::Failed>())
            })?
            .ok_or_else(|| format!("device did not complete {} with {}", handle.path, M::PATH))?;
        if let Some(oer_hil_protocol::network::Failed(reason)) = message.decode() {
            return Err(format!(
                "target operation {} ({}) failed: {reason:?}",
                handle.request_id, handle.path
            )
            .into());
        }
        Ok(message.decode::<M>().expect("the accepted message decodes"))
    }

    /// The messages of this capture from `first` on, with the link's state
    /// checked first: what a family's own evidence analysis reads.
    pub fn messages_since<T>(
        &self,
        first: usize,
        read: impl FnOnce(&[Received]) -> T,
    ) -> Result<T> {
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.check()?;
        Ok(read(state.messages.get(first..).unwrap_or_default()))
    }

    /// A fresh identity for a session of this capture.
    pub fn next_session_id(&self) -> u64 {
        self.next_session_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn query_link_health(&self, timeout: Duration) -> Result<LinkHealth> {
        self.request(0, GetLinkHealth, timeout)
    }

    /// The previous boot's post-mortem checkpoints, oldest first: `count`
    /// of them, as its boot evidence reports, fetched page by page.
    pub fn post_mortem_checkpoints(
        &self,
        count: u8,
    ) -> Result<Vec<oer_hil_protocol::base::Checkpoint>> {
        let mut checkpoints = Vec::with_capacity(usize::from(count));
        while checkpoints.len() < usize::from(count) {
            let first = checkpoints.len() as u8;
            let page: PostMortemCheckpoints = self.request(
                0,
                GetPostMortemCheckpoints { first },
                Duration::from_secs(5),
            )?;
            if page.first != first || page.checkpoints.is_empty() {
                return Err(format!("invalid post-mortem page {first}: {page:?}").into());
            }
            checkpoints.extend(page.checkpoints);
        }
        checkpoints.truncate(usize::from(count));
        Ok(checkpoints)
    }

    /// Arm the program-counter profile once the boot's hello arrived; the
    /// capture drains it into `profile.json` when it finishes.
    pub fn profiled(mut self, profile: Profile) -> Result<Self> {
        self.wait_for_message_after(0, PROTOCOL_READY_TIMEOUT, |message| message.is::<Hello>())?
            .ok_or("device did not publish a HIL protocol hello before the profile")?;
        match self.call(
            0,
            oer_hil_protocol::telemetry::ControlProfile(profile.control),
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::telemetry::ProfileState(status)) if status.armed => {}
            response => {
                return Err(format!("the image did not arm the profile: {response:?}").into());
            }
        }
        self.profile = Some(profile);
        Ok(self)
    }

    /// The closed window's status and each hart's raw `(pc, ra)` samples;
    /// the profile is disarmed afterwards.
    pub(crate) fn drain_profile(&self) -> Result<DrainedProfile> {
        let status = match self.call(
            0,
            oer_hil_protocol::telemetry::ControlProfile(
                oer_hil_protocol::telemetry::ProfileControl::Status,
            ),
            Duration::from_secs(5),
        )? {
            Ok(oer_hil_protocol::telemetry::ProfileState(status)) => status,
            response => return Err(format!("profile status rejected: {response:?}").into()),
        };
        let mut samples = [Vec::new(), Vec::new()];
        if !status.open {
            for (hart, pairs) in samples.iter_mut().enumerate() {
                while (pairs.len() as u32) < status.samples[hart] {
                    let first = pairs.len() as u32;
                    match self.call(
                        0,
                        oer_hil_protocol::telemetry::GetProfileSamples {
                            hart: hart as u8,
                            first,
                        },
                        Duration::from_secs(5),
                    )? {
                        Ok(oer_hil_protocol::telemetry::ProfileSamples(page))
                            if page.first == first && !page.samples.is_empty() =>
                        {
                            pairs.extend(page.samples);
                        }
                        response => {
                            return Err(format!(
                                "profile page {first} of hart {hart} rejected: {response:?}"
                            )
                            .into());
                        }
                    }
                }
            }
        }
        let _ = self.call(
            0,
            oer_hil_protocol::telemetry::ControlProfile(
                oer_hil_protocol::telemetry::ProfileControl::Disarm,
            ),
            Duration::from_secs(5),
        );
        Ok((status, samples))
    }

    /// Console bytes captured so far: a mark for
    /// [`Self::console_shows_boot_since`].
    pub fn console_length(&self) -> usize {
        self.bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Whether the console bytes after `mark` show a chip starting: the ROM
    /// banner or the bootloader's lines.
    pub fn console_shows_boot_since(&self, mark: usize) -> bool {
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        console_shows_boot(bytes.get(mark..).unwrap_or_default())
    }

    pub fn latest_boot_id(&self) -> Option<u64> {
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        latest_boot_id_in(&state.messages)
    }

    pub fn observed_protocol_ipv4(
        &self,
        network_interface: WifiNetworkInterface,
    ) -> Option<Ipv4Addr> {
        let state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let messages = &state.messages;
        let boot_id = latest_boot_id_in(messages)?;
        messages
            .iter()
            .rev()
            .filter(|message| message.boot_id == boot_id)
            .find_map(|message| match message.decode() {
                Some(oer_hil_protocol::network::Ready(network))
                    if network.network_interface == network_interface =>
                {
                    Some(Ipv4Addr::from(network.address))
                }
                _ => None,
            })
    }

    /// The cursor of the next message this capture decodes: waits from it
    /// see only what arrives later.
    pub fn event_cursor(&self) -> usize {
        self.protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .messages
            .len()
    }

    /// The link's failure, a closed capture or a cancellation, as an error.
    pub fn check_link(&self) -> Result<()> {
        oer_process::check_cancelled()?;
        self.protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .check()
    }

    /// The first `M` from message `start` on that `accept` takes, with its
    /// header, waiting up to `timeout`.
    pub fn wait_for_after<M: Message>(
        &self,
        start: usize,
        timeout: Duration,
        accept: impl Fn(&Received, &M) -> bool,
    ) -> Result<Option<(Received, M)>> {
        let mut cursor = start;
        self.wait_for_cursor(&mut cursor, timeout, accept)
    }

    /// [`Self::wait_for_after`] from `cursor`, which it advances past what
    /// it read.
    pub fn wait_for_cursor<M: Message>(
        &self,
        cursor: &mut usize,
        timeout: Duration,
        accept: impl Fn(&Received, &M) -> bool,
    ) -> Result<Option<(Received, M)>> {
        Ok(self
            .wait_for_message_cursor(cursor, timeout, |message| {
                message
                    .decode::<M>()
                    .is_some_and(|body| accept(message, &body))
            })?
            .map(|message| {
                let body = message.decode::<M>().expect("the accepted message decodes");
                (message, body)
            }))
    }

    /// The first message from message `start` on that `predicate` accepts,
    /// waiting up to `timeout`.
    pub fn wait_for_message_after(
        &self,
        start: usize,
        timeout: Duration,
        predicate: impl Fn(&Received) -> bool,
    ) -> Result<Option<Received>> {
        let mut cursor = start;
        self.wait_for_message_cursor(&mut cursor, timeout, predicate)
    }

    fn wait_for_message_cursor(
        &self,
        cursor: &mut usize,
        timeout: Duration,
        predicate: impl Fn(&Received) -> bool,
    ) -> Result<Option<Received>> {
        let deadline = crate::transport::events::deadline_after(timeout);
        let mut state = self
            .protocol
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            oer_process::check_cancelled()?;
            state.check()?;
            if let Some((relative, message)) = state
                .messages
                .get(*cursor..)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .find(|(_, message)| predicate(message))
            {
                *cursor += relative + 1;
                return Ok(Some(message.clone()));
            }
            *cursor = state.messages.len();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let (next, _) = self
                .protocol
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
        }
    }
}

/// The boot of the newest Hello among `messages`: the boot they belong to now.
pub fn latest_boot_id_in(messages: &[Received]) -> Option<u64> {
    messages.iter().rev().find_map(|message| {
        message
            .is::<oer_hil_protocol::base::Hello>()
            .then_some(message.boot_id)
    })
}

pub(super) fn decode_counters_are_clean(counters: DecodeCounters) -> bool {
    counters.cobs_errors == 0
        && counters.too_short == 0
        && counters.header_errors == 0
        && counters.framing_version_errors == 0
        && counters.message_kind_errors == 0
        && counters.payload_length_errors == 0
        && counters.checksum_errors == 0
        && counters.deserialize_errors == 0
        && counters.overflows == 0
}

pub(super) fn validate_target_link_health(health: LinkHealth) -> Result<()> {
    if health.rx_cobs_errors != 0
        || health.rx_checksum_errors != 0
        || health.rx_decode_errors != 0
        || health.rx_overflows != 0
        || health.tx_dropped != 0
    {
        return Err(LinkError::protocol(format!(
            "target reported unhealthy serialized console transport: {health:?}"
        ))
        .into());
    }
    Ok(())
}

/// A drained profile: the target's status and each hart's raw `(pc, ra)`
/// samples.
pub(crate) type DrainedProfile = (
    oer_hil_protocol::telemetry::ProfileStatus,
    [Vec<(u32, u32)>; 2],
);

/// Whether console bytes show a chip starting: the ROM banner or the ESP-IDF
/// bootloader's lines.
fn console_shows_boot(bytes: &[u8]) -> bool {
    [b"ESP-ROM:".as_slice(), b" boot: ".as_slice()]
        .iter()
        .any(|marker| bytes.windows(marker.len()).any(|window| window == *marker))
}
