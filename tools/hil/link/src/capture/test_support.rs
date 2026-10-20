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

pub fn capability_keys() -> Vec<Key> {
    let capabilities = oer_hil_model::image_class::agent_chips()
        .find_map(|chip| {
            oer_hil_model::image_class::image_keys_on(
                oer_hil_model::schema::image::ImageClass::SystemWatchdog,
                chip,
            )
        })
        .expect("a chip's HIL agent builds the Bluetooth GATT image");
    capabilities.keys().iter().copied().collect()
}

/// The fake target's boot `Hello`, announcing [`capability_keys`].
pub fn hello(boot_id: u64, message_sequence: u32) -> Envelope<Hello> {
    Envelope::new(
        boot_id,
        message_sequence,
        0,
        0,
        ImageKeySet::new(&capability_keys()).hello(1),
    )
}
