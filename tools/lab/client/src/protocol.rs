//! The IPC protocol between the service and its clients over one Unix
//! stream socket.
//!
//! Every frame is a big-endian `u32` length of the rest, one kind byte and
//! the payload: a JSON [`Request`] or [`Reply`], or the bytes of an
//! operation's stream (its output from the service, its input from the
//! client) after the operation's big-endian `u64` identity. A frame longer
//! than [`MAX_FRAME`] ends the connection. The first request is
//! [`Request::Hello`]: a client of another [`PROTOCOL`] gets
//! [`Reply::Refused`] with the reason, and the connection ends.
//!
//! An operation's input flows by credit: a client sends at most
//! [`INPUT_WINDOW`] input frames ahead of the operation, and
//! [`Reply::Credit`] returns one for each frame the operation took. Input
//! beyond the window fails the operation.

use std::{
    io::{self, Read, Write},
    net::Ipv4Addr,
};

use oer_tools_lab_arbiter::claims::Claim;
use oer_tools_lab_arbiter::readiness::Readiness;
use serde::{Deserialize, Serialize};

pub mod station_link;

/// The protocol version; another one is refused at the hello, before any
/// operation. Version 3 added [`Reply::Ready`]; version 4 adds
/// [`Operation::FlashConsole`], which a version 3 service cannot parse.
pub const PROTOCOL: u32 = 4;
/// The token a command runs under: its client joins the lease the token
/// belongs to, for no more than the token allows, instead of queueing.
pub const LEASE_ENV: &str = "OER_LAB_LEASE";
/// The longest frame either side sends or accepts.
pub const MAX_FRAME: usize = 1 << 20;
/// Input frames a client may send an operation ahead of what it took.
pub const INPUT_WINDOW: usize = 64;

const MESSAGE: u8 = 0;
const DATA: u8 = 1;

/// One frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    /// A JSON request or reply.
    Message(Vec<u8>),
    /// Bytes of operation `operation`'s stream.
    Data { operation: u64, bytes: Vec<u8> },
}

impl Frame {
    pub fn message(message: &impl Serialize) -> io::Result<Self> {
        Ok(Self::Message(serde_json::to_vec(message)?))
    }

    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        let (kind, head, body): (u8, &[u8], &[u8]) = match self {
            Self::Message(json) => (MESSAGE, &[], json),
            Self::Data { operation, bytes } => (DATA, &operation.to_be_bytes(), bytes),
        };
        let length = 1 + head.len() + body.len();
        if length > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("a frame of {length} bytes exceeds {MAX_FRAME}"),
            ));
        }
        let length = u32::try_from(length).map_err(io::Error::other)?;
        let mut frame = Vec::with_capacity(4 + 1 + head.len() + body.len());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.push(kind);
        frame.extend_from_slice(head);
        frame.extend_from_slice(body);
        out.write_all(&frame)?;
        out.flush()
    }

    /// The next frame, `None` at the end of the stream between frames.
    pub fn read(input: &mut impl Read) -> io::Result<Option<Self>> {
        let mut length = [0u8; 4];
        match input.read_exact(&mut length) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
        }
        let length = usize::try_from(u32::from_be_bytes(length)).map_err(io::Error::other)?;
        if length == 0 || length > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("a frame of {length} bytes is outside 1..={MAX_FRAME}"),
            ));
        }
        let mut payload = vec![0u8; length];
        input.read_exact(&mut payload)?;
        let Some((&kind, rest)) = payload.split_first() else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "an empty frame"));
        };
        match kind {
            MESSAGE => Ok(Some(Self::Message(rest.to_vec()))),
            DATA => {
                let Some((operation, bytes)) = rest.split_first_chunk::<8>() else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "a data frame without its operation",
                    ));
                };
                Ok(Some(Self::Data {
                    operation: u64::from_be_bytes(*operation),
                    bytes: bytes.to_vec(),
                }))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown frame kind {other}"),
            )),
        }
    }
}

/// What a lease is requested for.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LeaseRequest {
    pub owner: String,
    pub work: String,
    #[serde(default)]
    pub scenarios: Vec<String>,
    #[serde(default)]
    pub run: Option<String>,
    /// None claims the whole stand.
    pub claims: Vec<Claim>,
    /// Stand maintenance and its reason: served first, preempting the
    /// holders it conflicts with.
    #[serde(default)]
    pub maintenance: Option<String>,
    /// Work that can stop at a boundary and queue again: after
    /// [`oer_tools_lab_arbiter::balance::MIN_SLICE`] it is asked to yield to a
    /// waiter whose owner's balance is higher.
    #[serde(default)]
    pub divisible: bool,
}

/// A request of a client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "request", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    /// The first request: the client's protocol and a description of it.
    Hello { protocol: u32, client: String },
    /// Queue for a lease; [`Reply::Granted`] follows.
    Acquire { lease: LeaseRequest },
    /// Join the lease `token` belongs to, for `claims` the token allows: a
    /// command inside a lease uses its resources without queueing.
    /// [`Reply::Granted`] carries a token of its own that allows `claims`
    /// only, for the commands it runs in turn.
    Join { token: String, claims: Vec<Claim> },
    /// End this connection's lease; [`Reply::Ended`] follows once its
    /// operations stopped and its resources' recoveries ended.
    Release,
    /// Start operation `id` under the lease; [`Reply::Finished`] follows.
    Start { id: u64, operation: Operation },
    /// Stop operation `id`.
    Cancel { id: u64 },
    /// The input of operation `id` ended.
    CloseInput { id: u64 },
    /// The service's state.
    Status,
    /// A person returns an unavailable resource: the service recovers it
    /// again.
    Return { resource: String },
    /// The stand's full report: holders, queue, balances, boards and
    /// recent leases.
    Report,
    /// Take board `mac` (or the stand, `"stand"`) out of service for every
    /// owner but `owner`.
    Maintain {
        mac: String,
        owner: String,
        reason: String,
    },
    /// Return board `mac` (or the stand) to service.
    EndMaintenance { mac: String },
    /// Charge `old`'s balance and history to `new`.
    MergeOwner { old: String, new: String },
    /// Drop `name`'s balance.
    ForgetOwner { name: String },
    /// Grant nothing more and answer once no lease is held: the service may
    /// then be replaced.
    Drain,
    /// Journal a flash of board `mac` that a command under a lease made.
    RecordFlash {
        owner: String,
        mac: String,
        image: oer_tools_lab_arbiter::journal::ImageIdentity,
    },
    /// Journal `event` of board `device` by `owner`.
    Record {
        owner: String,
        device: Option<String>,
        event: oer_tools_lab_arbiter::journal::BoardEventKind,
    },
    /// End held lease `lease` for `reason`, on behalf of `by`.
    Preempt {
        lease: u64,
        by: String,
        reason: String,
    },
    /// The boards attached to the host, with their hub ports:
    /// [`Reply::Inventory`].
    Inventory,
    /// `uhubctl`'s report of the stand's hubs: [`Reply::Hubs`].
    Hubs,
    /// Read the stand file again, after `cargo stand init` wrote it: new
    /// boards are recovered to their end state before they are granted.
    Reload,
}

/// Work on the leased resources, run by the service's drivers. A board is
/// named as the stand file names it (its id, or its MAC) and must be among
/// the lease's claims.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Operation {
    /// Write the image bundle in directory `bundle` into `board` and start
    /// it, unless `skip_carried` and the board already runs it; the flash
    /// is journaled as `image` by `owner`. Result: `{written, receipt}`.
    Flash {
        board: String,
        bundle: String,
        image: oer_tools_lab_arbiter::journal::ImageIdentity,
        owner: String,
        via: FlashVia,
        skip_carried: bool,
    },
    /// Write the image bundle in directory `bundle` into `board` over USB,
    /// leaving the chip in its ROM loader; then open `board`'s console and
    /// start the image by the console's reset, the only reset: no writer
    /// reset runs before it. The flash is journaled as `image` by `owner`
    /// once it started. The operation holds the board's port from the write
    /// until the client cancels it and streams the console as its output
    /// from before the start's reset. The first bytes are what the port
    /// still held just before that reset (drained right before its edge;
    /// bytes in flight may follow the drain), then the boot from the ROM
    /// banner on, while the chip's USB Serial/JTAG stays enumerated across
    /// the reset. `Reply::Ready` follows the reset's edge, in order with the
    /// stream. Output the client reads too slowly is dropped and counted
    /// (`Reply::Lost`). Result: `{written, receipt}`. Every failure after
    /// the write (the start, its confirmation, the journal, the console)
    /// is an error naming its stage, what was reached and the receipt.
    FlashConsole {
        board: String,
        bundle: String,
        image: oer_tools_lab_arbiter::journal::ImageIdentity,
        owner: String,
    },
    /// Reset `board` through `path`, or through the first reset its stand
    /// file lists when none. Result: the ROM's reset line.
    Reset {
        board: String,
        path: Option<oer_tools_lab_model::rom::ResetPath>,
    },
    /// `board`'s console as the operation's stream, opened without a reset,
    /// or with a reset into its application when `reset`, until the client
    /// cancels it. With `reset`, `Reply::Ready` follows the reset's edge,
    /// in order with the stream: every byte after it was read after the
    /// edge (bytes in flight at the edge may still be the earlier boot's).
    Console { board: String, reset: bool },
    /// Read `board`'s console for `watch_ms` without a reset. Result: the
    /// text.
    ReadConsole { board: String, watch_ms: u64 },
    /// Power `board` off and on at its hub port, watching it leave and
    /// return. Result: how long it took to return.
    PowerCycle { board: String },
    /// A power cycle with the chip's reset cause read after it, proving the
    /// port cuts the board's power.
    ProvePowerLoss { board: String },
    /// An operation on the lab router; the lease must hold the router.
    Router { request: RouterOperation },
    /// Probe the equipment the stand reaches over SSH (the lab router, the
    /// air observer). The lease must hold the router. Result: the
    /// `oer_tools_lab_router::fixtures::Fixture` list.
    ProbeRouter,
    /// The chip of every board attached to the host, its ROM asked when no
    /// earlier answer is known; the lease must hold the whole stand.
    /// Result: a list of `{mac, chip}`, the chip a chip profile's id.
    Detect,
    /// Where `board` is attached and what its flash holds. Result:
    /// `{port, image}`, both text.
    Describe { board: String },
    /// Switch port `port` of hub `hub` (stand-file ids) off for `off_secs`,
    /// then on: a person sees which button it is. The lease must hold the
    /// port (`hub-port:LOCATION:PORT`).
    HubPort {
        hub: String,
        port: u8,
        off_secs: u64,
    },
}

/// How a flash reaches the board.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlashVia {
    Usb,
    Jtag,
}

/// A reply or notice of the service.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "reply", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Reply {
    Welcome {
        protocol: u32,
    },
    /// The request is refused; after a refused `Hello` the connection ends.
    Refused {
        reason: String,
    },
    /// The lease request waits as ticket `ticket`.
    Waiting {
        ticket: u64,
        message: String,
    },
    /// The lease is granted, or joined. `token` lets the commands this
    /// client runs join it, for what this client holds.
    Granted {
        lease: u64,
        token: String,
    },
    /// The service ended the lease for `reason`; its operations are being
    /// stopped.
    Revoked {
        reason: String,
    },
    /// Divisible work should release the lease at its next boundary and
    /// queue again, for `reason`.
    Yield {
        reason: String,
    },
    /// The lease ended: its operations stopped and its resources'
    /// recoveries ended. `unavailable` names each resource no end state was
    /// confirmed for, with why.
    Ended {
        unavailable: Vec<(String, String)>,
    },
    /// Operation `id` took one input frame: the client may send one more.
    Credit {
        id: u64,
    },
    /// Operation `id` finished.
    Finished {
        id: u64,
        result: Result<serde_json::Value, String>,
    },
    /// `bytes` of operation `id`'s output were dropped for a slow reader.
    Lost {
        id: u64,
        bytes: u64,
    },
    /// Operation `id` is ready: what it streams from now on is what it was
    /// asked for (a monitor capture listens; a console's reset edge has
    /// passed). Sent apart from the output, in order with it, at most once;
    /// an operation without a readiness of its own never sends it.
    Ready {
        id: u64,
    },
    Status {
        status: Status,
    },
    Report {
        status: Box<oer_tools_lab_arbiter::Status>,
    },
    /// The request was served; `changed` when it changed anything.
    Done {
        changed: bool,
    },
    Inventory {
        boards: Vec<AttachedBoard>,
    },
    /// `uhubctl`'s report, as it printed it (`oer_tools_lab_model::hubs::parse`).
    Hubs {
        report: String,
    },
}

/// A board attached to the host, as the service finds it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttachedBoard {
    /// Its stand-file id, when the stand file names it.
    pub id: Option<String>,
    pub mac: String,
    /// Its USB location.
    pub location: String,
    pub tty: Option<String>,
    /// The hub port it hangs on, `HUB:PORT`.
    pub hub_port: Option<String>,
    /// Whether the stand may power-cycle it, or why not.
    pub power: Result<(), String>,
    /// What the stand file says of it that the host contradicts.
    pub warning: Option<String>,
}

/// The service's state, for `cargo stand queue` and the tests.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Status {
    pub holders: Vec<HolderStatus>,
    pub waiting: Vec<u64>,
    /// Resources whose driver runs a recovery.
    pub recovering: Vec<String>,
    pub resources: Vec<(String, Readiness)>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HolderStatus {
    pub lease: u64,
    pub owner: String,
    pub work: String,
    /// The lease ended and its operations are being stopped.
    pub ending: bool,
    /// Its running operations.
    pub operations: usize,
}

/// The width of the station profile's channel.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouterChannelWidth {
    /// HT 20 MHz.
    #[default]
    Ht20,
    /// HT 40 MHz, the secondary channel below the primary on channels 5 to
    /// 13 and above it on 1 to 4.
    Ht40,
    /// HE 20 MHz, 802.11ax on: the HE20 station profile. Every other width
    /// runs with HE off.
    He20,
}

/// An access category of the AP radio's airtime queue limits.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouterAccessCategory {
    Vo,
    Vi,
    Be,
    Bk,
}

/// One operation on the lab router.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "router", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RouterOperation {
    /// Apply the WPA2 station profile with `ssid` and `passphrase` on
    /// `channel`, one the stand file's `[router]` allows, `width` wide, to
    /// the stand's AP; its isolated network forwards UDP and TCP between its
    /// stations and `forward_host` when given (a run's load generator or TCP
    /// peer on the host), and its DHCP server serves. Result: the applied
    /// profile's description.
    ApplyStationProfile {
        ssid: String,
        passphrase: String,
        channel: u8,
        /// Absent from a client older than the width: HT20.
        #[serde(default)]
        width: RouterChannelWidth,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forward_host: Option<Ipv4Addr>,
    },
    /// Check that the router has its observation tools.
    VerifyObservationTools,
    /// Check that the AP's runtime state matches a WPA2 profile.
    VerifyRuntime,
    /// The AP's group rekey interval, in seconds.
    GroupRekeySeconds,
    /// What the AP knows of station `mac`.
    Station { mac: String },
    /// What the AP's driver reports of station `mac`'s link: its counters,
    /// the rates of its last frames each way, the AP's channel width, the
    /// survey of the channel in use, and the station's per-TID and the
    /// radio's transmit queues. Result: a [`station_link::StationLink`].
    StationLink { mac: String },
    /// Station `mac`'s DHCP lease, if it has one.
    Lease { mac: String },
    /// `count` pings from the router to `address`.
    Ping { address: Ipv4Addr, count: u16 },
    /// Ask the AP to deauthenticate station `mac`.
    Deauthenticate { mac: String },
    /// Have the isolated station network's DHCP server serve (`serve`) or
    /// stop answering; refused before a station profile applied the
    /// network. A later station profile serves again. Result:
    /// `{ "serving": bool }`, as read back from the router.
    StationDhcp { serve: bool },
    /// Prepare the routed network's check. Result: the state before.
    PrepareNetwork,
    /// Check the routed network against its state `before`.
    VerifyNetwork { before: String },
    /// Watch `count` echo requests to `address` and their replies until
    /// the operation stops or the observer ends. Result: the echoes.
    CaptureEchoes { address: Ipv4Addr, count: u16 },
    /// Watch the AP's log for station `mac`'s departure until the
    /// operation stops. Result: what it saw.
    Departure { mac: String },
    /// Set access category `ac`'s airtime queue limits (mac80211's AQL) on
    /// the AP radio to `low_us`/`high_us` of estimated airtime, and station
    /// `station`'s own limits too when given: mac80211 keeps a station's
    /// limits that differ from the radio's old ones. A station associating
    /// later starts with the radio's. Needs the router held exclusively.
    /// The service journals the intent before anything is written, then the
    /// confirmed or unconfirmed result; an intent it cannot journal writes
    /// nothing. The router's end state after the lease has every category
    /// at mac80211's defaults again; a station's own limits end with the
    /// station. Result: the router's `station::airtime::Airtime`, read back
    /// after the write.
    SetAirtimeLimit {
        ac: RouterAccessCategory,
        low_us: u32,
        high_us: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        station: Option<String>,
    },
    /// The AP radio's airtime limits and total threshold, and station
    /// `mac`'s own limits and pending airtime when given. Result: the
    /// router's `station::airtime::Airtime`.
    AirtimeLimits {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac: Option<String>,
    },
    /// Capture `channel`, one the stand file's `[router]` allows, on a
    /// monitor interface the operation creates on the idle AP radio, until
    /// the operation stops, IPv6 off on it and only received packets
    /// captured (`tcpdump -Q in`: what the host's own stack sends through
    /// it is left out; copies mac80211 delivers of transmitted frames may
    /// stay). The output is the pcap stream alone (radiotap
    /// headers, link type 127); an output the client does not take ends
    /// the capture. Once `tcpdump` listens the client hears `Ready`, apart
    /// from the bytes; a capture that never listens ends without it. Every
    /// ending stops `tcpdump`, waits for its exit and removes the
    /// interface. Needs the router held exclusively. Result: the router's
    /// `monitor::Report`.
    MonitorCapture { channel: u8 },
}

#[cfg(test)]
mod tests;
