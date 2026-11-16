//! Publication evidence of the ordinary transmit path, for the image
//! declaring trace and debug-control.
use oer_portable_trace::{Channel, Domain, Kind};

/// The channel of the driver's publication evidence.
const CHANNEL: Channel = const {
    match Channel::new(Domain::Ieee80211, 19) {
        Ok(channel) => channel,
        Err(_) => panic!("invalid trace channel"),
    }
};

/// Snapshot of the encoded DMA input immediately before reservation.
pub const ENCODED_MPDU: Kind = Kind::new(Domain::Ieee80211, 101);
#[cfg(oer_trace)]
#[inline(always)]
pub(crate) fn encoded_mpdu(bytes: &[u8]) {
    if oer_portable_trace_recorder::enabled(CHANNEL) {
        oer_portable_trace_recorder::capture(ENCODED_MPDU, |snapshot| {
            snapshot.push(bytes.len() as u32);
            for chunk in bytes.chunks(4) {
                let mut word = [0; 4];
                for (out, byte) in word.iter_mut().zip(chunk) {
                    *out = *byte;
                }
                snapshot.push(u32::from_le_bytes(word));
            }
        });
    }
}
