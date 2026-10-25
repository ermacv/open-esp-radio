//! Partitions, parts and unclaimed words: every MMIO word held once.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    Claim, Context, Elements, Error, ErrorKind, Holder, Index, PartId, PolicyFile, Target,
    Triggers, lower_snake, upper_camel,
};
use crate::chip_model::FieldClass;

/// How one word is held while the policy loads.
enum State {
    Whole(Holder),
    /// Field positions of a word of write-trigger fields.
    Fields(BTreeMap<usize, Holder>),
}

/// The words each part holds whole.
pub(super) struct PartWords {
    pub(super) id: PartId,
    pub(super) whole: Vec<usize>,
}

/// Every MMIO word's holder, after every part and unclaimed entry.
pub(super) struct Coverage {
    states: Vec<Option<State>>,
    pub(super) parts: Vec<PartWords>,
}

impl Coverage {
    /// The part that holds the whole word, or the holder that is not one.
    pub(super) fn whole(&self, word: usize) -> Result<PartId, Option<Holder>> {
        match self.states.get(word) {
            Some(Some(State::Whole(Holder::Part(part)))) => Ok(*part),
            Some(Some(State::Whole(holder))) => Err(Some(*holder)),
            _ => Err(None),
        }
    }

    /// The holders of the fields at `positions` of a split word, or `None`
    /// when the word is not split.
    pub(super) fn split(&self, word: usize) -> Option<&BTreeMap<usize, Holder>> {
        match self.states.get(word) {
            Some(Some(State::Fields(fields))) => Some(fields),
            _ => None,
        }
    }

    pub(super) fn claims(&self, index: &Index<'_>) -> BTreeMap<String, Claim> {
        let mut claims = BTreeMap::new();
        for (word, state) in index.words.iter().zip(&self.states) {
            let claim = match state {
                Some(State::Whole(holder)) => Claim::Whole(*holder),
                Some(State::Fields(fields)) => Claim::Fields(
                    fields
                        .iter()
                        .filter_map(|(position, holder)| {
                            word.fields()
                                .get(*position)
                                .map(|field| (field.name.clone(), *holder))
                        })
                        .collect(),
                ),
                None => continue,
            };
            claims.insert(word.identity().to_owned(), claim);
        }
        claims
    }

    fn label(file: &PolicyFile, holder: Holder) -> String {
        match holder {
            Holder::Part(id) => file
                .partitions
                .get(id.partition)
                .and_then(|partition| {
                    partition
                        .parts
                        .get(id.part)
                        .map(|part| format!("part {}.{}", partition.name, part.name))
                })
                .unwrap_or_default(),
            Holder::Unclaimed(position) => format!("unclaimed entry {position}"),
        }
    }

    fn mark_whole(
        &mut self,
        context: Context<'_>,
        file: &PolicyFile,
        index: &Index<'_>,
        word: usize,
        holder: Holder,
    ) -> Result<(), Error> {
        let identity = index.word(word).map_or("", |word| word.identity());
        match self.states.get_mut(word) {
            Some(slot @ None) => {
                *slot = Some(State::Whole(holder));
                Ok(())
            }
            Some(Some(State::Whole(other))) => {
                Err(context.error(identity, ErrorKind::ClaimedTwice(Self::label(file, *other))))
            }
            Some(Some(State::Fields(fields))) => Err(context.error(
                identity,
                ErrorKind::ClaimedTwice(
                    fields
                        .values()
                        .next()
                        .map(|other| Self::label(file, *other))
                        .unwrap_or_default(),
                ),
            )),
            None => Err(context.error(identity, ErrorKind::UnknownRegister(identity.to_owned()))),
        }
    }

    fn mark_field(
        &mut self,
        context: Context<'_>,
        file: &PolicyFile,
        entity: &str,
        word: usize,
        field: usize,
        holder: Holder,
    ) -> Result<(), Error> {
        match self.states.get_mut(word) {
            Some(slot @ None) => {
                *slot = Some(State::Fields(BTreeMap::from([(field, holder)])));
                Ok(())
            }
            Some(Some(State::Whole(other))) => {
                Err(context.error(entity, ErrorKind::ClaimedTwice(Self::label(file, *other))))
            }
            Some(Some(State::Fields(fields))) => match fields.get(&field) {
                Some(other) => {
                    Err(context.error(entity, ErrorKind::ClaimedTwice(Self::label(file, *other))))
                }
                None => {
                    fields.insert(field, holder);
                    Ok(())
                }
            },
            None => Err(context.error(entity, ErrorKind::UnknownRegister(entity.to_owned()))),
        }
    }
}

/// One selection of words of one instance.
struct Selection<'a> {
    instance: &'a str,
    registers: &'a [String],
    elements: &'a [Elements],
    triggers: &'a [Triggers],
}

impl Selection<'_> {
    fn is_empty(&self) -> bool {
        self.registers.is_empty() && self.elements.is_empty() && self.triggers.is_empty()
    }
}

/// Check every partition, part and unclaimed entry and that every MMIO
/// word is held exactly once.
pub(super) fn check(
    context: Context<'_>,
    index: &Index<'_>,
    file: &PolicyFile,
) -> Result<Coverage, Error> {
    let mut states = Vec::new();
    states
        .try_reserve_exact(index.words.len())
        .map_err(|error| context.error("", ErrorKind::Allocation(error)))?;
    states.extend(index.words.iter().map(|_| None));
    let mut coverage = Coverage {
        states,
        parts: Vec::new(),
    };
    let mut names = BTreeSet::new();
    let mut members = BTreeSet::new();
    for (partition_position, partition) in file.partitions.iter().enumerate() {
        let entity = partition.name.as_str();
        if !upper_camel(entity) || entity == "Self" {
            return Err(context.error(
                entity,
                ErrorKind::InvalidName(format!("partition {entity:?} is not UpperCamelCase")),
            ));
        }
        if !names.insert(entity) {
            return Err(context.error(entity, ErrorKind::Duplicate(format!("partition {entity}"))));
        }
        if !lower_snake(&partition.member) {
            return Err(context.error(
                entity,
                ErrorKind::InvalidName(format!(
                    "member {:?} is not a lower_snake_case identifier",
                    partition.member
                )),
            ));
        }
        if !members.insert(partition.member.as_str()) {
            return Err(context.error(
                entity,
                ErrorKind::Duplicate(format!("member {}", partition.member)),
            ));
        }
        context.description(entity, &partition.description)?;
        if partition.parts.is_empty() {
            return Err(context.error(entity, ErrorKind::EmptyPartition));
        }
        let mut part_names = BTreeSet::new();
        for (part_position, part) in partition.parts.iter().enumerate() {
            let entity = format!("{}.{}", partition.name, part.name);
            if !upper_camel(&part.name) {
                return Err(context.error(
                    &entity,
                    ErrorKind::InvalidName(format!("part {:?} is not UpperCamelCase", part.name)),
                ));
            }
            if !part_names.insert(part.name.as_str()) {
                return Err(context.error(&entity, ErrorKind::Duplicate(format!("part {entity}"))));
            }
            check_flags(context, &entity, part)?;
            let id = PartId {
                partition: partition_position,
                part: part_position,
            };
            let selection = Selection {
                instance: &part.instance,
                registers: &part.registers,
                elements: &part.elements,
                triggers: &part.triggers,
            };
            let whole = select(
                context,
                index,
                file,
                &mut coverage,
                &entity,
                &selection,
                Holder::Part(id),
            )?;
            coverage.parts.push(PartWords { id, whole });
        }
    }
    for (position, unclaimed) in file.unclaimed.iter().enumerate() {
        let entity = format!("unclaimed entry {position}");
        context.description(&entity, &unclaimed.reason)?;
        let holder = Holder::Unclaimed(position);
        match (&unclaimed.instance, unclaimed.instances.is_empty()) {
            (None, false) => {
                if !unclaimed.registers.is_empty()
                    || !unclaimed.elements.is_empty()
                    || !unclaimed.triggers.is_empty()
                {
                    return Err(context.error(
                        &entity,
                        ErrorKind::UnclaimedForm(
                            "whole instances select no registers, elements or triggers".to_owned(),
                        ),
                    ));
                }
                for instance in &unclaimed.instances {
                    let words = index.instances.get(instance.as_str()).ok_or_else(|| {
                        context.error(&entity, ErrorKind::UnknownInstance(instance.clone()))
                    })?;
                    for word in words {
                        coverage.mark_whole(context, file, index, *word, holder)?;
                    }
                }
            }
            (Some(instance), true) => {
                let selection = Selection {
                    instance,
                    registers: &unclaimed.registers,
                    elements: &unclaimed.elements,
                    triggers: &unclaimed.triggers,
                };
                select(
                    context,
                    index,
                    file,
                    &mut coverage,
                    &entity,
                    &selection,
                    holder,
                )?;
            }
            _ => {
                return Err(context.error(
                    &entity,
                    ErrorKind::UnclaimedForm(
                        "names either whole instances or one instance with its words".to_owned(),
                    ),
                ));
            }
        }
    }
    for (word, state) in index.words.iter().zip(&coverage.states) {
        match state {
            None => return Err(context.error(word.identity(), ErrorKind::Uncovered)),
            Some(State::Fields(fields)) => {
                if let Some((_, field)) = word
                    .fields()
                    .iter()
                    .enumerate()
                    .find(|(position, _)| !fields.contains_key(position))
                {
                    return Err(context.error(
                        format!("{}.{}", word.identity(), field.name),
                        ErrorKind::Uncovered,
                    ));
                }
            }
            Some(State::Whole(_)) => {}
        }
    }
    check_parts(context, index, file, &coverage)?;
    Ok(coverage)
}

/// The flags of one part agree with each other and their evidence.
fn check_flags(context: Context<'_>, entity: &str, part: &super::Part) -> Result<(), Error> {
    let flagged = part.irq_side
        || part.shared
        || part.read_only
        || !part.observes.is_empty()
        || !part.triggers.is_empty();
    if flagged {
        context.description(entity, part.description.as_deref().unwrap_or_default())?;
        context.sources(entity, &part.sources)?;
    } else if !part.sources.is_empty() {
        context.sources(entity, &part.sources)?;
    }
    let conflict = if part.shared && part.irq_side {
        Some("a shared part is reached from every context, not one interrupt handler")
    } else if part.shared && (part.read_only || part.copy) {
        Some("a shared part is no read-only owner")
    } else if part.shared && !(part.observes.is_empty() && part.triggers.is_empty()) {
        Some("a shared part holds whole words and observes nothing")
    } else if part.copy && !part.read_only {
        Some("only a read-only part is Copy")
    } else if part.read_only && !part.triggers.is_empty() {
        Some("a read-only part writes no trigger")
    } else {
        None
    };
    match conflict {
        Some(reason) => Err(context.error(entity, ErrorKind::Flags(reason.to_owned()))),
        None => Ok(()),
    }
}

/// Mark the selected words for `holder`; the words it holds whole.
fn select(
    context: Context<'_>,
    index: &Index<'_>,
    file: &PolicyFile,
    coverage: &mut Coverage,
    entity: &str,
    selection: &Selection<'_>,
    holder: Holder,
) -> Result<Vec<usize>, Error> {
    if !index.instances.contains_key(selection.instance) {
        return Err(context.error(
            entity,
            ErrorKind::UnknownInstance(selection.instance.to_owned()),
        ));
    }
    if selection.is_empty() {
        return Err(context.error(entity, ErrorKind::EmptySelection));
    }
    let mut whole = Vec::new();
    for register in selection.registers {
        let reference = format!("{}.{register}", selection.instance);
        let target = index
            .target(&reference)
            .ok_or_else(|| context.error(entity, ErrorKind::UnknownRegister(reference.clone())))?;
        for word in index.reach(target) {
            coverage.mark_whole(context, file, index, word, holder)?;
            whole.push(word);
        }
    }
    for range in selection.elements {
        let reference = format!("{}.{}", selection.instance, range.register);
        let Some(Target::Template(template)) = index.target(&reference) else {
            return Err(context.error(entity, ErrorKind::ElementRange(reference)));
        };
        let words = index.templates.get(template).map_or(&[][..], Vec::as_slice);
        let first = usize::try_from(range.first).unwrap_or(usize::MAX);
        let count = usize::try_from(range.count).unwrap_or(usize::MAX);
        let selected = first
            .checked_add(count)
            .filter(|_| count > 0)
            .and_then(|end| words.get(first..end))
            .ok_or_else(|| context.error(entity, ErrorKind::ElementRange(reference.clone())))?;
        for word in selected {
            coverage.mark_whole(context, file, index, *word, holder)?;
            whole.push(*word);
        }
    }
    for triggers in selection.triggers {
        let reference = format!("{}.{}", selection.instance, triggers.register);
        let Some(Target::Word(position)) = index.target(&reference) else {
            return Err(context.error(entity, ErrorKind::UnknownRegister(reference)));
        };
        let word = index
            .word(position)
            .ok_or_else(|| context.error(entity, ErrorKind::UnknownRegister(reference.clone())))?;
        if word.fields().is_empty()
            || word
                .fields()
                .iter()
                .any(|field| field.class != FieldClass::WriteTrigger || field.width != 1)
            || !word.storable()
        {
            return Err(context.error(
                &reference,
                ErrorKind::Triggers(
                    "only a word of one-bit write-trigger fields is split by field".to_owned(),
                ),
            ));
        }
        if triggers.fields.is_empty() {
            return Err(context.error(&reference, ErrorKind::EmptySelection));
        }
        for name in &triggers.fields {
            let field_entity = format!("{reference}.{name}");
            let (field, _) = word.field(name).ok_or_else(|| {
                context.error(entity, ErrorKind::UnknownField(field_entity.clone()))
            })?;
            coverage.mark_field(context, file, &field_entity, position, field, holder)?;
        }
    }
    Ok(whole)
}

/// Rules that need every word's holder: what a part observes and what a
/// shared or read-only part may hold.
fn check_parts(
    context: Context<'_>,
    index: &Index<'_>,
    file: &PolicyFile,
    coverage: &Coverage,
) -> Result<(), Error> {
    for words in &coverage.parts {
        let Some(partition) = file.partitions.get(words.id.partition) else {
            continue;
        };
        let Some(part) = partition.parts.get(words.id.part) else {
            continue;
        };
        let entity = format!("{}.{}", partition.name, part.name);
        let mut observed_targets = BTreeSet::new();
        for observed in &part.observes {
            let reference = format!("{}.{observed}", part.instance);
            let target = index.target(&reference).ok_or_else(|| {
                context.error(&entity, ErrorKind::UnknownRegister(reference.clone()))
            })?;
            if !observed_targets.insert(target) {
                return Err(context.error(
                    &entity,
                    ErrorKind::Duplicate(format!("observed {reference}")),
                ));
            }
            for word in index.reach(target) {
                let identity = index.word(word).map_or("", |word| word.identity());
                if !index.word(word).is_some_and(|word| word.reads_freely()) {
                    return Err(context.error(
                        &entity,
                        ErrorKind::Observes(format!("{identity} has no side-effect-free read")),
                    ));
                }
                match coverage.whole(word) {
                    Ok(holder) if holder == words.id => {
                        return Err(context.error(
                            &entity,
                            ErrorKind::Observes(format!("{identity} is the part's own word")),
                        ));
                    }
                    Ok(_) => {}
                    Err(_) => {
                        return Err(context.error(
                            &entity,
                            ErrorKind::Observes(format!("{identity} is held by no part")),
                        ));
                    }
                }
            }
        }
        for word in words.whole.iter().filter_map(|word| index.word(*word)) {
            if part.read_only && !word.reads_freely() {
                return Err(context.error(
                    &entity,
                    ErrorKind::ReadOnly(format!(
                        "{} has no side-effect-free read",
                        word.identity()
                    )),
                ));
            }
            if part.shared {
                let written = word
                    .fields()
                    .iter()
                    .filter(|field| field.class.written())
                    .collect::<Vec<_>>();
                let strobes = written
                    .iter()
                    .all(|field| field.class == FieldClass::WriteTrigger);
                let stored = matches!(word.fields(), [field]
                    if matches!(field.class, FieldClass::Plain | FieldClass::WriteOnly));
                let side_effects = word.template.fifo.is_some()
                    || word.template.read_side_effect.is_some()
                    || word.template.data_port.is_some()
                    || word.command
                    || word.has(FieldClass::ReadClears)
                    || word.has(FieldClass::WriteOneToClear);
                if side_effects || !(written.is_empty() || strobes || stored) {
                    return Err(context.error(
                        &entity,
                        ErrorKind::Shared(format!(
                            "{} is no side-effect-free read, write-trigger strobe or one field stored whole",
                            word.identity()
                        )),
                    ));
                }
            }
        }
    }
    Ok(())
}
