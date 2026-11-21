//! Transactions: each names fields and registers of the root, writes only
//! what their classes, write constraints and DMA pointer fields allow,
//! reaches one part and renders a method no field accessor expresses.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    Context, Error, ErrorKind, Holder, Index, InterruptSnapshot, PartId, PolicyFile, Projection,
    SequenceStep, Target, WholeWordBits, Word, field_mask, lower_snake, ownership::Coverage,
    stored,
};
use crate::chip_model::{ExpandedField, FieldClass};

/// The values a transaction may give one field.
#[derive(Clone, Copy)]
enum Values {
    Fixed(u32),
    /// Any subset of these bits, including zero for unacknowledged events.
    Bits(u32),
}

impl Values {
    /// Every value is one `constraint` admits.
    fn within(self, constraint: Option<super::Constraint<'_>>) -> bool {
        let Some(constraint) = constraint else {
            return true;
        };
        match self {
            Self::Fixed(value) => constraint.admits(value),
            Self::Bits(mask) => constraint.admits_bits(mask),
        }
    }
}

/// Every whole-word write uses the same class, W1C and constraint checks.
/// Fixed writes have a sourced whole-word exception; only images may
/// carry an acknowledgement proof. A sequence never acknowledges events.
#[derive(Clone, Copy)]
enum WordWrite {
    /// A separately sourced complete zero publication, without replay.
    Zero,
    /// A write from zero giving `written` its value.
    Store {
        written: u32,
    },
    Image {
        value: u32,
        whole_word: bool,
    },
    /// A sequence step: a whole-word image.
    Sequence {
        value: u32,
    },
    Acknowledge {
        mask: u32,
    },
}

/// Who holds every word of a repeated template.
#[derive(Clone)]
enum TemplateOwner {
    One(PartId),
    /// The first word that is held by no part, whole.
    Not(String),
    /// The first word held by another part than the first.
    Several(String),
}

/// A field a read-modify-write may replace.
fn modified(class: FieldClass) -> bool {
    class == FieldClass::Plain
}

/// A field whose bits a fixed image may set.
fn imaged(class: FieldClass) -> bool {
    matches!(
        class,
        FieldClass::Plain
            | FieldClass::WriteOnly
            | FieldClass::WriteTrigger
            | FieldClass::SetOnly
            | FieldClass::SelfClearing
            | FieldClass::HardwareWritten
    )
}

/// A field a transaction may read.
fn read(class: FieldClass) -> bool {
    class.readable() && class != FieldClass::ReadClears
}

/// The lanes of DMA pointer fields of the words a target reaches: set in
/// any of them, and set in all of them.
#[derive(Clone, Copy, Default)]
struct Lanes {
    any: u32,
    all: u32,
}

/// What a projected modify takes: its register, the bits it changes and
/// whether it is a single projection from input bit zero.
struct Projected {
    target: Target,
    mask: u32,
    single: bool,
}

struct Checker<'a, 'm> {
    context: Context<'a>,
    index: &'a Index<'m>,
    coverage: &'a Coverage,
    file: &'a PolicyFile,
    templates: Vec<TemplateOwner>,
    /// The clear words of the interrupt snapshots.
    clears: BTreeSet<usize>,
    /// The templates that repeat a clear word.
    clear_templates: BTreeSet<usize>,
    /// The lanes of each word's pointer fields: set only from the image of
    /// a typed DMA reference, or to zero.
    pointers: BTreeMap<usize, u32>,
    names: BTreeSet<&'a str>,
    owners: BTreeMap<String, PartId>,
}

/// Check every transaction; the part each transaction reaches.
pub(super) fn check(
    context: Context<'_>,
    index: &Index<'_>,
    coverage: &Coverage,
    file: &PolicyFile,
) -> Result<BTreeMap<String, PartId>, Error> {
    let templates = index
        .templates
        .iter()
        .map(|words| {
            let mut owner = None;
            for word in words {
                let identity = index
                    .word(*word)
                    .map_or_else(String::new, |word| word.identity().to_owned());
                match (coverage.whole(*word), owner) {
                    (Err(_), _) => return TemplateOwner::Not(identity),
                    (Ok(part), Some(first)) if part != first => {
                        return TemplateOwner::Several(identity);
                    }
                    (Ok(part), _) => owner = Some(part),
                }
            }
            owner.map_or_else(|| TemplateOwner::Not(String::new()), TemplateOwner::One)
        })
        .collect();
    let mut checker = Checker {
        context,
        index,
        coverage,
        file,
        templates,
        clears: BTreeSet::new(),
        clear_templates: BTreeSet::new(),
        pointers: BTreeMap::new(),
        names: BTreeSet::new(),
        owners: BTreeMap::new(),
    };
    checker.pointers();
    checker.snapshots()?;
    checker.reads()?;
    checker.stores()?;
    checker.images()?;
    checker.modifies()?;
    Ok(checker.owners)
}

impl<'a, 'm: 'a> Checker<'a, 'm> {
    fn error(&self, entity: impl Into<String>, kind: ErrorKind) -> Error {
        self.context.error(entity, kind)
    }

    /// A transaction's name is a distinct `lower_snake_case` method name
    /// and its sources are cited.
    fn begin(&mut self, name: &'a str, sources: &[String]) -> Result<(), Error> {
        if !lower_snake(name) {
            return Err(self.error(
                name,
                ErrorKind::InvalidName(format!("transaction {name:?} is not lower_snake_case")),
            ));
        }
        if !self.names.insert(name) {
            return Err(self.error(name, ErrorKind::Duplicate(format!("transaction {name}"))));
        }
        self.context.sources(name, sources)
    }

    fn register(&self, entity: &str, reference: &str) -> Result<Target, Error> {
        self.index
            .target(reference)
            .ok_or_else(|| self.error(entity, ErrorKind::UnknownRegister(reference.to_owned())))
    }

    /// The words a sequence step of `steps` steps writes, in order: its one
    /// element, or the `count` elements of its template from `first`. A
    /// template step writes whole words and is its sequence's only step.
    fn step_words(
        &self,
        entity: &str,
        step: &SequenceStep,
        steps: usize,
    ) -> Result<Vec<usize>, Error> {
        let (first, count) = match (step.first, step.count) {
            (None, None) => return Ok(vec![self.element(entity, &step.register)?]),
            (Some(first), Some(count)) => (first, count),
            _ => {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(format!(
                        "{} names first and count together",
                        step.register
                    )),
                ));
            }
        };
        if steps != 1 {
            return Err(self.error(
                entity,
                ErrorKind::Transaction(format!(
                    "{}: an element range is its sequence's only step",
                    step.register
                )),
            ));
        }
        let Target::Template(template) = self.register(entity, &step.register)? else {
            return Err(self.error(entity, ErrorKind::ElementRange(step.register.clone())));
        };
        let words = self
            .index
            .templates
            .get(template)
            .map_or(&[][..], Vec::as_slice);
        let first = usize::try_from(first).unwrap_or(usize::MAX);
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        first
            .checked_add(count)
            .filter(|_| count > 0)
            .and_then(|end| words.get(first..end))
            .map(<[usize]>::to_vec)
            .ok_or_else(|| self.error(entity, ErrorKind::ElementRange(step.register.clone())))
    }

    /// One element, not a template.
    fn element(&self, entity: &str, reference: &str) -> Result<usize, Error> {
        match self.register(entity, reference)? {
            Target::Word(word) => Ok(word),
            Target::Template(_) => Err(self.error(
                entity,
                ErrorKind::Transaction(format!("{reference} must name one element")),
            )),
        }
    }

    fn field(&self, entity: &str, reference: &str) -> Result<(Target, &ExpandedField), Error> {
        let (register, _) = reference
            .rsplit_once('.')
            .ok_or_else(|| self.error(entity, ErrorKind::UnknownField(reference.to_owned())))?;
        let target = self.register(entity, register)?;
        let (_, position) = self
            .index
            .field(reference)
            .ok_or_else(|| self.error(entity, ErrorKind::UnknownField(reference.to_owned())))?;
        let field = self
            .word(entity, target)?
            .fields()
            .get(position)
            .ok_or_else(|| self.error(entity, ErrorKind::UnknownField(reference.to_owned())))?;
        Ok((target, field))
    }

    fn word(&self, entity: &str, target: Target) -> Result<&Word<'m>, Error> {
        self.index
            .first(target)
            .ok_or_else(|| self.error(entity, ErrorKind::UnknownRegister(entity.to_owned())))
    }

    fn on_clear(&self, target: Target) -> bool {
        let paired = match target {
            Target::Word(word) => self.clears.contains(&word),
            Target::Template(template) => self.clear_templates.contains(&template),
        };
        // A word consisting entirely of W1C fields is its own implicit
        // status/clear pair, even without a named policy snapshot.
        paired
            || self.index.first(target).is_some_and(|word| {
                !word.fields().is_empty()
                    && word
                        .fields()
                        .iter()
                        .all(|field| field.class == FieldClass::WriteOneToClear)
            })
    }

    /// The DMA pointer lanes of the words `target` reaches.
    fn lanes(&self, target: Target) -> Lanes {
        let mut lanes = Lanes {
            any: 0,
            all: u32::MAX,
        };
        for word in self.index.reach(target) {
            let mask = self.pointers.get(&word).copied().unwrap_or(0);
            lanes.any |= mask;
            lanes.all &= mask;
        }
        lanes
    }

    /// `bits` set no DMA pointer field of `target`.
    fn no_dma(&self, entity: &str, target: Target, bits: u32) -> Result<(), Error> {
        if bits & self.lanes(target).any != 0 {
            return Err(self.error(
                entity,
                ErrorKind::Pointer(
                    "a DMA address field is set only from a typed reference or to zero".to_owned(),
                ),
            ));
        }
        Ok(())
    }

    /// `values` written to `field` are ones its write constraint admits.
    fn constrained(
        &self,
        entity: &str,
        word: &Word<'_>,
        field: &ExpandedField,
        values: Values,
    ) -> Result<(), Error> {
        if values.within(word.constraint(field)) {
            return Ok(());
        }
        Err(self.error(
            entity,
            ErrorKind::WriteConstraint(format!(
                "{} admits fewer values than the transaction writes",
                field.identity
            )),
        ))
    }

    /// The part whose words `target` are; `bits` are the bits a write
    /// sets, which choose the holder of a split word and lie in its
    /// declared fields.
    fn owner(&self, entity: &str, target: Target, bits: Option<u32>) -> Result<PartId, Error> {
        let identity = |word: usize| {
            self.index
                .word(word)
                .map_or_else(String::new, |word| word.identity().to_owned())
        };
        match target {
            Target::Template(template) => match self.templates.get(template) {
                Some(TemplateOwner::One(part)) => Ok(*part),
                Some(TemplateOwner::Several(word)) => {
                    Err(self.error(entity, ErrorKind::SeveralOwners(word.clone())))
                }
                Some(TemplateOwner::Not(word)) => {
                    Err(self.error(entity, ErrorKind::ReachesUnclaimed(word.clone())))
                }
                None => Err(self.error(entity, ErrorKind::UnknownRegister(entity.to_owned()))),
            },
            Target::Word(word) => match self.coverage.whole(word) {
                Ok(part) => Ok(part),
                Err(Some(_)) => {
                    Err(self.error(entity, ErrorKind::ReachesUnclaimed(identity(word))))
                }
                Err(None) => {
                    let fields = self.coverage.split(word).ok_or_else(|| {
                        self.error(entity, ErrorKind::ReachesUnclaimed(identity(word)))
                    })?;
                    let register = self.word(entity, target)?;
                    let bits = bits.unwrap_or(0);
                    if bits & !register.declared() != 0 {
                        return Err(self.error(
                            entity,
                            ErrorKind::ReachesUnclaimed(format!(
                                "bits 0x{:08x} of the split word {} outside its trigger fields",
                                bits & !register.declared(),
                                identity(word)
                            )),
                        ));
                    }
                    let mut owner = None;
                    for (position, field) in register.fields().iter().enumerate() {
                        if field_mask(field) & bits == 0 {
                            continue;
                        }
                        match (fields.get(&position), owner) {
                            (Some(Holder::Part(part)), None) => owner = Some(*part),
                            (Some(Holder::Part(part)), Some(first)) if *part == first => {}
                            (Some(Holder::Part(_)), Some(_)) => {
                                return Err(self.error(
                                    entity,
                                    ErrorKind::SeveralOwners(format!(
                                        "{}.{}",
                                        identity(word),
                                        field.name
                                    )),
                                ));
                            }
                            _ => {
                                return Err(self.error(
                                    entity,
                                    ErrorKind::ReachesUnclaimed(format!(
                                        "{}.{}",
                                        identity(word),
                                        field.name
                                    )),
                                ));
                            }
                        }
                    }
                    owner.ok_or_else(|| {
                        self.error(
                            entity,
                            ErrorKind::Transaction(format!(
                                "writes no field of the split word {}",
                                identity(word)
                            )),
                        )
                    })
                }
            },
        }
    }

    /// Record the one part `owners` name; a shared part takes no
    /// transaction and a read-only part no write.
    fn take(&mut self, entity: &str, owners: &[PartId], writes: bool) -> Result<(), Error> {
        let Some(owner) = owners.first().copied() else {
            return Ok(());
        };
        if owners.iter().any(|other| *other != owner) {
            return Err(self.error(
                entity,
                ErrorKind::SeveralOwners("its words are held by different parts".to_owned()),
            ));
        }
        let part = self
            .file
            .partitions
            .get(owner.partition)
            .and_then(|partition| partition.parts.get(owner.part));
        if part.is_some_and(|part| part.shared) {
            return Err(self.error(
                entity,
                ErrorKind::Shared(
                    "takes no transaction: it only reads, strobes and stores".to_owned(),
                ),
            ));
        }
        if writes && part.is_some_and(|part| part.read_only) {
            return Err(self.error(
                entity,
                ErrorKind::ReadOnly("its words are only read".to_owned()),
            ));
        }
        self.owners.insert(entity.to_owned(), owner);
        Ok(())
    }

    /// Record the one part a fixed image's `owners` name, as [`Self::take`]
    /// does; a shared part with a hart's proof takes it too, when the image
    /// is a stored image of whole words: the only stores such a part has.
    fn take_image(
        &mut self,
        entity: &str,
        owners: &[PartId],
        image: &super::FixedRegisterImage,
    ) -> Result<(), Error> {
        let proven = owners.first().and_then(|owner| {
            self.file
                .partitions
                .get(owner.partition)
                .and_then(|partition| partition.parts.get(owner.part))
                .filter(|part| part.shared && part.hart_proof.is_some())
                .map(|_| *owner)
        });
        let Some(owner) = proven else {
            return self.take(entity, owners, true);
        };
        if owners.iter().any(|other| *other != owner) {
            return Err(self.error(
                entity,
                ErrorKind::SeveralOwners("its words are held by different parts".to_owned()),
            ));
        }
        if !image.whole_word || image.masks.is_some() || image.consumed_event.is_some() {
            return Err(self.error(
                entity,
                ErrorKind::Shared(
                    "a shared part with a hart's proof stores only whole-word images".to_owned(),
                ),
            ));
        }
        self.owners.insert(entity.to_owned(), owner);
        Ok(())
    }

    fn class(
        &self,
        entity: &str,
        field: &ExpandedField,
        allowed: bool,
        what: &str,
    ) -> Result<(), Error> {
        if allowed {
            return Ok(());
        }
        if field.class == FieldClass::WriteOneToClear {
            return Err(self.error(
                entity,
                ErrorKind::WriteOneToClear(format!(
                    "{} is written only by its snapshot's acknowledgement",
                    field.identity
                )),
            ));
        }
        Err(self.error(
            entity,
            ErrorKind::FieldClass(format!(
                "{} is {}, which {what} cannot reach",
                field.identity,
                field.class.label()
            )),
        ))
    }

    fn snapshots(&mut self) -> Result<(), Error> {
        let file = self.file;
        for snapshot in &file.interrupt_snapshots {
            let entity = snapshot.name.as_str();
            self.begin(entity, &snapshot.sources)?;
            let status = self.element(entity, &snapshot.status)?;
            let clear = self.element(entity, &snapshot.clear)?;
            let status_word = self.word(entity, Target::Word(status))?;
            let clear_word = self.word(entity, Target::Word(clear))?;
            // One word of write-one-to-clear fields is its own status: the
            // sample reads it and the acknowledgement writes back ones.
            let own_status = status == clear;
            if !status_word.reads_freely()
                || (status_word.has(FieldClass::WriteOneToClear) && !own_status)
                || (own_status
                    && status_word
                        .fields()
                        .iter()
                        .any(|field| field.class != FieldClass::WriteOneToClear))
            {
                return Err(self.error(
                    entity,
                    ErrorKind::Snapshot(
                        "its status word reads without a side effect and is acknowledged elsewhere"
                            .to_owned(),
                    ),
                ));
            }
            let mut mask = status_word.declared() & clear_word.declared();
            if !snapshot.events.is_empty() {
                let mut events = 0;
                for reference in &snapshot.events {
                    let (target, field) = self.field(entity, reference)?;
                    if target != Target::Word(status) || field_mask(field) & !mask != 0 {
                        return Err(self.error(
                            entity,
                            ErrorKind::Snapshot(format!(
                                "event {reference} is no field of {} its clear word acknowledges",
                                snapshot.status
                            )),
                        ));
                    }
                    if events & field_mask(field) != 0 {
                        return Err(self.error(entity, ErrorKind::Duplicate(reference.clone())));
                    }
                    events |= field_mask(field);
                }
                mask = events;
            }
            if mask == 0 {
                return Err(self.error(
                    entity,
                    ErrorKind::Snapshot("its clear word acknowledges no status field".to_owned()),
                ));
            }
            for field in status_word
                .fields()
                .iter()
                .filter(|field| field_mask(field) & mask != 0)
            {
                if !read(field.class) {
                    return Err(self.error(
                        entity,
                        ErrorKind::Snapshot(format!(
                            "status field {} has no side-effect-free readable value",
                            field.identity
                        )),
                    ));
                }
                if field_mask(field) & !mask != 0 {
                    return Err(self.error(
                        entity,
                        ErrorKind::Snapshot(format!(
                            "status field {} is acknowledged only in part",
                            field.identity
                        )),
                    ));
                }
            }
            // A write-trigger clear field acknowledges sampled bits only:
            // it lies wholly inside the sample or outside it.
            if let Some(field) = clear_word.fields().iter().find(|field| {
                field.class == FieldClass::WriteTrigger
                    && field_mask(field) & mask != 0
                    && field_mask(field) & !mask != 0
            }) {
                return Err(self.error(
                    entity,
                    ErrorKind::Snapshot(format!(
                        "clear field {} acknowledges bits no status field samples",
                        field.identity
                    )),
                ));
            }
            let consumed = self.consumed(entity, snapshot, clear)?;
            self.write_word(
                entity,
                Target::Word(clear),
                WordWrite::Acknowledge {
                    mask: mask | consumed,
                },
            )?;
            // The part holding the clear word acknowledges; it reads the
            // status word as its own or an observed one.
            let owner = self.owner(entity, Target::Word(clear), None)?;
            if !self.coverage.reads(owner, status) {
                return Err(self.error(
                    entity,
                    ErrorKind::SeveralOwners(format!(
                        "{} is neither held nor observed by the part holding {}",
                        snapshot.status, snapshot.clear
                    )),
                ));
            }
            self.take(entity, &[owner], true)?;
            self.clears.insert(clear);
            if let Some(Some(template)) = self.index.template_of.get(clear) {
                self.clear_templates.insert(*template);
            }
        }
        Ok(())
    }

    /// The bits of a snapshot's consumed fields: one-bit acknowledging
    /// fields of its clear word, one of which the acknowledgement writes
    /// beside the sample for an event the caller consumed elsewhere.
    fn consumed(
        &self,
        entity: &str,
        snapshot: &InterruptSnapshot,
        clear: usize,
    ) -> Result<u32, Error> {
        match (
            &snapshot.consumed_event,
            snapshot.consumed_fields.is_empty(),
        ) {
            (None, true) => return Ok(0),
            (Some(event), false) => self.context.description(entity, event)?,
            _ => {
                return Err(self.error(
                    entity,
                    ErrorKind::Snapshot(
                        "names consumed-event and consumed-fields together".to_owned(),
                    ),
                ));
            }
        }
        let mut bits = 0u32;
        for reference in &snapshot.consumed_fields {
            let (target, field) = self.field(entity, reference)?;
            if target != Target::Word(clear)
                || field.width != 1
                || !matches!(
                    field.class,
                    FieldClass::WriteOneToClear | FieldClass::WriteTrigger
                )
            {
                return Err(self.error(
                    entity,
                    ErrorKind::Snapshot(format!(
                        "consumed field {reference} is no one-bit acknowledging field of {}",
                        snapshot.clear
                    )),
                ));
            }
            if bits & field_mask(field) != 0 {
                return Err(self.error(entity, ErrorKind::Duplicate(reference.clone())));
            }
            bits |= field_mask(field);
        }
        Ok(bits)
    }

    fn reads(&mut self) -> Result<(), Error> {
        let file = self.file;
        let mut recorded = std::collections::BTreeSet::new();
        for record in &file.record_words {
            let entity = record.register.as_str();
            if !recorded.insert(entity) {
                return Err(self.error(
                    entity,
                    ErrorKind::Duplicate(format!("record word {entity}")),
                ));
            }
            if record.reason.trim().is_empty() {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction("a record word names its reason".to_owned()),
                ));
            }
            self.context.sources(entity, &record.sources)?;
            let target = self.register(entity, &record.register)?;
            self.reads_freely(entity, target)?;
            let word = self.word(entity, target)?;
            // A byte register's record is its byte, zero-extended.
            if !matches!(word.register.width, 8 | 32) {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(
                        "a record word is one 32-bit word or one byte register".to_owned(),
                    ),
                ));
            }
            for field in word.fields() {
                self.class(
                    entity,
                    field,
                    matches!(
                        field.class,
                        FieldClass::Plain
                            | FieldClass::ReadOnly
                            | FieldClass::ReadObserved
                            | FieldClass::SetOnly
                            | FieldClass::SelfClearing
                            | FieldClass::HardwareWritten
                            | FieldClass::DebugOnly
                    ),
                    "a record word",
                )?;
            }
            // Some owner reads the word: the method is of its read value.
            self.owner(entity, target, None)?;
        }
        for read in &file.partition_image_reads {
            let entity = read.name.as_str();
            self.begin(entity, &read.sources)?;
            let partition = file
                .partitions
                .iter()
                .position(|partition| partition.name == read.partition)
                .ok_or_else(|| {
                    self.error(
                        entity,
                        ErrorKind::Transaction(format!(
                            "partition {} is not declared",
                            read.partition
                        )),
                    )
                })?;
            for words in self.coverage.partition(partition) {
                for word in words.whole.iter().filter_map(|word| self.index.word(*word)) {
                    let readable = word.fields().iter().any(|field| field.class.readable());
                    if readable && !word.reads_freely() {
                        return Err(self.error(
                            entity,
                            ErrorKind::Register(format!(
                                "{} is a readable word of the image without a side-effect-free read",
                                word.identity()
                            )),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn reads_freely(&self, entity: &str, target: Target) -> Result<(), Error> {
        let word = self.word(entity, target)?;
        if !word.reads_freely() {
            return Err(self.error(
                entity,
                ErrorKind::Register(format!("{} has no side-effect-free read", word.identity())),
            ));
        }
        Ok(())
    }

    /// A write of the whole of `target` from zero that gives `written`
    /// its value.
    fn store(&mut self, entity: &str, target: Target, written: u32) -> Result<(), Error> {
        self.write_word(entity, target, WordWrite::Store { written })?;
        let owner = self.owner(entity, target, Some(written))?;
        self.take(entity, &[owner], true)
    }

    /// Qualify the word independently of set bits, then check every lane
    /// the write gives a value. A store's selected fields are checked by
    /// its input-specific validation; its remaining fields receive zero.
    fn write_word(&self, entity: &str, target: Target, write: WordWrite) -> Result<(), Error> {
        let word = self.word(entity, target)?;
        let on_clear = self.on_clear(target);
        let acknowledges = matches!(write, WordWrite::Acknowledge { .. })
            || (matches!(write, WordWrite::Image { .. }) && on_clear);
        if matches!(write, WordWrite::Acknowledge { .. })
            && (!word.storable()
                || word.fields().is_empty()
                || word.fields().iter().any(|field| {
                    !matches!(
                        field.class,
                        FieldClass::WriteOneToClear | FieldClass::WriteTrigger
                    )
                }))
        {
            return Err(self.error(
                entity,
                ErrorKind::Snapshot(
                    "its clear word holds only write-one-to-clear or write-trigger fields"
                        .to_owned(),
                ),
            ));
        }
        if (on_clear || word.has(FieldClass::WriteOneToClear)) && !acknowledges {
            return Err(self.error(
                entity,
                ErrorKind::WriteOneToClear(
                    "a snapshot's clear word or a word of write-one-to-clear fields is written only by an acknowledgement"
                        .to_owned(),
                ),
            ));
        }
        if !word.storable() {
            return Err(self.error(
                entity,
                ErrorKind::Register(
                    "a FIFO port or command register is not written by a transaction".to_owned(),
                ),
            ));
        }
        let observed = word.has(FieldClass::ReadObserved);
        if observed && !matches!(write, WordWrite::Zero) {
            return Err(self.error(
                entity,
                ErrorKind::Register(
                    "a read-observed word admits only a sourced zero-register-write".to_owned(),
                ),
            ));
        }
        if let WordWrite::Store { .. } = write {
            if let Some(reason) = word.unstorable() {
                return Err(self.error(entity, ErrorKind::Register(reason.to_owned())));
            }
            if word.template.data_port.is_some() {
                return Err(self.error(
                    entity,
                    ErrorKind::Register("a data port's word takes no write from zero".to_owned()),
                ));
            }
        }
        let fixed = match write {
            WordWrite::Image { value, whole_word } => Some((value, whole_word)),
            WordWrite::Sequence { value } => Some((value, true)),
            _ => None,
        };
        for field in word.fields() {
            let mask = field_mask(field);
            if observed && matches!(write, WordWrite::Zero) {
                self.class(
                    entity,
                    field,
                    matches!(field.class, FieldClass::ReadObserved | FieldClass::ReadOnly)
                        || stored(field.class),
                    "a complete zero publication",
                )?;
            }
            if let Some((value, whole_word)) = fixed
                && value & mask != 0
            {
                let allowed = imaged(field.class)
                    || (field.class == FieldClass::WriteOneToClear && acknowledges)
                    || (field.class == FieldClass::ReadOnly && whole_word);
                self.class(entity, field, allowed, "a fixed write")?;
            }
            let values = match write {
                WordWrite::Zero => Values::Fixed(0),
                WordWrite::Store { written, .. } if mask & written != 0 => continue,
                WordWrite::Store { .. } => Values::Fixed(0),
                WordWrite::Image { value, .. } | WordWrite::Sequence { value } => {
                    Values::Fixed((value & mask).checked_shr(field.low).unwrap_or(0))
                }
                WordWrite::Acknowledge { mask: sampled } => {
                    Values::Bits((sampled & mask).checked_shr(field.low).unwrap_or(0))
                }
            };
            self.constrained(entity, word, field, values)?;
        }
        if !(word.fields().iter().any(|field| field.class.written())
            || (observed && matches!(write, WordWrite::Zero)))
        {
            return Err(self.error(
                entity,
                ErrorKind::Register("the word has no field a write gives a value".to_owned()),
            ));
        }
        if let Some((value, whole_word)) = fixed {
            if value & !word.declared() != 0 && !whole_word {
                return Err(self.error(
                    entity,
                    ErrorKind::Register(
                        "sets bits no field declares; only a whole-word image does".to_owned(),
                    ),
                ));
            }
            self.no_dma(entity, target, value)?;
        }
        Ok(())
    }

    /// The lanes of each word's hardware pointer fields: set only from a
    /// typed reference's image.
    fn pointers(&mut self) {
        for (position, word) in self.index.words.iter().enumerate() {
            for field in word.fields() {
                if word
                    .source(field)
                    .is_some_and(|source| source.pointer.is_some())
                {
                    *self.pointers.entry(position).or_default() |= field_mask(field);
                }
            }
        }
    }

    fn stores(&mut self) -> Result<(), Error> {
        let file = self.file;
        for write in &file.zero_based_field_writes {
            let entity = write.name.as_str();
            self.begin(entity, &write.sources)?;
            let mut register = None;
            let mut mask = 0u32;
            if write.fields.is_empty() {
                return Err(self.error(entity, ErrorKind::EmptySelection));
            }
            for reference in &write.fields {
                let (target, field) = self.field(entity, reference)?;
                self.class(entity, field, stored(field.class), "a write from zero")?;
                self.same(entity, &mut register, target)?;
                let bits = field_mask(field);
                if mask & bits != 0 {
                    return Err(self.error(entity, ErrorKind::Duplicate(reference.clone())));
                }
                // A pointer field takes the image of a typed reference; an
                // address field only its own full-register write.
                let pointers = self.lanes(target);
                if pointers.any & bits != 0 && pointers.all & bits != bits {
                    return Err(self.error(
                        entity,
                        ErrorKind::Pointer(format!(
                            "{reference} is a pointer field of some elements only"
                        )),
                    ));
                }
                // The field's own type: its named values when it takes
                // only those; no bounded integer type exists.
                let word = self.word(entity, target)?;
                if word
                    .constraint(field)
                    .is_some_and(|constraint| constraint.range.is_some())
                {
                    return Err(self.error(
                        entity,
                        ErrorKind::WriteConstraint(format!(
                            "{} has a write range narrower than its width",
                            field.identity
                        )),
                    ));
                }
                mask |= bits;
            }
            if let Some(target) = register {
                let whole = self.whole_word_bits(entity, target, mask, &write.whole_word_bits)?;
                self.store(entity, target, mask | whole)?;
            }
        }
        for write in &file.zero_register_writes {
            let entity = write.name.as_str();
            self.begin(entity, &write.sources)?;
            let target = self.register(entity, &write.register)?;
            // Any other word's zero is a write of its fields, which the
            // generic accessors express: the record would render nothing.
            if !self.word(entity, target)?.has(FieldClass::ReadObserved) {
                return Err(self.error(
                    entity,
                    ErrorKind::Register(
                        "a zero register write publishes zero to a read-observed word".to_owned(),
                    ),
                ));
            }
            self.element(entity, &write.register)?;
            self.write_word(entity, target, WordWrite::Zero)?;
            let owner = self.owner(entity, target, Some(0))?;
            self.take(entity, &[owner], true)?;
        }
        Ok(())
    }

    /// The fixed ones a write from zero stores outside its fields: sourced,
    /// nonzero and only bits of `read-only` fields or of no field, each
    /// such field's value one its constraint admits.
    fn whole_word_bits(
        &self,
        entity: &str,
        target: Target,
        fields: u32,
        bits: &WholeWordBits,
    ) -> Result<u32, Error> {
        self.context.sources(entity, &bits.sources)?;
        if bits.value == 0 || bits.value & fields != 0 {
            return Err(self.error(
                entity,
                ErrorKind::Transaction(
                    "whole-word bits are nonzero and lie outside its fields".to_owned(),
                ),
            ));
        }
        let word = self.word(entity, target)?;
        for field in word
            .fields()
            .iter()
            .filter(|field| field_mask(field) & bits.value != 0)
        {
            self.class(
                entity,
                field,
                field.class == FieldClass::ReadOnly,
                "whole-word bits",
            )?;
            let value = (bits.value & field_mask(field))
                .checked_shr(field.low)
                .unwrap_or(0);
            self.constrained(entity, word, field, Values::Fixed(value))?;
        }
        self.no_dma(entity, target, bits.value)?;
        Ok(bits.value)
    }

    /// Every field of one transaction is in one register.
    fn same(
        &self,
        entity: &str,
        register: &mut Option<Target>,
        target: Target,
    ) -> Result<(), Error> {
        match register {
            Some(first) if *first != target => Err(self.error(
                entity,
                ErrorKind::Transaction("its fields lie in more than one register".to_owned()),
            )),
            Some(_) => Ok(()),
            None => {
                *register = Some(target);
                Ok(())
            }
        }
    }

    fn images(&mut self) -> Result<(), Error> {
        let file = self.file;
        for image in &file.fixed_register_images {
            let entity = image.name.as_str();
            self.begin(entity, &image.sources)?;
            let target = self.register(entity, &image.register)?;
            self.write_word(
                entity,
                target,
                WordWrite::Image {
                    value: image.value,
                    whole_word: image.whole_word,
                },
            )?;
            let mut owners = vec![self.owner(entity, target, Some(image.value))?];
            match (self.on_clear(target), &image.masks, &image.consumed_event) {
                (true, Some(masks), None) => {
                    let enable = self.element(entity, masks)?;
                    let word = self.word(entity, Target::Word(enable))?;
                    if Target::Word(enable) == target
                        || self.clears.contains(&enable)
                        || !word.modifiable()
                        || !word.fields().iter().all(|field| modified(field.class))
                    {
                        return Err(self.error(
                            entity,
                            ErrorKind::WriteOneToClear(format!(
                                "masks {masks}, which is no enable word to modify"
                            )),
                        ));
                    }
                    // The enable fields of every acknowledged event are
                    // cleared first: each acknowledged bit is one.
                    if image.value & !word.declared() != 0 {
                        return Err(self.error(
                            entity,
                            ErrorKind::WriteOneToClear(format!(
                                "acknowledges bits 0x{:08x}, which no enable field of {masks} masks",
                                image.value & !word.declared()
                            )),
                        ));
                    }
                    self.no_dma(entity, Target::Word(enable), image.value)?;
                    if let Some(field) = word.fields().iter().find(|field| {
                        field_mask(field) & image.value != 0 && word.constraint(field).is_some()
                    }) {
                        return Err(self.error(
                            entity,
                            ErrorKind::WriteConstraint(format!(
                                "{} has a write constraint the mask does not check",
                                field.identity
                            )),
                        ));
                    }
                    owners.push(self.owner(entity, Target::Word(enable), None)?);
                }
                (true, None, Some(event)) => self.context.description(entity, event)?,
                (true, _, _) => {
                    return Err(self.error(
                        entity,
                        ErrorKind::WriteOneToClear(
                            "acknowledges events no sample captured: names one proof, masks or consumed-event"
                                .to_owned(),
                        ),
                    ));
                }
                (false, None, None) => {}
                (false, _, _) => {
                    return Err(self.error(
                        entity,
                        ErrorKind::WriteOneToClear(
                            "names a proof, but its word is no snapshot's clear word".to_owned(),
                        ),
                    ));
                }
            }
            self.take_image(entity, &owners, image)?;
        }
        for sequence in &file.fixed_register_sequences {
            let entity = sequence.name.as_str();
            self.begin(entity, &sequence.sources)?;
            if sequence.steps.is_empty() {
                return Err(self.error(entity, ErrorKind::EmptySelection));
            }
            let mut owners = Vec::new();
            for step in &sequence.steps {
                for word in self.step_words(entity, step, sequence.steps.len())? {
                    self.write_word(
                        entity,
                        Target::Word(word),
                        WordWrite::Sequence { value: step.value },
                    )?;
                    owners.push(self.owner(entity, Target::Word(word), Some(step.value))?);
                }
            }
            self.take(entity, &owners, true)?;
        }
        Ok(())
    }

    /// A read-modify-write of `target` replacing `bits`, none of a field
    /// with a write constraint.
    fn modify(&mut self, entity: &str, target: Target, bits: u32) -> Result<(), Error> {
        let word = self.word(entity, target)?;
        if !word.modifiable() || self.on_clear(target) {
            return Err(self.error(
                entity,
                ErrorKind::Register(format!(
                    "{} is not read, modified and written back",
                    word.identity()
                )),
            ));
        }
        let mut covered = 0;
        for field in word.fields() {
            if bits & field_mask(field) != 0 {
                self.class(entity, field, modified(field.class), "a read-modify-write")?;
                if word.constraint(field).is_some() {
                    return Err(self.error(
                        entity,
                        ErrorKind::WriteConstraint(format!(
                            "{} admits fewer values than the transaction writes",
                            field.identity
                        )),
                    ));
                }
                covered |= field_mask(field);
            }
        }
        if bits & !covered != 0 || bits == 0 {
            return Err(self.error(
                entity,
                ErrorKind::Register("modifies no bits or bits no field declares".to_owned()),
            ));
        }
        let owner = self.owner(entity, target, Some(bits))?;
        self.take(entity, &[owner], true)
    }

    /// The projections' register and the bits they replace.
    fn projections(&self, entity: &str, fields: &[Projection]) -> Result<Projected, Error> {
        let mut register = None;
        let mut mask = 0u32;
        let mut single = false;
        for projection in fields {
            let (target, field) = self.field(entity, &projection.field)?;
            self.same(entity, &mut register, target)?;
            if mask & field_mask(field) != 0 {
                return Err(self.error(entity, ErrorKind::Duplicate(projection.field.clone())));
            }
            let offset = u32::from(projection.source_bit_offset);
            if offset.saturating_add(field.width) > 32 {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(format!(
                        "{} takes input bits above bit 31",
                        projection.field
                    )),
                ));
            }
            mask |= field_mask(field);
            single = fields.len() == 1 && offset == 0;
        }
        let target = register.ok_or_else(|| self.error(entity, ErrorKind::EmptySelection))?;
        Ok(Projected {
            target,
            mask,
            single,
        })
    }

    /// Whether `projected` is the one pointer field of every word it
    /// reaches, from input bit zero: it takes a typed reference's image.
    fn pointer_image(&self, projected: &Projected) -> bool {
        projected.single && self.lanes(projected.target).all & projected.mask == projected.mask
    }

    /// A field-or-modify ORs a typed reference's image into a fresh read
    /// of one pointer field; any other OR of fields is a write the generic
    /// accessors express.
    fn modifies(&mut self) -> Result<(), Error> {
        let file = self.file;
        for modify in &file.field_or_modifies {
            let entity = modify.name.as_str();
            self.begin(entity, &modify.sources)?;
            let projected = self.projections(entity, &modify.fields)?;
            if !self.pointer_image(&projected) {
                return Err(self.error(
                    entity,
                    ErrorKind::Pointer(
                        "a field-or-modify takes one pointer field of every word it reaches, from input bit zero"
                            .to_owned(),
                    ),
                ));
            }
            self.modify(entity, projected.target, projected.mask)?;
        }
        Ok(())
    }
}
