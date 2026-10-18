//! Operation-owned mutable memory. All regions and event capacity die with the session.
use crate::*;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegionKind {
    Image,
    Stack,
    Ram(RegionLifetime),
    Allocation {
        lifetime: RegionLifetime,
        requested: u32,
    },
}
impl RegionKind {
    /// Memory that `finish_phase` releases.
    fn is_transient(self) -> bool {
        matches!(
            self,
            RegionKind::Stack
                | RegionKind::Ram(RegionLifetime::Phase)
                | RegionKind::Allocation {
                    lifetime: RegionLifetime::Phase,
                    ..
                }
        )
    }
}
struct Mapping {
    address: u32,
    length: usize,
    flags: u32,
    kind: RegionKind,
}
struct Region<'a> {
    address: u32,
    bytes: ScratchBytes<'a>,
    known: ScratchBytes<'a>,
    flags: u32,
    kind: RegionKind,
}
pub(super) struct Session<'a> {
    regions: Vec<Region<'a>>,
    memory: &'a WorkingMemory,
    _metadata: MemoryReservation<'a>,
    devices: crate::devices::Devices<'a>,
    calls: crate::external_calls::Calls<'a>,
    capture: Option<(CallCapture, MemoryReservation<'a>)>,
    timeline: TimelineCapture,
    pc: Option<u32>,
    final_memory: Vec<FinalMemoryChunk>,
    final_memory_capacity: Option<MemoryReservation<'a>>,
    /// Coalesced persistent writes of the phase, when the timeline selects them.
    written: Vec<WrittenRange>,
    written_capacity: Option<MemoryReservation<'a>>,
    call_observations: Vec<CallObservation>,
    model_observations: Vec<ModelObservation>,
    events: Vec<ExecutionEvent>,
    /// Admitted capacity of `events`, grown on demand up to `max_events`.
    event_capacity: Option<MemoryReservation<'a>>,
    max_events: usize,
    /// Exact four-byte reservation; never shared between implementations/phases.
    reservation: Option<u32>,
    /// Code reached by this side, carried from session to session.
    coverage: crate::execution_coverage::CodeCoverage<'a>,
    /// Region of the most recent lookup; nearby accesses usually repeat it.
    last_region: std::cell::Cell<usize>,
    /// Step log for observation dependence, when requested.
    steps: Option<crate::execution_steps::StepLog<'a>>,
}
/// Bytes one image patch may replace: a few instructions.
const MAX_IMAGE_PATCH_BYTES: usize = 64;
/// Events admitted by the first growth of a session's event buffer.
const INITIAL_EVENT_CAPACITY: usize = 1024;
impl<'a> Session<'a> {
    pub fn new(
        memory: &'a WorkingMemory,
        max_events: u32,
        coverage: crate::execution_coverage::CodeCoverage<'a>,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let metadata = memory.reserve(
            1024 * 1024 + (MAX_DEVICE_MODELS + MAX_CALL_MODELS) as u64 * 512,
            c.position(),
        )?;
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(2048)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "region allocation refused"))?;
        let mut model_observations = Vec::new();
        model_observations
            .try_reserve_exact(MAX_DEVICE_MODELS)
            .map_err(|_| {
                Error::new(
                    ErrorCode::ResourceLimited,
                    "model observation allocation refused",
                )
            })?;
        let mut call_observations = Vec::new();
        call_observations
            .try_reserve_exact(MAX_CALL_MODELS)
            .map_err(|_| {
                Error::new(
                    ErrorCode::ResourceLimited,
                    "call observations allocation refused",
                )
            })?;
        Ok(Self {
            regions,
            memory,
            _metadata: metadata,
            devices: crate::devices::Devices::new(memory),
            calls: crate::external_calls::Calls::new(memory),
            capture: None,
            timeline: TimelineCapture::default(),
            pc: None,
            final_memory: Vec::new(),
            final_memory_capacity: None,
            written: Vec::new(),
            written_capacity: None,
            call_observations,
            model_observations,
            events: Vec::new(),
            event_capacity: None,
            max_events: max_events as usize,
            reservation: None,
            coverage,
            last_region: std::cell::Cell::new(0),
            steps: None,
        })
    }
    /// One read and its value: known, unknown or unavailable.
    pub(super) fn read_value(
        &mut self,
        address: u32,
        width: u8,
        access: MemoryAccess,
        c: &mut dyn RunControl,
    ) -> Result<MemoryReadValue> {
        c.checkpoint(self.regions.len() as u64 + 1)?;
        if !matches!(access, MemoryAccess::Read | MemoryAccess::Fetch)
            || !matches!(width, 1 | 2 | 4)
        {
            return Ok(MemoryReadValue::Unavailable);
        }
        // Ordinary memory is byte-addressable in this execution environment.
        // Misaligned MMIO never dispatches or decomposes into device accesses.
        if !address.is_multiple_of(u32::from(width))
            && (access == MemoryAccess::Fetch
                || self.devices.overlaps(address, u64::from(width), c)?)
        {
            return Ok(MemoryReadValue::Unavailable);
        }
        if access == MemoryAccess::Read
            && let Some(value) = self.devices.read(address, width, c)?
        {
            let Some(value) = value else {
                return Ok(MemoryReadValue::Unavailable);
            };
            self.log(
                crate::execution_steps::StepEntry::Read {
                    address,
                    width,
                    device: true,
                },
                c,
            )?;
            self.event(
                ExecutionEvent::Read {
                    address,
                    width,
                    value,
                },
                c,
            )?;
            return Ok(MemoryReadValue::Known { value });
        }
        let Some((i, offset)) = self.region_index(address, width) else {
            return Ok(MemoryReadValue::Unavailable);
        };
        let r = &self.regions[i];
        let required = if access == MemoryAccess::Fetch { 1 } else { 4 };
        let value = if r.flags & required == 0 {
            MemoryReadValue::Unavailable
        } else if r.known[offset..offset + width as usize].contains(&0) {
            MemoryReadValue::Unknown
        } else {
            let mut bytes = [0; 4];
            bytes[..width as usize].copy_from_slice(&r.bytes[offset..offset + width as usize]);
            MemoryReadValue::Known {
                value: u32::from_le_bytes(bytes),
            }
        };
        if access == MemoryAccess::Read {
            self.log(
                crate::execution_steps::StepEntry::Read {
                    address,
                    width,
                    device: false,
                },
                c,
            )?;
            self.trace_memory(
                MemoryTransaction::Read {
                    address,
                    width,
                    value,
                },
                c,
            )?;
        }
        Ok(value)
    }

    /// Replace `patch.original` with `patch.replacement` in executable image
    /// code. A patch outside one executable image region, or whose original
    /// bytes differ from the loaded ones, is invalid.
    pub fn patch(
        &mut self,
        patch: &crate::in_process::ImagePatch,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        c.checkpoint(self.regions.len() as u64 + 1)?;
        let invalid = |message: &str| Error::new(ErrorCode::InvalidRequest, message);
        let length = patch.original.len();
        if length == 0 || length != patch.replacement.len() || length > MAX_IMAGE_PATCH_BYTES {
            return Err(invalid("image patch lengths are invalid"));
        }
        let Some((i, offset)) = u8::try_from(length)
            .ok()
            .and_then(|width| self.region_index(patch.address, width))
        else {
            return Err(invalid("image patch is outside the loaded image"));
        };
        let r = &mut self.regions[i];
        if r.kind != RegionKind::Image || r.flags & 1 == 0 {
            return Err(invalid("image patch is outside executable image code"));
        }
        if r.known[offset..offset + length].contains(&0)
            || r.bytes[offset..offset + length] != patch.original[..]
        {
            return Err(invalid("image patch original bytes differ from the image"));
        }
        r.bytes[offset..offset + length].copy_from_slice(&patch.replacement);
        Ok(())
    }
    /// Write `preload`'s known bytes into the one mapped writable region that
    /// holds all of them: an input of the phase, never a guest effect.
    fn preload(&mut self, preload: &MemoryPreload, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(self.regions.len() as u64 + 1)?;
        let (start, end) = (
            u64::from(preload.address),
            u64::from(preload.address) + preload.bytes.len() as u64,
        );
        let Some(r) = self.regions.iter_mut().find(|r| {
            u64::from(r.address) <= start && end <= u64::from(r.address) + r.bytes.len() as u64
        }) else {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "preload {:#x}+{:#x} is not inside one mapped region",
                    preload.address,
                    preload.bytes.len()
                ),
            ));
        };
        if r.flags & 2 == 0 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "preload {:#x}+{:#x} targets read-only {:?} memory",
                    preload.address,
                    preload.bytes.len(),
                    r.kind
                ),
            ));
        }
        if let Some(steps) = &mut self.steps {
            steps.push(
                crate::execution_steps::StepEntry::Input {
                    address: preload.address,
                    length: preload.bytes.len() as u32,
                    transient: r.kind.is_transient(),
                },
                c,
            )?;
        }
        let offset = (start - u64::from(r.address)) as usize;
        for (index, chunk) in preload.bytes.chunks(WORK_BLOCK).enumerate() {
            c.checkpoint(1)?;
            let at = offset + index * WORK_BLOCK;
            r.bytes[at..at + chunk.len()].copy_from_slice(chunk);
            r.known[at..at + chunk.len()].fill(1);
        }
        Ok(())
    }
    /// Record this session's steps from now on.
    pub fn record_steps(&mut self) {
        self.steps = Some(crate::execution_steps::StepLog::new(self.memory));
    }
    /// The step log, when recording.
    pub fn steps(&mut self) -> Option<&mut crate::execution_steps::StepLog<'a>> {
        self.steps.as_mut()
    }
    /// Stop recording and return the step log.
    pub fn take_steps(&mut self) -> Option<crate::execution_steps::StepLog<'a>> {
        self.steps.take()
    }
    #[inline]
    pub(super) fn log(
        &mut self,
        entry: crate::execution_steps::StepEntry,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        match &mut self.steps {
            Some(steps) => steps.push(entry, c),
            None => Ok(()),
        }
    }
    /// Double the event capacity, bounded by `max_events`. The new capacity is
    /// admitted before allocation and while the old buffer is still held.
    fn grow_events(&mut self, c: &mut dyn RunControl) -> Result<()> {
        let count = (self.events.capacity() * 2)
            .max(INITIAL_EVENT_CAPACITY)
            .min(self.max_events);
        let reservation = self.memory.reserve(
            (count * std::mem::size_of::<ExecutionEvent>()) as u64,
            c.position(),
        )?;
        self.events
            .try_reserve_exact(count - self.events.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "event allocation refused"))?;
        self.event_capacity = Some(reservation);
        Ok(())
    }
    fn region(
        &mut self,
        mapping: Mapping,
        fill: Option<u8>,
        bytes: &[u8],
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let Mapping {
            address,
            length,
            flags,
            kind,
        } = mapping;
        let end = u64::from(address) + length as u64;
        if self.regions.len() == 2048 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "session region capacity exhausted",
            ));
        }
        if end >= u64::from(u32::MAX - 1) || bytes.len() > length {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "invalid session region {address:#x}+{length:#x}: it leaves the address space \
                     or its {} initial bytes exceed it",
                    bytes.len()
                ),
            ));
        }
        if self.devices.overlaps(address, length as u64, c)? {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!("memory region {address:#x}+{length:#x} overlaps a live device model"),
            ));
        }
        for binding in self.calls.bindings() {
            c.checkpoint(1)?;
            if u64::from(binding.address) < end
                && u64::from(address) < u64::from(binding.address) + 2
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!(
                        "memory region {address:#x}+{length:#x} overlaps the live call boundary at {:#x}",
                        binding.address
                    ),
                ));
            }
        }
        for region in &self.regions {
            c.checkpoint(1)?;
            if u64::from(address) < u64::from(region.address) + region.bytes.len() as u64
                && u64::from(region.address) < end
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!(
                        "{kind:?} memory region {address:#x}+{length:#x} overlaps the session's \
                         {:?} region {:#x}+{:#x}; an image segment is shared by every case of \
                         both targets",
                        region.kind,
                        region.address,
                        region.bytes.len()
                    ),
                ));
            }
        }
        let mut data = self.memory.bytes(length, c.position())?;
        let mut known = self.memory.bytes(length, c.position())?;
        for offset in (0..length).step_by(WORK_BLOCK) {
            c.checkpoint(1)?;
            let end = (offset + WORK_BLOCK).min(length);
            data[offset..end].fill(fill.unwrap_or(0));
            known[offset..end].fill(u8::from(fill.is_some()));
        }
        for (chunk, source) in data.chunks_mut(WORK_BLOCK).zip(bytes.chunks(WORK_BLOCK)) {
            c.checkpoint(1)?;
            chunk[..source.len()].copy_from_slice(source);
        }
        for chunk in known[..bytes.len()].chunks_mut(WORK_BLOCK) {
            c.checkpoint(1)?;
            chunk.fill(1);
        }
        self.regions.push(Region {
            address,
            bytes: data,
            known,
            flags,
            kind,
        });
        self.log(
            crate::execution_steps::StepEntry::Input {
                address,
                length: length as u32,
                transient: kind.is_transient(),
            },
            c,
        )
    }
    /// Map one executable ELF segment: file bytes, then zeroed memory tail.
    pub fn segment(
        &mut self,
        address: u32,
        memory_size: usize,
        flags: u32,
        bytes: &[u8],
        c: &mut dyn RunControl,
    ) -> Result<()> {
        self.region(
            Mapping {
                address,
                length: memory_size,
                flags,
                kind: RegionKind::Image,
            },
            Some(0),
            bytes,
            c,
        )?;
        if flags & 1 != 0 {
            self.coverage.map(address, memory_size, self.memory, c)?;
        }
        Ok(())
    }
    /// The side's accumulated coverage, for the next session of that side.
    pub fn into_coverage(self) -> crate::execution_coverage::CodeCoverage<'a> {
        self.coverage
    }
    pub fn phase(
        &mut self,
        target: &ExecutionTarget,
        stack_fill: Option<u8>,
        input: &Invocation,
        c: &mut dyn RunControl,
    ) -> Result<u32> {
        let stack = input.entry_stack(&target.stack)?;
        self.reservation = None;
        self.finish_phase();
        self.log(crate::execution_steps::StepEntry::Phase, c)?;
        self.events.clear();
        self.timeline = input.observe_timeline;
        if self.timeline.written {
            let reservation = self.memory.reserve(
                (MAX_WRITTEN_RANGES * std::mem::size_of::<WrittenRange>()) as u64,
                c.position(),
            )?;
            self.written
                .try_reserve_exact(MAX_WRITTEN_RANGES)
                .map_err(|_| {
                    Error::new(
                        ErrorCode::ResourceLimited,
                        "written-range allocation refused",
                    )
                })?;
            self.written_capacity = Some(reservation);
        }
        if let Some(profile) = &input.observe_calls {
            profile.validate()?;
            let capacity = self.memory.reserve(profile.payload_bytes(), c.position())?;
            let mut overrides = Vec::new();
            overrides
                .try_reserve_exact(profile.overrides.len())
                .map_err(|_| {
                    Error::new(
                        ErrorCode::ResourceLimited,
                        "call capture allocation refused",
                    )
                })?;
            c.checkpoint(profile.overrides.len() as u64 + 1)?;
            overrides.extend_from_slice(&profile.overrides);
            overrides.sort_unstable_by_key(|o| o.target);
            self.capture = Some((
                CallCapture {
                    include_tail: profile.include_tail,
                    argument_words: profile.argument_words,
                    overrides,
                },
                capacity,
            ));
        }
        self.region(
            Mapping {
                address: target.stack.address,
                length: target.stack.length as usize,
                flags: 6,
                kind: RegionKind::Stack,
            },
            stack_fill.or(target.stack.fill),
            &target.stack.bytes,
            c,
        )?;
        // The newly owned stack is the only destination for argument setup.
        // Unknown argument words override seeds; setup is not a guest MMIO event.
        let region = self.regions.last_mut().unwrap();
        let base = (stack - target.stack.address) as usize;
        for (index, word) in input.arguments.iter().skip(8).enumerate() {
            c.checkpoint(1)?;
            let offset = base + index * 4;
            region.bytes[offset..offset + 4].copy_from_slice(&word.unwrap_or(0).to_le_bytes());
            region.known[offset..offset + 4].fill(u8::from(word.is_some()));
        }
        for declared in &input.memory {
            let seed = &declared.seed;
            c.checkpoint(self.regions.len() as u64)?;
            // Only a declared RAM region can be reseeded; never replace ELF data.
            if let Some(r) = self.regions.iter_mut().find(|r| {
                r.address == seed.address
                    && r.kind == RegionKind::Ram(declared.lifetime)
                    && r.bytes.len() == seed.length as usize
            }) {
                if let Some(steps) = &mut self.steps {
                    steps.push(
                        crate::execution_steps::StepEntry::Input {
                            address: r.address,
                            length: r.bytes.len() as u32,
                            transient: r.kind.is_transient(),
                        },
                        c,
                    )?;
                }
                for offset in (0..r.bytes.len()).step_by(WORK_BLOCK) {
                    c.checkpoint(1)?;
                    let end = (offset + WORK_BLOCK).min(r.bytes.len());
                    if let Some(fill) = seed.fill {
                        r.bytes[offset..end].fill(fill);
                        r.known[offset..end].fill(1);
                    }
                }
                for (dst, src) in r
                    .bytes
                    .chunks_mut(WORK_BLOCK)
                    .zip(seed.bytes.chunks(WORK_BLOCK))
                {
                    c.checkpoint(1)?;
                    dst[..src.len()].copy_from_slice(src);
                }
                for chunk in r.known[..seed.bytes.len()].chunks_mut(WORK_BLOCK) {
                    c.checkpoint(1)?;
                    chunk.fill(1);
                }
            } else {
                self.region(
                    Mapping {
                        address: seed.address,
                        length: seed.length as usize,
                        flags: 6,
                        kind: RegionKind::Ram(declared.lifetime),
                    },
                    seed.fill,
                    &seed.bytes,
                    c,
                )?;
            }
        }
        for preload in &input.preload {
            self.preload(preload, c)?;
        }
        let regions = &self.regions;
        self.devices
            .install(&input.models, c, &mut |address, length, c| {
                c.checkpoint(regions.len() as u64 + 1)?;
                let (start, end) = (u64::from(address), u64::from(address) + length);
                if regions.iter().any(|r| {
                    start < u64::from(r.address) + r.bytes.len() as u64
                        && u64::from(r.address) < end
                }) {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "device overlaps captured memory, stack or RAM",
                    ));
                }
                Ok(())
            })?;
        self.calls.install(&input.calls, c)?;
        // Validate live bindings after phase memory/devices are installed. No hidden symbol lookup.
        for binding in self.calls.bindings() {
            c.checkpoint(self.regions.len() as u64 + 1)?;
            if self.devices.overlaps(binding.address, 2, c)? {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "call boundary overlaps device",
                ));
            }
            let containing = self.region_index(binding.address, 2);
            let valid = match binding.boundary {
                CallBoundary::Unmapped => !self.regions.iter().any(|r| {
                    u64::from(binding.address) < u64::from(r.address) + r.bytes.len() as u64
                        && u64::from(r.address) < u64::from(binding.address) + 2
                }),
                CallBoundary::CapturedCode => containing.is_some_and(|(i, o)| {
                    self.regions[i].kind == RegionKind::Image
                        && self.regions[i].flags & 1 != 0
                        && !self.regions[i].known[o..o + 2].contains(&0)
                }),
            };
            if !valid {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "call boundary does not match captured/unmapped declaration",
                ));
            }
        }
        Ok(stack)
    }
    pub fn observation(
        &mut self,
        stop: ExecutionStop,
        steps: u64,
        close_chain: bool,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionObservation> {
        self.devices
            .finish(close_chain, &mut self.model_observations, c)?;
        self.calls
            .finish(close_chain, &mut self.call_observations, c)?;
        Ok(ExecutionObservation {
            stop,
            steps,
            events: std::mem::take(&mut self.events),
            models: std::mem::take(&mut self.model_observations),
            calls: std::mem::take(&mut self.call_observations),
            final_memory: std::mem::take(&mut self.final_memory),
            written: std::mem::take(&mut self.written),
        })
    }
    pub fn recycle(&mut self, mut observation: ExecutionObservation) {
        observation.events.clear();
        self.events = observation.events;
        observation.models.clear();
        self.model_observations = observation.models;
        observation.calls.clear();
        self.call_observations = observation.calls;
        drop(observation.final_memory);
        self.final_memory_capacity = None;
        drop(observation.written);
        self.finish_phase();
    }
    fn finish_phase(&mut self) {
        self.capture = None;
        self.written = Vec::new();
        self.written_capacity = None;
        self.timeline = TimelineCapture::default();
        self.pc = None;
        self.reservation = None;
        self.regions.retain(|r| !r.kind.is_transient());
    }
    fn region_index(&self, address: u32, width: u8) -> Option<(usize, usize)> {
        let contains = |r: &Region<'_>| {
            let offset = u64::from(address).checked_sub(u64::from(r.address))?;
            (offset + u64::from(width)
                <= match r.kind {
                    RegionKind::Allocation { requested, .. } => u64::from(requested),
                    _ => r.bytes.len() as u64,
                })
            .then_some(offset as usize)
        };
        // Regions never overlap, so the last hit is the only candidate when
        // it contains the access.
        let last = self.last_region.get();
        if let Some(offset) = self.regions.get(last).and_then(contains) {
            return Some((last, offset));
        }
        let (i, offset) = self
            .regions
            .iter()
            .enumerate()
            .find_map(|(i, r)| Some((i, contains(r)?)))?;
        self.last_region.set(i);
        Some((i, offset))
    }
    /// Merge a store into region `i` into the phase's written ranges when the
    /// timeline selects them and the region outlives the phase.
    fn note_written(
        &mut self,
        i: usize,
        address: u32,
        width: u8,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let persistent = match self.regions[i].kind {
            RegionKind::Image => true,
            RegionKind::Ram(lifetime) | RegionKind::Allocation { lifetime, .. } => {
                lifetime == RegionLifetime::Session
            }
            RegionKind::Stack => false,
        };
        if !self.timeline.written || !persistent {
            return Ok(());
        }
        c.checkpoint(self.written.len().max(1).ilog2() as u64 + 1)?;
        let (mut start, mut end) = (u64::from(address), u64::from(address) + u64::from(width));
        let first = self.written.partition_point(|r| r.end() < start);
        let mut last = first;
        while let Some(r) = self
            .written
            .get(last)
            .filter(|r| u64::from(r.address) <= end)
        {
            start = start.min(u64::from(r.address));
            end = end.max(r.end());
            last += 1;
        }
        if first == last && self.written.len() == MAX_WRITTEN_RANGES {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "written-range capacity exhausted",
            ));
        }
        c.checkpoint((self.written.len() - last) as u64 / 64 + 1)?;
        self.written.splice(
            first..last,
            [WrittenRange {
                address: start as u32,
                length: (end - start) as u32,
            }],
        );
        Ok(())
    }
    fn invalidate_reservation(&mut self, address: u32, width: u8) {
        if self.reservation.is_some_and(|reserved| {
            u64::from(address) < u64::from(reserved) + 4
                && u64::from(reserved) < u64::from(address) + u64::from(width)
        }) {
            self.reservation = None;
        }
    }
    fn atomic_region(
        &self,
        address: u32,
        flags: u32,
        known: bool,
        c: &mut dyn RunControl,
    ) -> Result<Option<(usize, usize)>> {
        c.checkpoint(self.regions.len() as u64 + 1)?;
        if !address.is_multiple_of(4) {
            return Ok(None);
        }
        let Some((i, offset)) = self.region_index(address, 4) else {
            return Ok(None);
        };
        let r = &self.regions[i];
        Ok(
            (r.flags & flags == flags && (!known || !r.known[offset..offset + 4].contains(&0)))
                .then_some((i, offset)),
        )
    }
}
impl ExecutionMemory for Session<'_> {
    fn instruction(&mut self, pc: u32) {
        self.pc = Some(pc);
        self.coverage.instruction(pc);
    }
    fn fetch(&mut self, pc: u32, c: &mut dyn RunControl) -> Result<(Option<u32>, Option<u32>)> {
        self.instruction(pc);
        self.log(crate::execution_steps::StepEntry::Instruction { pc }, c)?;
        // Both halfwords known in one executable region: one lookup. Anything
        // else takes the two ordinary fetch reads.
        if pc.is_multiple_of(2)
            && let Some((i, offset)) = self.region_index(pc, 4)
        {
            let r = &self.regions[i];
            if r.flags & 1 != 0 && !r.known[offset..offset + 4].contains(&0) {
                c.checkpoint(1)?;
                let word =
                    |at: usize| u32::from(u16::from_le_bytes([r.bytes[at], r.bytes[at + 1]]));
                return Ok((Some(word(offset)), Some(word(offset + 2))));
            }
        }
        let low = self.read(pc, 2, MemoryAccess::Fetch, c)?;
        let high = match (low, pc.checked_add(2)) {
            (Some(_), Some(next)) => self.read(next, 2, MemoryAccess::Fetch, c)?,
            _ => None,
        };
        Ok((low, high))
    }
    fn observe_call(&mut self, input: &CallInput, c: &mut dyn RunControl) -> Result<()> {
        self.capture_call(input, c)
    }
    fn call(&mut self, input: &CallInput, c: &mut dyn RunControl) -> Result<CallDispatch> {
        if input.indirect {
            self.coverage
                .transfer(input.site, input.target, self.memory, c)?;
        }
        let dispatch = self.external_call(input, c)?;
        if matches!(dispatch, CallDispatch::Returned { .. }) {
            self.log(crate::execution_steps::StepEntry::CallReturn, c)?;
        }
        Ok(dispatch)
    }
    fn read(
        &mut self,
        address: u32,
        width: u8,
        access: MemoryAccess,
        c: &mut dyn RunControl,
    ) -> Result<Option<u32>> {
        Ok(match self.read_value(address, width, access, c)? {
            MemoryReadValue::Known { value } => Some(value),
            _ => None,
        })
    }
    fn load(&mut self, address: u32, width: u8, c: &mut dyn RunControl) -> Result<MemoryReadValue> {
        self.read_value(address, width, MemoryAccess::Read, c)
    }

    fn write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
        c: &mut dyn RunControl,
    ) -> Result<bool> {
        c.checkpoint(self.regions.len() as u64 + 1)?;
        if !matches!(width, 1 | 2 | 4) {
            return Ok(false);
        }
        if !address.is_multiple_of(u32::from(width))
            && self.devices.overlaps(address, u64::from(width), c)?
        {
            return Ok(false);
        }
        let value = if width == 4 {
            value
        } else {
            value & ((1 << (width * 8)) - 1)
        };
        if let Some(written) = self.devices.write(address, width, value, c)? {
            if !written {
                return Ok(false);
            }
            self.log(
                crate::execution_steps::StepEntry::Write {
                    address,
                    width,
                    device: true,
                },
                c,
            )?;
            self.event(
                ExecutionEvent::Write {
                    address,
                    width,
                    value,
                },
                c,
            )?;
            return Ok(true);
        }
        let Some((i, offset)) = self.region_index(address, width) else {
            return Ok(false);
        };
        if self.regions[i].flags & 2 == 0 {
            return Ok(false);
        }
        self.admit_memory_event(self.timeline.writes, c)?;
        self.invalidate_reservation(address, width);
        let r = &mut self.regions[i];
        r.bytes[offset..offset + width as usize]
            .copy_from_slice(&value.to_le_bytes()[..width as usize]);
        r.known[offset..offset + width as usize].fill(1);
        self.note_written(i, address, width, c)?;
        self.log(
            crate::execution_steps::StepEntry::Write {
                address,
                width,
                device: false,
            },
            c,
        )?;
        self.trace_memory(
            MemoryTransaction::Write {
                address,
                width,
                value,
            },
            c,
        )?;
        Ok(true)
    }
    fn write_unknown(&mut self, address: u32, width: u8, c: &mut dyn RunControl) -> Result<bool> {
        c.checkpoint(self.regions.len() as u64 + 1)?;
        if !matches!(width, 1 | 2 | 4) || self.devices.overlaps(address, u64::from(width), c)? {
            return Ok(false);
        }
        let Some((i, offset)) = self.region_index(address, width) else {
            return Ok(false);
        };
        // A recorded write timeline would observe the value.
        if self.regions[i].flags & 2 == 0 || self.timeline.writes {
            return Ok(false);
        }
        self.invalidate_reservation(address, width);
        self.regions[i].known[offset..offset + width as usize].fill(0);
        self.note_written(i, address, width, c)?;
        self.log(
            crate::execution_steps::StepEntry::Write {
                address,
                width,
                device: false,
            },
            c,
        )?;
        Ok(true)
    }
    fn load_reserved(
        &mut self,
        address: u32,
        order: ExecutionOrdering,
        c: &mut dyn RunControl,
    ) -> Result<Option<u32>> {
        self.reservation = None;
        let Some((i, offset)) = self.atomic_region(address, 4, true, c)? else {
            return Ok(None);
        };
        let value = u32::from_le_bytes(
            self.regions[i].bytes[offset..offset + 4]
                .try_into()
                .unwrap(),
        );
        self.admit_memory_event(self.timeline.atomics, c)?;
        self.reservation = Some(address);
        self.log(
            crate::execution_steps::StepEntry::Read {
                address,
                width: 4,
                device: false,
            },
            c,
        )?;
        self.trace_memory(
            MemoryTransaction::LoadReserved {
                address,
                order,
                value,
            },
            c,
        )?;
        Ok(Some(value))
    }
    fn store_conditional(
        &mut self,
        address: u32,
        value: u32,
        order: ExecutionOrdering,
        c: &mut dyn RunControl,
    ) -> Result<Option<bool>> {
        let reserved = self.reservation.take();
        let Some((i, offset)) = self.atomic_region(address, 2, false, c)? else {
            return Ok(None);
        };
        self.admit_memory_event(self.timeline.atomics, c)?;
        if reserved != Some(address) {
            self.trace_memory(
                MemoryTransaction::StoreConditional {
                    address,
                    order,
                    value,
                    stored: false,
                },
                c,
            )?;
            return Ok(Some(false));
        }
        let r = &mut self.regions[i];
        r.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        r.known[offset..offset + 4].fill(1);
        self.note_written(i, address, 4, c)?;
        self.log(
            crate::execution_steps::StepEntry::Write {
                address,
                width: 4,
                device: false,
            },
            c,
        )?;
        self.trace_memory(
            MemoryTransaction::StoreConditional {
                address,
                order,
                value,
                stored: true,
            },
            c,
        )?;
        Ok(Some(true))
    }
    fn modify_word(
        &mut self,
        address: u32,
        order: ExecutionOrdering,
        update: &mut dyn FnMut(u32) -> u32,
        c: &mut dyn RunControl,
    ) -> Result<Option<u32>> {
        let Some((i, offset)) = self.atomic_region(address, 6, true, c)? else {
            return Ok(None);
        };
        let old = u32::from_le_bytes(
            self.regions[i].bytes[offset..offset + 4]
                .try_into()
                .unwrap(),
        );
        self.admit_memory_event(self.timeline.atomics, c)?;
        let value = update(old);
        self.invalidate_reservation(address, 4);
        self.regions[i].bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        self.note_written(i, address, 4, c)?;
        for entry in [
            crate::execution_steps::StepEntry::Read {
                address,
                width: 4,
                device: false,
            },
            crate::execution_steps::StepEntry::Write {
                address,
                width: 4,
                device: false,
            },
        ] {
            self.log(entry, c)?;
        }
        self.trace_memory(
            MemoryTransaction::ReadModifyWrite {
                address,
                order,
                old,
                value,
            },
            c,
        )?;
        Ok(Some(old))
    }
    fn event(&mut self, event: ExecutionEvent, c: &mut dyn RunControl) -> Result<()> {
        if let ExecutionEvent::Branch { site, taken, .. } = event {
            self.coverage.branch(site, taken);
            if !self.timeline.branches {
                return Ok(());
            }
        }
        c.checkpoint(1)?;
        if self.events.len() == self.max_events {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "execution event capacity exhausted",
            ));
        }
        if self.events.len() == self.events.capacity() {
            self.grow_events(c)?;
        }
        self.events.push(event);
        self.log(
            crate::execution_steps::StepEntry::Event {
                index: self.events.len() as u32 - 1,
            },
            c,
        )
    }
}

#[cfg(test)]
#[path = "execution_memory_tests.rs"]
mod tests;

#[path = "execution_calls.rs"]
mod execution_calls;

#[path = "execution_observation.rs"]
mod execution_observation;

#[path = "execution_capture.rs"]
mod execution_capture;

#[path = "execution_timeline.rs"]
mod execution_timeline;
