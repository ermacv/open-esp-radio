//! The base module every image serves, and the console's request intake.
//!
//! A console feeds the bytes it reads to [`Intake::receive`]. Each complete
//! request comes back either answered — by the base module, or refused
//! because it names another boot, carries a session, or has a key the image
//! does not serve — or as one of the image's own requests to serve.

use oer_hil_protocol::base::{
    BootEvidence, GetBootStatus, GetHello, GetImageKeys, GetLinkHealth, GetPostMortemCheckpoints,
    Hello, ImageKeySet, LinkHealth, PostMortemCheckpoints, RejectReason,
};
use oer_hil_protocol::{
    DecodeCounters, DecodeError, Frame, FrameDecoder, Message, RequestIdentity, WireKind,
};

/// What the base module reads from the platform.
pub trait Platform {
    fn boot_evidence(&self) -> BootEvidence;
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
    pub text_dropped: u32,
    pub text_truncated: u32,
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
        text_dropped: sent.text_dropped,
        text_truncated: sent.text_truncated,
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

/// Declares an image's requests: an enum with one variant per endpoint it
/// serves, decoded by key.
///
/// ```ignore
/// oer_hil_agent::requests! {
///     /// The requests this image serves.
///     pub enum Request (sessions = false) {
///         Watchdog(oer_hil_protocol::system::WatchdogTest),
///         #[cfg(feature = "system-panic-reset")]
///         Panic(oer_hil_protocol::system::InjectPanic),
///     }
/// }
/// ```
///
/// `sessions` says whether a request may belong to a session; a variant
/// takes `#[cfg(...)]` attributes only.
#[macro_export]
macro_rules! requests {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident (sessions = $sessions:literal) {
            $($(#[cfg($cfg:meta)])* $variant:ident($ty:ty)),* $(,)?
        }
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

requests! {
    /// The system watchdog images' one request.
    pub enum WatchdogRequest (sessions = false) {
        Test(oer_hil_protocol::system::WatchdogTest),
    }
}

requests! {
    /// The USB Serial/JTAG-off image's request beside the watchdog's.
    pub enum UsbRequest (sessions = false) {
        Disable(oer_hil_protocol::system::DisableUsb),
    }
}

requests! {
    /// The panic-reset image's one request.
    pub enum PanicRequest (sessions = false) {
        Inject(oer_hil_protocol::system::InjectPanic),
    }
}

#[cfg(test)]
mod tests;
