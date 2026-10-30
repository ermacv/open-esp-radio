//! A simulated device for the link's and its users' tests: it writes frames
//! of its own boot and sequence and reads the host's requests.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
};

use oer_hil_protocol::{Envelope, FrameDecoder, FrameEncoder, Key, Message, WireKind, base::Hello};

use crate::Link;

/// The Hello of an image that serves no keys.
pub const HELLO: Hello = Hello {
    keys: 0,
    keys_digest: 0,
    maximum_payload_bytes: 64,
    maximum_wire_frame_bytes: 128,
    image_crc32: 0,
};

/// The device end of a link.
pub struct Device {
    stream: UnixStream,
    encoder: FrameEncoder,
    pub boot_id: u64,
    /// The sequence number of the next frame the device writes.
    pub sequence: u32,
}

/// A request the device read.
pub struct Request {
    pub boot_id: u64,
    pub request_id: u32,
    pub key: Key,
    pub payload: Vec<u8>,
}

impl Request {
    /// The request as `M`, when it is one.
    pub fn decode<M: Message>(&self) -> Option<M> {
        (self.key == M::KEY)
            .then(|| postcard::from_bytes(&self.payload).ok())
            .flatten()
    }
}

impl Device {
    /// A device of boot `boot_id` and the host's link to it.
    pub fn new(boot_id: u64) -> (Self, Link) {
        let (device, host) = UnixStream::pair().expect("a socket pair");
        let link = Link::start(Box::new(host)).expect("the link starts");
        let device = Self {
            stream: device,
            encoder: FrameEncoder::new(),
            boot_id,
            sequence: 0,
        };
        (device, link)
    }

    /// Write `body` as the answer to `request_id` (0: unsolicited).
    pub fn send<M: Message>(&mut self, request_id: u32, body: M) {
        let frame = self
            .encoder
            .encode(&Envelope::new(
                self.boot_id,
                self.sequence,
                0,
                request_id,
                body,
            ))
            .expect("the frame encodes")
            .to_vec();
        self.sequence += 1;
        self.stream.write_all(&frame).expect("the host reads");
    }

    /// Restart as boot `boot_id`: its Hello, `hello`, first.
    pub fn reboot(&mut self, boot_id: u64, hello: Hello) {
        self.boot_id = boot_id;
        self.sequence = 0;
        self.send(0, hello);
    }

    /// Write raw bytes, such as a diagnostic line.
    pub fn write(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).expect("the host reads");
    }

    /// The next request the host wrote; `None` once the host closed.
    pub fn request(&mut self) -> Option<Request> {
        let mut decoder = FrameDecoder::new();
        let mut buffer = [0; 256];
        loop {
            let length = self.stream.read(&mut buffer).ok()?;
            if length == 0 {
                return None;
            }
            let mut request = None;
            decoder.feed(WireKind::Command, &buffer[..length], |frame| {
                if let Ok(frame) = frame {
                    request = Some(Request {
                        boot_id: frame.boot_id,
                        request_id: frame.request_id,
                        key: frame.key,
                        payload: frame.payload.to_vec(),
                    });
                }
            });
            if request.is_some() {
                return request;
            }
        }
    }
}
