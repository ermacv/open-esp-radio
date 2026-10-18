//! Session-owned device instances and an admitted exact-port index.
use crate::*;
struct Instance<'m> {
    declaration: DeviceDeclaration,
    commands: Option<crate::command_bank::CommandState<'m>>,
    definition: ArtifactId,
    _payload: MemoryReservation<'m>,
    cells: AdmittedVec<'m, RegisterCell>,
    values: AdmittedVec<'m, u32>,
    /// Written words of a retained aperture, sorted by address.
    words: AdmittedVec<'m, (u32, u32)>,
    value: u32,
    index: Option<u32>,
    reads: u64,
    writes: u64,
    read_cursor: usize,
    run_used: u32,
    sequence_remaining: u32,
    write_cursor: usize,
    issue: Option<DeviceIssue>,
}
/// Port slot of an access that falls back to a retained aperture.
const APERTURE_SLOT: usize = usize::MAX;
#[derive(Clone, Copy)]
struct Port {
    address: u32,
    width: u8,
    model: usize,
    slot: usize,
}
pub(crate) struct Devices<'m> {
    memory: &'m WorkingMemory,
    instances: AdmittedVec<'m, Option<Instance<'m>>>,
    ports: AdmittedVec<'m, Port>,
    /// Retained apertures as (start, end, model), sorted and disjoint.
    apertures: AdmittedVec<'m, (u32, u64, usize)>,
    /// `[start, end)` of the last searched range holding no port and no
    /// aperture; ordinary memory accesses repeat it.
    gap: std::cell::Cell<(u64, u64)>,
}
impl<'m> Devices<'m> {
    pub fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            memory,
            instances: AdmittedVec::new(memory),
            ports: AdmittedVec::new(memory),
            apertures: AdmittedVec::new(memory),
            gap: std::cell::Cell::new((0, 0)),
        }
    }
    /// Whether `[address, end)` lies in the remembered device-free range.
    fn in_gap(&self, address: u32, end: u64) -> bool {
        let (low, high) = self.gap.get();
        low <= u64::from(address) && end <= high
    }
    /// Remember the device-free range around `[address, end)`, which starts
    /// before port `index` and misses every port and aperture.
    fn remember_gap(&self, index: usize, address: u32, end: u64) {
        let mut low = index.checked_sub(1).map_or(0, |i| {
            u64::from(self.ports[i].address) + u64::from(self.ports[i].width)
        });
        let mut high = self
            .ports
            .get(index)
            .map_or(1 << u32::BITS, |p| u64::from(p.address));
        for (start, stop, _) in &*self.apertures {
            if *stop <= u64::from(address) {
                low = low.max(*stop);
            } else if end <= u64::from(*start) {
                high = high.min(u64::from(*start));
            } else {
                return;
            }
        }
        self.gap.set((low, high));
    }
    pub fn overlaps(&self, address: u32, length: u64, c: &mut dyn RunControl) -> Result<bool> {
        c.checkpoint(self.ports.len().max(1).ilog2() as u64 + 1)?;
        let end = u64::from(address) + length;
        // The same work is charged whether or not the search runs.
        if self.in_gap(address, end) {
            c.checkpoint(self.apertures.len() as u64 + 1)?;
            return Ok(false);
        }
        let i = self
            .ports
            .partition_point(|p| u64::from(p.address) + u64::from(p.width) <= u64::from(address));
        c.checkpoint(self.apertures.len() as u64 + 1)?;
        let overlaps = self
            .ports
            .get(i)
            .is_some_and(|p| u64::from(p.address) < end)
            || self
                .apertures
                .iter()
                .any(|(start, stop, _)| u64::from(*start) < end && u64::from(address) < *stop);
        if !overlaps {
            self.remember_gap(i, address, end);
        }
        Ok(overlaps)
    }
    pub fn install(
        &mut self,
        declarations: &[DeviceDeclaration],
        c: &mut dyn RunControl,
        check: &mut dyn FnMut(u32, u64, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        for declaration in declarations {
            c.checkpoint(self.instances.len() as u64 + 1)?;
            if self
                .instances
                .iter()
                .flatten()
                .any(|i| i.declaration.id == declaration.id)
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "device id is already live; omit it on warm continuation",
                ));
            }
            let instance = Instance::new(declaration, self.memory, c)?;
            if let Some(slot) = self.instances.iter().position(Option::is_none) {
                self.instances[slot] = Some(instance);
            } else {
                if self.instances.len() == MAX_DEVICE_MODELS {
                    return Err(Error::new(
                        ErrorCode::ResourceLimited,
                        "active device capacity exhausted",
                    ));
                }
                self.instances.push(Some(instance), c.position())?;
            }
        }
        self.rebuild(c)?;
        for port in &*self.ports {
            check(port.address, u64::from(port.width), c)?;
        }
        for (start, end, _) in &*self.apertures {
            check(*start, end - u64::from(*start), c)?;
        }
        Ok(())
    }
    fn rebuild(&mut self, c: &mut dyn RunControl) -> Result<()> {
        self.gap.set((0, 0));
        while self.ports.pop().is_some() {}
        while self.apertures.pop().is_some() {}
        for (model, instance) in self.instances.iter().enumerate() {
            let Some(instance) = instance else {
                continue;
            };
            let mut push = |address, width, slot| -> Result<()> {
                c.checkpoint(1)?;
                if self.ports.len() == MAX_DEVICE_PORTS {
                    return Err(Error::new(
                        ErrorCode::ResourceLimited,
                        "device port capacity exhausted",
                    ));
                }
                self.ports.push(
                    Port {
                        address,
                        width,
                        model,
                        slot,
                    },
                    c.position(),
                )
            };
            match &instance.declaration.behavior {
                DeviceBehavior::CommandBank(bank) => {
                    for (slot, port) in bank.ports.iter().enumerate() {
                        push(port.address, 4, slot)?;
                    }
                }
                DeviceBehavior::RegisterBank { cells } => {
                    for (slot, cell) in cells.iter().enumerate() {
                        push(cell.address, cell.width, slot)?;
                    }
                }
                DeviceBehavior::IndexedBank {
                    index_address,
                    data_address,
                    width,
                    ..
                } => {
                    push(*index_address, *width, 0)?;
                    push(*data_address, *width, 1)?;
                }
                DeviceBehavior::SelectedSequence {
                    selector_address,
                    value_address,
                    width,
                    ..
                } => {
                    push(*selector_address, *width, 0)?;
                    push(*value_address, *width, 1)?;
                }
                DeviceBehavior::ConstantRead { address, width, .. }
                | DeviceBehavior::SequenceRead { address, width, .. }
                | DeviceBehavior::W1c { address, width, .. }
                | DeviceBehavior::ReadClear { address, width, .. }
                | DeviceBehavior::SelfClearing { address, width, .. }
                | DeviceBehavior::Fifo { address, width, .. }
                | DeviceBehavior::CyclicRead { address, width, .. } => push(*address, *width, 0)?,
                DeviceBehavior::RetainedAperture { start, length, .. } => {
                    c.checkpoint(1)?;
                    self.apertures.push(
                        (*start, u64::from(*start) + u64::from(*length), model),
                        c.position(),
                    )?;
                }
            }
        }
        self.apertures.sort_unstable_by_key(|a| a.0);
        if self
            .apertures
            .windows(2)
            .any(|a| a[0].1 > u64::from(a[1].0))
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "retained apertures overlap",
            ));
        }
        c.checkpoint(self.ports.len() as u64 * (self.ports.len().max(1).ilog2() as u64 + 1))?;
        self.ports.sort_unstable_by_key(|p| p.address);
        if self
            .ports
            .windows(2)
            .any(|p| u64::from(p[0].address) + u64::from(p[0].width) > u64::from(p[1].address))
        {
            return Err(Error::new(ErrorCode::Conflict, "device ports overlap"));
        }
        Ok(())
    }
    fn port(&self, address: u32, width: u8, c: &mut dyn RunControl) -> Result<Option<Port>> {
        c.checkpoint(self.ports.len().max(1).ilog2() as u64 + 1)?;
        let end = u64::from(address) + u64::from(width);
        if self.in_gap(address, end) {
            c.checkpoint(self.apertures.len() as u64 + 1)?;
            return Ok(None);
        }
        let index = self
            .ports
            .partition_point(|p| u64::from(p.address) + u64::from(p.width) <= u64::from(address));
        let exact = self
            .ports
            .get(index)
            .copied()
            .filter(|p| u64::from(p.address) < end);
        if exact.is_some() {
            return Ok(exact);
        }
        // Exact ports take precedence; an aligned remainder falls to an aperture.
        c.checkpoint(self.apertures.len() as u64 + 1)?;
        let aperture = self
            .apertures
            .iter()
            .find(|(start, stop, _)| *start <= address && end <= *stop)
            .map(|(_, _, model)| Port {
                address,
                width,
                model: *model,
                slot: APERTURE_SLOT,
            });
        if aperture.is_none() {
            self.remember_gap(index, address, end);
        }
        Ok(aperture)
    }
    /// Outer None means no device claims these bytes; inner None is a model gap.
    pub fn read(
        &mut self,
        address: u32,
        width: u8,
        c: &mut dyn RunControl,
    ) -> Result<Option<Option<u32>>> {
        let Some(port) = self.port(address, width, c)? else {
            return Ok(None);
        };
        let instance = self.instances[port.model].as_mut().unwrap();
        if address != port.address || width != port.width {
            instance.issue = Some(DeviceIssue::AccessWidth);
            return Ok(Some(None));
        }
        if port.slot == APERTURE_SLOT {
            return Ok(Some(instance.aperture_read(address, width, c)?));
        }
        Ok(Some(instance.read(port.slot, c)?))
    }
    pub fn write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
        c: &mut dyn RunControl,
    ) -> Result<Option<bool>> {
        let Some(port) = self.port(address, width, c)? else {
            return Ok(None);
        };
        let instance = self.instances[port.model].as_mut().unwrap();
        if address != port.address || width != port.width {
            instance.issue = Some(DeviceIssue::AccessWidth);
            return Ok(Some(false));
        }
        if port.slot == APERTURE_SLOT {
            return instance.aperture_write(address, width, value, c).map(Some);
        }
        instance.write(port.slot, value, c).map(Some)
    }
    pub fn finish(
        &mut self,
        close_chain: bool,
        output: &mut Vec<ModelObservation>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        output.clear();
        for slot in &mut *self.instances {
            let Some(instance) = slot else {
                continue;
            };
            c.checkpoint(1)?;
            let closed = close_chain || instance.declaration.lifetime == RegionLifetime::Phase;
            output.push(instance.observation(closed)); // Session admits MAX_DEVICE_MODELS bounded rows.
            if closed {
                *slot = None;
            }
        }
        self.rebuild(c)
    }
}
impl<'m> Instance<'m> {
    fn new(
        declaration: &DeviceDeclaration,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        declaration.validate()?;
        let payload = memory.reserve(declaration.payload_bytes() + 64, c.position())?;
        let definition = declaration.identity(c)?;
        let mut instance = Self {
            declaration: declaration.clone(),
            commands: None,
            definition,
            _payload: payload,
            cells: AdmittedVec::new(memory),
            values: AdmittedVec::new(memory),
            words: AdmittedVec::new(memory),
            value: 0,
            index: None,
            reads: 0,
            writes: 0,
            read_cursor: 0,
            run_used: 0,
            sequence_remaining: 0,
            write_cursor: 0,
            issue: None,
        };
        match &declaration.behavior {
            DeviceBehavior::SequenceRead { runs, .. } => {
                c.checkpoint(runs.len() as u64)?;
                instance.sequence_remaining = ReadRun::total(runs)?;
            }
            DeviceBehavior::CommandBank(bank) => {
                instance.commands = Some(crate::command_bank::CommandState::new(bank, memory, c)?);
            }
            DeviceBehavior::RegisterBank { cells } => {
                for cell in cells {
                    instance.cells.push(cell.clone(), c.position())?;
                }
            }
            DeviceBehavior::IndexedBank { values, index, .. } => {
                instance.index = *index;
                for value in values {
                    instance.values.push(*value, c.position())?;
                }
            }
            DeviceBehavior::SelectedSequence { sequences, .. } => {
                // Per sequence: the run it reads and the reads that run gave.
                for sequence in sequences {
                    instance.sequence_remaining += ReadRun::total(&sequence.runs)?;
                    instance.values.push(0, c.position())?;
                    instance.values.push(0, c.position())?;
                }
            }
            DeviceBehavior::W1c { initial, .. }
            | DeviceBehavior::ReadClear { initial, .. }
            | DeviceBehavior::SelfClearing { initial, .. } => instance.value = *initial,
            _ => {}
        }
        Ok(instance)
    }
    fn read(&mut self, slot: usize, c: &mut dyn RunControl) -> Result<Option<u32>> {
        c.checkpoint(1)?;
        let next = self.reads.checked_add(1).ok_or_else(|| {
            Error::new(ErrorCode::ResourceLimited, "model read counter exhausted")
        })?;
        let value = match &self.declaration.behavior {
            DeviceBehavior::CommandBank(bank) => {
                Ok(self.commands.as_mut().unwrap().read(bank, slot)?)
            }
            DeviceBehavior::RegisterBank { .. } => Ok(self.cells[slot].value),
            DeviceBehavior::ConstantRead { value, .. } => Ok(*value),
            DeviceBehavior::SequenceRead { runs, .. } => match runs.get(self.read_cursor) {
                Some(run) => {
                    self.run_used += 1;
                    self.sequence_remaining -= 1;
                    if self.run_used == run.count {
                        self.read_cursor += 1;
                        self.run_used = 0;
                    }
                    Ok(run.value)
                }
                None => Err(DeviceIssue::ExhaustedReads),
            },
            DeviceBehavior::Fifo { reads: values, .. } => match values.get(self.read_cursor) {
                Some(value) => {
                    self.read_cursor += 1;
                    Ok(*value)
                }
                None => Err(DeviceIssue::ExhaustedReads),
            },
            DeviceBehavior::W1c {
                read_clear_mask: mask,
                ..
            }
            | DeviceBehavior::ReadClear {
                clear_mask: mask, ..
            } => {
                let old = self.value;
                self.value &= !mask;
                Ok(old)
            }
            DeviceBehavior::SelfClearing { .. } => Ok(self.value),
            DeviceBehavior::CyclicRead { values, .. } => {
                let value = values[self.read_cursor];
                self.read_cursor = (self.read_cursor + 1) % values.len();
                Ok(value)
            }
            DeviceBehavior::RetainedAperture { .. } => {
                unreachable!("apertures dispatch by address")
            }
            DeviceBehavior::IndexedBank { .. } => match self.index {
                Some(index) => Ok(if slot == 0 {
                    index
                } else {
                    self.values[index as usize]
                }),
                None => Err(DeviceIssue::UnknownIndex),
            },
            DeviceBehavior::SelectedSequence { sequences, .. } => {
                if slot == 0 {
                    Err(DeviceIssue::WriteOnly)
                } else if let Some(selected) = self.index {
                    let selected = selected as usize;
                    let (run, used) = (self.values[2 * selected], self.values[2 * selected + 1]);
                    match sequences[selected].runs.get(run as usize) {
                        Some(read) => {
                            self.sequence_remaining -= 1;
                            if used + 1 == read.count {
                                self.values[2 * selected] = run + 1;
                                self.values[2 * selected + 1] = 0;
                            } else {
                                self.values[2 * selected + 1] = used + 1;
                            }
                            Ok(read.value)
                        }
                        None => Err(DeviceIssue::ExhaustedReads),
                    }
                } else {
                    Err(DeviceIssue::UnknownIndex)
                }
            }
        };
        match value {
            Ok(value) => {
                self.reads = next;
                Ok(Some(value))
            }
            Err(issue) => {
                self.issue = Some(issue);
                Ok(None)
            }
        }
    }
    fn write(&mut self, slot: usize, value: u32, c: &mut dyn RunControl) -> Result<bool> {
        c.checkpoint(1)?;
        let next = self.writes.checked_add(1).ok_or_else(|| {
            Error::new(ErrorCode::ResourceLimited, "model write counter exhausted")
        })?;
        let result = match &self.declaration.behavior {
            DeviceBehavior::CommandBank(bank) => self
                .commands
                .as_mut()
                .unwrap()
                .write(bank, slot, value, c)?,
            DeviceBehavior::RegisterBank { .. } => {
                self.cells[slot].value = value;
                Ok(())
            }
            DeviceBehavior::ConstantRead { .. }
            | DeviceBehavior::SequenceRead { .. }
            | DeviceBehavior::CyclicRead { .. }
            | DeviceBehavior::ReadClear { .. } => Err(DeviceIssue::ReadOnly),
            DeviceBehavior::W1c { clear_mask, .. } => {
                self.value &= !(value & clear_mask);
                Ok(())
            }
            DeviceBehavior::SelfClearing { store_mask, .. } => {
                self.value = (self.value & !store_mask) | (value & store_mask);
                Ok(())
            }
            DeviceBehavior::Fifo { writes, .. } => match writes.get(self.write_cursor) {
                None => Err(DeviceIssue::UnexpectedWrite),
                Some(expected) if *expected != value => Err(DeviceIssue::WriteMismatch {
                    expected: *expected,
                    actual: value,
                }),
                Some(_) => {
                    self.write_cursor += 1;
                    Ok(())
                }
            },
            DeviceBehavior::RetainedAperture { .. } => {
                unreachable!("apertures dispatch by address")
            }
            DeviceBehavior::SelectedSequence { sequences, .. } => {
                if slot == 1 {
                    Err(DeviceIssue::ReadOnly)
                } else {
                    match sequences.iter().position(|s| s.selector == value) {
                        Some(selected) => {
                            self.index = Some(selected as u32);
                            Ok(())
                        }
                        None => Err(DeviceIssue::UnknownSelector { selector: value }),
                    }
                }
            }
            DeviceBehavior::IndexedBank { .. } => {
                if slot == 0 {
                    if value as usize >= self.values.len() {
                        Err(DeviceIssue::IndexOutOfRange { index: value })
                    } else {
                        self.index = Some(value);
                        Ok(())
                    }
                } else if let Some(index) = self.index {
                    self.values[index as usize] = value;
                    Ok(())
                } else {
                    Err(DeviceIssue::UnknownIndex)
                }
            }
        };
        match result {
            Ok(()) => {
                self.writes = next;
                Ok(true)
            }
            Err(issue) => {
                self.issue = Some(issue);
                Ok(false)
            }
        }
    }
    /// Aligned read inside a retained aperture; misalignment is a model gap.
    fn aperture_read(
        &mut self,
        address: u32,
        width: u8,
        c: &mut dyn RunControl,
    ) -> Result<Option<u32>> {
        c.checkpoint(self.words.len().max(1).ilog2() as u64 + 1)?;
        let DeviceBehavior::RetainedAperture { initial, .. } = self.declaration.behavior else {
            unreachable!()
        };
        if !address.is_multiple_of(u32::from(width)) {
            self.issue = Some(DeviceIssue::AccessWidth);
            return Ok(None);
        }
        let word = address & !3;
        let value = match self.words.binary_search_by_key(&word, |w| w.0) {
            Ok(i) => self.words[i].1,
            Err(_) => initial,
        };
        self.reads = self.reads.checked_add(1).ok_or_else(|| {
            Error::new(ErrorCode::ResourceLimited, "model read counter exhausted")
        })?;
        let shift = 8 * (address & 3);
        Ok(Some(if width == 4 {
            value
        } else {
            (value >> shift) & ((1 << (u32::from(width) * 8)) - 1)
        }))
    }
    fn aperture_write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
        c: &mut dyn RunControl,
    ) -> Result<bool> {
        c.checkpoint(self.words.len().max(1).ilog2() as u64 + 1)?;
        let DeviceBehavior::RetainedAperture { initial, .. } = self.declaration.behavior else {
            unreachable!()
        };
        if !address.is_multiple_of(u32::from(width)) {
            self.issue = Some(DeviceIssue::AccessWidth);
            return Ok(false);
        }
        let word = address & !3;
        let (index, old) = match self.words.binary_search_by_key(&word, |w| w.0) {
            Ok(i) => (Ok(i), self.words[i].1),
            Err(i) => (Err(i), initial),
        };
        let merged = if width == 4 {
            value
        } else {
            let shift = 8 * (address & 3);
            let mask = ((1u32 << (u32::from(width) * 8)) - 1) << shift;
            (old & !mask) | ((value << shift) & mask)
        };
        match index {
            Ok(i) => self.words[i].1 = merged,
            Err(i) => {
                if self.words.len() == MAX_APERTURE_WORDS {
                    return Err(Error::new(
                        ErrorCode::ResourceLimited,
                        "retained aperture word capacity exhausted",
                    ));
                }
                self.words.push((word, merged), c.position())?;
                c.checkpoint((self.words.len() - i) as u64)?;
                self.words[i..].rotate_right(1);
            }
        }
        self.writes = self.writes.checked_add(1).ok_or_else(|| {
            Error::new(ErrorCode::ResourceLimited, "model write counter exhausted")
        })?;
        Ok(true)
    }
    fn observation(&self, closed: bool) -> ModelObservation {
        let (reads, writes) = match &self.declaration.behavior {
            DeviceBehavior::Fifo { reads, writes, .. } => (reads.len(), writes.len()),
            _ => (0, 0),
        };
        let mut result = ModelObservation {
            commands: self.commands.as_ref().map(|c| c.observation()),
            id: self.declaration.id.clone(),
            definition: self.definition.clone(),
            lifetime: self.declaration.lifetime,
            reads: self.reads,
            writes: self.writes,
            remaining_reads: match &self.declaration.behavior {
                DeviceBehavior::SequenceRead { .. } | DeviceBehavior::SelectedSequence { .. } => {
                    self.sequence_remaining
                }
                DeviceBehavior::CommandBank(_) => self.commands.as_ref().unwrap().remaining(),
                DeviceBehavior::CyclicRead { .. } => 0,
                _ => (reads - self.read_cursor) as u32,
            },
            remaining_writes: (writes - self.write_cursor) as u32,
            closed,
            issue: self.issue,
            status: ModelStatus::Open,
        };
        result.status = result.expected_status();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_selected_sequence_answers_each_selector_from_its_own_runs() {
        let mut d = declaration(RegionLifetime::Phase);
        d.behavior = DeviceBehavior::SelectedSequence {
            selector_address: 0x1000,
            value_address: 0x1004,
            width: 4,
            sequences: vec![
                SelectedRuns {
                    selector: 0x38,
                    runs: vec![ReadRun::once(1), ReadRun { value: 2, count: 2 }],
                },
                SelectedRuns {
                    selector: 7,
                    runs: vec![ReadRun::once(9)],
                },
            ],
        };
        d.validate().unwrap();
        let memory = WorkingMemory::new(1 << 16).unwrap();
        let mut instance = Instance::new(&d, &memory, &mut || Ok(())).unwrap();
        let control = &mut || Ok(());
        // Reading before any selection is an issue.
        let mut fresh = Instance::new(&d, &memory, control).unwrap();
        assert_eq!(fresh.read(1, control).unwrap(), None);
        assert_eq!(fresh.observation(true).status, ModelStatus::Incomplete);
        assert!(instance.write(0, 0x38, control).unwrap());
        assert_eq!(instance.read(1, control).unwrap(), Some(1));
        // Another selector keeps the first sequence's position.
        assert!(instance.write(0, 7, control).unwrap());
        assert_eq!(instance.read(1, control).unwrap(), Some(9));
        assert!(instance.write(0, 0x38, control).unwrap());
        assert_eq!(instance.read(1, control).unwrap(), Some(2));
        assert_eq!(instance.observation(true).status, ModelStatus::Incomplete);
        assert_eq!(instance.read(1, control).unwrap(), Some(2));
        assert_eq!(instance.observation(true).status, ModelStatus::Complete);
        // Past its runs, an undeclared selector or a selector read.
        assert_eq!(instance.read(1, control).unwrap(), None);
        let mut other = Instance::new(&d, &memory, control).unwrap();
        other.write(0, 0xc01, control).unwrap();
        assert_eq!(
            other.observation(false).issue,
            Some(DeviceIssue::UnknownSelector { selector: 0xc01 })
        );
        let mut reader = Instance::new(&d, &memory, control).unwrap();
        assert_eq!(reader.read(0, control).unwrap(), None);
        assert_eq!(
            reader.observation(false).issue,
            Some(DeviceIssue::WriteOnly)
        );
    }

    #[test]
    fn repeated_reads_keep_constant_capacity_and_cancel_before_cursor_progress() {
        let mut d = declaration(RegionLifetime::Session);
        d.behavior = DeviceBehavior::SequenceRead {
            address: 0x1000,
            width: 4,
            runs: vec![ReadRun {
                value: 7,
                count: u32::MAX,
            }],
        };
        let memory = WorkingMemory::new(1024).unwrap();
        let mut instance = Instance::new(&d, &memory, &mut || Ok(())).unwrap();
        assert!(memory.observation().reserved_bytes < 512);
        assert_eq!(instance.read(0, &mut || Ok(())).unwrap(), Some(7));
        assert_eq!(instance.observation(false).remaining_reads, u32::MAX - 1);
        assert_eq!(
            instance
                .read(0, &mut || Err(Error::new(ErrorCode::Cancelled, "stop")))
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        assert_eq!(instance.observation(false).remaining_reads, u32::MAX - 1);
        drop(instance);
        assert_eq!(memory.observation().reserved_bytes, 0);
    }
    fn declaration(lifetime: RegionLifetime) -> DeviceDeclaration {
        DeviceDeclaration {
            id: "large".into(),
            applicability: "synthetic lifecycle".into(),
            lifetime,
            behavior: DeviceBehavior::IndexedBank {
                index_address: 0x1000,
                data_address: 0x1004,
                width: 4,
                index: Some(0),
                values: vec![7; MAX_DEVICE_VALUES],
            },
        }
    }
    #[test]
    fn accesses_outside_every_port_charge_the_same_work_as_a_search() {
        struct Units(u64);
        impl RunControl for Units {
            fn checkpoint(&mut self, units: u64) -> Result<()> {
                self.0 += units;
                Ok(())
            }
        }
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut devices = Devices::new(&memory);
        let mut c = || Ok(());
        devices
            .install(
                &[declaration(RegionLifetime::Session)],
                &mut c,
                &mut |_, _, _| Ok(()),
            )
            .unwrap();
        let probe = |devices: &Devices, address: u32| {
            let mut units = Units(0);
            let hit = devices.overlaps(address, 4, &mut units).unwrap();
            (hit, units.0)
        };
        let (outside, outside_units) = probe(&devices, 0x100);
        let (straddling, straddling_units) = probe(&devices, 0x1002);
        let (port, port_units) = probe(&devices, 0x1004);
        assert_eq!((outside, straddling, port), (false, true, true));
        assert_eq!(outside_units, port_units);
        assert_eq!(straddling_units, port_units);
        let (after, after_units) = probe(&devices, 0x1008);
        assert!(!after);
        assert_eq!(after_units, port_units);
        // The remembered range below the ports answers without a search.
        assert_eq!(probe(&devices, 0x100), (false, outside_units));
        assert_eq!(probe(&devices, 0xffc), (false, outside_units));
        assert_eq!(probe(&devices, 0xffe), (true, port_units));
        let mut units = Units(0);
        assert!(devices.port(0x200, 4, &mut units).unwrap().is_none());
        let searched = units.0;
        let mut units = Units(0);
        assert!(devices.port(0x204, 4, &mut units).unwrap().is_none());
        assert_eq!(units.0, searched);
        let mut units = Units(0);
        assert_eq!(
            devices.port(0x1004, 4, &mut units).unwrap().unwrap().slot,
            1
        );
    }
    #[test]
    fn closure_releases_owned_payload_and_state_while_warm_keeps_them() {
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut devices = Devices::new(&memory);
        let mut output = Vec::with_capacity(MAX_DEVICE_MODELS);
        let mut c = || Ok(());
        devices
            .install(
                &[declaration(RegionLifetime::Phase)],
                &mut c,
                &mut |_, _, _| Ok(()),
            )
            .unwrap();
        let live = memory.observation().reserved_bytes;
        devices.finish(false, &mut output, &mut c).unwrap();
        let reusable = memory.observation().reserved_bytes;
        assert!(live > reusable + 2 * (MAX_DEVICE_VALUES * 4) as u64);
        assert!(!devices.overlaps(0x1000, 8, &mut c).unwrap());
        devices
            .install(
                &[declaration(RegionLifetime::Session)],
                &mut c,
                &mut |_, _, _| Ok(()),
            )
            .unwrap();
        let live = memory.observation().reserved_bytes;
        devices.write(0x1004, 4, 99, &mut c).unwrap();
        devices.finish(false, &mut output, &mut c).unwrap();
        assert_eq!(memory.observation().reserved_bytes, live);
        assert_eq!(output[0].status, ModelStatus::Open);
        assert_eq!(devices.read(0x1004, 4, &mut c).unwrap(), Some(Some(99)));
        devices.finish(true, &mut output, &mut c).unwrap();
        assert_eq!(memory.observation().reserved_bytes, reusable);
        assert_eq!(output[0].status, ModelStatus::Complete);
        drop(devices);
        assert_eq!(memory.observation().reserved_bytes, 0);
    }
    #[test]
    fn failed_admission_releases_partial_state_and_cancelled_access_does_not_consume() {
        // Declaration clone fits, but its separate mutable bank cannot grow to full size.
        let memory = WorkingMemory::new(24 * 1024).unwrap();
        let mut devices = Devices::new(&memory);
        let error = devices
            .install(
                &[declaration(RegionLifetime::Session)],
                &mut || Ok(()),
                &mut |_, _, _| Ok(()),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        assert_eq!(memory.observation().reserved_bytes, 0);
        let mut d = declaration(RegionLifetime::Phase);
        d.behavior = DeviceBehavior::SequenceRead {
            address: 0x1000,
            width: 4,
            runs: vec![42].into_iter().map(ReadRun::once).collect(),
        };
        devices
            .install(&[d], &mut || Ok(()), &mut |_, _, _| Ok(()))
            .unwrap();
        let error = devices
            .read(0x1000, 4, &mut || {
                Err(Error::new(ErrorCode::Cancelled, "cancel fixture"))
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(
            devices.read(0x1000, 4, &mut || Ok(())).unwrap(),
            Some(Some(42))
        );
        drop(devices);
        assert_eq!(memory.observation().reserved_bytes, 0);
    }
}
