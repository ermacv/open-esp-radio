//! Value domains and transactions: each names fields and registers of the
//! root, writes only what their classes, write constraints and DMA address
//! fields allow and reaches one part.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    BitwiseComposedDomain, BoundedDomain, Constraint, Context, DomainValue, EnumDomain, Error,
    ErrorKind, FlagDomain, Holder, Index, InputTransform, PartId, PointerEncoding, PolicyFile,
    Projection, Target, Word, field_mask, field_ones, lower_snake, ownership::Coverage, stored,
    upper_camel, upper_snake,
};
use crate::chip_model::{ChipModel, ExpandedField, FieldClass};

/// A declared domain; a register-bound one names its register.
#[derive(Clone, Copy)]
enum Domain<'a> {
    Flag(&'a FlagDomain),
    Enum(&'a EnumDomain),
    Bounded(&'a BoundedDomain),
    Opaque(Target),
    Composed(&'a BitwiseComposedDomain, Target),
}

impl<'a> Domain<'a> {
    fn bound(self) -> Option<Target> {
        match self {
            Self::Opaque(target) | Self::Composed(_, target) => Some(target),
            _ => None,
        }
    }

    /// Every bit a value of the domain may set.
    fn bits(self) -> u32 {
        let or = |values: &[DomainValue]| values.iter().fold(0, |mask, value| mask | value.value);
        match self {
            Self::Flag(domain) => or(&domain.values),
            Self::Enum(domain) => or(&domain.values),
            Self::Bounded(domain) => ones_through(domain.max),
            Self::Opaque(_) => u32::MAX,
            Self::Composed(domain, _) => domain.output_mask,
        }
    }

    /// The domain's values, as a write constraint checks them.
    fn values(self) -> Values<'a> {
        match self {
            Self::Enum(domain) => Values::List(&domain.values),
            Self::Bounded(domain) => Values::Range(domain.min, domain.max),
            Self::Flag(_) | Self::Opaque(_) | Self::Composed(..) => Values::Any,
        }
    }
}

/// The values a transaction may give one field.
#[derive(Clone, Copy)]
enum Values<'a> {
    Fixed(u32),
    Range(u32, u32),
    List(&'a [DomainValue]),
    /// Any subset of these bits, including zero for unacknowledged events.
    Bits(u32),
    /// Unknown to the loader: a combination of flags, an opaque or composed
    /// word, an address or a transformed input.
    Any,
}

impl Values<'_> {
    /// Every value is one `constraint` admits.
    fn within(self, constraint: Option<Constraint<'_>>) -> bool {
        let Some(constraint) = constraint else {
            return true;
        };
        match self {
            Self::Fixed(value) => constraint.admits(value),
            Self::Range(min, max) => constraint.admits_range(min, max),
            Self::List(values) => values.iter().all(|value| constraint.admits(value.value)),
            Self::Bits(mask) => constraint.admits_bits(mask),
            Self::Any => false,
        }
    }
}

/// Every whole-word write uses the same class, W1C and constraint checks.
/// Fixed writes have a sourced whole-word exception; only images may
/// carry an acknowledgement proof. A sequence never acknowledges events.
#[derive(Clone, Copy)]
enum WordWrite {
    Store { written: u32, whole: bool },
    Image { value: u32, whole_word: bool },
    Sequence { value: u32, whole_word: bool },
    Acknowledge { mask: u32 },
}

/// The ones from bit zero through the highest one of `value`.
fn ones_through(value: u32) -> u32 {
    u32::MAX.checked_shr(value.leading_zeros()).unwrap_or(0)
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

/// The lanes of DMA address fields of the words a target reaches: set in
/// any of them, and set in all of them.
#[derive(Clone, Copy, Default)]
struct Lanes {
    any: u32,
    all: u32,
}

/// What a projected modify takes: its register, the bits it changes, the
/// input bits its projections take and the field of a single projection
/// from input bit zero.
struct Projected<'f> {
    target: Target,
    mask: u32,
    source: u32,
    single: Option<&'f ExpandedField>,
}

struct Checker<'a, 'm> {
    context: Context<'a>,
    index: &'a Index<'m>,
    coverage: &'a Coverage,
    file: &'a PolicyFile,
    model: &'a ChipModel,
    domains: BTreeMap<&'a str, Domain<'a>>,
    templates: Vec<TemplateOwner>,
    /// The clear words of the interrupt snapshots.
    clears: BTreeSet<usize>,
    /// The templates that repeat a clear word.
    clear_templates: BTreeSet<usize>,
    /// The lanes of each word's pointer fields: set only from the image of
    /// a typed DMA reference, or to zero.
    pointers: BTreeMap<usize, u32>,
    /// The lanes of each word's address fields: set only by their
    /// full-register write of a DMA reference or buffer address.
    addresses: BTreeMap<usize, u32>,
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
        pointers: BTreeMap::new(),
        addresses: BTreeMap::new(),
        names: BTreeSet::new(),
        owners: BTreeMap::new(),
    };
    checker.domains()?;
    checker.snapshots()?;
    checker.reads()?;
    checker.addresses();
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

    fn declare(
        &mut self,
        name: &'a str,
        description: &str,
        domain: Domain<'a>,
    ) -> Result<(), Error> {
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
            self.declare(&domain.name, &domain.description, Domain::Flag(domain))?;
        }
        for domain in &file.enum_domains {
            self.declare(&domain.name, &domain.description, Domain::Enum(domain))?;
        }
        for domain in &file.bounded_domains {
            self.declare(&domain.name, &domain.description, Domain::Bounded(domain))?;
        }
        for domain in &file.opaque_domains {
            let target = self.register(&domain.name, &domain.register)?;
            self.declare(&domain.name, &domain.description, Domain::Opaque(target))?;
        }
        for domain in &file.bitwise_composed_domains {
            let target = self.register(&domain.name, &domain.register)?;
            self.declare(
                &domain.name,
                &domain.description,
                Domain::Composed(domain, target),
            )?;
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

    /// The DMA address lanes (pointer and address fields) of the words
    /// `target` reaches.
    fn lanes(&self, target: Target, which: &BTreeMap<usize, u32>) -> Lanes {
        let mut lanes = Lanes {
            any: 0,
            all: u32::MAX,
        };
        for word in self.index.reach(target) {
            let mask = which.get(&word).copied().unwrap_or(0);
            lanes.any |= mask;
            lanes.all &= mask;
        }
        lanes
    }

    /// The lanes of `target`'s pointer and address fields together.
    fn dma_lanes(&self, target: Target) -> u32 {
        self.lanes(target, &self.pointers).any | self.lanes(target, &self.addresses).any
    }

    /// `bits` set no DMA address field of `target`.
    fn no_dma(&self, entity: &str, target: Target, bits: u32) -> Result<(), Error> {
        if bits & self.dma_lanes(target) != 0 {
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
        values: Values<'_>,
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

    /// A domain that exists and, when bound to a register, is used there.
    fn domain(&self, entity: &str, name: &str, target: Target) -> Result<Domain<'a>, Error> {
        let domain = self.domains.get(name).copied().ok_or_else(|| {
            self.error(entity, ErrorKind::Domain(format!("{name} is not declared")))
        })?;
        if domain.bound().is_some_and(|bound| bound != target) {
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
            let mask = status_word.declared() & clear_word.declared();
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
            self.write_word(entity, Target::Word(clear), WordWrite::Acknowledge { mask })?;
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

    fn reads(&mut self) -> Result<(), Error> {
        let file = self.file;
        for operation in &file.field_reads {
            let entity = operation.name.as_str();
            self.begin(entity, &operation.sources)?;
            let (target, field) = self.field(entity, &operation.field)?;
            self.class(entity, field, read(field.class), "a read")?;
            self.reads_freely(entity, target)?;
            if operation.signed && !matches!(field.width, 8 | 16 | 32) {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(
                        "a signed read takes an 8-, 16- or 32-bit field".to_owned(),
                    ),
                ));
            }
            let owner = self.owner(entity, target, None)?;
            self.take(entity, &[owner], false)?;
        }
        for operation in &file.full_register_reads {
            let entity = operation.name.as_str();
            self.begin(entity, &operation.sources)?;
            let (target, field) = self.field(entity, &operation.field)?;
            let Target::Word(_) = target else {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(format!("{} must name one element", operation.field)),
                ));
            };
            self.class(entity, field, read(field.class), "a read")?;
            self.reads_freely(entity, target)?;
            if self
                .word(entity, target)?
                .whole_field()
                .is_none_or(|whole| whole.name != field.name)
            {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(
                        "a full-register read names its word's one 32-bit field".to_owned(),
                    ),
                ));
            }
            match self.domain(entity, &operation.domain, target)? {
                Domain::Opaque(_) => {}
                Domain::Bounded(domain) if domain.min == 0 && domain.max == u32::MAX => {}
                _ => {
                    return Err(self.error(
                        entity,
                        ErrorKind::Domain(format!(
                            "{} does not represent every u32 the word may hold",
                            operation.domain
                        )),
                    ));
                }
            }
            let owner = self.owner(entity, target, None)?;
            self.take(entity, &[owner], false)?;
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
    /// its value; `whole` for a write of a data port's one field.
    fn store(
        &mut self,
        entity: &str,
        target: Target,
        written: u32,
        whole: bool,
    ) -> Result<(), Error> {
        self.write_word(entity, target, WordWrite::Store { written, whole })?;
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
                || word
                    .fields()
                    .iter()
                    .any(|field| field.class != FieldClass::WriteOneToClear))
        {
            return Err(self.error(
                entity,
                ErrorKind::Snapshot(
                    "its clear word holds only write-one-to-clear fields".to_owned(),
                ),
            ));
        }
        if (on_clear || word.has(FieldClass::WriteOneToClear)) && !acknowledges {
            return Err(self.error(
                entity,
                ErrorKind::WriteOneToClear(
                    "a word of write-one-to-clear fields is written only by an acknowledgement"
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
        if let WordWrite::Store { whole, .. } = write {
            if let Some(reason) = word.unstorable() {
                return Err(self.error(entity, ErrorKind::Register(reason.to_owned())));
            }
            if word.template.data_port.is_some() && !whole {
                return Err(self.error(
                    entity,
                    ErrorKind::Register(
                        "a data port's word is written only whole by a full-register write"
                            .to_owned(),
                    ),
                ));
            }
        }
        let fixed = match write {
            WordWrite::Image { value, whole_word } | WordWrite::Sequence { value, whole_word } => {
                Some((value, whole_word))
            }
            _ => None,
        };
        for field in word.fields() {
            let mask = field_mask(field);
            if let Some((value, whole_word)) = fixed
                && value & mask != 0
            {
                let allowed = imaged(field.class)
                    || (field.class == FieldClass::WriteOneToClear && acknowledges)
                    || (field.class == FieldClass::ReadOnly && whole_word);
                self.class(entity, field, allowed, "a fixed write")?;
            }
            let values = match write {
                WordWrite::Store { written, .. } if mask & written != 0 => continue,
                WordWrite::Store { .. } => Values::Fixed(0),
                WordWrite::Image { value, .. } | WordWrite::Sequence { value, .. } => {
                    Values::Fixed((value & mask).checked_shr(field.low).unwrap_or(0))
                }
                WordWrite::Acknowledge { mask: sampled } => {
                    Values::Bits((sampled & mask).checked_shr(field.low).unwrap_or(0))
                }
            };
            self.constrained(entity, word, field, values)?;
        }
        if !word.fields().iter().any(|field| field.class.written()) {
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

    /// The lanes of the full-register writes of a DMA reference or buffer
    /// address, before any transaction is checked against them. A write
    /// whose field does not resolve fails its own check later.
    fn addresses(&mut self) {
        let file = self.file;
        for write in &file.full_register_writes {
            if write.points_to.is_none() && write.placement.is_none() {
                continue;
            }
            let Some((target, position)) = self.index.field(&write.field) else {
                continue;
            };
            for word in self.index.reach(target) {
                if let Some(field) = self
                    .index
                    .word(word)
                    .and_then(|word| word.fields().get(position))
                {
                    *self.addresses.entry(word).or_default() |= field_mask(field);
                }
            }
        }
    }

    /// A layout a `DmaRef` may refer to: placed in the DMA window alone.
    fn dma_layout(&self, entity: &str, name: &str) -> Result<u32, Error> {
        self.model
            .layouts()
            .iter()
            .find(|layout| layout.name == name && layout.is_dma_target())
            .map(|layout| layout.size)
            .ok_or_else(|| {
                self.error(
                    entity,
                    ErrorKind::Pointer(format!(
                        "{name} is no word or array layout placed in the {} region alone",
                        crate::layouts::DMA_REGION
                    )),
                )
            })
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
                matches!(field.class, FieldClass::Plain | FieldClass::WriteOnly),
                "a pointer",
            )?;
            let word = self.word(entity, target)?;
            let fits = match pointer.encoding {
                None => field.width == 32,
                Some(PointerEncoding::BleCompressed) => field.width == 20,
                Some(PointerEncoding::LowBits) => field.width < 32,
            };
            if !fits {
                return Err(self.error(
                    entity,
                    ErrorKind::Pointer(format!(
                        "{} is {} bits wide, which its encoding does not fill",
                        pointer.field, field.width
                    )),
                ));
            }
            if word.constraint(field).is_some()
                || word
                    .source(field)
                    .is_some_and(|source| !source.values.is_empty())
            {
                return Err(self.error(
                    entity,
                    ErrorKind::Pointer(format!(
                        "{} has named values or a write constraint",
                        pointer.field
                    )),
                ));
            }
            let size = self.dma_layout(entity, &pointer.points_to)?;
            if pointer.member_offset >= size
                || (pointer.encoding == Some(PointerEncoding::BleCompressed)
                    && !pointer.member_offset.is_multiple_of(4))
            {
                return Err(self.error(
                    entity,
                    ErrorKind::Pointer(
                        "member-offset lies outside the layout or, compressed, off a word"
                            .to_owned(),
                    ),
                ));
            }
            let mask = field_mask(field);
            if self.lanes(target, &self.addresses).any & mask != 0 {
                return Err(self.error(
                    entity,
                    ErrorKind::Pointer(format!(
                        "{} is written by a full-register write of an address",
                        pointer.field
                    )),
                ));
            }
            if self.lanes(target, &self.pointers).any & mask != 0 {
                return Err(self.error(
                    entity,
                    ErrorKind::Duplicate(format!("pointer field {}", pointer.field)),
                ));
            }
            let words = self.index.reach(target).collect::<Vec<_>>();
            for word in words {
                *self.pointers.entry(word).or_default() |= mask;
            }
            self.store(entity, target, mask, false)?;
        }
        Ok(())
    }

    fn stores(&mut self) -> Result<(), Error> {
        let file = self.file;
        for write in &file.full_register_writes {
            let entity = write.name.as_str();
            self.begin(entity, &write.sources)?;
            let (target, field) = self.field(entity, &write.field)?;
            let word = self.word(entity, target)?;
            if word
                .whole_field()
                .is_none_or(|whole| whole.name != field.name)
            {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(
                        "a full-register write names its word's one 32-bit field".to_owned(),
                    ),
                ));
            }
            let port = word.template.data_port.is_some();
            self.class(
                entity,
                field,
                stored(field.class) || (port && field.class == FieldClass::HardwareWritten),
                "a write from zero",
            )?;
            let mask = field_mask(field);
            let values = match (&write.domain, &write.points_to, &write.placement) {
                (Some(domain), None, None) => {
                    self.no_dma(entity, target, mask)?;
                    self.domain(entity, domain, target)?.values()
                }
                (None, Some(layout), None) => {
                    self.dma_layout(entity, layout)?;
                    Values::Any
                }
                (None, None, Some(region)) => {
                    if region != crate::layouts::DMA_REGION
                        || !self
                            .model
                            .ram_regions()
                            .iter()
                            .any(|known| known.name == *region)
                    {
                        return Err(self.error(
                            entity,
                            ErrorKind::Pointer(format!(
                                "a buffer address lies in the {} region of the root",
                                crate::layouts::DMA_REGION
                            )),
                        ));
                    }
                    Values::Any
                }
                _ => {
                    return Err(self.error(
                        entity,
                        ErrorKind::Transaction(
                            "takes exactly one of domain, points-to and placement".to_owned(),
                        ),
                    ));
                }
            };
            if write.domain.is_none() && self.lanes(target, &self.pointers).any & mask != 0 {
                return Err(self.error(
                    entity,
                    ErrorKind::Pointer(format!(
                        "{} is a pointer field, set from its image",
                        write.field
                    )),
                ));
            }
            self.constrained(entity, word, field, values)?;
            self.store(entity, target, mask, true)?;
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
                let bits = field_mask(field);
                if mask & bits != 0 {
                    return Err(self.error(entity, ErrorKind::Duplicate(reference.clone())));
                }
                // A pointer field takes the image of a typed reference; an
                // address field only its own full-register write.
                let pointers = self.lanes(target, &self.pointers);
                if self.lanes(target, &self.addresses).any & bits != 0
                    || (pointers.any & bits != 0 && pointers.all & bits != bits)
                {
                    return Err(self.error(
                        entity,
                        ErrorKind::Pointer(format!(
                            "{reference} is an address field or a pointer field of some elements only"
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
                self.store(entity, target, mask, false)?;
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
            self.reads_freely(entity, target)?;
            let mask = field_mask(field);
            self.no_dma(entity, target, mask)?;
            let word = self.word(entity, target)?;
            self.constrained(entity, word, field, Values::Any)?;
            self.store(entity, target, mask, false)?;
        }
        for write in &file.zero_register_writes {
            let entity = write.name.as_str();
            self.begin(entity, &write.sources)?;
            let target = self.register(entity, &write.register)?;
            self.store(entity, target, 0, false)?;
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
                self.write_word(
                    entity,
                    Target::Word(word),
                    WordWrite::Sequence {
                        value: step.value,
                        whole_word: step.whole_word,
                    },
                )?;
                owners.push(self.owner(entity, Target::Word(word), Some(step.value))?);
            }
            self.take(entity, &owners, true)?;
        }
        Ok(())
    }

    /// A read-modify-write of `target` replacing `bits`; unless `checked`,
    /// no replaced field has a write constraint.
    fn modify(
        &mut self,
        entity: &str,
        target: Target,
        bits: u32,
        checked: bool,
    ) -> Result<(), Error> {
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
                if !checked {
                    self.constrained(entity, word, field, Values::Any)?;
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

    /// The projections' register, the bits they replace and the input bits
    /// they take.
    fn projections<'f>(
        &'f self,
        entity: &str,
        fields: &[Projection],
    ) -> Result<Projected<'f>, Error> {
        let mut register = None;
        let mut mask = 0u32;
        let mut source = 0u32;
        let mut single = None;
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
            source |= field_ones(field.width).checked_shl(offset).unwrap_or(0);
            single = (fields.len() == 1 && offset == 0).then_some(field);
        }
        let target = register.ok_or_else(|| self.error(entity, ErrorKind::EmptySelection))?;
        Ok(Projected {
            target,
            mask,
            source,
            single,
        })
    }

    /// The input of a projected modify fits the input bits its projections
    /// take, so no admitted value is truncated.
    fn fits(&self, entity: &str, admitted: u32, source: u32) -> Result<(), Error> {
        if admitted & !source != 0 {
            return Err(self.error(
                entity,
                ErrorKind::Transaction(format!(
                    "its input sets bits 0x{:08x} its projections do not take",
                    admitted & !source
                )),
            ));
        }
        Ok(())
    }

    /// Whether `projected` is the one pointer field of every word it
    /// reaches, from input bit zero: it takes a typed reference's image.
    fn pointer_image(&self, projected: &Projected<'_>) -> bool {
        projected.single.is_some()
            && self.lanes(projected.target, &self.pointers).all & projected.mask == projected.mask
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
            if modify.preserve_mask | modify.input_mask | modify.set_mask != u32::MAX {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(
                        "preserve, input and set masks cover every bit of the word".to_owned(),
                    ),
                ));
            }
            // An opaque domain is a raw word its input mask selects from;
            // every other domain's values fit the input mask.
            let domain = self.domain(entity, &modify.domain, target)?;
            if !matches!(domain, Domain::Opaque(_)) && domain.bits() & !modify.input_mask != 0 {
                return Err(self.error(
                    entity,
                    ErrorKind::Transaction(format!(
                        "{} admits bits 0x{:08x} its input mask discards",
                        modify.domain,
                        domain.bits() & !modify.input_mask
                    )),
                ));
            }
            self.no_dma(entity, target, modify.input_mask | modify.set_mask)?;
            self.modify(entity, target, modify.input_mask | modify.set_mask, false)?;
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
            let projected = self.projections(entity, &modify.fields)?;
            let image = or
                && modify.domain.is_none()
                && modify.value.is_none()
                && self.pointer_image(&projected);
            let (target, mask, source) = (projected.target, projected.mask, projected.source);
            if !image {
                self.no_dma(entity, target, mask)?;
            }
            let admitted = match (modify.domain.as_deref(), modify.value) {
                (Some(name), None) => match self.domain(entity, name, target)? {
                    domain @ (Domain::Bounded(BoundedDomain { min: 0, .. }) | Domain::Enum(_)) => {
                        domain.bits()
                    }
                    _ => {
                        return Err(self.error(
                            entity,
                            ErrorKind::Domain(format!(
                                "{name} is no bounded domain from zero or enum domain"
                            )),
                        ));
                    }
                },
                (None, Some(value)) => value,
                (None, None) if image => source,
                _ => {
                    return Err(self.error(
                        entity,
                        ErrorKind::Transaction(
                            "takes exactly one of domain and value, or neither for one pointer field"
                                .to_owned(),
                        ),
                    ));
                }
            };
            self.fits(entity, admitted, source)?;
            self.modify(entity, target, mask, false)?;
        }
        for modify in &file.field_replace_modifies {
            let entity = modify.name.as_str();
            self.begin(entity, &modify.sources)?;
            let projected = self.projections(entity, &modify.fields)?;
            let (target, mask, source) = (projected.target, projected.mask, projected.source);
            let pointers = self.lanes(target, &self.pointers);
            let image = modify.domain.is_none()
                && modify.value.is_none()
                && modify.input_transform.is_none()
                && self.pointer_image(&projected);
            // Zero is the null reference of a pointer field of every word
            // the transaction reaches; an address field takes no zero here.
            let null = modify.value == Some(0)
                && self.lanes(target, &self.addresses).any & mask == 0
                && pointers.any & mask & !pointers.all == 0;
            if !(image || null) {
                self.no_dma(entity, target, mask)?;
            }
            let transform = modify.input_transform;
            self.transform(entity, transform, modify.domain.is_some())?;
            let domain = match modify.domain.as_deref() {
                Some(name) => Some(self.domain(entity, name, target)?),
                None => None,
            };
            let admitted = match (domain, modify.value) {
                (
                    Some(
                        domain @ (Domain::Bounded(BoundedDomain { min: 0, .. })
                        | Domain::Enum(_)
                        | Domain::Composed(..)),
                    ),
                    None,
                ) => transform.map_or(domain.bits(), |transform| transform.retain_mask),
                (Some(_), None) => {
                    return Err(self.error(
                        entity,
                        ErrorKind::Domain(format!(
                            "{} is no bounded domain from zero, enum or bitwise-composed domain",
                            modify.domain.as_deref().unwrap_or_default()
                        )),
                    ));
                }
                (None, Some(value)) => value,
                (None, None) if image => source,
                _ => {
                    return Err(self.error(
                        entity,
                        ErrorKind::Transaction(
                            "takes exactly one of domain and value, or neither for one pointer field"
                                .to_owned(),
                        ),
                    ));
                }
            };
            self.fits(entity, admitted, source)?;
            // A field with a write constraint takes a fixed value or a
            // closed domain from input bit zero, untransformed.
            let word = self.word(entity, target)?;
            for projection in &modify.fields {
                let (_, field) = self.field(entity, &projection.field)?;
                let values = match (domain, modify.value) {
                    (None, Some(value)) => Values::Fixed(
                        value
                            .checked_shr(u32::from(projection.source_bit_offset))
                            .unwrap_or(0)
                            & field_ones(field.width),
                    ),
                    (Some(domain), None)
                        if transform.is_none()
                            && modify.fields.len() == 1
                            && projection.source_bit_offset == 0 =>
                    {
                        domain.values()
                    }
                    _ => Values::Any,
                };
                self.constrained(entity, word, field, values)?;
            }
            self.modify(entity, target, mask, true)?;
        }
        for modify in &file.indexed_bit_set_modifies {
            let entity = modify.name.as_str();
            self.begin(entity, &modify.sources)?;
            let (target, field) = self.field(entity, &modify.field)?;
            let mask = field_mask(field);
            let width = field.width;
            let below = match self.domain(entity, &modify.domain, target)? {
                Domain::Bounded(domain) => domain.min == 0 && domain.max < width,
                Domain::Enum(domain) => domain.values.iter().all(|value| value.value < width),
                _ => false,
            };
            if !below {
                return Err(self.error(
                    entity,
                    ErrorKind::Domain(format!(
                        "{} is no bounded domain from zero or enum domain of indices below {width}",
                        modify.domain
                    )),
                ));
            }
            self.no_dma(entity, target, mask)?;
            self.modify(entity, target, mask, false)?;
        }
        Ok(())
    }

    /// An input transform transforms a domain's value, adds or subtracts
    /// and keeps some bit.
    fn transform(
        &self,
        entity: &str,
        transform: Option<InputTransform>,
        domain: bool,
    ) -> Result<(), Error> {
        let Some(transform) = transform else {
            return Ok(());
        };
        if !domain || transform.retain_mask == 0 {
            return Err(self.error(
                entity,
                ErrorKind::Transaction(
                    "an input transform takes a domain and a nonzero retain mask".to_owned(),
                ),
            ));
        }
        if transform.wrapping_add != 0 && transform.wrapping_subtract != 0 {
            return Err(self.error(
                entity,
                ErrorKind::Transaction("an input transform adds or subtracts, not both".to_owned()),
            ));
        }
        Ok(())
    }
}
