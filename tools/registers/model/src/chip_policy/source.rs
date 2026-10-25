//! The ownership policy file as written.

use serde::Deserialize;

/// The schema of an ownership policy over a chip root.
pub const POLICY_SCHEMA: u32 = 6;

/// One ownership policy: partitions of parts, unclaimed words, value
/// domains and transactions over the instances of one chip root.
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
    pub flag_domains: Vec<FlagDomain>,
    #[serde(default)]
    pub enum_domains: Vec<EnumDomain>,
    #[serde(default)]
    pub bounded_domains: Vec<BoundedDomain>,
    #[serde(default)]
    pub opaque_domains: Vec<OpaqueDomain>,
    #[serde(default)]
    pub bitwise_composed_domains: Vec<BitwiseComposedDomain>,
    #[serde(default)]
    pub interrupt_snapshots: Vec<InterruptSnapshot>,
    #[serde(default)]
    pub field_reads: Vec<FieldRead>,
    #[serde(default)]
    pub full_register_reads: Vec<FullRegisterRead>,
    #[serde(default)]
    pub partition_image_reads: Vec<PartitionImageRead>,
    #[serde(default)]
    pub full_register_writes: Vec<FullRegisterWrite>,
    #[serde(default)]
    pub pointer_fields: Vec<PointerField>,
    #[serde(default)]
    pub zero_based_field_writes: Vec<ZeroBasedFieldWrite>,
    #[serde(default)]
    pub sampled_bit_zero_writes: Vec<SampledBitZeroWrite>,
    #[serde(default)]
    pub zero_register_writes: Vec<ZeroRegisterWrite>,
    #[serde(default)]
    pub fixed_register_images: Vec<FixedRegisterImage>,
    #[serde(default)]
    pub fixed_register_sequences: Vec<FixedRegisterSequence>,
    #[serde(default)]
    pub masked_register_modifies: Vec<MaskedRegisterModify>,
    #[serde(default)]
    pub field_or_modifies: Vec<FieldBitModify>,
    #[serde(default)]
    pub field_clear_modifies: Vec<FieldBitModify>,
    #[serde(default)]
    pub field_replace_modifies: Vec<FieldReplaceModify>,
    #[serde(default)]
    pub indexed_bit_set_modifies: Vec<IndexedBitSetModify>,
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
    /// Reached by every execution context at once through `&self`.
    #[serde(default)]
    pub shared: bool,
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

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FlagDomain {
    pub name: String,
    pub description: String,
    pub values: Vec<DomainValue>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct EnumDomain {
    pub name: String,
    pub description: String,
    pub values: Vec<DomainValue>,
}

/// One named value of a flag (`UPPER_SNAKE`) or enum (`UpperCamel`) domain.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DomainValue {
    pub name: String,
    pub value: u32,
    pub description: String,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BoundedDomain {
    pub name: String,
    pub description: String,
    pub min: u32,
    pub max: u32,
    pub sources: Vec<String>,
}

/// A full-width value of one register, used only on that register.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OpaqueDomain {
    pub name: String,
    pub description: String,
    /// `INSTANCE.REGISTER`.
    pub register: String,
    pub sources: Vec<String>,
}

/// A value of one register composed from shifted logical arguments.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BitwiseComposedDomain {
    pub name: String,
    pub description: String,
    /// `INSTANCE.REGISTER`.
    pub register: String,
    pub arguments: Vec<ComposedArgument>,
    #[serde(default)]
    pub fixed_value: u32,
    pub output_mask: u32,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ComposedArgument {
    pub name: String,
    pub value_type: ComposedValueType,
    #[serde(default)]
    pub right_shift: u8,
    #[serde(default = "all_bits")]
    pub retain_mask: u32,
    pub left_shift: u8,
}

const fn all_bits() -> u32 {
    u32::MAX
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ComposedValueType {
    U8,
    U16,
    U32,
}

/// A status word sampled into an affine token and acknowledged, exactly
/// as sampled, in its clear word.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct InterruptSnapshot {
    pub name: String,
    /// `INSTANCE.REGISTER`: one element.
    pub status: String,
    /// `INSTANCE.REGISTER`: one element of write-one-to-clear fields.
    pub clear: String,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldRead {
    pub name: String,
    /// `INSTANCE.REGISTER.FIELD`.
    pub field: String,
    #[serde(default)]
    pub signed: bool,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FullRegisterRead {
    pub name: String,
    /// `INSTANCE.REGISTER.FIELD`.
    pub field: String,
    pub domain: String,
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

/// A write of the whole word from zero with one field's typed value:
/// exactly one of `domain`, `points-to` and `placement`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FullRegisterWrite {
    pub name: String,
    /// `INSTANCE.REGISTER.FIELD`.
    pub field: String,
    #[serde(default)]
    pub domain: Option<String>,
    /// A memory layout of the root: the value is a DMA reference to it.
    #[serde(default)]
    pub points_to: Option<String>,
    /// A RAM region of the root: the value is a buffer address in it.
    #[serde(default)]
    pub placement: Option<String>,
    pub sources: Vec<String>,
}

/// A field set only from the image of a typed DMA reference.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PointerField {
    pub name: String,
    /// `INSTANCE.REGISTER.FIELD`.
    pub field: String,
    pub points_to: String,
    #[serde(default)]
    pub member_offset: u32,
    #[serde(default)]
    pub encoding: Option<PointerEncoding>,
    pub sources: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum PointerEncoding {
    /// Nonzero 20-bit word index under the 0x2f SRAM prefix.
    BleCompressed,
    /// The address's low bits, as wide as the field.
    LowBits,
}

/// A write from zero of `fields` of one register.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ZeroBasedFieldWrite {
    pub name: String,
    /// `INSTANCE.REGISTER.FIELD`, all of one register.
    pub fields: Vec<String>,
    pub sources: Vec<String>,
}

/// One fresh one-bit sample written back into an otherwise zero word.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SampledBitZeroWrite {
    pub name: String,
    /// `INSTANCE.REGISTER.FIELD`.
    pub field: String,
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
    /// `INSTANCE.REGISTER`: one element.
    pub register: String,
    pub value: u32,
    #[serde(default)]
    pub whole_word: bool,
}

/// A read-modify-write of caller (`input-mask`) and fixed (`set-mask`)
/// bits, keeping `preserve-mask`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MaskedRegisterModify {
    pub name: String,
    /// `INSTANCE.REGISTER`.
    pub register: String,
    pub domain: String,
    pub preserve_mask: u32,
    pub input_mask: u32,
    pub set_mask: u32,
    pub sources: Vec<String>,
}

/// A read-modify-write that ORs (or clears) fields from one logical input.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldBitModify {
    pub name: String,
    pub fields: Vec<Projection>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub value: Option<u32>,
    pub sources: Vec<String>,
}

/// A read-modify-write that replaces fields from one logical input.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldReplaceModify {
    pub name: String,
    pub fields: Vec<Projection>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub value: Option<u32>,
    #[serde(default)]
    pub input_transform: Option<InputTransform>,
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

/// Arithmetic and truncation applied to the input before projection.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct InputTransform {
    #[serde(default)]
    pub wrapping_add: u32,
    #[serde(default)]
    pub wrapping_subtract: u32,
    #[serde(default)]
    pub clamp_maximum: Option<u32>,
    pub retain_mask: u32,
}

/// A read-modify-write that sets the one bit a bounded index selects.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct IndexedBitSetModify {
    pub name: String,
    /// `INSTANCE.REGISTER.FIELD`.
    pub field: String,
    pub domain: String,
    pub sources: Vec<String>,
}
