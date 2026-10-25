//! The ownership policy over a chip root: partitions of parts, unclaimed
//! words, value domains and transactions.
//!
//! A part is the words of one instance one owner holds; a partition is one
//! owner holding parts of several instances. Every word of every MMIO
//! instance of the root is held by exactly one part or declared unclaimed;
//! a word of write-trigger fields may be split between holders by field.
//! Transactions name `INSTANCE.REGISTER.FIELD` of the root, and every
//! transaction reaches the words of one part. Loading checks every rule
//! and fails with a typed `chip_policy::Error`. The format is specified in
//! the crate README ("Ownership policy").

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::{
    RegisterEvidenceSet,
    chip_model::{
        ChipModel, ExpandedField, ExpandedRegister, Field, FieldClass, Location, Register, Value,
        WriteRange,
    },
};

mod error;
mod ownership;
mod source;
mod transactions;

pub use error::{Error, ErrorKind};
pub use source::{
    BitwiseComposedDomain, BoundedDomain, ComposedArgument, ComposedValueType, DomainValue,
    Elements, EnumDomain, FieldBitModify, FieldDomain, FieldRead, FieldReplaceModify,
    FixedRegisterImage, FixedRegisterSequence, FlagDomain, FullRegisterRead, FullRegisterWrite,
    IndexedBitSetModify, InputTransform, InterruptSnapshot, MaskedRegisterModify, OpaqueDomain,
    POLICY_SCHEMA, Part, Partition, PartitionImageRead, PointerField, PolicyFile, Projection,
    SampledBitZeroWrite, SequenceStep, Triggers, Unclaimed, WholeWordBits, ZeroBasedFieldWrite,
    ZeroRegisterWrite,
};

/// One part: its partition's position in the file and its own in the
/// partition.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PartId {
    pub partition: usize,
    pub part: usize,
}

/// Who holds a word or a field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Holder {
    Part(PartId),
    /// The `[[unclaimed]]` entry at this position.
    Unclaimed(usize),
}

/// How a word is held.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Claim {
    Whole(Holder),
    /// A word of write-trigger fields split by field name.
    Fields(BTreeMap<String, Holder>),
}

/// A loaded and checked policy over one chip model.
#[derive(Clone, Debug)]
pub struct ChipPolicy {
    path: PathBuf,
    file: PolicyFile,
    claims: BTreeMap<String, Claim>,
    owners: BTreeMap<String, PartId>,
}

impl ChipPolicy {
    /// Load the policy at `path` over `model` and check every rule. Every
    /// `sources` entry must be an ID of `evidence`.
    pub fn load(
        path: &Path,
        model: &ChipModel,
        evidence: &RegisterEvidenceSet,
    ) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| Error::new(path, "", ErrorKind::Read(error)))?;
        let file: PolicyFile = toml_edit::de::from_str(&text)
            .map_err(|error| Error::new(path, "", ErrorKind::Parse(error.to_string())))?;
        let ids = evidence.source_ids();
        let context = Context {
            path,
            evidence: &ids,
        };
        if file.schema != POLICY_SCHEMA {
            return Err(context.error(
                &file.chip,
                ErrorKind::Schema {
                    expected: POLICY_SCHEMA,
                    found: file.schema,
                },
            ));
        }
        if file.chip != model.chip() {
            return Err(context.error(
                &file.chip,
                ErrorKind::Chip {
                    expected: model.chip().to_owned(),
                    found: file.chip.clone(),
                },
            ));
        }
        let index = Index::new(context, model)?;
        let coverage = ownership::check(context, &index, &file)?;
        let owners = transactions::check(context, &index, &coverage, &file)?;
        let claims = coverage.claims(&index);
        Ok(Self {
            path: path.to_owned(),
            file,
            claims,
            owners,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The policy as written.
    pub fn file(&self) -> &PolicyFile {
        &self.file
    }

    pub fn partitions(&self) -> &[Partition] {
        &self.file.partitions
    }

    /// The part `id`.
    pub fn part(&self, id: PartId) -> Option<&Part> {
        self.file
            .partitions
            .get(id.partition)
            .and_then(|partition| partition.parts.get(id.part))
    }

    /// How the word `INSTANCE.REGISTER` (an expanded element) is held.
    pub fn claim(&self, register: &str) -> Option<&Claim> {
        self.claims.get(register)
    }

    /// Every MMIO word by identity and how it is held.
    pub fn claims(&self) -> &BTreeMap<String, Claim> {
        &self.claims
    }

    /// The part whose words the transaction `name` reaches.
    pub fn owner(&self, transaction: &str) -> Option<PartId> {
        self.owners.get(transaction).copied()
    }
}

/// Where a check runs: the policy file and the evidence catalogs' IDs.
#[derive(Clone, Copy)]
struct Context<'a> {
    path: &'a Path,
    evidence: &'a BTreeSet<&'a str>,
}

impl Context<'_> {
    fn error(&self, entity: impl Into<String>, kind: ErrorKind) -> Error {
        Error::new(self.path, entity, kind)
    }

    /// Sources are nonempty, distinct, nonblank and defined in a catalog.
    fn sources(&self, entity: &str, sources: &[String]) -> Result<(), Error> {
        let mut seen = BTreeSet::new();
        if sources.is_empty()
            || sources
                .iter()
                .any(|source| source.trim().is_empty() || !seen.insert(source))
        {
            return Err(self.error(entity, ErrorKind::InvalidSources));
        }
        if let Some(unknown) = sources
            .iter()
            .find(|source| !self.evidence.contains(source.as_str()))
        {
            return Err(self.error(entity, ErrorKind::UnknownSource(unknown.clone())));
        }
        Ok(())
    }

    fn description(&self, entity: &str, text: &str) -> Result<(), Error> {
        if text.trim().is_empty() {
            return Err(self.error(entity, ErrorKind::MissingDescription));
        }
        Ok(())
    }
}

/// One MMIO word of the model: an expanded element and its template.
struct Word<'m> {
    register: &'m ExpandedRegister,
    template: &'m Register,
    /// A command register of its type.
    command: bool,
}

impl<'m> Word<'m> {
    fn identity(&self) -> &str {
        &self.register.identity
    }

    fn fields(&self) -> &[ExpandedField] {
        &self.register.fields
    }

    fn field(&self, name: &str) -> Option<(usize, &ExpandedField)> {
        self.register
            .fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.name == name)
    }

    fn has(&self, class: FieldClass) -> bool {
        self.fields().iter().any(|field| field.class == class)
    }

    /// A read of the word has a value and changes nothing.
    fn reads_freely(&self) -> bool {
        self.template.fifo.is_none()
            && self.template.read_side_effect.is_none()
            && !self.has(FieldClass::ReadClears)
            && self.fields().iter().any(|field| field.class.readable())
    }

    /// The word may be read, modified and written back: the read changes
    /// nothing, and writing back what was read neither replays a bit the
    /// hardware sets or clears nor writes a port, a command or a field
    /// with no readable value.
    fn modifiable(&self) -> bool {
        self.reads_freely()
            && !self.command
            && self.template.data_port.is_none()
            && !self.fields().iter().any(|field| {
                matches!(
                    field.class,
                    FieldClass::WriteOnly
                        | FieldClass::WriteOneToClear
                        | FieldClass::SelfClearing
                        | FieldClass::SetOnly
                        | FieldClass::HardwareWritten
                )
            })
    }

    /// Why a transaction may not write the word whole from zero, if it may
    /// not: a FIFO port or a command register, a word of a field the
    /// hardware also changes or a one written sets (a data port's field
    /// excepted), or one with no field a write gives a value. A
    /// write-one-to-clear field has its own rule.
    fn unstorable(&self) -> Option<&'static str> {
        if !self.storable() {
            return Some("a FIFO port or command register is not written by a transaction");
        }
        let port = self.template.data_port.is_some();
        if self.fields().iter().any(|field| match field.class {
            FieldClass::SelfClearing | FieldClass::SetOnly => true,
            FieldClass::HardwareWritten => !port,
            _ => false,
        }) {
            return Some(
                "a word of a self-clearing, set-only or hardware-written field is not written from zero; a data port's field is written only whole",
            );
        }
        if !self.fields().iter().any(|field| {
            stored(field.class) || (port && field.class == FieldClass::HardwareWritten)
        }) {
            return Some("the word has no field a write gives a value");
        }
        None
    }

    /// Neither a FIFO port nor a command register.
    fn storable(&self) -> bool {
        self.template.fifo.is_none() && !self.command
    }

    /// The bits of declared fields.
    fn declared(&self) -> u32 {
        self.fields()
            .iter()
            .fold(0, |mask, field| mask | field_mask(field))
    }

    /// The word's one field covering all 32 bits, if it has one.
    fn whole_field(&self) -> Option<&ExpandedField> {
        match self.fields() {
            [field] if field.low == 0 && field.width == 32 => Some(field),
            _ => None,
        }
    }

    /// The type file's declaration of `field`.
    fn source(&self, field: &ExpandedField) -> Option<&'m Field> {
        self.template
            .fields
            .iter()
            .find(|candidate| candidate.name == field.name)
    }

    /// The values a write may give `field`, when they are fewer than every
    /// value of its width.
    fn constraint(&self, field: &ExpandedField) -> Option<Constraint<'_>> {
        let source = self.source(field)?;
        let ones = field_ones(field.width);
        let range = source
            .write_range
            .or_else(|| {
                source.value_range.as_ref().map(|range| WriteRange {
                    min: range.min,
                    max: range.max,
                })
            })
            .filter(|range| range.min != 0 || range.max != ones);
        let values = source
            .write_values_only
            .then_some(source.values.as_slice())
            .filter(|values| {
                field.width >= 32
                    || u64::try_from(values.len()).unwrap_or(u64::MAX) <= u64::from(ones)
            });
        (range.is_some() || values.is_some()).then_some(Constraint { range, values })
    }
}

/// A field a write from zero may give a value.
fn stored(class: FieldClass) -> bool {
    matches!(
        class,
        FieldClass::Plain | FieldClass::WriteOnly | FieldClass::WriteTrigger
    )
}

/// The values a write may give one field: a write range or the field's
/// named values only.
#[derive(Clone, Copy)]
struct Constraint<'m> {
    range: Option<WriteRange>,
    values: Option<&'m [Value]>,
}

impl Constraint<'_> {
    fn admits(&self, value: u32) -> bool {
        self.range
            .is_none_or(|range| range.min <= value && value <= range.max)
            && self
                .values
                .is_none_or(|values| values.iter().any(|named| named.value == value))
    }

    /// Every value of `min..=max` is admitted, without enumerating it.
    fn admits_range(&self, min: u32, max: u32) -> bool {
        self.range
            .is_none_or(|range| range.min <= min && max <= range.max)
            && self.values.is_none_or(|values| {
                let inside = values
                    .iter()
                    .filter(|named| min <= named.value && named.value <= max)
                    .count();
                u64::try_from(inside).unwrap_or(u64::MAX)
                    == u64::from(max)
                        .saturating_sub(u64::from(min))
                        .saturating_add(1)
            })
    }

    /// Every subset of `mask` is admitted, including zero. Named values
    /// are distinct, so counting the subsets they contain proves coverage
    /// without enumerating the mask's (up to 2^32) possible values.
    fn admits_bits(&self, mask: u32) -> bool {
        self.range
            .is_none_or(|range| range.min == 0 && mask <= range.max)
            && self.values.is_none_or(|values| {
                let inside = values
                    .iter()
                    .filter(|named| named.value & !mask == 0)
                    .count();
                u64::try_from(inside).unwrap_or(u64::MAX) == 1u64 << mask.count_ones()
            })
    }
}

/// The ones of a `width`-bit value.
fn field_ones(width: u32) -> u32 {
    u32::MAX
        .checked_shr(32u32.saturating_sub(width))
        .unwrap_or(0)
}

fn field_mask(field: &ExpandedField) -> u32 {
    field_ones(field.width).checked_shl(field.low).unwrap_or(0)
}

/// What a register reference of a transaction or a part names.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Target {
    /// One element.
    Word(usize),
    /// Every element of the template at this position in `Index::templates`.
    Template(usize),
}

/// The MMIO words of the model and how a reference finds them. Its size
/// follows the model's, which the model's loader bounds.
struct Index<'m> {
    words: Vec<Word<'m>>,
    by_identity: BTreeMap<&'m str, usize>,
    /// Template keys holding `%s`, by `(instance, key)`.
    by_template: BTreeMap<(&'m str, &'m str), usize>,
    /// The words of each such template, in index order.
    templates: Vec<Vec<usize>>,
    /// The template each word repeats, by word.
    template_of: Vec<Option<usize>>,
    /// The words of each MMIO instance.
    instances: BTreeMap<&'m str, Vec<usize>>,
}

impl<'m> Index<'m> {
    fn new(context: Context<'_>, model: &'m ChipModel) -> Result<Self, Error> {
        let mut types = BTreeMap::new();
        for instance in model.instances() {
            if let Some(block_type) = model.types().get(&instance.block_type) {
                types.insert(instance.name.as_str(), block_type);
            }
        }
        let mut words = Vec::new();
        words
            .try_reserve_exact(model.registers().len())
            .map_err(|error| context.error("", ErrorKind::Allocation(error)))?;
        let mut index = Self {
            words: Vec::new(),
            by_identity: BTreeMap::new(),
            by_template: BTreeMap::new(),
            templates: Vec::new(),
            template_of: Vec::new(),
            instances: types
                .keys()
                .map(|instance| (*instance, Vec::new()))
                .collect(),
        };
        for register in model.registers() {
            let Location::Mmio { .. } = register.location else {
                continue;
            };
            let block_type = types.get(register.instance.as_str()).ok_or_else(|| {
                context.error("", ErrorKind::UnknownInstance(register.instance.clone()))
            })?;
            let template = block_type.templates.get(&register.key).ok_or_else(|| {
                context.error("", ErrorKind::UnknownRegister(register.identity.clone()))
            })?;
            let position = words.len();
            words.push(Word {
                register,
                template,
                command: block_type
                    .file
                    .commands
                    .iter()
                    .any(|command| command.register == register.key),
            });
            index
                .by_identity
                .insert(register.identity.as_str(), position);
            if let Some(list) = index.instances.get_mut(register.instance.as_str()) {
                list.push(position);
            }
            let mut repeats = None;
            if register.key.contains("%s") {
                let key = (register.instance.as_str(), register.key.as_str());
                let template = match index.by_template.get(&key) {
                    Some(template) => *template,
                    None => {
                        index.by_template.insert(key, index.templates.len());
                        index.templates.push(Vec::new());
                        index.templates.len() - 1
                    }
                };
                if let Some(list) = index.templates.get_mut(template) {
                    list.push(position);
                }
                repeats = Some(template);
            }
            index.template_of.push(repeats);
        }
        index.words = words;
        Ok(index)
    }

    fn word(&self, position: usize) -> Option<&Word<'m>> {
        self.words.get(position)
    }

    /// `INSTANCE.REGISTER`: an element identity, or `INSTANCE.KEY` with a
    /// template key holding `%s`.
    fn target(&self, reference: &str) -> Option<Target> {
        if let Some(position) = self.by_identity.get(reference) {
            return Some(Target::Word(*position));
        }
        let (instance, key) = reference.split_once('.')?;
        self.by_template
            .get(&(instance, key))
            .map(|template| Target::Template(*template))
    }

    /// The words a target reaches.
    fn reach(&self, target: Target) -> impl Iterator<Item = usize> + '_ {
        let (single, many) = match target {
            Target::Word(position) => (Some(position), &[][..]),
            Target::Template(template) => (
                None,
                self.templates.get(template).map_or(&[][..], Vec::as_slice),
            ),
        };
        single.into_iter().chain(many.iter().copied())
    }

    /// The first word a target reaches: the template's words share its
    /// fields and register constructs.
    fn first(&self, target: Target) -> Option<&Word<'m>> {
        match target {
            Target::Word(position) => self.word(position),
            Target::Template(template) => self
                .templates
                .get(template)
                .and_then(|words| words.first())
                .and_then(|position| self.word(*position)),
        }
    }

    /// `INSTANCE.REGISTER.FIELD`: the register's target and the field's
    /// position in the register.
    fn field(&self, reference: &str) -> Option<(Target, usize)> {
        let (register, field) = reference.rsplit_once('.')?;
        let target = self.target(register)?;
        let (position, _) = self.first(target)?.field(field)?;
        Some((target, position))
    }
}

fn upper_camel(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_uppercase())
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn upper_snake(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_uppercase())
        && name
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn lower_snake(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && !matches!(
            name,
            "as" | "async"
                | "await"
                | "break"
                | "const"
                | "continue"
                | "crate"
                | "dyn"
                | "else"
                | "enum"
                | "extern"
                | "false"
                | "fn"
                | "for"
                | "gen"
                | "if"
                | "impl"
                | "in"
                | "let"
                | "loop"
                | "match"
                | "mod"
                | "move"
                | "mut"
                | "pub"
                | "ref"
                | "return"
                | "self"
                | "static"
                | "struct"
                | "super"
                | "trait"
                | "true"
                | "type"
                | "unsafe"
                | "use"
                | "where"
                | "while"
        )
}
