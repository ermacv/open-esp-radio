//! Project authenticated historical series without losing retired observations.
use super::*;
use oer_vendor_phy::calibration_capture::space::Register;

pub(super) fn image(provenance: &Provenance, images: &RegisterImages) -> Result<Vec<Register>> {
    let mut addresses: BTreeSet<_> = images.registers.iter().map(|r| r.address).collect();
    provenance
        .retired_mmio
        .iter()
        .map(|(name, entry)| {
            let address = entry.address.ok_or("retired MMIO has no address")?;
            if entry.space != Space::Mmio
                || entry.width != 4
                || entry.count != 1
                || !addresses.insert(address)
            {
                return Err("retired MMIO overlaps active coverage or has invalid metadata".into());
            }
            Ok(Register {
                name: name.clone(),
                address,
                mask: u32::MAX,
            })
        })
        .collect()
}

pub(super) fn take(readings: &mut Readings, retired: &[Register]) -> Result<Readings> {
    if readings
        .values
        .keys()
        .any(|address| readings.unreadable.contains(address))
    {
        return Err("inconsistent register readability".into());
    }
    let mut observations = Readings::default();
    for register in retired {
        if let Some(value) = readings.values.remove(&register.address) {
            observations.values.insert(register.address, value);
        }
        if readings.unreadable.remove(&register.address) {
            observations.unreadable.insert(register.address);
        }
    }
    Ok(observations)
}

pub(super) fn validate(
    provenance: &Provenance,
    boards: &BTreeMap<String, Board>,
    images: &RegisterImages,
) -> Result<()> {
    let retired = image(provenance, images)?;
    if retired.is_empty() {
        if !provenance.retired_series.is_empty() {
            return Err("retired observations have no register metadata".into());
        }
        return Ok(());
    }
    let mut sources = BTreeSet::new();
    for series in &provenance.retired_series {
        let board = boards
            .get(&series.board)
            .ok_or("unknown retired-series board")?;
        if !board.sources.contains(&series.source)
            || series.boots.len() != series.source.samples
            || !sources.insert((
                &series.board,
                &series.source.run_id,
                &series.source.scenario,
                series.source.repetition,
            ))
        {
            return Err("retired observations differ from source provenance".into());
        }
        for boot in &series.boots {
            complete(boot, &retired, true)?;
        }
    }
    let expected_sources = boards
        .values()
        .try_fold(0_usize, |count, board| {
            count.checked_add(board.sources.len())
        })
        .ok_or("source count overflow")?;
    if sources.len() != expected_sources {
        return Err("missing retired-series observations".into());
    }
    Ok(())
}

impl Reference {
    /// Rebuild the same primary series under current metadata, retaining every
    /// no-longer-selected MMIO observation as capture provenance.
    pub fn regenerate(
        self,
        signature: Signature,
        inputs: Vec<Input>,
        model: &Model,
    ) -> Result<Self> {
        let images = &model.images;
        if self.signature != signature
            || self.schema != SCHEMA
            || self.chip != CHIP
            || self.lifecycle != Lifecycle::Cold
            || self.scenario != SCENARIO
        {
            return Err("archive reference has a different libphy or capture point".into());
        }
        let active: BTreeSet<_> = images.registers.iter().map(|r| r.address).collect();
        let mut provenance = self.provenance;
        let previous_series = std::mem::take(&mut provenance.retired_series);
        for (name, entry) in self.entries {
            if entry.space == Space::Mmio
                && !active.contains(&entry.address.ok_or("missing MMIO address")?)
                && provenance.retired_mmio.insert(name, entry).is_some()
            {
                return Err("duplicate retired MMIO metadata".into());
            }
        }
        let reference = generate(signature, provenance, inputs, model)?;
        if reference.boards != self.boards
            || reference.channel != self.channel
            || reference.vendor_object_bytes != self.vendor_object_bytes
        {
            return Err("regeneration must retain every original board and source series".into());
        }
        for previous in previous_series {
            let current = reference
                .provenance
                .retired_series
                .iter()
                .find(|series| series.board == previous.board && series.source == previous.source)
                .ok_or("missing original retired series")?;
            if current.boots.len() != previous.boots.len() {
                return Err("retired series boot count changed".into());
            }
            for (old, new) in previous.boots.iter().zip(&current.boots) {
                if old
                    .values
                    .iter()
                    .any(|(address, value)| new.values.get(address) != Some(value))
                    || !old.unreadable.is_subset(&new.unreadable)
                {
                    return Err("original retired observations changed".into());
                }
            }
        }
        Ok(reference)
    }
}
