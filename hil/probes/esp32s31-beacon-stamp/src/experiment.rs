//! TEMPORARY experiment: the rounds of one run.
//!
//! Every round publishes one subject frame (`frames.rs`) with the caller's
//! value K in MPDU octets 24..32 on the VO queue (hardware queue 0). In a
//! delayed round a 1500-octet group-addressed blocker at 1 Mb/s (about
//! 12.4 ms on the air) is published first on the VI queue (hardware queue
//! 1), and the subject is published `BLOCKER_LEAD` later, while the blocker
//! occupies the medium: the subject waits for the medium between its
//! publication and its transmission. A hardware queue holds one chain, so
//! the blocker cannot sit in the subject's own VO queue
//! (`MacTxPrepareRefused::QueueActive`). Both use one attempt: group frames
//! await no response, and the individually addressed subjects go to an
//! absent receiver and end on their ACK timeout without a retry.
//!
//! Around the subject's publication the device takes the pair (MAC local
//! time, AP TSF) before and after; it takes the same pair when it observes
//! each completion; after completion it reads octets 24..32 back from the
//! subject's buffer. Nothing is decided here: the records are printed
//! after the last round.
use super::{CHANNEL, TX_BUFFER, usb};
use crate::frames::{self, Addresses, Arm, Kind, MPDU_OFFSET, Round};
use crate::record::{self, Blocker, Completion, Record, Sample};
use core::pin::Pin;
use oer_chips_esp32s31_hal::{
    radio::ieee80211::mac::MacInterface,
    system::owner::RadioAccess,
    types::{MacLegacyRate, MacTxQueueIndex},
};
use oer_chips_esp32s31_radio_ieee80211_backend::{
    mac::tx::{
        CompletionObservation, LegacyTxConfig, LegacyTxQueue, MacLegacyTxResponse, TxControlFrame,
        TxCookie, TxPhyRate, TxSlot, TxSlotAccess, protection::TxProtection,
    },
    tx::{CappedTxPowerProfile, WifiTxPowerProfile},
};
use oer_chips_esp32s31_radio_phy::PhyTxTargetPowerProfile;
use oer_portable_diagnostics::{Name, name};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_time::{Clock, Duration, Instant, Timer};
use oer_protocols_radio_regulatory::TxCeiling;

/// Every frame's rate: DSSS 1 Mb/s, long preamble.
const RATE: MacLegacyRate = MacLegacyRate::Dsss1MLong;
/// From the blocker's publication to the subject's: well inside the
/// blocker's 12.4 ms on the air, so the subject's wait for the medium is
/// about 11 ms. The capture, not this constant, establishes the wait.
const BLOCKER_LEAD: Duration = Duration::from_millis(1);
/// One round every 100 ms from the run's start.
const PERIOD: Duration = Duration::from_millis(100);
/// From MAC start to the AP TSF's restart: the MAC local time is then at
/// least this far ahead of the AP TSF for the whole run, and the two clock
/// hypotheses differ by more than any round's bracket.
const CLOCK_SEPARATION: Duration = Duration::from_secs(2);
/// A completion poll.
const POLL: Duration = Duration::from_micros(100);
/// A round's frames complete within this, or the run ends: a blocker and
/// its subject need about 13 ms.
const COMPLETION_BOUND: Duration = Duration::from_millis(100);

static RECORDS: static_cell::ConstStaticCell<[Record; frames::ROUNDS as usize]> =
    static_cell::ConstStaticCell::new([Record::EMPTY; frames::ROUNDS as usize]);

pub(super) struct Resources<'a> {
    pub(super) access: &'a mut RadioAccess,
    pub(super) subject: Pin<&'static mut TxSlot<TX_BUFFER>>,
    pub(super) blocker: Pin<&'static mut TxSlot<TX_BUFFER>>,
    pub(super) tx_power: PhyTxTargetPowerProfile,
    pub(super) tx_ceiling: TxCeiling,
    pub(super) ap: [u8; 6],
    pub(super) run: u32,
}

/// The pair (MAC local time, AP TSF): local before and after one snapshot.
fn sample(access: &mut RadioAccess) -> Sample {
    let mut hal = access.wifi_mac_hal();
    let local_before = hal.mac_local_time();
    let tsf = hal.access_point_tsf();
    let local_after = hal.mac_local_time();
    Sample {
        local_before,
        tsf,
        local_after,
    }
}

async fn wait_until(at: Instant) {
    EmbassyClock.wait_until(at).await;
}

async fn wait(by: Duration) -> Result<(), Name<32>> {
    let at = EmbassyClock
        .now()
        .checked_add(by)
        .ok_or(name!("wait-deadline"; 32))?;
    wait_until(at).await;
    Ok(())
}

/// The calibrated power codes of the data and control rate, bounded by the
/// regulatory ceiling, as the ordinary transmitter publishes them.
#[derive(Clone, Copy)]
struct Power {
    data: u8,
    control: TxControlFrame,
}

fn power(calibrated: PhyTxTargetPowerProfile, ceiling: TxCeiling) -> Result<Power, Name<32>> {
    let capped = CappedTxPowerProfile::new(calibrated, ceiling);
    let data = u8::try_from(capped.power_pair(RATE.code()).primary)
        .map_err(|_| name!("tx-power-negative"; 32))?;
    let mut control = TxControlFrame {
        protection: TxProtection::None,
        ..TxControlFrame::UNPROTECTED
    };
    let pair = capped.power_pair(control.rate(TxPhyRate::Legacy(RATE)).code());
    control.power_primary =
        u8::try_from(pair.primary).map_err(|_| name!("tx-power-negative"; 32))?;
    control.power_alternate =
        u8::try_from(pair.alternate).map_err(|_| name!("tx-power-negative"; 32))?;
    Ok(Power { data, control })
}

/// One attempt from the AP interface at `RATE`.
fn config(psdu: u16, power: Power, response: MacLegacyTxResponse) -> LegacyTxConfig {
    let mut config = LegacyTxConfig::management(RATE, psdu);
    config.control = power.control;
    config.data_power = power.data;
    config.interface = MacInterface::AccessPoint;
    config.response = response;
    config
}

/// Write a frame into the free `slot` and reserve its descriptor.
fn prepare(
    slot: Pin<&mut TxSlot<TX_BUFFER>>,
    write: impl FnOnce(&mut [u8]) -> Result<usize, frames::FrameTooLong>,
) -> Result<(TxCookie, u16), Name<32>> {
    let TxSlotAccess::Free(mut free) = slot.access() else {
        return Err(name!("tx-slot-not-free"; 32));
    };
    let buffer = free.buffer_mut().map_err(|_| name!("tx-buffer"; 32))?;
    let mpdu = write(
        buffer
            .get_mut(MPDU_OFFSET..)
            .ok_or(name!("tx-buffer-short"; 32))?,
    )
    .map_err(|_| name!("frame-too-long"; 32))?;
    let transfer = frames::metadata(buffer, mpdu).map_err(|_| name!("frame-too-long"; 32))?;
    let cookie = free
        .reserve(transfer.capacity, transfer.length)
        .map_err(|_| name!("tx-reserve"; 32))?;
    Ok((cookie, transfer.psdu))
}

/// Acknowledge `slot`'s completion if the hardware reports one.
fn completion(
    slot: Pin<&mut TxSlot<TX_BUFFER>>,
    access: &mut RadioAccess,
) -> Result<Option<Completion>, Name<32>> {
    match slot
        .acknowledge_completion(access)
        .map_err(|_| name!("tx-completion"; 32))?
    {
        Some(CompletionObservation::Observed(done)) => Ok(Some(Completion {
            at: sample(access),
            status: done.status(),
            detail: done.detail(),
        })),
        Some(CompletionObservation::NoEffect) | None => Ok(None),
    }
}

/// The subject's octets 24..32 in its buffer, read after completion.
fn readback(slot: Pin<&mut TxSlot<TX_BUFFER>>) -> Result<[u8; 8], Name<32>> {
    let TxSlotAccess::Free(mut free) = slot.access() else {
        return Err(name!("tx-slot-not-free"; 32));
    };
    let buffer = free.buffer_mut().map_err(|_| name!("tx-buffer"; 32))?;
    frames::stamp_octets(buffer).ok_or(name!("tx-buffer-short"; 32))
}

async fn round(
    resources: &mut Resources<'_>,
    round: Round,
    addresses: Addresses,
    power: Power,
) -> Result<Record, Name<32>> {
    let Resources {
        access,
        subject,
        blocker,
        run,
        ..
    } = resources;
    let response = match round.kind {
        Kind::Beacon => MacLegacyTxResponse::None,
        Kind::ProbeResponse | Kind::Action => MacLegacyTxResponse::Ack,
    };
    let (cookie, psdu) = prepare(subject.as_mut(), |buffer| {
        frames::subject(buffer, round, addresses)
    })?;
    let mut record = Record {
        n: round.n,
        kind: round.kind,
        arm: round.arm,
        k: round.k,
        ..Record::EMPTY
    };
    let mut blocker_cookie = None;
    if round.arm == Arm::Delayed {
        let (cookie, psdu) = prepare(blocker.as_mut(), |buffer| {
            frames::blocker(buffer, round.n, *run, addresses.ap)
        })?;
        let published = sample(access);
        blocker
            .as_mut()
            .submit_legacy(
                *access,
                cookie,
                LegacyTxQueue::Video,
                config(psdu, power, MacLegacyTxResponse::None),
            )
            .map_err(|_| name!("blocker-submit"; 32))?;
        blocker_cookie = Some(cookie);
        wait(BLOCKER_LEAD).await?;
        let queue = access
            .wifi_mac_hal()
            .ordinary_tx_queue_snapshot(MacTxQueueIndex::One);
        record.blocker = Some(Blocker {
            published,
            done: None,
            queue: u8::from(queue.valid)
                | u8::from(queue.enabled) << 1
                | u8::from(queue.completion_pending) << 2,
        });
    }
    record.published = sample(access);
    subject
        .as_mut()
        .submit_legacy(
            *access,
            cookie,
            LegacyTxQueue::Voice,
            config(psdu, power, response),
        )
        .map_err(|_| name!("subject-submit"; 32))?;
    record.handed = sample(access);
    let deadline = EmbassyClock
        .now()
        .checked_add(COMPLETION_BOUND)
        .ok_or(name!("completion-deadline"; 32))?;
    loop {
        if record.done.is_none() {
            record.done = completion(subject.as_mut(), access)?;
        }
        if let Some(state) = record.blocker.as_mut()
            && state.done.is_none()
        {
            state.done = completion(blocker.as_mut(), access)?;
        }
        let blocker_done = record.blocker.is_none_or(|state| state.done.is_some());
        if record.done.is_some() && blocker_done {
            break;
        }
        if EmbassyClock.now() >= deadline {
            return Err(name!("completion-timeout"; 32));
        }
        wait(POLL).await?;
    }
    subject
        .as_mut()
        .detach_completed(*access, cookie)
        .map_err(|_| name!("subject-detach"; 32))?;
    if let Some(cookie) = blocker_cookie {
        blocker
            .as_mut()
            .detach_completed(*access, cookie)
            .map_err(|_| name!("blocker-detach"; 32))?;
    }
    record.readback = Some(readback(subject.as_mut())?);
    Ok(record)
}

/// What the run left: the records it completed and why it ended early.
struct Ran {
    count: usize,
    start: Sample,
    end: Option<Sample>,
    error: Option<Name<32>>,
}

async fn rounds(resources: &mut Resources<'_>, records: &mut [Record]) -> Result<Ran, Name<32>> {
    let addresses = Addresses {
        ap: resources.ap,
        absent: frames::ABSENT,
    };
    let power = power(resources.tx_power, resources.tx_ceiling)?;
    {
        let mut hal = resources.access.wifi_mac_hal();
        hal.program_interface_bssid(MacInterface::AccessPoint, resources.ap);
        hal.configure_access_point_receive_policy(resources.ap);
    }
    wait(CLOCK_SEPARATION).await?;
    resources
        .access
        .wifi_mac_hal()
        .reset_and_start_access_point_tsf();
    let start = sample(resources.access);
    let first = EmbassyClock
        .now()
        .checked_add(PERIOD)
        .ok_or(name!("round-deadline"; 32))?;
    let mut ran = Ran {
        count: 0,
        start,
        end: None,
        error: None,
    };
    let mut at = first;
    for (n, slot) in (0..frames::ROUNDS).zip(records.iter_mut()) {
        wait_until(at).await;
        at = at.checked_add(PERIOD).ok_or(name!("round-deadline"; 32))?;
        let Some((kind, arm)) = frames::round(n) else {
            ran.error = Some(name!("round-schedule"; 32));
            return Ok(ran);
        };
        let this = Round {
            n,
            kind,
            arm,
            run: resources.run,
            k: frames::k_of(n),
            channel: CHANNEL,
        };
        match round(resources, this, addresses, power).await {
            Ok(record) => {
                *slot = record;
                ran.count = ran.count.saturating_add(1);
            }
            Err(error) => {
                ran.error = Some(error);
                return Ok(ran);
            }
        }
    }
    ran.end = Some(sample(resources.access));
    Ok(ran)
}

async fn line(console: &mut usb::Console, bytes: &[u8]) -> Result<(), Name<32>> {
    usb::text(console, bytes)
        .await
        .map_err(|_| name!("console-write"; 32))
}

/// Run every round, then print the records.
pub(super) async fn run(
    console: &mut usb::Console,
    mut resources: Resources<'_>,
) -> Result<(), Name<32>> {
    let records = RECORDS.try_take().ok_or(name!("records-claimed"; 32))?;
    let ran = rounds(&mut resources, records).await?;
    let rx_full = resources.access.wifi_mac_hal().rx_buffer_full_count();
    let mut begin = crate::line::Line::<160>::new();
    begin
        .text(b"STAMP v1 begin run=")
        .and_then(|l| l.hex32(resources.run))
        .and_then(|l| l.text(b" channel="))
        .and_then(|l| l.decimal(u64::from(CHANNEL)))
        .and_then(|l| l.text(b" rounds="))
        .and_then(|l| l.decimal(u64::from(frames::ROUNDS)))
        .and_then(|l| l.text(b" ap="))
        .and_then(|l| l.hex_bytes(&resources.ap))
        .and_then(|l| l.text(b" absent="))
        .and_then(|l| l.hex_bytes(&frames::ABSENT))
        .and_then(|l| l.text(b" blocker="))
        .and_then(|l| l.decimal(frames::BLOCKER_BODY as u64))
        .and_then(|l| l.text(b" rate=dsss1\n"))
        .map_err(|_| name!("line-full"; 32))?;
    line(console, begin.as_bytes()).await?;
    let start = record::clock_line(b"start", ran.start).map_err(|_| name!("line-full"; 32))?;
    line(console, start.as_bytes()).await?;
    for record in records.iter().take(ran.count) {
        let text = record::record_line(record).map_err(|_| name!("line-full"; 32))?;
        line(console, text.as_bytes()).await?;
    }
    if let Some(end) = ran.end {
        let end = record::clock_line(b"end", end).map_err(|_| name!("line-full"; 32))?;
        line(console, end.as_bytes()).await?;
    }
    let mut last = crate::line::Line::<96>::new();
    last.text(b"STAMP end records=")
        .and_then(|l| l.decimal(ran.count as u64))
        .and_then(|l| l.text(b" rxfull="))
        .and_then(|l| l.decimal(u64::from(rx_full)))
        .and_then(|l| l.text(b" error="))
        .and_then(|l| {
            l.text(
                ran.error
                    .as_ref()
                    .map_or(b"-".as_slice(), |e| e.as_str().as_bytes()),
            )
        })
        .and_then(|l| l.text(b"\n"))
        .map_err(|_| name!("line-full"; 32))?;
    line(console, last.as_bytes()).await?;
    match ran.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
