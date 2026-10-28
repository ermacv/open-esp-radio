//! In-memory wire messages for session and workload tests.
use super::*;
use oer_hil_protocol::base::{Hello, ImageKeySet};
use oer_hil_protocol::{Key, Message};

/// Encode one message as a wire frame.
pub fn frame<M: Message>(message: Envelope<M>) -> Vec<u8> {
    FrameEncoder::new().encode(&message).unwrap().to_vec()
}

/// One message as the host receives it.
pub fn received<M: Message>(message: Envelope<M>) -> Received {
    let bytes = frame(message);
    let mut received = None;
    FrameDecoder::new().feed(M::WIRE_KIND, &bytes, |frame| {
        received = Some(Received::from_frame(&frame.unwrap()).unwrap());
    });
    received.unwrap()
}

/// The keys of the fake target's image: the base module and the system
/// watchdog image's services.
pub fn capability_keys() -> Vec<Key> {
    use oer_hil_protocol::{base, system};
    let mut keys = vec![
        base::GetHello::KEY,
        base::Hello::KEY,
        base::GetImageKeys::KEY,
        base::ImageKeyPage::KEY,
        base::Rejected::KEY,
        base::GetBootStatus::KEY,
        base::BootEvidence::KEY,
        base::GetPostMortemCheckpoints::KEY,
        base::PostMortemCheckpoints::KEY,
        base::GetLinkHealth::KEY,
        base::LinkHealth::KEY,
        system::WatchdogTest::KEY,
        system::WatchdogArmed::KEY,
        system::InjectPanic::KEY,
        system::PanicInjected::KEY,
    ];
    keys.sort();
    keys
}

/// The fake target's boot `Hello`, announcing [`capability_keys`].
pub fn hello(boot_id: u64, message_sequence: u32) -> Envelope<Hello> {
    Envelope::new(
        boot_id,
        message_sequence,
        0,
        0,
        ImageKeySet::new(&capability_keys()).unwrap().hello(1),
    )
}
