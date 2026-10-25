//! Value domains and transactions: each names fields and registers of the
//! root, writes only what their classes allow and reaches one part.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    Context, Error, ErrorKind, Holder, Index, PartId, PolicyFile, Projection, Target, Word,
    field_mask, lower_snake, ownership::Coverage, upper_camel, upper_snake,
};
use crate::chip_model::{ChipModel, ExpandedField, FieldClass};

/// What a domain is and, for a register-bound one, its register.
#[derive(Clone, Copy)]
enum Domain {
    Flag,
    Enum,
    Bounded { max: u32 },
    Bound(Target),
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
    matches!(class, FieldClass::Plain | FieldClass::HardwareWritten)
}

/// A field a write from zero may give a value.
fn stored(class: FieldClass) -> bool {
    matches!(
        class,
        FieldClass::Plain
            | FieldClass::WriteOnly
            | FieldClass::WriteTrigger
            | FieldClass::HardwareWritten
    )
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

struct Checker<'a, 'm> {
    context: Context<'a>,
    index: &'a Index<'m>,
    coverage: &'a Coverage,
    file: &'a PolicyFile,
    model: &'a ChipModel,
    domains: BTreeMap<&'a str, Domain>,
    templates: Vec<TemplateOwner>,
    /// The clear words of the interrupt snapshots.
    clears: BTreeSet<usize>,
    /// The templates that repeat a clear word.
    clear_templates: BTreeSet<usize>,
    pointers: BTreeSet<(Target, usize)>,
    names: BTreeSet<&'a str>,
    owners: BTreeMap<String, PartId>,
}

/// Check every domain and transaction; the part each transaction reaches.
pub(super) fn check(
    context: Context<'_>,
    index: &Index<'_>,
    coverage: &Coverage,
    file: &PolicyFile,
    model: &ChipModel,
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
        model,
        domains: BTreeMap::new(),
        templates,
        clears: BTreeSet::new(),
        clear_templates: BTreeSet::new(),
        pointers: BTreeSet::new(),
        names: BTreeSet::new(),
        owners: BTreeMap::new(),
    };
    checker.domains()?;
    checker.snapshots()?;
    checker.reads()?;
    checker.pointers()?;
    checker.stores()?;
    checker.images()?;
    checker.modifies()?;
    Ok(checker.owners)
}

impl<'a> Checker<'a, '_> {
    fn error(&self, entity: impl Into<String>, kind: ErrorKind) -> Error {
        self.context.error(entity, kind)
    }

    fn declare(&mut self, name: &'a str, description: &str, domain: Domain) -> Result<(), Error> {
        if !upper_camel(name) {
            return Err(self.error(
                name,
                ErrorKind::InvalidName(format!("domain {name:?} is not UpperCamelCase")),
            ));
        }
        if self.domains.insert(name, domain).is_some() {
            return Err(self.error(name, ErrorKind::Duplicate(format!("domain {name}"))));
        }
        self.context.description(name, description)
    }

    fn domains(&mut self) -> Result<(), Error> {
        let file = self.file;
        for domain in &file.flag_domains {
            self.declare(&domain.name, &domain.description, Domain::Flag)?;
        }
        for domain in &file.enum_domains {
            self.declare(&domain.name, &domain.description, Domain::Enum)?;
        }
        for domain in &file.bounded_domains {
            self.declare(
                &domain.name,
                &domain.description,
                Domain::Bounded { max: domain.max },
            )?;
        }
        for domain in &file.opaque_domains {
            let target = self.register(&domain.name, &domain.register)?;
            self.declare(&domain.name, &domain.description, Domain::Bound(target))?;
        }
        for domain in &file.bitwise_composed_domains {
            let target = self.register(&domain.name, &domain.register)?;
            self.declare(&domain.name, &domain.description, Domain::Bound(target))?;
        }
        for (domain, values, named) in file
            .flag_domains
            .iter()
            .map(|domain| {
                (
                    &domain.name,
                    &domain.values,
                    upper_snake as fn(&str) -> bool,
                )
            })
            .chain(file.enum_domains.iter().map(|domain| {
                (
                    &domain.name,
                    &domain.values,
                    upper_camel as fn(&str) -> bool,
                )
            }))
        {
            if values.is_empty() {
                return Err(self.error(domain, ErrorKind::Domain("has no value".to_owned())));
            }
            let mut names = BTreeSet::new();
            let mut numbers = BTreeSet::new();
            for value in values {
                let entity = format!("{domain}::{}", value.name);
                if !named(&value.name) {
                    return Err(self.error(
                        &entity,
                        ErrorKind::InvalidName(format!("value {:?}", value.name)),
                    ));
                }
                if !names.insert(&value.name) || !numbers.insert(value.value) {
                    return Err(
                        self.error(&entity, ErrorKind::Duplicate(format!("value {entity}")))
                    );
                }
                self.context.description(&entity, &value.description)?;
                self.context.sources(&entity, &value.sources)?;
            }
        }
        for domain in &file.bounded_domains {
            if domain.min > domain.max {
                return Err(self.error(
                    &domain.name,
                    ErrorKind::Domain("min is above max".to_owned()),
                ));
            }
            self.context.sources(&domain.name, &domain.sources)?;
        }
        for domain in &file.opaque_domains {
            self.context.sources(&domain.name, &domain.sources)?;
        }
        for domain in &file.bitwise_composed_domains {
            let mut names = BTreeSet::new();
            if domain.arguments.is_empty() || domain.output_mask == 0 {
                return Err(self.error(
                    &domain.name,
                    ErrorKind::Domain("needs arguments and a nonzero output mask".to_owned()),
                ));
            }
            for argument in &domain.arguments {
                if !lower_snake(&argument.name)
                    || !names.insert(&argument.name)
                    || argument.left_shift >= 32
                    || argument.right_shift >= 32
                {
                    return Err(self.error(
                        &domain.name,
                        ErrorKind::Domain(format!(
                            "argument {:?} needs a distinct lower_snake_case name and shifts below 32",
                            argument.name
                        )),
                    ));
                }
            }
            self.context.sources(&domain.name, &domain.sources)?;
        }
        Ok(())
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

    fn word(&self, entity: &str, target: Target) -> Result<&Word<'_>, Error> {
        self.index
            .first(target)
            .ok_or_else(|| self.error(entity, ErrorKind::UnknownRegister(entity.to_owned())))
    }

    fn on_clear(&self, target: Target) -> bool {
        match target {
            Target::Word(word) => self.clears.contains(&word),
            Target::Template(template) => self.clear_templates.contains(&template),
        }
    }

    /// The part whose words `target` are; `bits` are the bits a write
    /// sets, which choose the holder of a split word.
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

    /// A domain that exists and, when bound to a register, is used there.
    fn domain(&self, entity: &str, name: &str, target: Target) -> Result<Domain, Error> {
        let domain = self.domains.get(name).copied().ok_or_else(|| {
            self.error(entity, ErrorKind::Domain(format!("{name} is not declared")))
        })?;
        if let Domain::Bound(bound) = domain
            && bound != target
        {
            return Err(self.error(
                entity,
                ErrorKind::Domain(format!("{name} belongs to another register")),
            ));
        }
        Ok(domain)
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
            if !status_word.reads_freely() || status_word.has(FieldClass::WriteOneToClear) {
                return Err(self.error(
                    entity,
                    ErrorKind::Snapshot(
                        "its status word reads without a side effect and is acknowledged elsewhere"
                            .to_owned(),
                    ),
                ));
            }
            if !clear_word.storable()
                || clear_word.fields().is_empty()
                || clear_word
                    .fields()
                    .iter()
                    .any(|field| field.class != FieldClass::WriteOneToClear)
            {
                return Err(self.error(
                    entity,
                    ErrorKind::Snapshot(
                        "its clear word holds only write-one-to-clear fields".to_owned(),
                    ),
                ));
            }
            let mask = status_word.declared() & clear_word.declared();
            if mask == 0 {
                return Err(self.error(
                    entity,
                    ErrorKind::Snapshot("its clear word acknowledges no status field".to_owned()),
                ));
            }
            if let Some(field) = status_word
                .fields()
                .iter()
                .find(|field| field_mask(field) & mask != 0 && field_mask(field) & !mask != 0)
            {
                return Err(self.error(
                    entity,
                    ErrorKind::Snapshot(format!(
                        "status field {} is acknowledged only in part",
                        field.identity
                    )),
                ));
            }
            let owners = [
                self.owner(entity, Target::Word(status), None)?,
                self.owner(entity, Target::Word(clear), None)?,
            ];
            self.take(entity, &owners, true)?;
            self.clears.insert(clear);
            if let Some(Some(template)) = self.index.template_of.get(clear) {
                self.clear_templates.insert(*template);
            }
        }
        Ok(())
    }

    fn reads(&mut self) -> Result<(), Error> {
        let file = self.file;
        let reads = file
            .field_reads
            .iter()
            .map(|read| (&read.name, &read.field, None, &read.sources))
            .chain(
                file.full_register_reads
                    .iter()
                    .map(|read| (&read.name, &read.field, Some(&read.domain), &read.sources)),
            );
        for (name, reference, domain, sources) in reads {
            let entity = name.as_str();
            self.begin(entity, sources)?;
            let (target, field) = self.field(entity, reference)?;
            self.class(entity, field, read(field.class), "a read")?;
            if !self.word(entity, target)?.reads_freely() {
                return Err(self.error(
                    entity,
                    ErrorKind::Register(format!("{reference} has no side-effect-free read")),
                ));
            }
            if let Some(domain) = domain {
                self.domain(entity, domain, target)?;
            }
            let owner = self.owner(entity, target, None)?;
            self.take(entity, &[owner], false)?;
        }
        for read in &file.partition_image_reads {
            self.begin(&read.name, &read.sources)?;
            if !file
                .partitions
                .iter()
                .any(|partition| partition.name == read.partition)
            {
                return Err(self.error(
                    &read.name,
                    ErrorKind::Transaction(format!("partition {} is not declared", read.partition)),
                ));
            }
        }
        Ok(())
    }

    /// A write of the whole of `target` that sets `bits`.
    fn store(&mut self, entity: &str, target: Target, bits: Option<u32>) -> Result<(), Error> {
        if self.on_clear(target) {
            return Err(self.error(
                entity,
                ErrorKind::WriteOneToClear(
                    "a snapshot's clear word is written only by its acknowledgement".to_owned(),
                ),
            ));
        }
        if !self.word(entity, target)?.storable() {
            return Err(self.error(
                entity,
                ErrorKind::Register(
                    "a FIFO port or command register is not written by a transaction".to_owned(),
                ),
            ));
        }
        let owner = self.owner(entity, target, bits)?;
        self.take(entity, &[owner], true)
    }

    fn pointers(&mut self) -> Result<(), Error> {
        let file = self.file;
        for pointer in &file.pointer_fields {
            let entity = pointer.name.as_str();
            self.begin(entity, &pointer.sources)?;
            let (target, field) = self.field(entity, &pointer.field)?;
            self.class(
                entity,
                field,
                matches!(
                    field.class,
                    FieldClass::Plain | FieldClass::WriteOnly | FieldClass::HardwareWritten
                ),
                "a pointer",
            )?;
            let mask = field_mask(field);
            let (_, position) = self.index.field(&pointer.field).ok_or_else(|| {
                self.error(entity, ErrorKind::UnknownField(pointer.field.clone()))
            })?;
            let layout = self
                .model
                .layouts()
                .iter()
                .find(|layout| layout.name == pointer.points_to)
                .ok_or_else(|| {
                    self.error(
                        entity,
                        ErrorKind::Transaction(format!("no layout is {}", pointer.points_to)),
                    )
                })?;
            if pointer.member_offset >= layout.size {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction("member-offset lies outside the layout".to_owned()),
                ));
            }
            if !self.pointers.insert((target, position)) {
                return Err(self.error(
                    entity,
                    ErrorKind::Duplicate(format!("pointer field {}", pointer.field)),
                ));
            }
            self.store(entity, target, Some(mask))?;
        }
        Ok(())
    }

    fn stores(&mut self) -> Result<(), Error> {
        let file = self.file;
        for write in &file.full_register_writes {
            let entity = write.name.as_str();
            self.begin(entity, &write.sources)?;
            let (target, field) = self.field(entity, &write.field)?;
            self.class(entity, field, stored(field.class), "a write from zero")?;
            let mask = field_mask(field);
            match (&write.domain, &write.points_to, &write.placement) {
                (Some(domain), None, None) => {
                    self.domain(entity, domain, target)?;
                }
                (None, Some(layout), None) => {
                    if !self
                        .model
                        .layouts()
                        .iter()
                        .any(|known| known.name == *layout)
                    {
                        return Err(self.error(
                            entity,
                            ErrorKind::Transaction(format!("no layout is {layout}")),
                        ));
                    }
                }
                (None, None, Some(region)) => {
                    if !self
                        .model
                        .ram_regions()
                        .iter()
                        .any(|known| known.name == *region)
                    {
                        return Err(self.error(
                            entity,
                            ErrorKind::Transaction(format!("no RAM region is {region}")),
                        ));
                    }
                }
                _ => {
                    return Err(self.error(
                        entity,
                        ErrorKind::Transaction(
                            "takes exactly one of domain, points-to and placement".to_owned(),
                        ),
                    ));
                }
            }
            self.store(entity, target, Some(mask))?;
        }
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
                if mask & field_mask(field) != 0 {
                    return Err(self.error(entity, ErrorKind::Duplicate(reference.clone())));
                }
                mask |= field_mask(field);
            }
            if let Some(target) = register {
                self.store(entity, target, Some(mask))?;
            }
        }
        for write in &file.sampled_bit_zero_writes {
            let entity = write.name.as_str();
            self.begin(entity, &write.sources)?;
            let (target, field) = self.field(entity, &write.field)?;
            self.class(
                entity,
                field,
                field.width == 1 && read(field.class) && stored(field.class),
                "a sampled one-bit write",
            )?;
            let mask = field_mask(field);
            self.store(entity, target, Some(mask))?;
        }
        for write in &file.zero_register_writes {
            let entity = write.name.as_str();
            self.begin(entity, &write.sources)?;
            let target = self.register(entity, &write.register)?;
            self.store(entity, target, None)?;
        }
        Ok(())
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

    /// The bits `value` sets in `target` may be set by a fixed image.
    fn image(
        &self,
        entity: &str,
        target: Target,
        value: u32,
        whole_word: bool,
    ) -> Result<(), Error> {
        let word = self.word(entity, target)?;
        if !word.storable() {
            return Err(self.error(
                entity,
                ErrorKind::Register(
                    "a FIFO port or command register is not written by an image".to_owned(),
                ),
            ));
        }
        let on_clear = self.on_clear(target);
        for field in word.fields() {
            if value & field_mask(field) == 0 {
                continue;
            }
            let allowed = imaged(field.class)
                || (field.class == FieldClass::WriteOneToClear && on_clear)
                || (field.class == FieldClass::ReadOnly && whole_word);
            self.class(entity, field, allowed, "an image")?;
        }
        if value & !word.declared() != 0 && !whole_word {
            return Err(self.error(
                entity,
                ErrorKind::Register(
                    "sets bits no field declares; only a whole-word image does".to_owned(),
                ),
            ));
        }
        Ok(())
    }

    fn images(&mut self) -> Result<(), Error> {
        let file = self.file;
        for image in &file.fixed_register_images {
            let entity = image.name.as_str();
            self.begin(entity, &image.sources)?;
            let target = self.register(entity, &image.register)?;
            self.image(entity, target, image.value, image.whole_word)?;
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
            self.take(entity, &owners, true)?;
        }
        for sequence in &file.fixed_register_sequences {
            let entity = sequence.name.as_str();
            self.begin(entity, &sequence.sources)?;
            if sequence.steps.is_empty() {
                return Err(self.error(entity, ErrorKind::EmptySelection));
            }
            let mut owners = Vec::new();
            for step in &sequence.steps {
                let word = self.element(entity, &step.register)?;
                if self.clears.contains(&word) {
                    return Err(self.error(
                        entity,
                        ErrorKind::WriteOneToClear(
                            "a snapshot's clear word is written only by its acknowledgement"
                                .to_owned(),
                        ),
                    ));
                }
                self.image(entity, Target::Word(word), step.value, step.whole_word)?;
                owners.push(self.owner(entity, Target::Word(word), Some(step.value))?);
            }
            self.take(entity, &owners, true)?;
        }
        Ok(())
    }

    /// A read-modify-write of `target` replacing `bits`.
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
    fn projections(&self, entity: &str, fields: &[Projection]) -> Result<(Target, u32), Error> {
        let mut register = None;
        let mut mask = 0u32;
        for projection in fields {
            let (target, field) = self.field(entity, &projection.field)?;
            self.same(entity, &mut register, target)?;
            if mask & field_mask(field) != 0 {
                return Err(self.error(entity, ErrorKind::Duplicate(projection.field.clone())));
            }
            if u32::from(projection.source_bit_offset).saturating_add(field.width) > 32 {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(format!(
                        "{} takes input bits above bit 31",
                        projection.field
                    )),
                ));
            }
            mask |= field_mask(field);
        }
        register
            .map(|target| (target, mask))
            .ok_or_else(|| self.error(entity, ErrorKind::EmptySelection))
    }

    fn modifies(&mut self) -> Result<(), Error> {
        let file = self.file;
        for modify in &file.masked_register_modifies {
            let entity = modify.name.as_str();
            self.begin(entity, &modify.sources)?;
            let target = self.register(entity, &modify.register)?;
            if modify.input_mask & modify.set_mask != 0
                || modify.preserve_mask & (modify.input_mask | modify.set_mask) != 0
            {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction("preserve, input and set masks are disjoint".to_owned()),
                ));
            }
            self.domain(entity, &modify.domain, target)?;
            self.modify(entity, target, modify.input_mask | modify.set_mask)?;
        }
        let bitwise = file
            .field_or_modifies
            .iter()
            .map(|modify| (modify, true))
            .chain(
                file.field_clear_modifies
                    .iter()
                    .map(|modify| (modify, false)),
            );
        for (modify, or) in bitwise {
            let entity = modify.name.as_str();
            self.begin(entity, &modify.sources)?;
            let (target, mask) = self.projections(entity, &modify.fields)?;
            let pointer = or
                && matches!(modify.fields.as_slice(), [projection]
                    if projection.source_bit_offset == 0
                    && self.index.field(&projection.field).is_some_and(|field| self.pointers.contains(&field)));
            self.input(
                entity,
                target,
                modify.domain.as_deref(),
                modify.value,
                pointer,
            )?;
            self.modify(entity, target, mask)?;
        }
        for modify in &file.field_replace_modifies {
            let entity = modify.name.as_str();
            self.begin(entity, &modify.sources)?;
            let (target, mask) = self.projections(entity, &modify.fields)?;
            self.input(
                entity,
                target,
                modify.domain.as_deref(),
                modify.value,
                false,
            )?;
            if modify.input_transform.is_some_and(|transform| {
                transform.wrapping_add != 0 && transform.wrapping_subtract != 0
            }) {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(
                        "an input transform adds or subtracts, not both".to_owned(),
                    ),
                ));
            }
            self.modify(entity, target, mask)?;
        }
        for modify in &file.indexed_bit_set_modifies {
            let entity = modify.name.as_str();
            self.begin(entity, &modify.sources)?;
            let (target, field) = self.field(entity, &modify.field)?;
            let mask = field_mask(field);
            let width = field.width;
            match self.domain(entity, &modify.domain, target)? {
                Domain::Bounded { max } if max < width => {}
                _ => {
                    return Err(self.error(
                        entity,
                        ErrorKind::Domain(format!(
                            "{} is no bounded domain of indices below {width}",
                            modify.domain
                        )),
                    ));
                }
            }
            self.modify(entity, target, mask)?;
        }
        Ok(())
    }

    /// A modify's input: exactly one of a domain and a fixed value, or
    /// neither for an OR of one pointer field from bit zero.
    fn input(
        &self,
        entity: &str,
        target: Target,
        domain: Option<&str>,
        value: Option<u32>,
        pointer: bool,
    ) -> Result<(), Error> {
        match (domain, value) {
            (Some(domain), None) => self.domain(entity, domain, target).map(|_| ()),
            (None, Some(_)) => Ok(()),
            (None, None) if pointer => Ok(()),
            _ => Err(self.error(
                entity,
                ErrorKind::Transaction(
                    "takes exactly one of domain and value, or neither for one pointer field"
                        .to_owned(),
                ),
            )),
        }
    }
}
