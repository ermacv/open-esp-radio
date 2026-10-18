//! The envelope every message travels in.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Envelope<T> {
    pub boot_id: u64,
    pub message_sequence: u32,
    pub session_id: u64,
    pub request_id: u32,
    pub body: T,
}

impl<T> Envelope<T> {
    pub const fn new(
        boot_id: u64,
        message_sequence: u32,
        session_id: u64,
        request_id: u32,
        body: T,
    ) -> Self {
        Self {
            boot_id,
            message_sequence,
            session_id,
            request_id,
            body,
        }
    }
}

/// Message direction encoded in the fixed wire header.
///
/// Keeping this outside the postcard body lets a decoder reject a frame sent
/// to the wrong endpoint before interpreting the command or event enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum WireKind {
    Command = 1,
    Event = 2,
}
