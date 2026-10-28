//! Boot-scoped host cache exchange. The host never decodes a chip's record.
use crate::Result;
use oer_esp32s31_phy_probe::*;
use oer_hil_lab::calibration_cache::{CacheKey, Loaded, Provenance, SnapshotDownload, Store};
use oer_hil_link::SerialCapture;
use oer_hil_protocol::Endpoint;
use std::time::{Duration, Instant};

/// Typed HIL transport and monotonic deadline, replaceable without serial I/O in tests.
pub trait Transport {
    fn now(&self) -> Duration;
    fn request<E: Endpoint>(
        &self,
        boot: u64,
        session: u64,
        body: E,
        timeout: Duration,
    ) -> Result<E::Response>;
}

pub struct Link<'a> {
    capture: &'a SerialCapture,
    epoch: Instant,
}
impl<'a> Link<'a> {
    pub fn new(capture: &'a SerialCapture) -> Self {
        Self {
            capture,
            epoch: Instant::now(),
        }
    }
}
impl Transport for Link<'_> {
    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }
    fn request<E: Endpoint>(
        &self,
        boot: u64,
        session: u64,
        body: E,
        timeout: Duration,
    ) -> Result<E::Response> {
        self.capture.request_boot(boot, session, body, timeout)
    }
}

const RPC_TIMEOUT: Duration = Duration::from_secs(3);
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(120);

/// The result retains caller admission and native outcome as separate evidence.
#[derive(Debug, serde::Serialize)]
pub struct Started {
    pub boot_id: u64,
    pub status: CalibrationStartStatus,
    pub host_load_us: u64,
    pub host_transfer_us: u64,
    /// Host time from identity lookup to terminal status, on one monotonic clock.
    pub host_start_us: u64,
}

/// Identity lookup and disk read precede Start, outside the target's 500 ms window.
/// Every request is pinned to the observed boot; reconnection never changes it.
pub struct StartRequest<'a> {
    pub boot_id: u64,
    pub session_id: u64,
    pub chip: &'a str,
    pub board: oer_lab_board::DeviceId,
    pub capture: PrepareCapture,
    pub source: CalibrationSource,
}

pub fn start<T: Transport>(
    link: &T,
    store: &Store,
    request: StartRequest<'_>,
) -> Result<(CacheKey, Started)> {
    let StartRequest {
        boot_id: boot,
        session_id: session,
        chip,
        board,
        capture,
        source,
    } = request;
    let overall_began = link.now();
    if boot == 0 {
        return Err("cache exchange requires an observed boot".into());
    }
    let identity = link.request(boot, session, GetCalibrationCacheIdentity, RPC_TIMEOUT)?;
    if identity.active_ticket.is_some() {
        return Err("calibration already started in this boot".into());
    }
    let key = CacheKey::new(chip, board, identity.target)?;
    let load_began = link.now();
    let loaded = if source == CalibrationSource::HostCache {
        store.load(&key)
    } else {
        Loaded::Miss(CalibrationCacheMiss::Missing)
    };
    let host_load_us = elapsed_us(link, load_began);
    let began = link.now();
    let started = link.request(
        boot,
        session,
        StartCalibration { capture, source },
        RPC_TIMEOUT,
    )?;
    if started.ticket == 0 {
        return Err("target issued a zero calibration ticket".into());
    }
    let transfer_began = link.now();
    match (&started.state, source) {
        (CalibrationStartState::AwaitingCache(request), CalibrationSource::HostCache) => {
            if &request.target != key.target() || request.remaining_ms > CALIBRATION_CACHE_WAIT_MS {
                return Err("target changed calibration identity or wait budget".into());
            }
            // Start transit counts against our conservative host deadline as well.
            let deadline = began + Duration::from_millis(u64::from(request.remaining_ms));
            upload(link, boot, session, started.ticket, loaded, deadline)?;
        }
        (
            CalibrationStartState::Registering {
                decision: CalibrationCacheDecision::FullRequested,
            },
            CalibrationSource::Full,
        ) => {}
        _ => return Err("unexpected initial calibration state".into()),
    }
    let host_transfer_us = elapsed_us(link, transfer_began);
    let deadline = link.now() + REGISTRATION_TIMEOUT;
    loop {
        let status = link.request(
            boot,
            session,
            GetCalibrationStartStatus {
                ticket: started.ticket,
            },
            remaining(link, deadline)?,
        )?;
        if status.ticket != started.ticket {
            return Err("stale calibration status".into());
        }
        match status.state {
            CalibrationStartState::Ready { .. } => {
                return Ok((
                    key,
                    Started {
                        boot_id: boot,
                        status,
                        host_load_us,
                        host_transfer_us,
                        host_start_us: elapsed_us(link, overall_began),
                    },
                ));
            }
            CalibrationStartState::Failed { .. } => {
                return Err("native calibration registration failed".into());
            }
            CalibrationStartState::AwaitingCache(_) | CalibrationStartState::Registering { .. } => {
            }
        }
    }
}

fn elapsed_us(link: &impl Transport, since: Duration) -> u64 {
    u64::try_from(link.now().saturating_sub(since).as_micros()).unwrap_or(u64::MAX)
}

fn remaining(link: &impl Transport, deadline: Duration) -> Result<Duration> {
    deadline
        .checked_sub(link.now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| "calibration exchange deadline reached".into())
}

fn upload(
    link: &impl Transport,
    boot: u64,
    session: u64,
    ticket: u32,
    loaded: Loaded,
    deadline: Duration,
) -> Result<()> {
    let offer = match loaded {
        Loaded::Miss(reason) => CalibrationCacheOffer::NoCache(reason),
        Loaded::Snapshot(record) => {
            let bytes = record.bytes()?;
            let mut first = 0;
            for chunk in bytes.chunks(CALIBRATION_CACHE_PAGE) {
                let response = link.request(
                    boot,
                    session,
                    WriteCalibrationCachePage {
                        ticket,
                        first,
                        bytes: chunk.try_into().map_err(|_| "cache page exceeds bound")?,
                    },
                    remaining(link, deadline)?,
                )?;
                first += u16::try_from(chunk.len())?;
                if response.ticket != ticket || response.next != first {
                    return Err("stale cache page acknowledgement".into());
                }
            }
            CalibrationCacheOffer::Snapshot(record.metadata())
        }
    };
    let response = link.request(
        boot,
        session,
        CompleteCalibrationCache { ticket, offer },
        remaining(link, deadline)?,
    )?;
    if response.ticket != ticket {
        return Err("stale calibration completion".into());
    }
    Ok(())
}

/// Downloads only an immutable successful full snapshot and promotes after all checks.
/// Callers retain the resulting JSON in the run's immutable artifacts too.
pub fn save(
    link: &impl Transport,
    session: u64,
    store: &Store,
    key: &CacheKey,
    started: &Started,
    provenance: Provenance,
    artifact: &std::path::Path,
) -> Result<std::path::PathBuf> {
    let mut download =
        SnapshotDownload::new(started.boot_id, key.target().clone(), &started.status)?;
    while let Some(request) = download.next_request() {
        download.push(link.request(started.boot_id, session, request, RPC_TIMEOUT)?)?;
    }
    store.promote_archived(key, &download.finish()?, provenance, artifact)
}

#[cfg(test)]
mod tests;
