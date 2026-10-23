//! One boot's bounded cache admission and immutable full export, owned by P2.
#[path = "cache/adapter.rs"]
mod adapter;

use oer_esp32s31_knowledge::calibration_cache as codec;
use oer_esp32s31_phy::{PhyCalibrationCache, PhyCalibrationIdentity, PhyCalibrationSnapshot};
use oer_hil_protocol::{base::RejectReason, phy::*};
use oer_time::{Duration, Instant};

type Result<T> = core::result::Result<T, RejectReason>;
const TICKET: u32 = 1;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Idle,
    Awaiting,
    Pending,
    Registering,
    Ready,
    Failed,
}

pub struct Cache {
    bytes: [u8; CALIBRATION_CACHE_CAPACITY],
    next: u16,
    phase: Phase,
    deadline: Instant,
    config: PrepareCapture,
    decision: CalibrationCacheDecision,
    snapshot: Option<CalibrationCacheMetadata>,
    registration_us: u32,
    temperature: i16,
    released: bool,
    ready: Option<CaptureReady>,
}

impl Cache {
    #[inline(never)]
    pub fn place(storage: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        oer_memory::init_in_place!(
            storage,
            Self {
                bytes: [0; CALIBRATION_CACHE_CAPACITY],
                next: 0,
                phase: Phase::Idle,
                deadline: Instant::EPOCH,
                config: PrepareCapture {
                    channel: 1,
                    radio: CaptureRadio::Wifi,
                },
                decision: CalibrationCacheDecision::FullRequested,
                snapshot: None,
                registration_us: 0,
                temperature: 0,
                released: false,
                ready: None,
            }
        )
    }

    pub fn identity(&self, identity: PhyCalibrationIdentity) -> CalibrationCacheIdentity {
        CalibrationCacheIdentity {
            target: target(identity),
            active_ticket: (self.phase != Phase::Idle).then_some(TICKET),
        }
    }

    pub fn begin(
        &mut self,
        command: StartCalibration,
        identity: PhyCalibrationIdentity,
        now: Instant,
    ) -> Result<CalibrationStarted> {
        if !command.capture.valid() {
            return Err(RejectReason::InvalidConfiguration);
        }
        if self.phase != Phase::Idle {
            return Err(RejectReason::InvalidState);
        }
        self.config = command.capture;
        self.deadline =
            now.saturating_add(Duration::from_millis(u32::from(CALIBRATION_CACHE_WAIT_MS)));
        match command.source {
            CalibrationSource::Full => {
                self.phase = Phase::Pending;
                self.decision = CalibrationCacheDecision::FullRequested;
            }
            CalibrationSource::HostCache => {
                self.phase = Phase::Awaiting;
            }
        }
        Ok(CalibrationStarted {
            ticket: TICKET,
            state: self.status(TICKET, identity, now)?.state,
        })
    }

    /// Expiry wins at the exact boundary, before any upload or replay side effect.
    pub fn expire(&mut self, now: Instant) {
        if self.phase == Phase::Awaiting && now >= self.deadline {
            self.next = 0;
            self.decision = CalibrationCacheDecision::TimedOut;
            self.phase = Phase::Pending;
            self.released = true;
        }
        // Once sealed, a stalled console cannot postpone native registration forever.
        if self.phase == Phase::Pending && now >= self.deadline {
            self.released = true;
        }
    }
    pub fn deadline(&self) -> Option<Instant> {
        (self.phase == Phase::Awaiting || (self.phase == Phase::Pending && !self.released))
            .then_some(self.deadline)
    }
    pub fn pending(&self) -> bool {
        self.phase == Phase::Pending && self.released
    }
    /// P1 has published Started/Completed and returned the shared request slot.
    pub fn release(&mut self) {
        if self.phase == Phase::Pending {
            self.released = true;
        }
    }

    fn window(&mut self, ticket: u32, now: Instant) -> Result<()> {
        self.expire(now);
        if ticket != TICKET || self.phase != Phase::Awaiting {
            return Err(RejectReason::InvalidState);
        }
        Ok(())
    }
    pub fn write(
        &mut self,
        page: WriteCalibrationCachePage,
        now: Instant,
    ) -> Result<CalibrationCachePageWritten> {
        self.window(page.ticket, now)?;
        let range = page
            .range(TICKET, self.next)
            .ok_or(RejectReason::InvalidConfiguration)?;
        self.bytes
            .get_mut(range.clone())
            .ok_or(RejectReason::InvalidConfiguration)?
            .copy_from_slice(&page.bytes);
        self.next = range.end as u16;
        Ok(CalibrationCachePageWritten {
            ticket: TICKET,
            next: self.next,
        })
    }
    pub fn complete(
        &mut self,
        command: CompleteCalibrationCache,
        identity: PhyCalibrationIdentity,
        now: Instant,
    ) -> Result<CalibrationCacheCompleted> {
        self.window(command.ticket, now)?;
        self.decision = match command.offer {
            CalibrationCacheOffer::NoCache(reason) => CalibrationCacheDecision::NoCache(reason),
            CalibrationCacheOffer::Snapshot(metadata) => match self.admit(metadata, identity) {
                Ok(()) => CalibrationCacheDecision::CandidateSupplied,
                Err(reason) => CalibrationCacheDecision::Rejected(reason),
            },
        };
        if self.decision != CalibrationCacheDecision::CandidateSupplied {
            self.next = 0;
        }
        self.phase = Phase::Pending;
        Ok(CalibrationCacheCompleted {
            ticket: TICKET,
            decision: self.decision,
        })
    }
    fn admit(
        &self,
        metadata: CalibrationCacheMetadata,
        identity: PhyCalibrationIdentity,
    ) -> core::result::Result<(), CalibrationCacheRejection> {
        let expected = target(identity);
        let reject = if metadata.format != expected.format {
            Some(CalibrationCacheRejection::Format)
        } else if metadata.schema != expected.schema {
            Some(CalibrationCacheRejection::Schema)
        } else if metadata.algorithm_revision != expected.algorithm_revision {
            Some(CalibrationCacheRejection::Algorithm)
        } else if metadata.profile != expected.profile {
            Some(CalibrationCacheRejection::Profile)
        } else if usize::from(metadata.length) != codec::RECORD_BYTES
            || metadata.length != self.next
        {
            Some(CalibrationCacheRejection::Length)
        } else {
            None
        };
        if let Some(reason) = reject {
            return Err(reason);
        }
        let bytes = self
            .bytes
            .get(..usize::from(self.next))
            .ok_or(CalibrationCacheRejection::Length)?;
        if calibration_cache_crc32c(bytes) != metadata.crc32c {
            return Err(CalibrationCacheRejection::Checksum);
        }
        codec::CalibrationRecord::decode(metadata.format.0, bytes)
            .map_err(|_| CalibrationCacheRejection::Decode)?;
        Ok(())
    }

    /// Decode only structurally; native registration owns identity/completion/table validation.
    #[inline(never)]
    pub fn take_registration(&mut self) -> Result<(PrepareCapture, Option<PhyCalibrationCache>)> {
        if !self.pending() {
            return Err(RejectReason::InvalidState);
        }
        self.phase = Phase::Registering;
        let candidate = if self.decision == CalibrationCacheDecision::CandidateSupplied {
            let bytes = self
                .bytes
                .get(..usize::from(self.next))
                .ok_or(RejectReason::Internal)?;
            let record = codec::CalibrationRecord::decode(codec::S31_SEMANTIC_V1, bytes)
                .map_err(|_| RejectReason::Internal)?;
            Some(
                PhyCalibrationCache::from_snapshot(adapter::native(record))
                    .ok_or(RejectReason::Internal)?,
            )
        } else {
            None
        };
        Ok((self.config, candidate))
    }

    #[inline(never)]
    pub fn freeze(
        &mut self,
        full: bool,
        snapshot: PhyCalibrationSnapshot,
        registration_us: u32,
    ) -> Result<()> {
        if self.phase != Phase::Registering {
            return Err(RejectReason::InvalidState);
        }
        self.registration_us = registration_us;
        if full {
            self.temperature = snapshot.common.temperature;
            let bytes = adapter::record(snapshot)
                .encode()
                .map_err(|_| RejectReason::Internal)?;
            self.bytes
                .get_mut(..codec::RECORD_BYTES)
                .ok_or(RejectReason::Internal)?
                .copy_from_slice(&bytes);
            self.snapshot = Some(CalibrationCacheMetadata {
                format: CalibrationCacheFormat(codec::S31_SEMANTIC_V1),
                schema: codec::SNAPSHOT_SCHEMA,
                algorithm_revision: codec::CALIBRATION_ALGORITHM_REVISION,
                profile: codec::CALIBRATION_PROFILE_ID,
                length: codec::RECORD_BYTES as u16,
                crc32c: calibration_cache_crc32c(&bytes),
            });
        }
        Ok(())
    }
    pub fn ready(&mut self, ready: CaptureReady) -> Result<()> {
        if self.phase != Phase::Registering {
            return Err(RejectReason::InvalidState);
        }
        self.ready = Some(ready);
        self.phase = Phase::Ready;
        Ok(())
    }
    pub fn fail(&mut self) {
        self.phase = Phase::Failed;
        self.snapshot = None;
    }

    pub fn status(
        &self,
        ticket: u32,
        identity: PhyCalibrationIdentity,
        now: Instant,
    ) -> Result<CalibrationStartStatus> {
        if ticket != TICKET || self.phase == Phase::Idle {
            return Err(RejectReason::InvalidState);
        }
        let state = match self.phase {
            Phase::Awaiting => CalibrationStartState::AwaitingCache(CalibrationCacheRequest {
                target: target(identity),
                remaining_ms: (self.deadline.saturating_duration_since(now).as_micros() / 1000)
                    as u16,
            }),
            Phase::Pending | Phase::Registering => CalibrationStartState::Registering {
                decision: self.decision,
            },
            Phase::Ready => CalibrationStartState::Ready {
                decision: self.decision,
                capture: self.ready.ok_or(RejectReason::InvalidState)?,
                registration_us: self.registration_us,
                snapshot: self.snapshot,
            },
            Phase::Failed => CalibrationStartState::Failed {
                decision: self.decision,
            },
            Phase::Idle => return Err(RejectReason::InvalidState),
        };
        Ok(CalibrationStartStatus {
            ticket: TICKET,
            state,
        })
    }
    pub fn read(&self, request: ReadCalibrationSnapshot) -> Result<CalibrationSnapshot> {
        if self.phase != Phase::Ready {
            return Err(RejectReason::InvalidState);
        }
        let metadata = self.snapshot.ok_or(RejectReason::InvalidState)?;
        let range = request
            .range(TICKET, metadata.length)
            .ok_or(RejectReason::InvalidConfiguration)?;
        Ok(CalibrationSnapshot {
            ticket: TICKET,
            first: request.first,
            metadata,
            temperature: CalibrationTemperature {
                value: self.temperature,
                scale: CalibrationTemperatureScale::DegreesCelsius,
            },
            bytes: self
                .bytes
                .get(range)
                .ok_or(RejectReason::InvalidConfiguration)?
                .try_into()
                .map_err(|_| RejectReason::Internal)?,
        })
    }
}

pub fn target(identity: PhyCalibrationIdentity) -> CalibrationCacheTarget {
    let identity = codec::CalibrationIdentity {
        rf_cal_version: identity.rf_cal_version,
        base_mac_address: identity.base_mac_address,
        mac_extension: identity.mac_extension,
    };
    CalibrationCacheTarget {
        identity: identity
            .encode()
            .as_slice()
            .try_into()
            .expect("12-byte physical identity fits"),
        format: CalibrationCacheFormat(codec::S31_SEMANTIC_V1),
        schema: codec::SNAPSHOT_SCHEMA,
        algorithm_revision: codec::CALIBRATION_ALGORITHM_REVISION,
        profile: codec::CALIBRATION_PROFILE_ID,
    }
}

#[cfg(test)]
#[path = "cache/tests.rs"]
mod tests;
