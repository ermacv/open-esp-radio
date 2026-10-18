//! Bounded validation of model declarations and lifetime evidence, not device execution.
use super::*;
struct Live<'a> {
    declaration: &'a DeviceDeclaration,
    definition: ArtifactId,
    reads: u64,
    writes: u64,
    commands: Option<CommandObservation>,
    issue: Option<DeviceIssue>,
    seen: bool,
    closed: bool,
}
pub(super) struct Models<'a> {
    live: Vec<Live<'a>>,
}
impl<'a> Models<'a> {
    pub fn new() -> Self {
        Self { live: Vec::new() }
    }
    /// Outer execution-manifest admission covers these bounded borrowed entries.
    pub fn begin(
        &mut self,
        declarations: &'a [DeviceDeclaration],
        reset: SessionReset,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if reset == SessionReset::Cold && !self.live.is_empty() {
            return Err(integrity("cold reset has unclosed model evidence"));
        }
        for instance in &mut self.live {
            instance.seen = false;
        }
        if blocked {
            return Ok(());
        }
        for declaration in declarations {
            c.checkpoint(self.live.len() as u64 + 1)?;
            if self.live.len() == MAX_DEVICE_MODELS
                || self.live.iter().any(|m| m.declaration.id == declaration.id)
            {
                return Err(integrity(
                    "duplicate or excessive active model declarations",
                ));
            }
            self.live.try_reserve(1).map_err(|_| {
                Error::new(
                    ErrorCode::ResourceLimited,
                    "model validation allocation refused",
                )
            })?;
            self.live.push(Live {
                declaration,
                definition: declaration.identity(c)?,
                reads: 0,
                writes: 0,
                commands: match &declaration.behavior {
                    DeviceBehavior::CommandBank(bank) => Some(CommandObservation {
                        pending: bank.initial_pending(),
                        ..Default::default()
                    }),
                    _ => None,
                },
                issue: None,
                seen: false,
                closed: false,
            });
        }
        Ok(())
    }
    pub fn observe(
        &mut self,
        observed: &ModelObservation,
        close_chain: bool,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        c.checkpoint(self.live.len() as u64 + 1)?;
        let instance = self
            .live
            .iter_mut()
            .find(|i| i.declaration.id == observed.id)
            .ok_or_else(|| integrity("model observation has no live declaration"))?;
        if instance.seen
            || observed.definition != instance.definition
            || observed.lifetime != instance.declaration.lifetime
            || observed.closed != (close_chain || observed.lifetime == RegionLifetime::Phase)
            || observed.status != observed.expected_status()
            || observed.reads < instance.reads
            || observed.writes < instance.writes
            || (instance.issue.is_some() && observed.issue != instance.issue)
            || (blocked
                && (observed.reads != instance.reads
                    || observed.writes != instance.writes
                    || observed.issue != instance.issue))
        {
            return Err(integrity("model identity, counters or closure differs"));
        }
        let (reads, writes) = match &instance.declaration.behavior {
            DeviceBehavior::SequenceRead { runs, .. } => {
                c.checkpoint(runs.len() as u64)?;
                (Some(ReadRun::total(runs)? as usize), Some(0))
            }
            DeviceBehavior::Fifo { reads, writes, .. } => (Some(reads.len()), Some(writes.len())),
            DeviceBehavior::SelectedSequence { sequences, .. } => {
                c.checkpoint(sequences.len() as u64)?;
                let mut total = 0usize;
                for sequence in sequences {
                    total += ReadRun::total(&sequence.runs)? as usize;
                }
                (Some(total), None)
            }
            DeviceBehavior::ConstantRead { .. }
            | DeviceBehavior::CyclicRead { .. }
            | DeviceBehavior::ReadClear { .. } => (None, Some(0)),
            _ => (None, None),
        };
        match (
            &instance.declaration.behavior,
            observed.commands,
            instance.commands,
        ) {
            (DeviceBehavior::CommandBank(bank), Some(now), Some(old)) => {
                c.checkpoint((bank.cells.len() + bank.ports.len()) as u64 + 1)?;
                let initial = u64::from(bank.initial_pending());
                let accounted = now
                    .completed
                    .checked_add(now.aborted)
                    .and_then(|n| n.checked_add(u64::from(now.pending)));
                if now.issued.checked_add(initial).is_none()
                    || now.issued.checked_add(initial) != accounted
                    || now.issued != observed.writes
                    || now.completed > observed.reads
                    || now.pending as usize > bank.ports.len()
                    || now.resets > now.issued
                    || now.aborted > now.resets
                    || u64::from(now.scripted_reads) > now.issued - now.resets
                    || u64::from(now.scripted_reads) + u64::from(observed.remaining_reads)
                        != bank.samples() as u64
                    || observed.remaining_writes != 0
                    || now.issued < old.issued
                    || now.completed < old.completed
                    || now.resets < old.resets
                    || now.aborted < old.aborted
                    || now.scripted_reads < old.scripted_reads
                    || (blocked && now != old)
                {
                    return Err(integrity(
                        "command accounting differs from declaration or prior phase",
                    ));
                }
                if now.completed - old.completed > observed.reads - instance.reads
                    || now.aborted - old.aborted > now.resets - old.resets
                    || now.resets - old.resets > now.issued - old.issued
                    || u64::from(now.scripted_reads - old.scripted_reads)
                        > (now.issued - old.issued) - (now.resets - old.resets)
                {
                    return Err(integrity(
                        "command progress has no corresponding port operations",
                    ));
                }
            }
            (DeviceBehavior::CommandBank(_), _, _) => {
                return Err(integrity("missing command obligations"));
            }
            (_, None, None) => {}
            _ => return Err(integrity("unexpected command obligations")),
        }
        for (expected, used, remaining) in [
            (reads, observed.reads, observed.remaining_reads),
            (writes, observed.writes, observed.remaining_writes),
        ] {
            if matches!(
                instance.declaration.behavior,
                DeviceBehavior::CommandBank(_)
            ) {
                continue;
            }
            if match expected {
                Some(expected) => used.checked_add(u64::from(remaining)) != Some(expected as u64),
                None => remaining != 0,
            } {
                return Err(integrity("model obligations differ from its declaration"));
            }
        }
        instance.seen = true;
        instance.closed = observed.closed;
        instance.reads = observed.reads;
        instance.writes = observed.writes;
        instance.issue = observed.issue;
        instance.commands = observed.commands;
        Ok(())
    }
    pub fn finish_side(&mut self) -> Result<()> {
        if self.live.iter().any(|i| !i.seen) {
            return Err(integrity("missing model participation evidence"));
        }
        self.live.retain(|i| !i.closed);
        Ok(())
    }
    pub fn closed(&self) -> bool {
        self.live.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn declaration() -> DeviceDeclaration {
        DeviceDeclaration {
            id: "bus".into(),
            applicability: "synthetic".into(),
            lifetime: RegionLifetime::Session,
            behavior: DeviceBehavior::CommandBank(CommandBank {
                selector_mask: 0xff,
                data_mask: 0xff00,
                read_command: 0x10000,
                write_command: 0x20000,
                busy_mask: 0x40000,
                reset_command: Some(0x80000),
                ports: vec![CommandPort {
                    address: 0x3000,
                    initial: 0,
                    initial_busy_reads: 0,
                    busy_reads: 1,
                }],
                cells: vec![CommandCell {
                    selector: 1,
                    initial: 0,
                    reads: Some(vec![7]),
                }],
            }),
        }
    }
    fn pending(d: &DeviceDeclaration) -> ModelObservation {
        ModelObservation {
            commands: Some(CommandObservation {
                issued: 1,
                pending: 1,
                scripted_reads: 1,
                ..Default::default()
            }),
            id: d.id.clone(),
            definition: d.identity(&mut || Ok(())).unwrap(),
            lifetime: d.lifetime,
            reads: 1,
            writes: 1,
            remaining_reads: 0,
            remaining_writes: 0,
            closed: false,
            issue: None,
            status: ModelStatus::Open,
        }
    }
    #[test]
    fn command_evidence_requires_transcript_and_pending_conservation() {
        let declarations = [declaration()];
        for change in 0..8 {
            let mut models = Models::new();
            models
                .begin(&declarations, SessionReset::Cold, false, &mut || Ok(()))
                .unwrap();
            let mut observed = pending(&declarations[0]);
            match change {
                0 => observed.commands = None,
                1 => observed.commands.as_mut().unwrap().pending = 0,
                2 => observed.remaining_reads = 1,
                3 => observed.commands.as_mut().unwrap().scripted_reads = 0,
                4 => observed.commands.as_mut().unwrap().aborted = 1,
                5 => observed.writes = 2,
                6 => {
                    observed.closed = true;
                    observed.status = ModelStatus::Complete;
                }
                7 => {
                    observed.commands = Some(CommandObservation {
                        issued: u64::MAX,
                        completed: u64::MAX,
                        resets: 1,
                        aborted: 1,
                        pending: 0,
                        scripted_reads: 1,
                    });
                    observed.reads = u64::MAX;
                    observed.writes = u64::MAX;
                }
                _ => unreachable!(),
            }
            assert_eq!(
                models
                    .observe(&observed, false, false, &mut || Ok(()))
                    .unwrap_err()
                    .code,
                ErrorCode::Integrity,
                "{change}"
            );
        }
    }
    #[test]
    fn ready_transition_needs_a_new_read_and_blocked_phases_cannot_consume() {
        let declarations = [declaration()];
        let mut models = Models::new();
        models
            .begin(&declarations, SessionReset::Cold, false, &mut || Ok(()))
            .unwrap();
        let observed = pending(&declarations[0]);
        models
            .observe(&observed, false, false, &mut || Ok(()))
            .unwrap();
        models.finish_side().unwrap();
        models
            .begin(&[], SessionReset::Warm, false, &mut || Ok(()))
            .unwrap();
        let mut ready = observed.clone();
        ready.commands.as_mut().unwrap().pending = 0;
        ready.commands.as_mut().unwrap().completed = 1;
        ready.closed = true;
        ready.status = ModelStatus::Complete;
        assert_eq!(
            models
                .observe(&ready, true, false, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
        ready.reads += 1;
        assert_eq!(
            models
                .observe(&ready, true, true, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
        models.observe(&ready, true, false, &mut || Ok(())).unwrap();
        models.finish_side().unwrap();
        assert!(models.closed());
    }
}
