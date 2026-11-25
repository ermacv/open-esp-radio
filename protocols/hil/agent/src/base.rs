//! The base module every image serves, and the console's request intake.
//!
//! A console feeds the bytes it reads to [`Intake::receive`]. Each complete
//! request comes back either answered — by the base module, or refused
//! because it names another boot, carries a session, or has a key the image
//! does not serve — or as one of the image's own requests to serve.

use oer_protocols_hil_protocol::base::{
    BootEvidence, GetBootStatus, GetHello, GetImageKeys, GetLinkHealth, GetPostMortemCheckpoints,
    Hello, ImageKeySet, LinkHealth, PostMortemCheckpoints, RejectReason, Rejected,
};
use oer_protocols_hil_protocol::{
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

    /// What the set may ask after an answer's write: `Infallible`, or
    /// [`Trap`](crate::serve::Trap) for a set that declares `traps`.
    type Trap: crate::serve::Traps;

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
    type Trap = core::convert::Infallible;

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

/// The console's intake of one boot: what it answers the base module with.
/// The frame decoder it feeds is the console's state, placed with the
/// console's writer and lent to each feed.
#[derive(Clone, Copy)]
pub struct Intake<'a> {
    boot_id: u64,
    image_keys: ImageKeySet<'a>,
    hello: Hello,
}

impl<'a> Intake<'a> {
    /// The intake of boot `boot_id` of the image of identity `image_crc32`,
    /// which serves `image_keys` and takes command payloads of up to
    /// `maximum_payload_bytes`.
    pub fn new(
        boot_id: u64,
        image_keys: ImageKeySet<'a>,
        maximum_payload_bytes: u16,
        image_crc32: u32,
    ) -> Self {
        Self {
            boot_id,
            image_keys,
            hello: image_keys.hello(maximum_payload_bytes, image_crc32),
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

    /// Feeds one byte to `decoder`: the request it completes, if any. One byte at a time,
    /// at most one frame completes per feed, so an answer that reports the
    /// counters reports them as of its request, and the console answers
    /// each request before it feeds the next byte.
    pub fn receive_byte<C: Requests>(
        &self,
        decoder: &mut FrameDecoder,
        byte: u8,
        sent: Sent,
    ) -> Option<Request<C>> {
        let boot_id = self.boot_id;
        let hello = self.hello;
        let mut request = None;
        let mut payload_error = false;
        decoder.feed(WireKind::Command, &[byte], |frame| {
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
            decoder.count_payload_error();
        }
        // Link health counts the request that asks for it.
        if let Some(Request::Answer(_, Answer::Link(link))) = &mut request {
            *link = link_health(decoder.counters(), sent);
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
    if let Err(reason) = oer_protocols_hil_protocol::base::validate_target(frame, boot_id) {
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
/// oer_protocols_hil_agent::__requests! {
///     /// The requests this image serves.
///     pub enum Request (sessions = false) {
///         Unwrap(oer_hil_services_aes::UnwrapKey),
///     }
///     publishes [oer_protocols_hil_protocol::base::Hello]
/// }
/// ```
///
/// `sessions` says whether a request may belong to a session; a variant
/// and a published message take `#[cfg(...)]` attributes only. A set
/// whose answers may end in a trap says `traps = true` after it: only such a set
/// can build [`After::Trap`](crate::mailbox::After::Trap).
#[doc(hidden)]
#[macro_export]
macro_rules! __requests {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident (sessions = $sessions:literal $(, traps = $traps:tt)?) {
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
            type Trap = $crate::__trap!($($traps)?);

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

/// Declares a service: its requests (the enum and keys), and the kind of
/// each endpoint. An **observation** is answered at once: a method of the
/// observing value, called in the console's step that decoded the request,
/// whose answer is serialized from a reference. A **command** is answered
/// later: an `async` method of the commanding value, which the console
/// awaits beside its read of the endpoint; one is in work at a time.
///
/// ```ignore
/// oer_protocols_hil_agent::service! {
///     /// The pair's receiver.
///     pub enum Request (sessions = false, observer = Observer, commands = Command, commander = Commander) {
///         SnapshotCold(SnapshotCold) => observe snapshot_cold,
///         BringUp(BringUp) => command bring_up,
///     }
/// }
/// ```
///
/// It generates the request enum, `trait Observer` (one method an
/// observation, returning its endpoint's response), `enum Command` (the
/// commands with their requests) and, with `commander = …`, `trait
/// Commander` (one `async` method a command) with the command's run on it.
/// Without `commander`, the image runs the commands itself
/// ([`Run`](crate::serve::Run) for `Command`). A set that says `traps = true` may
/// end an answer in a trap: its commands return the response and an
/// [`After`](crate::serve::After). A handler that cannot answer still returns
/// its endpoint's response type: failures belong in that type.
///
/// An image whose observer leaves an endpoint out does not compile:
///
/// ```compile_fail
/// # mod wire {
/// #     use oer_protocols_hil_protocol::{Endpoint, Message, WireKind};
/// #     #[derive(serde::Serialize, serde::Deserialize, postcard_schema::Schema)]
/// #     pub struct Ask;
/// #     impl Message for Ask { const PATH: &'static str = "doc/ask"; const WIRE_KIND: WireKind = WireKind::Command; }
/// #     impl Endpoint for Ask { type Response = Told; }
/// #     #[derive(serde::Serialize, serde::Deserialize, postcard_schema::Schema)]
/// #     pub struct Told;
/// #     impl Message for Told { const PATH: &'static str = "doc/told"; }
/// # }
/// oer_protocols_hil_agent::service! {
///     pub enum Request (sessions = false, observer = Observer, commands = Command) {
///         Ask(wire::Ask) => observe ask,
///     }
/// }
/// struct Image;
/// impl Observer for Image {}
/// ```
///
/// Two services of one image that serve the same request do not compile
/// either:
///
/// ```compile_fail
/// oer_protocols_hil_agent::__requests! {
///     pub enum First (sessions = false) { Status(oer_protocols_hil_protocol::base::GetBootStatus) }
/// }
/// oer_protocols_hil_agent::image_keys! {
///     KEYS = [First, First]
/// }
/// ```
#[macro_export]
macro_rules! service {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident (
            sessions = $sessions:literal,
            observer = $observer:ident,
            commands = $commands:ident
            $(, commander = $commander:ident)?
            $(, traps = $traps:tt)?
        ) {
            $($body:tt)*
        }
        $(publishes [$($published:ty),* $(,)?])?
    ) => {
        $crate::__service! {
            @split
            [$(#[$meta])*] [$vis] [$name] [$sessions] [$observer] [$commands]
            [$($commander)?] [$($traps)?] [$($($published),*)?]
            [] [] [] ; $($body)*
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __service {
    (
        @split $meta:tt $vis:tt $name:tt $sessions:tt $observer:tt $commands:tt $commander:tt
        $traps:tt $published:tt [$($all:tt)*] [$($observe:tt)*] [$($command:tt)*] ;
        $variant:ident($ty:ty) => observe $method:ident $(, $($rest:tt)*)?
    ) => {
        $crate::__service! {
            @split $meta $vis $name $sessions $observer $commands $commander $traps $published
            [$($all)* ($variant $ty)] [$($observe)* ($variant $method $ty)] [$($command)*] ;
            $($($rest)*)?
        }
    };
    (
        @split $meta:tt $vis:tt $name:tt $sessions:tt $observer:tt $commands:tt $commander:tt
        $traps:tt $published:tt [$($all:tt)*] [$($observe:tt)*] [$($command:tt)*] ;
        $variant:ident($ty:ty) => command $method:ident $(, $($rest:tt)*)?
    ) => {
        $crate::__service! {
            @split $meta $vis $name $sessions $observer $commands $commander $traps $published
            [$($all)* ($variant $ty)] [$($observe)*] [$($command)* ($variant $method $ty)] ;
            $($($rest)*)?
        }
    };
    (
        @split [$(#[$meta:meta])*] [$vis:vis] [$name:ident] [$sessions:literal] [$observer:ident]
        [$commands:ident] [$($commander:ident)?] $traps:tt [$($published:ty),*]
        [$(($variant:ident $ty:ty))*]
        [$(($ov:ident $om:ident $oty:ty))*]
        [$(($cv:ident $cm:ident $cty:ty))*] ;
    ) => {
        $crate::__requests! {
            $(#[$meta])*
            $vis enum $name (sessions = $sessions, traps = $traps) {
                $($variant($ty)),*
            }
            publishes [$($published),*]
        }

        /// What answers the service's observations, at once.
        $vis trait $observer {
            $(
                fn $om(
                    &mut self,
                    request: $oty,
                ) -> <$oty as $crate::__protocol::Endpoint>::Response;
            )*
        }

        $crate::__nothing_observes! { [$($ov)*] $observer }

        /// The service's commands, with their requests.
        $vis enum $commands {
            $($cv($cty),)*
        }

        impl<O: $observer> $crate::serve::Serve<O> for $name {
            type Command = $commands;
            fn serve<'f>(
                self,
                observer: &mut O,
                frame: $crate::serve::Frame<'f>,
            ) -> $crate::serve::Served<'f, $commands> {
                match (self, observer, frame) {
                    $((Self::$ov(request), observer, frame) => {
                        $crate::serve::Served::Observed(frame.write(&observer.$om(request)))
                    })*
                    $((Self::$cv(request), _, _) => {
                        $crate::serve::Served::Command($commands::$cv(request))
                    })*
                }
            }
        }

        $crate::__commander! {
            [$($commander)?] [$name] [$vis] [$commands] $traps
            [$(($cv $cm $cty))*]
        }
    };
}

/// The commanding trait of a service that names one, and its commands' run.
#[doc(hidden)]
#[macro_export]
macro_rules! __commander {
    ([] $($rest:tt)*) => {};
    (
        [$commander:ident] [$name:ident] [$vis:vis] [$commands:ident] $traps:tt
        [$(($cv:ident $cm:ident $cty:ty))*]
    ) => {
        /// What answers the service's commands, later.
        $vis trait $commander {
            $(
                fn $cm(
                    &mut self,
                    request: $cty,
                ) -> impl ::core::future::Future<
                    Output = $crate::__command_output!(
                        $traps <$cty as $crate::__protocol::Endpoint>::Response
                    ),
                >;
            )*
        }

        const _: () = {
            /// One command's answer, written from where it lies.
            pub enum Answer {
                $($cv(<$cty as $crate::__protocol::Endpoint>::Response),)*
            }

            impl $crate::serve::Answer for Answer {
                fn write<'f>(&self, frame: $crate::serve::Frame<'f>) -> $crate::serve::Written<'f> {
                    match *self {
                        $(Self::$cv(ref answer) => frame.write(answer),)*
                    }
                }
            }

            impl<C: $commander> $crate::serve::Run<C> for $commands {
                type Answer = Answer;
                type Trap = <$name as $crate::base::Requests>::Trap;
                async fn run(self, commander: &mut C) -> $crate::serve::Ran<Answer, Self::Trap> {
                    match self {
                        $($commands::$cv(request) => Ok($crate::__command_then!(
                            $traps Answer::$cv, commander.$cm(request).await
                        )),)*
                    }
                }
            }
        };
    };
}

/// A service without observations is observed by nothing: `()`.
#[doc(hidden)]
#[macro_export]
macro_rules! __nothing_observes {
    ([] $observer:ident) => {
        impl $observer for () {}
    };
    ([$($ov:ident)+] $observer:ident) => {};
}

/// A command's output: its response, and with `traps` what follows its write.
#[doc(hidden)]
#[macro_export]
macro_rules! __command_output {
    ([] $response:ty) => {
        $response
    };
    ([true] $response:ty) => {
        ($response, $crate::serve::After<$crate::serve::Trap>)
    };
}

/// A command's output as its answer and what follows its write.
#[doc(hidden)]
#[macro_export]
macro_rules! __command_then {
    ([] $wrap:path, $output:expr) => {
        ($wrap($output), $crate::serve::After::Nothing)
    };
    ([true] $wrap:path, $output:expr) => {{
        let (answer, after) = $output;
        ($wrap(answer), after)
    }};
}

/// The trap type of a `__requests!` set: `Infallible`
/// unless the set says `traps = true`.
#[doc(hidden)]
#[macro_export]
macro_rules! __trap {
    () => {
        ::core::convert::Infallible
    };
    ([]) => {
        ::core::convert::Infallible
    };
    (true) => {
        $crate::serve::Trap
    };
    ([true]) => {
        $crate::serve::Trap
    };
}

/// Declares an image's key set: the union of the keys of the [`Base`]
/// service and of every service the image mounts, sorted, as a constant
/// [`ImageKeySet`]. Its length, order and wire bound are checked at compile
/// time; a key two services share counts once.
///
/// ```ignore
/// oer_protocols_hil_agent::image_keys! {
///     /// What the AES image serves.
///     pub(crate) KEYS = [oer_hil_services_aes::Request]
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
    type Trap = <A::Trap as crate::serve::Traps>::Or<B::Trap>;

    fn decode(frame: &Frame<'_>) -> Option<Result<Self, DecodeError>> {
        A::decode(frame)
            .map(|request| request.map(Self::First))
            .or_else(|| B::decode(frame).map(|request| request.map(Self::Second)))
    }
}

#[cfg(test)]
mod tests;
