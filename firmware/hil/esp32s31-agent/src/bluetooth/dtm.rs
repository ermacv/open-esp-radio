//! Direct Test Mode and raw HCI exchanges through the HCI Controller.
//!
//! One task owns the Host end of the transport. It resets the Controller at
//! boot, then serves the console: every HIL DTM operation becomes the standard
//! HCI command and waits for its Command Complete (LE Transmitter Test v1 on
//! channel 0 with 37-byte PRBS9 payloads, LE Receiver Test v1 on channel 0,
//! LE Test End and HCI Reset), and every raw HCI request either sends one
//! command and returns its Command Complete or Command Status, or returns the
//! oldest queued Controller event. Events that arrive while a command waits
//! for its completion join that bounded queue; when it is full the oldest one
//! is dropped and counted. The Controller core runs every role over the radio
//! runtime.

use core::cell::RefCell;

use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    channel::Channel,
};
use embassy_time::{Duration, with_timeout};
use heapless::{Deque, Vec};
use oer_bluetooth_hci::bt_hci::{
    ControllerToHostPacket, PacketKind,
    cmd::{Opcode, OpcodeGroup},
    transport::{PacketToController, Transport},
};
use oer_esp32s31_bluetooth_system::BluetoothHostTransport;
use oer_hil_protocol::base::RejectReason;
use oer_hil_protocol::bluetooth::{DtmResult, ExchangeHci, HciResponse, RunDtm};
use oer_hil_protocol::{
    RequestIdentity, bluetooth::BLUETOOTH_HCI_EVENT_BYTES, bluetooth::BluetoothDtmEvidence,
    bluetooth::BluetoothDtmOperation as Operation, bluetooth::BluetoothDtmResult,
    bluetooth::BluetoothDtmRxDiagnostics, bluetooth::BluetoothHciRequest,
    bluetooth::BluetoothHciResponse,
};

use super::console;

const RESET: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0003);
const RECEIVER_TEST: Opcode = Opcode::new(OpcodeGroup::LE, 0x001d);
const TRANSMITTER_TEST: Opcode = Opcode::new(OpcodeGroup::LE, 0x001e);
const TEST_END: Opcode = Opcode::new(OpcodeGroup::LE, 0x001f);
/// DTM on channel 0 (2402 MHz) with 37-byte PRBS9 payloads.
const CHANNEL: u8 = 0;
const PAYLOAD_BYTES: u8 = 37;
const PRBS9: u8 = 0x00;
/// Bound on one DTM operation or raw command, including draining the event in
/// progress.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(2);
/// Controller events kept for [`BluetoothHciRequest::NextEvent`].
const EVENT_QUEUE: usize = 16;
const COMMAND_COMPLETE: u8 = 0x0e;
const COMMAND_STATUS: u8 = 0x0f;

enum Request {
    Dtm(Operation),
    Hci(BluetoothHciRequest),
}

enum Reply {
    Dtm(BluetoothDtmResult, u16),
    Hci(BluetoothHciResponse),
}

static REQUESTS: Channel<CriticalSectionRawMutex, Request, 1> = Channel::new();
static REPLIES: Channel<CriticalSectionRawMutex, Reply, 1> = Channel::new();

type Packet = Vec<u8, BLUETOOTH_HCI_EVENT_BYTES>;

/// Controller events no request has returned yet, with the count of those
/// dropped since the last returned one. Static, so that the task future
/// stays small.
struct Queue {
    events: Deque<Packet, EVENT_QUEUE>,
    dropped: u16,
}

static QUEUE: Mutex<CriticalSectionRawMutex, RefCell<Queue>> = Mutex::new(RefCell::new(Queue {
    events: Deque::new(),
    dropped: 0,
}));

pub(super) async fn run(
    spawner: embassy_executor::Spawner,
    host: BluetoothHostTransport,
    usb: crate::transport::Usb,
    boot: u64,
) -> ! {
    spawner.spawn(tester(host).expect("Bluetooth HCI task"));
    spawner.spawn(requests().expect("Bluetooth request task"));
    console::serve_console(usb, boot, &Profile).await
}

/// Serves the requests the console queued, in a task of its own.
#[embassy_executor::task]
async fn requests() {
    console::serve_requests(&Profile).await
}

struct Profile;

oer_hil_agent::requests! {
    /// The DTM/HCI image's requests.
    pub(super) enum Served (sessions = false) {
        Dtm(RunDtm),
        Hci(ExchangeHci),
    }
}

impl console::Profile for Profile {
    type Request = Served;

    fn queue(&self) -> &'static console::Queue<Served> {
        static QUEUE: console::Queue<Served> = console::Queue::new();
        &QUEUE
    }

    fn maximum_payload_bytes(&self) -> u16 {
        u16::from(PAYLOAD_BYTES)
    }

    async fn serve(&self, identity: RequestIdentity, body: Served) {
        match body {
            Served::Dtm(RunDtm(operation)) => {
                REQUESTS.send(Request::Dtm(operation)).await;
                let reply = match REPLIES.receive().await {
                    Reply::Dtm(result, received) => Ok(DtmResult(BluetoothDtmEvidence {
                        reset_reason: crate::system::boot_evidence().reset_reason,
                        operation,
                        result,
                        rx_diagnostics: BluetoothDtmRxDiagnostics {
                            counted_packets: u32::from(received),
                            ..BluetoothDtmRxDiagnostics::default()
                        },
                    })),
                    Reply::Hci(_) => Err(RejectReason::InvalidState),
                };
                console::respond(identity, reply).await;
            }
            Served::Hci(ExchangeHci(request)) => {
                REQUESTS.send(Request::Hci(request)).await;
                let reply = match REPLIES.receive().await {
                    Reply::Hci(response) => Ok(HciResponse(response)),
                    Reply::Dtm(..) => Err(RejectReason::InvalidState),
                };
                console::respond(identity, reply).await;
            }
        }
    }
}

/// The Host end of the transport.
struct Host {
    transport: BluetoothHostTransport,
}

impl Host {
    /// Read the next Controller event; ACL data is discarded.
    async fn read_event(&self) -> Option<Packet> {
        let mut buffer = [0; oer_esp32s31_bluetooth_system::PACKET];
        loop {
            let ControllerToHostPacket::Event(event) =
                self.transport.read(&mut buffer).await.ok()?
            else {
                continue;
            };
            let mut packet = Packet::new();
            packet.push(event.kind.0).ok()?;
            packet.push(event.data.len() as u8).ok()?;
            packet.extend_from_slice(event.data).ok()?;
            return Some(packet);
        }
    }

    fn queue(&self, packet: Packet) {
        QUEUE.lock(|queue| {
            let mut queue = queue.borrow_mut();
            if queue.events.is_full() {
                queue.events.pop_front();
                queue.dropped = queue.dropped.saturating_add(1);
            }
            let _ = queue.events.push_back(packet);
        });
    }

    /// Send one command and wait for its Command Complete or Command Status,
    /// queueing every other event. `None` when the transport failed.
    async fn command(&self, opcode: Opcode, parameters: &[u8]) -> Option<Packet> {
        self.transport
            .write(&RawCommand { opcode, parameters })
            .await
            .ok()?;
        loop {
            let packet = self.read_event().await?;
            let completes = match packet[0] {
                COMMAND_COMPLETE => {
                    packet.len() >= 5 && packet[3..5] == opcode.to_raw().to_le_bytes()
                }
                COMMAND_STATUS => {
                    packet.len() >= 6 && packet[4..6] == opcode.to_raw().to_le_bytes()
                }
                _ => false,
            };
            if completes {
                return Some(packet);
            }
            self.queue(packet);
        }
    }

    async fn next_event(&self, wait: Duration) -> BluetoothHciResponse {
        let queued = QUEUE.lock(|queue| queue.borrow_mut().events.pop_front());
        let packet = match queued {
            Some(packet) => packet,
            None => match with_timeout(wait, self.read_event()).await {
                Ok(Some(packet)) => packet,
                Ok(None) => return BluetoothHciResponse::TransportFailed,
                Err(_) => return BluetoothHciResponse::NoEvent,
            },
        };
        let dropped = QUEUE.lock(|queue| core::mem::take(&mut queue.borrow_mut().dropped));
        BluetoothHciResponse::Event { packet, dropped }
    }
}

#[embassy_executor::task]
async fn tester(transport: BluetoothHostTransport) {
    let host = Host { transport };
    if host
        .command(RESET, &[])
        .await
        .and_then(|packet| packet.get(5).copied())
        != Some(0)
    {
        crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-reset\r\n");
    }
    let mut received = 0;
    loop {
        let reply = match REQUESTS.receive().await {
            Request::Dtm(operation) => {
                let result = with_timeout(OPERATION_TIMEOUT, execute(&host, operation))
                    .await
                    .unwrap_or(BluetoothDtmResult::Timeout);
                if let BluetoothDtmResult::Complete {
                    received_packets: Some(count),
                } = result
                {
                    received = count;
                }
                Reply::Dtm(result, received)
            }
            Request::Hci(BluetoothHciRequest::Command { opcode, parameters }) => {
                let opcode = Opcode::new(OpcodeGroup::new((opcode >> 10) as u8), opcode & 0x03ff);
                Reply::Hci(
                    match with_timeout(OPERATION_TIMEOUT, host.command(opcode, &parameters)).await {
                        Ok(Some(packet)) => BluetoothHciResponse::Completed(packet),
                        Ok(None) => BluetoothHciResponse::TransportFailed,
                        Err(_) => BluetoothHciResponse::Timeout,
                    },
                )
            }
            Request::Hci(BluetoothHciRequest::NextEvent { wait_ms }) => Reply::Hci(
                host.next_event(Duration::from_millis(u64::from(wait_ms)))
                    .await,
            ),
        };
        REPLIES.send(reply).await;
    }
}

async fn execute(host: &Host, operation: Operation) -> BluetoothDtmResult {
    let (opcode, parameters): (Opcode, &[u8]) = match operation {
        Operation::Transmit => (TRANSMITTER_TEST, &[CHANNEL, PAYLOAD_BYTES, PRBS9]),
        Operation::Receive => (RECEIVER_TEST, &[CHANNEL]),
        Operation::End => (TEST_END, &[]),
        Operation::Reset => (RESET, &[]),
    };
    // Command Complete: code, length, packets, opcode, status, count.
    match host.command(opcode, parameters).await {
        Some(packet) if packet[0] == COMMAND_COMPLETE && packet.get(5) == Some(&0) => {
            BluetoothDtmResult::Complete {
                received_packets: (operation == Operation::End).then(|| {
                    packet
                        .get(6..8)
                        .map_or(0, |count| u16::from_le_bytes([count[0], count[1]]))
                }),
            }
        }
        _ => BluetoothDtmResult::HciRejected,
    }
}

/// One HCI command with its parameters, written as the transport expects.
struct RawCommand<'a> {
    opcode: Opcode,
    parameters: &'a [u8],
}

impl PacketToController for RawCommand<'_> {
    const KIND: PacketKind = PacketKind::Cmd;

    fn size(&self) -> usize {
        3 + self.parameters.len()
    }

    fn write_hci<W: embedded_io::Write>(&self, mut writer: W) -> Result<(), W::Error> {
        let opcode = self.opcode.to_raw().to_le_bytes();
        writer.write_all(&[opcode[0], opcode[1], self.parameters.len() as u8])?;
        writer.write_all(self.parameters)
    }

    async fn write_hci_async<W: embedded_io_async::Write>(
        &self,
        mut writer: W,
    ) -> Result<(), W::Error> {
        let opcode = self.opcode.to_raw().to_le_bytes();
        writer
            .write_all(&[opcode[0], opcode[1], self.parameters.len() as u8])
            .await?;
        writer.write_all(self.parameters).await
    }
}
