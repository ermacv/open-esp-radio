//! The base module: discovery, rejection, boot and link observations. Every
//! image serves it.

mod boot;
mod image_keys;

pub use boot::*;
pub use image_keys::{Hello, IMAGE_KEYS_PER_PAGE, ImageKeyPage, ImageKeySet, digest, sorted_keys};

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

use crate::{Envelope, Frame, Message};

crate::messages! {
    topic Hello = "base/hello";
    endpoint GetHello = "base/hello/get" => Hello;
    endpoint GetImageKeys = "base/image-keys/get" => ImageKeyPage;
    topic ImageKeyPage = "base/image-keys";
    topic Rejected = "base/rejected";
    endpoint GetBootStatus = "base/boot/get" => BootEvidence;
    topic BootEvidence = "base/boot";
    endpoint GetPostMortemCheckpoints = "base/post-mortem/get" => PostMortemCheckpoints;
    topic PostMortemCheckpoints = "base/post-mortem";
    endpoint GetLinkHealth = "base/link-health/get" => LinkHealth;
    topic LinkHealth = "base/link-health";
    topic Accepted = "base/accepted";
}

/// This boot's Hello again: for a host that attaches to a running device or
/// lost the boot's own Hello. The one request that may name no boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetHello;

/// Page `first..` of the keys the image serves.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetImageKeys {
    pub first: u16,
}

/// The device refused a request; the frame's request id names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Rejected(pub RejectReason);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum RejectReason {
    BootId,
    SessionId,
    InvalidState,
    InvalidConfiguration,
    /// The image serves no message with the request's key.
    Unsupported,
    Busy,
    Internal,
}

/// Query the current boot without changing any radio state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetBootStatus;

/// Page through the previous boot's post-mortem checkpoints from `first`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetPostMortemCheckpoints {
    pub first: u8,
}

/// Return boot-lifetime transport and serialized-text health counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetLinkHealth;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct LinkHealth {
    pub rx_frames: u32,
    pub rx_cobs_errors: u32,
    pub rx_checksum_errors: u32,
    pub rx_decode_errors: u32,
    pub rx_overflows: u32,
    pub tx_frames: u32,
    pub tx_dropped: u32,
    pub text_dropped: u32,
    pub text_truncated: u32,
}

/// Bind a request to one boot. Only a session-free [`GetHello`] may discover
/// an unknown boot; it cannot initialize or mutate the radio.
pub fn validate_target(frame: &Frame<'_>, target_boot_id: u64) -> Result<(), RejectReason> {
    let discovery = frame.boot_id == 0 && frame.session_id == 0 && frame.key == GetHello::KEY;
    if target_boot_id == 0 || (frame.boot_id != target_boot_id && !discovery) {
        return Err(RejectReason::BootId);
    }
    Ok(())
}

/// The reply to `request` with `body`: same boot, session and request id.
pub fn reply<M: Message>(
    boot_id: u64,
    message_sequence: u32,
    request: &Frame<'_>,
    body: M,
) -> Envelope<M> {
    Envelope::new(
        boot_id,
        message_sequence,
        request.session_id,
        request.request_id,
        body,
    )
}

/// `base/accepted`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Accepted;

#[cfg(test)]
mod tests;
