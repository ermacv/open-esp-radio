//! The ownership policy file as written.

use serde::Deserialize;

/// The schema of an ownership policy over a chip root.
pub const POLICY_SCHEMA: u32 = 7;

/// One ownership policy: partitions of parts, unclaimed words and the
/// transactions the PAC renders over the instances of one chip root.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PolicyFile {
    pub schema: u32,
    /// The root's `chip`.
    pub chip: String,
    #[serde(default)]
    pub partitions: Vec<Partition>,
    #[serde(default)]
    pub unclaimed: Vec<Unclaimed>,
    #[serde(default)]
    pub interrupt_snapshots: Vec<InterruptSnapshot>,
    #[serde(default)]
    pub record_words: Vec<RecordWord>,
    #[serde(default)]
    pub partition_image_reads: Vec<PartitionImageRead>,
    #[serde(default)]
    pub zero_based_field_writes: Vec<ZeroBasedFieldWrite>,
    #[serde(default)]
    pub zero_register_writes: Vec<ZeroRegisterWrite>,
    #[serde(default)]
    pub fixed_register_images: Vec<FixedRegisterImage>,
    #[serde(default)]
    pub fixed_register_sequences: Vec<FixedRegisterSequence>,
    #[serde(default)]
    pub field_or_modifies: Vec<FieldOrModify>,
}

/// One owner: a generated owner type holding parts of several instances.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Partition {
    /// The owner type, `UpperCamelCase`.
    pub name: String,
    /// Its member in the generated root, `lower_snake_case`.
    pub member: String,
    pub description: String,
    pub parts: Vec<Part>,
}

/// The words of one instance one owner holds, and how it holds them.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Part {
    /// The part's type suffix, `UpperCamelCase`, unique in its partition.
    pub name: String,
    /// An MMIO instance of the root.
    pub instance: String,
    /// Element names below the instance (`STATUS`, `REGCLK_1`,
    /// `T1.CONFIG`) or template keys holding `%s` (`CH%s_EVENT`,
    /// `T%s.CONFIG`), which select every element of the template.
    #[serde(default)]
    pub registers: Vec<String>,
    /// Consecutive elements of one repeated template.
    #[serde(default)]
    pub elements: Vec<Elements>,
    /// Fields of a word of write-trigger fields that several holders
    /// split by field.
    #[serde(default)]
    pub triggers: Vec<Triggers>,
    /// Registers of the instance the part reads, claimed by other parts.
    #[serde(default)]
    pub observes: Vec<String>,
    /// Owned by an interrupt handler, not a task.
    #[serde(default)]
    pub irq_side: bool,
    /// The interrupt sources of the root (their names) whose handler the
    /// part serves: one name, or a list when one bank of the part
    /// acknowledges several sources. Each is bound to the part's instance
    /// and named by no other part. An interrupt handler's part names one.
    #[serde(default, deserialize_with = "one_or_many")]
    pub interrupt: Vec<String>,
    /// Reached by every execution context at once through `&self`.
    #[serde(default)]
    pub shared: bool,
    /// The proof of a hart a `shared` part on a hart-local instance takes
    /// for each store (a type path generic over the hart, `Proof<H>`): such
    /// a part only stores its fixed images, each taking the proof.
    #[serde(default)]
    pub hart_proof: Option<String>,
    /// Only reads its words: the owner gave up their writes.
    #[serde(default)]
    pub read_only: bool,
    /// A `read-only` part whose owner is `Copy`.
    #[serde(default)]
    pub copy: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
}

/// A string, or a list of strings.
fn one_or_many<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(name) => vec![name],
        OneOrMany::Many(names) => names,
    })
}

/// `count` elements of template `register` from index `first`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Elements {
    pub register: String,
    pub first: u32,
    pub count: u32,
}

/// The `fields` of one word of write-trigger fields.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Triggers {
    /// An element name below the instance.
    pub register: String,
    pub fields: Vec<String>,
}

/// Words no owner reaches: either whole `instances`, or the selected
/// words of one `instance`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Unclaimed {
    #[serde(default)]
    pub instances: Vec<String>,
    #[serde(default)]
    pub instance: Option<String>,
    /// As a part's `registers`.
    #[serde(default)]
    pub registers: Vec<String>,
    #[serde(default)]
    pub elements: Vec<Elements>,
    #[serde(default)]
    pub triggers: Vec<Triggers>,
    pub reason: String,
}

/// A status word sampled into an affine token and acknowledged, exactly
/// as sampled, in its clear word.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct InterruptSnapshot {
    pub name: String,
    /// `INSTANCE.REGISTER`: one element.
    pub status: String,
    /// `INSTANCE.REGISTER.FIELD`: the status fields the snapshot samples,
    /// when not every field its clear word acknowledges.
    #[serde(default)]
    pub events: Vec<String>,
    /// `INSTANCE.REGISTER`, not written in the file: the clear element the
    /// status register's type pairs with it (`[[interrupt-pairs]]`), or
    /// `status` itself when it holds only write-one-to-clear fields. The
    /// loader resolves it.
    #[serde(skip)]
    pub clear: String,
    /// The event the caller consumed elsewhere, whose one selected field of
    /// `consumed-fields` the acknowledgement writes beside the sample.
    #[serde(default)]
    pub consumed_event: Option<String>,
    /// `INSTANCE.REGISTER.FIELD`: one-bit fields of the clear word, one of
    /// which the caller selects.
    #[serde(default)]
    pub consumed_fields: Vec<String>,
    pub sources: Vec<String>,
}

/// A register whose observation a diagnostic record keeps as the
/// hardware has it: its read value gets the whole word, undecoded.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RecordWord {
    /// `INSTANCE.REGISTER`, one element or a register array's template:
    /// every element shares the read value.
    pub register: String,
    /// Which record keeps the word, and why the fields do not suffice.
    pub reason: String,
    pub sources: Vec<String>,
}

/// Every readable word of one partition, by index.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PartitionImageRead {
    pub name: String,
    pub partition: String,
    pub sources: Vec<String>,
}

/// A write from zero of `fields` of one register beside the vendor's fixed
/// ones, which no field accessor stores.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ZeroBasedFieldWrite {
    pub name: String,
    /// `INSTANCE.REGISTER.FIELD`, all of one register.
    pub fields: Vec<String>,
    /// Ones the source stores beside the fields: bits of `read-only`
    /// fields or of no field.
    pub whole_word_bits: WholeWordBits,
    pub sources: Vec<String>,
}

/// The fixed ones a write from zero stores outside its fields, with the
/// sources that show the store.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WholeWordBits {
    pub value: u32,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ZeroRegisterWrite {
    pub name: String,
    /// `INSTANCE.REGISTER`.
    pub register: String,
    pub sources: Vec<String>,
}

/// One reviewed whole-register image. On a snapshot's clear word it names
/// one proof: `masks` or `consumed-event`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FixedRegisterImage {
    pub name: String,
    /// `INSTANCE.REGISTER`.
    pub register: String,
    pub value: u32,
    #[serde(default)]
    pub whole_word: bool,
    /// `INSTANCE.REGISTER`: the enable word the same transaction clears first.
    #[serde(default)]
    pub masks: Option<String>,
    #[serde(default)]
    pub consumed_event: Option<String>,
    pub sources: Vec<String>,
}

/// An ordered transaction of fixed whole-register images.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FixedRegisterSequence {
    pub name: String,
    pub steps: Vec<SequenceStep>,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SequenceStep {
    /// `INSTANCE.REGISTER`: one element, or with `first` and `count` a
    /// template (`INSTANCE.REGISTER%s`) whose elements the step writes.
    pub register: String,
    pub value: u32,
    /// The first element of a template step.
    #[serde(default)]
    pub first: Option<u32>,
    /// The number of elements of a template step, from `first`.
    #[serde(default)]
    pub count: Option<u32>,
}

/// The vendor's OR of a typed DMA reference's image into a fresh read of
/// one pointer field.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldOrModify {
    pub name: String,
    pub fields: Vec<Projection>,
    pub sources: Vec<String>,
}

/// One field of a modify, taken from the input at `source-bit-offset`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Projection {
    /// `INSTANCE.REGISTER.FIELD`, all projections of one register.
    pub field: String,
    pub source_bit_offset: u8,
}
