//! The base module every image serves, and the console's request intake.
//!
//! A console feeds the bytes it reads to [`Intake::receive`]. Each complete
//! request comes back either answered — by the base module, or refused
//! because it names another boot, carries a session, or has a key the image
//! does not serve — or as one of the image's own requests to serve.

use oer_hil_protocol::base::{
    BootEvidence, GetBootStatus, GetHello, GetImageKeys, GetLinkHealth, GetPostMortemCheckpoints,
    Hello, ImageKeySet, LinkHealth, PostMortemCheckpoints, RejectReason, Rejected,
};
use oer_hil_protocol::{
    DecodeCounters, DecodeError, Endpoint, Frame, FrameDecoder, Key, Message, RequestIdentity,
    WireKind,
};

/// What the base module reads from the platform.
pub trait Platform {
    fn boot_evidence(&self) -> BootEvidence;
    /// Acknowledge only after the complete boot-evidence frame and newline
    /// have been written. Enqueuing or a refused write is not success.
    fn boot_evidence_written(&self);
    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints;
}

/// The image's own requests, beyond the base module.
pub trait Requests: Sized {
    /// Whether a request of this image may belong to a session; the base
    /// module's never do.
    const SESSIONS: bool;

    /// `frame` as one of the image's requests: `None` when its key is none
    /// of them.
    fn decode(frame: &Frame<'_>) -> Option<Result<Self, DecodeError>>;
}

/// What a mounted service puts on the wire: the keys of the endpoints it
/// serves, of their responses, of the topics it publishes and of its
/// properties. An image's key set is the union of its services' keys
/// ([`image_keys!`](crate::image_keys)).
pub trait Service {
    const KEYS: &'static [Key];
    /// The keys of the endpoints it serves: no two services of an image
    /// serve the same one ([`image_keys!`](crate::image_keys) checks it).
    const REQUESTS: &'static [Key];
}

/// The base module the intake serves for every image: discovery, image
/// keys, boot status, post-mortem checkpoints, link health and refusal.
pub enum Base {}

impl Service for Base {
    const KEYS: &'static [Key] = &[
        GetHello::KEY,
        <GetHello as Endpoint>::Response::KEY,
        GetImageKeys::KEY,
        <GetImageKeys as Endpoint>::Response::KEY,
        GetBootStatus::KEY,
        <GetBootStatus as Endpoint>::Response::KEY,
        GetPostMortemCheckpoints::KEY,
        <GetPostMortemCheckpoints as Endpoint>::Response::KEY,
        GetLinkHealth::KEY,
        <GetLinkHealth as Endpoint>::Response::KEY,
        Rejected::KEY,
    ];
    const REQUESTS: &'static [Key] = &[
        GetHello::KEY,
        GetImageKeys::KEY,
        GetBootStatus::KEY,
        GetPostMortemCheckpoints::KEY,
        GetLinkHealth::KEY,
    ];
}

/// An image that serves the base module alone.
pub enum NoRequests {}

impl Requests for NoRequests {
    const SESSIONS: bool = false;

    fn decode(_: &Frame<'_>) -> Option<Result<Self, DecodeError>> {
        None
    }
}

/// What the console sends back without the image's involvement.
#[derive(Clone, Debug)]
pub enum Answer {
    Hello(Hello),
    /// The image key page from this key on.
    ImageKeys(u16),
    /// The platform's boot evidence.
    Boot,
    /// The platform's post-mortem checkpoints from this one on.
    PostMortem(u8),
    Link(LinkHealth),
    Rejected(RejectReason),
}

/// One complete request.
pub enum Request<C> {
    /// Send this answer.
    Answer(RequestIdentity, Answer),
    /// Serve this request of the image.
    Serve(RequestIdentity, C),
}

/// What the console sent, for the link-health answer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sent {
    pub frames: u32,
    pub dropped: u32,
    pub notes_dropped: u32,
}

/// The link health of a console that decoded `received` and sent `sent`.
pub fn link_health(received: DecodeCounters, sent: Sent) -> LinkHealth {
    LinkHealth {
        rx_frames: received.frames,
        rx_cobs_errors: received.cobs_errors,
        rx_checksum_errors: received.checksum_errors,
        rx_decode_errors: received
            .deserialize_errors
            .saturating_add(received.header_errors)
            .saturating_add(received.framing_version_errors)
            .saturating_add(received.message_kind_errors)
            .saturating_add(received.payload_length_errors)
            .saturating_add(received.too_short),
        rx_overflows: received.overflows,
        tx_frames: sent.frames,
        tx_dropped: sent.dropped,
        notes_dropped: sent.notes_dropped,
    }
}

/// The console's intake of one boot.
pub struct Intake<'a> {
    boot_id: u64,
    image_keys: ImageKeySet<'a>,
    hello: Hello,
    decoder: FrameDecoder,
}

impl<'a> Intake<'a> {
    /// The intake of boot `boot_id`, whose image serves `image_keys` and
    /// takes command payloads of up to `maximum_payload_bytes`.
    pub fn new(boot_id: u64, image_keys: ImageKeySet<'a>, maximum_payload_bytes: u16) -> Self {
        Self {
            boot_id,
            image_keys,
            hello: image_keys.hello(maximum_payload_bytes),
            decoder: FrameDecoder::new(),
        }
    }

    /// What the image serves.
    pub const fn image_keys(&self) -> ImageKeySet<'a> {
        self.image_keys
    }

    /// The first frame of the boot.
    pub const fn hello(&self) -> Hello {
        self.hello
    }

    pub const fn counters(&self) -> DecodeCounters {
        self.decoder.counters()
    }

    /// Feeds `bytes` and hands each complete request to `handle`, in order.
    pub fn receive_each<C: Requests>(
        &mut self,
        bytes: &[u8],
        sent: Sent,
        mut handle: impl FnMut(Request<C>),
    ) {
        // One byte at a time, at most one frame completes per feed, so an
        // answer that reports the counters reports them as of its request.
        for byte in bytes {
            if let Some(request) = self.receive_byte(*byte, sent) {
                handle(request);
            }
        }
    }

    /// Feeds `bytes` and returns the last complete request among them; a
    /// console that reads one byte at a time sees every request.
    pub fn receive<C: Requests>(&mut self, bytes: &[u8], sent: Sent) -> Option<Request<C>> {
        let mut last = None;
        self.receive_each(bytes, sent, |request| last = Some(request));
        last
    }

    fn receive_byte<C: Requests>(&mut self, byte: u8, sent: Sent) -> Option<Request<C>> {
        let boot_id = self.boot_id;
        let hello = self.hello;
        let mut request = None;
        let mut payload_error = false;
        self.decoder.feed(WireKind::Command, &[byte], |frame| {
            let Ok(frame) = frame else { return };
            let identity = frame.identity();
            let answer = |answer| Some(Request::Answer(identity, answer));
            request = match serve(&frame, boot_id, hello) {
                Some(Served::Answer(served)) => answer(served),
                Some(Served::PayloadError) => {
                    payload_error = true;
                    answer(Answer::Rejected(RejectReason::InvalidConfiguration))
                }
                None => match C::decode(&frame) {
                    None => answer(Answer::Rejected(RejectReason::Unsupported)),
                    Some(Err(_)) => {
                        payload_error = true;
                        answer(Answer::Rejected(RejectReason::InvalidConfiguration))
                    }
                    Some(Ok(_)) if !C::SESSIONS && frame.session_id != 0 => {
                        answer(Answer::Rejected(RejectReason::InvalidState))
                    }
                    Some(Ok(own)) => Some(Request::Serve(identity, own)),
                },
            };
        });
        if payload_error {
            self.decoder.count_payload_error();
        }
        // Link health counts the request that asks for it.
        if let Some(Request::Answer(_, Answer::Link(link))) = &mut request {
            *link = link_health(self.decoder.counters(), sent);
        }
        request
    }
}

enum Served {
    Answer(Answer),
    PayloadError,
}

/// The base module's answer to `frame`, or `None` for another module's
/// request.
fn serve(frame: &Frame<'_>, boot_id: u64, hello: Hello) -> Option<Served> {
    if let Err(reason) = oer_hil_protocol::base::validate_target(frame, boot_id) {
        return Some(Served::Answer(Answer::Rejected(reason)));
    }
    let base = [
        GetHello::KEY,
        GetImageKeys::KEY,
        GetBootStatus::KEY,
        GetPostMortemCheckpoints::KEY,
        GetLinkHealth::KEY,
    ];
    if base.contains(&frame.key) && frame.session_id != 0 {
        return Some(Served::Answer(Answer::Rejected(RejectReason::InvalidState)));
    }
    let answer = if let Some(request) = frame.decode::<GetHello>() {
        request.map(|_| Answer::Hello(hello))
    } else if let Some(request) = frame.decode::<GetImageKeys>() {
        request.map(|request| Answer::ImageKeys(request.body.first))
    } else if let Some(request) = frame.decode::<GetBootStatus>() {
        request.map(|_| Answer::Boot)
    } else if let Some(request) = frame.decode::<GetPostMortemCheckpoints>() {
        request.map(|request| Answer::PostMortem(request.body.first))
    } else {
        frame
            .decode::<GetLinkHealth>()?
            .map(|_| Answer::Link(link_health(DecodeCounters::default(), Sent::default())))
    };
    Some(answer.map_or(Served::PayloadError, Served::Answer))
}

/// Declares a service's requests: an enum with one variant per endpoint it
/// serves, decoded by key, and the service's [`Service::KEYS`]: each
/// endpoint's key, its response's key and the key of every message in its
/// optional `publishes` list (unsolicited topics and properties).
///
/// ```ignore
/// oer_hil_agent::requests! {
///     /// The requests this image serves.
///     pub enum Request (sessions = false) {
///         Reset(oer_hil_protocol::lifecycle::RequestReset),
///     }
///     publishes [oer_hil_protocol::base::Hello]
/// }
/// ```
///
/// `sessions` says whether a request may belong to a session; a variant
/// and a published message take `#[cfg(...)]` attributes only.
#[macro_export]
macro_rules! requests {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident (sessions = $sessions:literal) {
            $($(#[cfg($cfg:meta)])* $variant:ident($ty:ty)),* $(,)?
        }
        $(publishes [$($(#[cfg($published_cfg:meta)])* $published:ty),* $(,)?])?
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $($(#[cfg($cfg)])* $variant($ty),)*
        }

        impl $crate::base::Requests for $name {
            const SESSIONS: bool = $sessions;

            fn decode(
                frame: &$crate::__protocol::Frame<'_>,
            ) -> Option<Result<Self, $crate::__protocol::DecodeError>> {
                $(
                    $(#[cfg($cfg)])*
                    if let Some(request) = frame.decode::<$ty>() {
                        return Some(request.map(|envelope| Self::$variant(envelope.body)));
                    }
                )*
                None
            }
        }

        impl $crate::base::Service for $name {
            const KEYS: &'static [$crate::__protocol::Key] = &[
                $(
                    $(#[cfg($cfg)])*
                    <$ty as $crate::__protocol::Message>::KEY,
                    $(#[cfg($cfg)])*
                    <<$ty as $crate::__protocol::Endpoint>::Response as $crate::__protocol::Message>::KEY,
                )*
                $($(
                    $(#[cfg($published_cfg)])*
                    <$published as $crate::__protocol::Message>::KEY,
                )*)?
            ];
            const REQUESTS: &'static [$crate::__protocol::Key] = &[
                $(
                    $(#[cfg($cfg)])*
                    <$ty as $crate::__protocol::Message>::KEY,
                )*
            ];
        }
    };
}

/// Declares a service whose every endpoint has a handler: the
/// [`requests!`](crate::requests) enum and keys, a handler trait with one
/// required method per endpoint, and `serve`, which awaits the handler of a
/// request and publishes its response as the request's answer. An image
/// implements the trait; a missing or extra method does not compile.
///
/// ```ignore
/// oer_hil_agent::service! {
///     /// The AES service's requests.
///     pub enum Request (sessions = false, handler = Handler) {
///         UnwrapKey(UnwrapKey) => unwrap_key,
///     }
/// }
/// ```
///
/// A handler that cannot answer still returns its endpoint's response type:
/// failures belong in that type, so every request is answered.
///
/// An image whose handler leaves an endpoint out does not compile:
///
/// ```compile_fail
/// # mod wire {
/// #     use oer_hil_protocol::{Endpoint, Message, WireKind};
/// #     #[derive(serde::Serialize, serde::Deserialize, postcard_schema::Schema)]
/// #     pub struct Ask;
/// #     impl Message for Ask { const PATH: &'static str = "doc/ask"; const WIRE_KIND: WireKind = WireKind::Command; }
/// #     impl Endpoint for Ask { type Response = Told; }
/// #     #[derive(serde::Serialize, serde::Deserialize, postcard_schema::Schema)]
/// #     pub struct Told;
/// #     impl Message for Told { const PATH: &'static str = "doc/told"; }
/// # }
/// oer_hil_agent::service! {
///     pub enum Request (sessions = false, handler = Handler) {
///         Ask(wire::Ask) => ask,
///     }
/// }
/// struct Image;
/// impl Handler for Image {}
/// ```
///
/// Two services of one image that serve the same request do not compile
/// either:
///
/// ```compile_fail
/// oer_hil_agent::requests! {
///     pub enum First (sessions = false) { Reset(oer_hil_protocol::lifecycle::RequestReset) }
/// }
/// oer_hil_agent::image_keys! {
///     KEYS = [First, oer_hil_agent::lifecycle::Request]
/// }
/// ```
#[macro_export]
macro_rules! service {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident (sessions = $sessions:literal, handler = $handler:ident) {
            $($variant:ident($ty:ty) => $method:ident),* $(,)?
        }
        $(publishes [$($published:ty),* $(,)?])?
    ) => {
        $crate::requests! {
            $(#[$meta])*
            $vis enum $name (sessions = $sessions) {
                $($variant($ty)),*
            }
            $(publishes [$($published),*])?
        }

        /// One method per endpoint of the service; each returns the
        /// endpoint's response.
        $vis trait $handler {
            $(
                fn $method(
                    &mut self,
                    request: $ty,
                ) -> impl ::core::future::Future<
                    Output = <$ty as $crate::__protocol::Endpoint>::Response,
                >;
            )*
        }

        impl $name {
            /// Answer `identity`'s request through `handler` on `console`;
            /// the sequence of the published response.
            $vis async fn serve<
                H: $handler,
                const NOTES: usize,
                const MESSAGES: usize,
                P: $crate::console::Immediate,
            >(
                self,
                handler: &mut H,
                console: &$crate::console::Console<NOTES, MESSAGES, P>,
                identity: $crate::__protocol::RequestIdentity,
            ) -> u32 {
                match self {
                    $(Self::$variant(request) => {
                        let response = handler.$method(request).await;
                        console
                            .publish_reliably(identity.session_id, identity.request_id, &response)
                            .await
                    })*
                }
            }
        }
    };
}

/// Declares an image's key set: the union of the keys of the [`Base`]
/// service and of every service the image mounts, sorted, as a constant
/// [`ImageKeySet`]. Its length, order and wire bound are checked at compile
/// time; a key two services share counts once.
///
/// ```ignore
/// oer_hil_agent::image_keys! {
///     /// What the reset-capable image serves.
///     pub(crate) KEYS = [oer_hil_agent::lifecycle::Request]
/// }
/// ```
#[macro_export]
macro_rules! image_keys {
    ($(#[$meta:meta])* $vis:vis $name:ident = [$($service:ty),* $(,)?]) => {
        $(#[$meta])*
        $vis const $name: $crate::__protocol::base::ImageKeySet<'static> = {
            use $crate::__protocol::{Key, base};
            const GROUPS: &[&[Key]] = &[
                <$crate::base::Base as $crate::base::Service>::KEYS,
                $(<$service as $crate::base::Service>::KEYS,)*
            ];
            const LENGTH: usize = match base::keys_len(GROUPS) {
                Some(length) => length,
                None => panic!("the image's services hold more keys than a usize counts"),
            };
            const SORTED: [Key; LENGTH] = match base::concat_keys(GROUPS) {
                Some(keys) => base::sorted_keys(keys),
                None => panic!("the image key array holds every service key"),
            };
            const REQUEST_GROUPS: &[&[Key]] = &[
                <$crate::base::Base as $crate::base::Service>::REQUESTS,
                $(<$service as $crate::base::Service>::REQUESTS,)*
            ];
            const REQUEST_LENGTH: usize = match base::keys_len(REQUEST_GROUPS) {
                Some(length) => length,
                None => panic!("the image's services serve more requests than a usize counts"),
            };
            const REQUESTS: [Key; REQUEST_LENGTH] = match base::concat_keys(REQUEST_GROUPS) {
                Some(keys) => base::sorted_keys(keys),
                None => panic!("the image request array holds every service request"),
            };
            if base::distinct_len(&REQUESTS) != REQUEST_LENGTH {
                panic!("two services of the image serve the same request");
            }
            const DISTINCT: [Key; base::distinct_len(&SORTED)] =
                match base::distinct_keys(&SORTED) {
                    Some(keys) => keys,
                    None => panic!("the image's distinct keys ascend"),
                };
            match base::ImageKeySet::new(&DISTINCT) {
                Ok(set) => set,
                Err(_) => panic!("the image key set must be sorted, distinct and wire-bounded"),
            }
        };
    };
}

/// The requests of two sets: the first set's when it decodes a frame,
/// otherwise the second's.
pub enum Either<A, B> {
    First(A),
    Second(B),
}

impl<A: Requests, B: Requests> Requests for Either<A, B> {
    const SESSIONS: bool = A::SESSIONS || B::SESSIONS;

    fn decode(frame: &Frame<'_>) -> Option<Result<Self, DecodeError>> {
        A::decode(frame)
            .map(|request| request.map(Self::First))
            .or_else(|| B::decode(frame).map(|request| request.map(Self::Second)))
    }
}

#[cfg(test)]
mod tests;
