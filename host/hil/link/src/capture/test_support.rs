//! A fake serial link to the target for session and workload tests.
use super::*;
use oer_hil_protocol::base::{GetImageKeys, Hello, ImageKeySet};
use oer_hil_protocol::{Key, Message, WireKind};
use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

/// A temporary capture output directory, removed on drop.
pub struct Output(pub PathBuf);

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

impl Output {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "oer-capture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub(crate) struct Serial {
    pub(crate) input: TestRead,
    pub(crate) fail_write: bool,
    pub(crate) writes: Option<mpsc::Sender<Vec<u8>>>,
}

pub(crate) struct TestRead {
    stream: std::os::unix::net::UnixStream,
    failure: Arc<Mutex<Option<io::Error>>>,
}
/// The target side of a fake serial link: send frames or a read failure.
#[derive(Clone)]
pub struct Input {
    stream: Arc<std::os::unix::net::UnixStream>,
    failure: Arc<Mutex<Option<io::Error>>>,
}
impl Input {
    pub fn send(&self, input: io::Result<Vec<u8>>) -> io::Result<()> {
        match input {
            Ok(bytes) => (&*self.stream).write_all(&bytes),
            Err(error) => {
                *self.failure.lock().unwrap() = Some(error);
                self.stream.shutdown(std::net::Shutdown::Write)
            }
        }
    }
}
pub(crate) fn serial_pair() -> (Input, TestRead) {
    let (host, peer) = std::os::unix::net::UnixStream::pair().unwrap();
    host.set_nonblocking(true).unwrap();
    let failure = Arc::new(Mutex::new(None));
    (
        Input {
            stream: Arc::new(peer),
            failure: Arc::clone(&failure),
        },
        TestRead {
            stream: host,
            failure,
        },
    )
}
impl AsRawFd for Serial {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.input.stream.as_raw_fd()
    }
}
impl Read for Serial {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.input.stream.read(buffer) {
            Ok(0) => self.input.failure.lock().unwrap().take().map_or(Ok(0), Err),
            result => result,
        }
    }
}

impl Write for Serial {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail_write {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            if let Some(writes) = &self.writes {
                writes
                    .send(bytes.to_vec())
                    .map_err(|_| io::ErrorKind::BrokenPipe)?;
            }
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A capture over a fake serial link whose writes succeed or fail.
pub fn capture(output: &Output, fail_write: bool) -> (SerialCapture, Input) {
    capture_at(&output.0, fail_write)
}

/// [`capture`] into the directory `output`, which the caller owns.
pub fn capture_at(output: &Path, fail_write: bool) -> (SerialCapture, Input) {
    let (input, rx) = serial_pair();
    let capture = SerialCapture::start_transport(output, move || {
        Ok(Serial {
            input: rx,
            fail_write,
            writes: None,
        })
    })
    .unwrap();
    (capture, input)
}

/// Encode one message as a wire frame.
pub fn frame<M: Message>(message: Envelope<M>) -> Vec<u8> {
    FrameEncoder::new().encode(&message).unwrap().to_vec()
}

/// A message of any type, framed once its header is known, so messages of
/// several types share one list.
pub struct AnyMessage(Box<dyn Fn(u64, u32, u64, u32) -> Vec<u8> + Send>);

impl AnyMessage {
    pub fn frame(
        &self,
        boot_id: u64,
        message_sequence: u32,
        session_id: u64,
        request_id: u32,
    ) -> Vec<u8> {
        (self.0)(boot_id, message_sequence, session_id, request_id)
    }
}

pub fn any<M: Message + Clone + Send + 'static>(body: M) -> AnyMessage {
    AnyMessage(Box::new(
        move |boot_id, message_sequence, session_id, request_id| {
            frame(Envelope::new(
                boot_id,
                message_sequence,
                session_id,
                request_id,
                body.clone(),
            ))
        },
    ))
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

/// Deliver the boot `Hello` and wait until the capture observes it.
pub fn activate(capture: &SerialCapture, input: &Input) {
    input.send(Ok(frame(hello(7, 0)))).unwrap();
    capture
        .wait_for_message_after(0, Duration::from_secs(2), Received::is::<Hello>)
        .unwrap()
        .unwrap();
}

/// A capture whose host commands are delivered to the returned receiver.
pub fn capture_with_commands(output: &Output) -> (SerialCapture, Input, mpsc::Receiver<Vec<u8>>) {
    capture_with_commands_at(&output.0)
}

/// [`capture_with_commands`] into the directory `output`, which the caller
/// owns.
pub fn capture_with_commands_at(output: &Path) -> (SerialCapture, Input, mpsc::Receiver<Vec<u8>>) {
    let (input, rx) = serial_pair();
    let (writes, commands) = mpsc::channel();
    let capture = SerialCapture::start_transport(output, move || {
        Ok(Serial {
            input: rx,
            fail_write: false,
            writes: Some(writes),
        })
    })
    .unwrap();
    (capture, input, commands)
}

/// Decode the next host request written to a [`capture_with_commands`] link.
pub fn receive_request(writes: &mpsc::Receiver<Vec<u8>>) -> Received {
    let bytes = writes.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut request = None;
    FrameDecoder::new().feed(WireKind::Command, &bytes, |frame| {
        request = Some(Received::from_frame(&frame.unwrap()).unwrap());
    });
    request.unwrap()
}

/// Decode the next host request, which must be an `M`.
pub fn receive<M: oer_hil_protocol::Message>(writes: &mpsc::Receiver<Vec<u8>>) -> (Received, M) {
    let request = receive_request(writes);
    let body = request
        .decode::<M>()
        .unwrap_or_else(|| panic!("the host sent {}, not {}", request.path(), M::PATH));
    (request, body)
}

/// The capabilities of the fake target: the Bluetooth GATT image's of the
/// first chip whose HIL agent builds one.
pub fn capability_keys() -> Vec<Key> {
    let capabilities = oer_hil_image_class::agent_chips()
        .find_map(|chip| {
            oer_hil_image_class::image_keys_on(
                oer_hil_schema::image::ImageClass::SystemWatchdog,
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

/// Answer the host's capability requests for [`capability_keys`] as boot
/// `boot_id`, starting at target message `message_sequence`; returns the
/// next free target message sequence.
pub fn answer_image_keys(
    input: &Input,
    writes: &mpsc::Receiver<Vec<u8>>,
    boot_id: u64,
    mut message_sequence: u32,
) -> u32 {
    let keys = capability_keys();
    let capabilities = ImageKeySet::new(&keys);
    let mut first = 0;
    while usize::from(first) < keys.len() {
        let request = receive_request(writes);
        let asked = request.decode::<GetImageKeys>().unwrap_or_else(|| {
            panic!("the host sent {}, not a capability request", request.path())
        });
        assert_eq!(asked.first, first);
        let page = capabilities.page(first);
        first += page.keys.len() as u16;
        input
            .send(Ok(frame(Envelope::new(
                boot_id,
                message_sequence,
                request.session_id,
                request.request_id,
                page,
            ))))
            .unwrap();
        message_sequence += 1;
    }
    message_sequence
}

/// A healthy fake target of boot `boot_id`: it greets with its `Hello` and
/// answers every link-health request until the host stops writing, so a
/// capture finishes cleanly.
pub fn healthy_target(
    input: Input,
    commands: mpsc::Receiver<Vec<u8>>,
    boot_id: u64,
) -> std::thread::JoinHandle<()> {
    input.send(Ok(frame(hello(boot_id, 0)))).unwrap();
    std::thread::spawn(move || {
        let mut sequence = 1;
        while let Ok(bytes) = commands.recv() {
            let mut requests = Vec::new();
            FrameDecoder::new().feed(WireKind::Command, &bytes, |frame| {
                if let Ok(frame) = frame
                    && let Ok(request) = Received::from_frame(&frame)
                {
                    requests.push(request);
                }
            });
            for request in requests {
                if request.is::<oer_hil_protocol::base::GetLinkHealth>() {
                    let _ = input.send(Ok(frame(Envelope::new(
                        boot_id,
                        sequence,
                        request.session_id,
                        request.request_id,
                        oer_hil_protocol::base::LinkHealth {
                            rx_frames: 1,
                            rx_cobs_errors: 0,
                            rx_checksum_errors: 0,
                            rx_decode_errors: 0,
                            rx_overflows: 0,
                            tx_frames: 1,
                            tx_dropped: 0,
                            text_dropped: 0,
                            text_truncated: 0,
                        },
                    ))));
                    sequence += 1;
                }
            }
        }
    })
}
