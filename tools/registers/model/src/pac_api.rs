//! Reviewed, target-owned safe PAC transaction declarations.

use std::{collections::BTreeSet, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PacApiPack {
    pub schema: u32,
    #[serde(default)]
    pub options: PacApiOptions,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ownership_partitions: Vec<OwnershipPartition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flag_domains: Vec<FlagDomain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_domains: Vec<EnumDomain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bounded_domains: Vec<BoundedDomain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opaque_domains: Vec<OpaqueDomain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bitwise_composed_domains: Vec<BitwiseComposedDomain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indirect_register_field_domains: Vec<IndirectRegisterFieldDomain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interrupt_snapshots: Vec<InterruptSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub full_register_writes: Vec<FullRegisterWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub full_register_reads: Vec<FullRegisterRead>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_reads: Vec<FieldRead>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_snapshot_reads: Vec<FieldSnapshotRead>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed_register_writes: Vec<FixedRegisterWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed_register_images: Vec<FixedRegisterImage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed_register_sequences: Vec<FixedRegisterSequence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub w1c_register_snapshots: Vec<W1cRegisterSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub register_image_reads: Vec<RegisterImageRead>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub register_image_writes: Vec<RegisterImageWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partition_image_reads: Vec<PartitionImageRead>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zero_based_field_writes: Vec<ZeroBasedFieldWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sampled_bit_zero_writes: Vec<SampledBitZeroWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zero_register_writes: Vec<ZeroRegisterWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masked_register_modifies: Vec<MaskedRegisterModify>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_or_modifies: Vec<FieldOrModify>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_replace_modifies: Vec<FieldReplaceModify>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_argument_modifies: Vec<FieldArgumentModify>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexed_bit_set_modifies: Vec<IndexedBitSetModify>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PacApiOptions {
    #[serde(default)]
    pub device_access: bool,
    #[serde(default)]
    pub allow_clippy_empty_docs: bool,
}

/// One target-declared, exhaustive ownership partition of raw SVD peripherals.
///
/// The partition is an API-design input, not a fact inferred from register
/// names, interrupt operations, addresses, or another analysis output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OwnershipPartition {
    /// Generated Rust owner type, for example `BluetoothControllerPeripherals`.
    pub name: String,
    /// Field name in the generated `PeripheralPartitions` root.
    pub member: String,
    /// Target-owned explanation of this ownership boundary.
    pub description: String,
    /// Exact SVD peripheral names consumed by this owner.
    pub peripherals: Vec<String>,
}

/// Closed writable bit-mask domain emitted into the public PAC facade.
///
/// Values are reviewed capabilities, not an exhaustive description of every
/// hardware bit. The generated type has no public integer constructor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FlagDomain {
    pub name: String,
    pub description: String,
    pub values: Vec<FlagValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FlagValue {
    pub name: String,
    pub value: u32,
    pub description: String,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct EnumDomain {
    pub name: String,
    pub description: String,
    pub values: Vec<EnumValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct EnumValue {
    pub name: String,
    pub value: u32,
    pub description: String,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BoundedDomain {
    pub name: String,
    pub description: String,
    pub min: u32,
    pub max: u32,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OpaqueDomain {
    pub name: String,
    pub description: String,
    pub peripheral: String,
    pub register: String,
    pub sources: Vec<String>,
}

/// Register-specific value composed from typed logical arguments.
///
/// Vendor encoders sometimes OR shifted arguments before applying one final
/// mask, so high bits from one logical argument deliberately contribute to an
/// adjacent physical field. This domain keeps that register-shaped transform
/// generated instead of requiring handwritten PAC code to pack bit images.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BitwiseComposedDomain {
    pub name: String,
    pub description: String,
    pub peripheral: String,
    pub register: String,
    pub arguments: Vec<BitwiseComposedDomainArgument>,
    #[serde(default)]
    pub fixed_value: u32,
    pub output_mask: u32,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BitwiseComposedDomainArgument {
    pub name: String,
    pub value_type: BitwiseComposedValueType,
    #[serde(default)]
    pub right_shift: u8,
    #[serde(default = "all_u32_bits")]
    pub retain_mask: u32,
    pub left_shift: u8,
}

const fn all_u32_bits() -> u32 {
    u32::MAX
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BitwiseComposedValueType {
    U8,
    U16,
    U32,
}

impl BitwiseComposedValueType {
    pub(crate) const fn rust_name(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
        }
    }

    pub(crate) const fn requires_u32_cast(self) -> bool {
        !matches!(self, Self::U32)
    }
}

/// Generated accessors for fields in registers reached through an indirect
/// command bus rather than the CPU MMIO address space.
///
/// The reviewed pack owns the bank/register identity and field geometry. The
/// generated accessor is the only place that masks or shifts a sampled
/// indirect-register value; handwritten code receives named constants and
/// calls `extract` or `replace`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct IndirectRegisterFieldDomain {
    pub name: String,
    pub module: String,
    pub description: String,
    pub value_bits: u8,
    /// Generate the domain's register image: its distinct registers in
    /// [`IndirectRegisterFieldDomain::image_registers`] order, for reading the
    /// reviewed indirect state back without naming a register outside the PAC.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub register_image: bool,
    pub fields: Vec<IndirectRegisterField>,
}

impl IndirectRegisterFieldDomain {
    /// The distinct `(bank, register)` pairs the domain's fields occupy,
    /// ascending: the order of its register image.
    pub fn image_registers(&self) -> Vec<(u8, u8)> {
        self.fields
            .iter()
            .map(|field| (field.bank, field.register))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct IndirectRegisterField {
    pub name: String,
    pub description: String,
    pub bank: u8,
    pub register: u8,
    pub bit_offset: u8,
    pub bit_width: u8,
    pub sources: Vec<String>,
}

/// Select whether an operation receives a direct closed-PAC facade bridge.
///
/// `RawOnly` keeps the generated leaf available to a hand-written affine
/// ownership module in the closed parent PAC without also generating a
/// redundant full-block facade wrapper.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PacApiExposure {
    Facade,
    RawOnly,
}

impl PacApiExposure {
    pub const fn exposes_facade(self) -> bool {
        matches!(self, Self::Facade)
    }
}

macro_rules! common_operation {
    ($name:ident { $($field:ident: $type:ty),* $(,)? }) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        pub struct $name {
            pub name: String,
            pub peripheral: String,
            $(pub $field: $type,)*
            pub sources: Vec<String>,
        }
    };
}

common_operation!(InterruptSnapshot {
    status_register: String,
    clear_register: String,
});
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FullRegisterWrite {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    pub field: String,
    /// Reviewed value domain accepted by the closed-PAC bridge.
    ///
    /// Schema 5 intentionally has no untyped compatibility path: every
    /// complete-register write crossing from the closed PAC into the raw PAC
    /// must carry a register-specific or otherwise reviewed value type.
    pub domain: String,
    pub exposure: PacApiExposure,
    pub sources: Vec<String>,
}
/// One complete-register observation returned through a reviewed data domain.
///
/// The raw generated leaf returns `u32`; the optional facade bridge wraps that
/// image in the declared domain. `RawOnly` is intended for affine ownership
/// modules which must not recover the complete SVD block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FullRegisterRead {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    pub field: String,
    pub domain: String,
    pub exposure: PacApiExposure,
    pub sources: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldRead {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    pub field: String,
    #[serde(default)]
    pub signed: bool,
    pub sources: Vec<String>,
}
common_operation!(FieldSnapshotRead {
    register: String,
    fields: Vec<String>,
});
common_operation!(FixedRegisterWrite {
    register: String,
    field: String,
    variant: String,
});
common_operation!(FixedRegisterImage {
    register: String,
    value: u32,
});
/// One reviewed ordered transaction composed only of fixed complete-register images.
///
/// Array indices are facts of the transaction and therefore remain in the
/// generated leaf instead of crossing into hand-written PAC code.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FixedRegisterSequence {
    pub name: String,
    pub peripheral: String,
    pub steps: Vec<FixedRegisterSequenceStep>,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FixedRegisterSequenceStep {
    pub register: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    pub value: u32,
}
/// One sampled, same-register write-one-to-clear transaction.
///
/// The generated sample is an affine token: callers may inspect declared
/// fields through generated accessors, but cannot construct, clone or
/// acknowledge it more than once. The acknowledge leaf consumes that token
/// and writes exactly the sampled declared fields back to the same register.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct W1cRegisterSnapshot {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    pub sources: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterImageWrite {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    /// Reviewed domain for the complete image crossing into the raw PAC.
    pub domain: String,
    pub exposure: PacApiExposure,
    pub sources: Vec<String>,
}
/// Read-only observation of every readable register of one ownership
/// partition, by index, for hardware cross-checks of calibrated state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PartitionImageRead {
    pub name: String,
    /// Ownership partition type whose readable registers are observed.
    pub partition: String,
    pub exposure: PacApiExposure,
    pub sources: Vec<String>,
}

/// One complete ordinary-register image captured into a reviewed opaque or
/// otherwise full-width value domain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterImageRead {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    pub domain: String,
    pub exposure: PacApiExposure,
    pub sources: Vec<String>,
}
common_operation!(ZeroBasedFieldWrite {
    register: String,
    fields: Vec<String>,
});
// One fresh one-bit sample republished into an otherwise zero register image.
// This is distinct from an RMW: all fields other than the sampled bit are
// deliberately written as zero.
common_operation!(SampledBitZeroWrite {
    register: String,
    field: String,
});
common_operation!(ZeroRegisterWrite { register: String });
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MaskedRegisterModify {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    /// Reviewed domain for the caller-controlled portion of the image.
    pub domain: String,
    pub preserve_mask: u32,
    pub input_mask: u32,
    pub set_mask: u32,
    pub exposure: PacApiExposure,
    pub sources: Vec<String>,
}

/// Read-modify-write transaction that ORs one reviewed, field-local value into
/// the fresh register observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldOrModify {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    /// Disjoint SVD fields updated by one fresh-read RMW. Each projection
    /// selects its value from the logical input image rather than exposing
    /// register geometry to the handwritten caller.
    pub fields: Vec<FieldOrProjection>,
    /// Zero-based bounded logical input for caller-controlled transactions.
    pub domain: Option<String>,
    /// Fixed logical input for argument-free initialization transactions.
    pub value: Option<u32>,
    pub exposure: PacApiExposure,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldOrProjection {
    pub field: String,
    pub source_bit_offset: u8,
}

/// Read-modify-write transaction that replaces reviewed SVD fields while
/// preserving every other bit from one fresh register observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldReplaceModify {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    /// Disjoint SVD fields replaced from one logical input image.
    pub fields: Vec<FieldOrProjection>,
    /// Zero-based bounded logical input for caller-controlled transactions.
    pub domain: Option<String>,
    /// Fixed logical input for argument-free initialization transactions.
    pub value: Option<u32>,
    /// Reviewed arithmetic and truncation applied before field projection.
    /// This keeps register-width transforms inside generated PAC code.
    pub input_transform: Option<FieldInputTransform>,
    pub exposure: PacApiExposure,
    pub sources: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldInputTransform {
    #[serde(default)]
    pub wrapping_add: u32,
    pub wrapping_subtract: u32,
    #[serde(default)]
    pub clamp_maximum: Option<u32>,
    pub retain_mask: u32,
}

/// One fresh-read RMW that replaces disjoint fields from independently typed
/// logical arguments.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldArgumentModify {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    /// Each argument owns one complete SVD field. An omitted domain denotes a
    /// Boolean argument and is valid only for a one-bit field.
    pub arguments: Vec<FieldModifyArgument>,
    pub exposure: PacApiExposure,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldModifyArgument {
    pub name: String,
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// Read-modify-write transaction that sets one bit selected by a reviewed
/// bounded index domain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct IndexedBitSetModify {
    pub name: String,
    pub peripheral: String,
    pub register: String,
    pub field: String,
    pub domain: String,
    pub exposure: PacApiExposure,
    pub sources: Vec<String>,
}

impl PacApiPack {
    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let pack: Self = toml_edit::de::from_str(&input)
            .map_err(|error| Error::manifest("PAC API pack", path, error))?;
        pack.validate()
            .map_err(|error| Error::manifest("PAC API pack", path, error))?;
        Ok(pack)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != 5 {
            return Err(Error::message(format!(
                "PAC API pack requires schema = 5, got {}",
                self.schema
            )));
        }
        self.validate_ownership_partitions()?;
        self.validate_domains()?;
        let domain_names = self.domain_names();
        self.validate_indirect_register_field_domains(&domain_names)?;
        validate_operations("interrupt-snapshot", &self.interrupt_snapshots)?;
        validate_operations("full-register-write", &self.full_register_writes)?;
        validate_operations("full-register-read", &self.full_register_reads)?;
        validate_operations("field-read", &self.field_reads)?;
        validate_operations("field-snapshot-read", &self.field_snapshot_reads)?;
        validate_operations("fixed-register-write", &self.fixed_register_writes)?;
        validate_operations("fixed-register-image", &self.fixed_register_images)?;
        validate_operations("fixed-register-sequence", &self.fixed_register_sequences)?;
        validate_operations("w1c-register-snapshot", &self.w1c_register_snapshots)?;
        validate_operations("register-image-read", &self.register_image_reads)?;
        validate_operations("register-image-write", &self.register_image_writes)?;
        self.validate_partition_image_reads()?;
        validate_operations("zero-based-field-write", &self.zero_based_field_writes)?;
        validate_operations("sampled-bit-zero-write", &self.sampled_bit_zero_writes)?;
        validate_operations("zero-register-write", &self.zero_register_writes)?;
        validate_operations("masked-register-modify", &self.masked_register_modifies)?;
        validate_operations("field-or-modify", &self.field_or_modifies)?;
        validate_operations("field-replace-modify", &self.field_replace_modifies)?;
        validate_operations("field-argument-modify", &self.field_argument_modifies)?;
        validate_operations("indexed-bit-set-modify", &self.indexed_bit_set_modifies)?;

        for operation in &self.fixed_register_sequences {
            if operation.steps.is_empty() {
                return Err(Error::message(format!(
                    "PAC API fixed-register-sequence {:?} requires at least one step",
                    operation.name
                )));
            }
            for step in &operation.steps {
                validate_component("register", &operation.name, &step.register)?;
            }
        }

        for operation in &self.field_or_modifies {
            if operation.fields.is_empty() {
                return Err(Error::message(format!(
                    "PAC API field-or-modify {:?} requires at least one field projection",
                    operation.name
                )));
            }
            let mut fields = BTreeSet::new();
            for projection in &operation.fields {
                validate_component("field", &operation.name, &projection.field)?;
                if !fields.insert(projection.field.as_str()) {
                    return Err(Error::message(format!(
                        "PAC API field-or-modify {:?} repeats field {:?}",
                        operation.name, projection.field
                    )));
                }
            }
            if operation.domain.is_some() == operation.value.is_some() {
                return Err(Error::message(format!(
                    "PAC API field-or-modify {:?} requires exactly one of domain or value",
                    operation.name
                )));
            }
            if !operation.exposure.exposes_facade() && operation.value.is_some() {
                return Err(Error::message(format!(
                    "PAC API fixed field-or-modify {:?} must expose its argument-free facade",
                    operation.name
                )));
            }
        }
        for operation in &self.field_replace_modifies {
            if operation.fields.is_empty() {
                return Err(Error::message(format!(
                    "PAC API field-replace-modify {:?} requires at least one field projection",
                    operation.name
                )));
            }
            let mut fields = BTreeSet::new();
            for projection in &operation.fields {
                validate_component("field", &operation.name, &projection.field)?;
                if !fields.insert(projection.field.as_str()) {
                    return Err(Error::message(format!(
                        "PAC API field-replace-modify {:?} repeats field {:?}",
                        operation.name, projection.field
                    )));
                }
            }
            if operation.domain.is_some() == operation.value.is_some() {
                return Err(Error::message(format!(
                    "PAC API field-replace-modify {:?} requires exactly one of domain or value",
                    operation.name
                )));
            }
            if operation.input_transform.is_some() && operation.domain.is_none() {
                return Err(Error::message(format!(
                    "PAC API field-replace-modify {:?} input transform requires a domain",
                    operation.name
                )));
            }
            if operation
                .input_transform
                .is_some_and(|transform| transform.retain_mask == 0)
            {
                return Err(Error::message(format!(
                    "PAC API field-replace-modify {:?} input transform requires a non-zero retain mask",
                    operation.name
                )));
            }
            if operation.input_transform.is_some_and(|transform| {
                transform.wrapping_add != 0 && transform.wrapping_subtract != 0
            }) {
                return Err(Error::message(format!(
                    "PAC API field-replace-modify {:?} input transform cannot combine wrapping addition and subtraction",
                    operation.name
                )));
            }
            if !operation.exposure.exposes_facade() && operation.value.is_some() {
                return Err(Error::message(format!(
                    "PAC API fixed field-replace-modify {:?} must expose its argument-free facade",
                    operation.name
                )));
            }
        }
        for operation in &self.field_argument_modifies {
            if operation.arguments.len() < 2 {
                return Err(Error::message(format!(
                    "PAC API field-argument-modify {:?} requires at least two arguments",
                    operation.name
                )));
            }
            let mut names = BTreeSet::new();
            let mut fields = BTreeSet::new();
            for argument in &operation.arguments {
                if !is_lower_snake_case(&argument.name) {
                    return Err(Error::message(format!(
                        "PAC API field-argument-modify {:?} argument {:?} is not lower snake case",
                        operation.name, argument.name
                    )));
                }
                validate_component("field", &operation.name, &argument.field)?;
                if !names.insert(argument.name.as_str()) {
                    return Err(Error::message(format!(
                        "PAC API field-argument-modify {:?} repeats argument {:?}",
                        operation.name, argument.name
                    )));
                }
                if !fields.insert(argument.field.as_str()) {
                    return Err(Error::message(format!(
                        "PAC API field-argument-modify {:?} repeats field {:?}",
                        operation.name, argument.field
                    )));
                }
            }
        }
        for operation in &self.indexed_bit_set_modifies {
            validate_component("field", &operation.name, &operation.field)?;
        }

        for operation in &self.fixed_register_writes {
            validate_component("field", &operation.name, &operation.field)?;
            validate_component("variant", &operation.name, &operation.variant)?;
        }
        for operation in &self.full_register_writes {
            validate_component("field", &operation.name, &operation.field)?;
            self.validate_operation_domain(
                "full-register-write",
                &operation.name,
                &operation.peripheral,
                &operation.register,
                &operation.domain,
                &domain_names,
            )?;
        }
        for operation in &self.full_register_reads {
            validate_component("field", &operation.name, &operation.field)?;
            self.validate_operation_domain(
                "full-register-read",
                &operation.name,
                &operation.peripheral,
                &operation.register,
                &operation.domain,
                &domain_names,
            )?;
            let covers_every_word = self.bounded_domains.iter().any(|domain| {
                domain.name == operation.domain && domain.min == 0 && domain.max == u32::MAX
            }) || self
                .opaque_domains
                .iter()
                .any(|domain| domain.name == operation.domain);
            if !covers_every_word {
                return Err(Error::message(format!(
                    "PAC API full-register-read {:?} domain must represent every u32",
                    operation.name
                )));
            }
        }
        for operation in &self.field_reads {
            validate_component("field", &operation.name, &operation.field)?;
        }
        for operation in &self.field_snapshot_reads {
            if operation.fields.len() < 2 {
                return Err(Error::message(format!(
                    "PAC API field-snapshot-read {:?} requires at least two fields",
                    operation.name
                )));
            }
            let mut fields = BTreeSet::new();
            for field in &operation.fields {
                validate_component("field", &operation.name, field)?;
                if !fields.insert(field) {
                    return Err(Error::message(format!(
                        "PAC API field-snapshot-read {:?} repeats field {field:?}",
                        operation.name
                    )));
                }
            }
        }
        for operation in &self.register_image_reads {
            self.validate_operation_domain(
                "register-image-read",
                &operation.name,
                &operation.peripheral,
                &operation.register,
                &operation.domain,
                &domain_names,
            )?;
            let covers_every_word = self.bounded_domains.iter().any(|domain| {
                domain.name == operation.domain && domain.min == 0 && domain.max == u32::MAX
            }) || self
                .opaque_domains
                .iter()
                .any(|domain| domain.name == operation.domain);
            if !covers_every_word {
                return Err(Error::message(format!(
                    "PAC API register-image-read {:?} domain must represent every u32",
                    operation.name
                )));
            }
        }
        for operation in &self.register_image_writes {
            self.validate_operation_domain(
                "register-image-write",
                &operation.name,
                &operation.peripheral,
                &operation.register,
                &operation.domain,
                &domain_names,
            )?;
        }
        for operation in &self.masked_register_modifies {
            self.validate_operation_domain(
                "masked-register-modify",
                &operation.name,
                &operation.peripheral,
                &operation.register,
                &operation.domain,
                &domain_names,
            )?;
        }
        for operation in &self.field_or_modifies {
            if let Some(domain_name) = &operation.domain {
                self.validate_operation_domain(
                    "field-or-modify",
                    &operation.name,
                    &operation.peripheral,
                    &operation.register,
                    domain_name,
                    &domain_names,
                )?;
                if let Some(domain) = self
                    .bounded_domains
                    .iter()
                    .find(|candidate| candidate.name == *domain_name)
                {
                    if domain.min != 0 {
                        return Err(Error::message(format!(
                            "PAC API field-or-modify {:?} bounded domain must start at zero",
                            operation.name
                        )));
                    }
                } else if !self
                    .enum_domains
                    .iter()
                    .any(|candidate| candidate.name == *domain_name)
                {
                    return Err(Error::message(format!(
                        "PAC API field-or-modify {:?} requires a bounded or enum domain",
                        operation.name
                    )));
                }
            }
        }
        for operation in &self.field_replace_modifies {
            if let Some(domain_name) = &operation.domain {
                self.validate_operation_domain(
                    "field-replace-modify",
                    &operation.name,
                    &operation.peripheral,
                    &operation.register,
                    domain_name,
                    &domain_names,
                )?;
                if let Some(domain) = self
                    .bounded_domains
                    .iter()
                    .find(|candidate| candidate.name == *domain_name)
                {
                    if domain.min != 0 {
                        return Err(Error::message(format!(
                            "PAC API field-replace-modify {:?} bounded domain must start at zero",
                            operation.name
                        )));
                    }
                } else if !self
                    .enum_domains
                    .iter()
                    .any(|candidate| candidate.name == *domain_name)
                    && !self
                        .bitwise_composed_domains
                        .iter()
                        .any(|candidate| candidate.name == *domain_name)
                {
                    return Err(Error::message(format!(
                        "PAC API field-replace-modify {:?} requires a bounded, enum, or bitwise-composed domain",
                        operation.name
                    )));
                }
            }
        }
        for operation in &self.field_argument_modifies {
            for argument in &operation.arguments {
                let Some(domain_name) = &argument.domain else {
                    continue;
                };
                self.validate_operation_domain(
                    "field-argument-modify",
                    &operation.name,
                    &operation.peripheral,
                    &operation.register,
                    domain_name,
                    &domain_names,
                )?;
                if !self
                    .flag_domains
                    .iter()
                    .any(|candidate| candidate.name == *domain_name)
                    && !self
                        .enum_domains
                        .iter()
                        .any(|candidate| candidate.name == *domain_name)
                    && !self
                        .bounded_domains
                        .iter()
                        .any(|candidate| candidate.name == *domain_name)
                {
                    return Err(Error::message(format!(
                        "PAC API field-argument-modify {:?} argument {:?} requires a flag, enum or bounded domain",
                        operation.name, argument.name
                    )));
                }
            }
        }
        for operation in &self.indexed_bit_set_modifies {
            self.validate_operation_domain(
                "indexed-bit-set-modify",
                &operation.name,
                &operation.peripheral,
                &operation.register,
                &operation.domain,
                &domain_names,
            )?;
            if let Some(domain) = self
                .bounded_domains
                .iter()
                .find(|candidate| candidate.name == operation.domain)
            {
                if domain.min != 0 || domain.max > 31 {
                    return Err(Error::message(format!(
                        "PAC API indexed-bit-set-modify {:?} bounded domain must be within 0..=31",
                        operation.name
                    )));
                }
                continue;
            }
            if let Some(domain) = self
                .enum_domains
                .iter()
                .find(|candidate| candidate.name == operation.domain)
            {
                if domain.values.iter().any(|value| value.value > 31) {
                    return Err(Error::message(format!(
                        "PAC API indexed-bit-set-modify {:?} enum values must be within 0..=31",
                        operation.name
                    )));
                }
                continue;
            }
            return Err(Error::message(format!(
                "PAC API indexed-bit-set-modify {:?} requires a bounded or enum domain",
                operation.name
            )));
        }
        for operation in &self.interrupt_snapshots {
            validate_component(
                "status-register",
                &operation.name,
                &operation.status_register,
            )?;
            validate_component("clear-register", &operation.name, &operation.clear_register)?;
        }
        for operation in &self.zero_based_field_writes {
            if operation.fields.is_empty() {
                return Err(Error::message(format!(
                    "PAC API operation {:?} requires at least one field",
                    operation.name
                )));
            }
            let mut fields = BTreeSet::new();
            for field in &operation.fields {
                validate_component("field", &operation.name, field)?;
                if !fields.insert(field) {
                    return Err(Error::message(format!(
                        "PAC API operation {:?} repeats field {field:?}",
                        operation.name
                    )));
                }
            }
        }
        for operation in &self.sampled_bit_zero_writes {
            validate_component("field", &operation.name, &operation.field)?;
        }
        for operation in &self.masked_register_modifies {
            if operation.preserve_mask & operation.input_mask != 0
                || operation.preserve_mask & operation.set_mask != 0
                || operation.input_mask & operation.set_mask != 0
            {
                return Err(Error::message(format!(
                    "PAC API masked-register-modify {:?} has overlapping masks",
                    operation.name
                )));
            }
            if operation.preserve_mask | operation.input_mask | operation.set_mask != u32::MAX {
                return Err(Error::message(format!(
                    "PAC API masked-register-modify {:?} masks do not partition all 32 bits",
                    operation.name
                )));
            }
            if let Some(domain) = self
                .bitwise_composed_domains
                .iter()
                .find(|domain| domain.name == operation.domain)
                && domain.output_mask & !operation.input_mask != 0
            {
                return Err(Error::message(format!(
                    "PAC API masked-register-modify {:?} truncates bits admitted by bitwise-composed domain {:?}",
                    operation.name, domain.name
                )));
            }
        }
        Ok(())
    }

    fn validate_partition_image_reads(&self) -> Result<()> {
        let mut names = BTreeSet::new();
        for operation in &self.partition_image_reads {
            if !is_lower_snake_case(&operation.name)
                || is_rust_keyword(&operation.name)
                || !names.insert(operation.name.as_str())
            {
                return Err(Error::message(format!(
                    "PAC API partition-image-read name {:?} is not a unique lower snake case Rust name",
                    operation.name
                )));
            }
            if !self
                .ownership_partitions
                .iter()
                .any(|partition| partition.name == operation.partition)
            {
                return Err(Error::message(format!(
                    "PAC API partition-image-read {:?} names unknown ownership partition {:?}",
                    operation.name, operation.partition
                )));
            }
            validate_sources("partition-image-read", &operation.name, &operation.sources)?;
        }
        Ok(())
    }

    fn validate_ownership_partitions(&self) -> Result<()> {
        let mut names = BTreeSet::new();
        let mut members = BTreeSet::new();
        let mut peripherals = BTreeSet::new();
        for partition in &self.ownership_partitions {
            if !is_upper_camel_case(&partition.name)
                || partition.name == "PeripheralPartitions"
                || is_rust_keyword(&partition.name)
            {
                return Err(Error::message(format!(
                    "PAC API ownership partition name {:?} is not an available UpperCamelCase Rust type",
                    partition.name
                )));
            }
            if !names.insert(partition.name.as_str()) {
                return Err(Error::message(format!(
                    "PAC API ownership partition repeats type {:?}",
                    partition.name
                )));
            }
            if !is_lower_snake_case(&partition.member) || is_rust_keyword(&partition.member) {
                return Err(Error::message(format!(
                    "PAC API ownership partition {} member {:?} is not lower_snake_case",
                    partition.name, partition.member
                )));
            }
            if !members.insert(partition.member.as_str()) {
                return Err(Error::message(format!(
                    "PAC API ownership partition repeats root member {:?}",
                    partition.member
                )));
            }
            if partition.description.trim().is_empty() || partition.peripherals.is_empty() {
                return Err(Error::message(format!(
                    "PAC API ownership partition {:?} requires a description and at least one exact peripheral",
                    partition.name
                )));
            }
            for peripheral in &partition.peripherals {
                if !is_svd_identifier(peripheral) {
                    return Err(Error::message(format!(
                        "PAC API ownership partition {} has invalid exact peripheral {:?}",
                        partition.name, peripheral
                    )));
                }
                if !peripherals.insert(peripheral.as_str()) {
                    return Err(Error::message(format!(
                        "PAC API ownership assigns peripheral {peripheral:?} more than once"
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_operation_domain(
        &self,
        kind: &str,
        operation: &str,
        peripheral: &str,
        register: &str,
        domain: &str,
        domain_names: &BTreeSet<&str>,
    ) -> Result<()> {
        validate_component("domain", operation, domain)?;
        if !domain_names.contains(domain) {
            return Err(Error::message(format!(
                "PAC API {kind} {operation:?} references unknown domain {domain:?}"
            )));
        }
        if let Some(opaque) = self
            .opaque_domains
            .iter()
            .find(|candidate| candidate.name == domain)
            && (opaque.peripheral != peripheral || opaque.register != register)
        {
            return Err(Error::message(format!(
                "PAC API {kind} {operation:?} uses opaque domain {domain:?} outside its register {}.{}",
                opaque.peripheral, opaque.register
            )));
        }
        if let Some(composed) = self
            .bitwise_composed_domains
            .iter()
            .find(|candidate| candidate.name == domain)
            && (composed.peripheral != peripheral || composed.register != register)
        {
            return Err(Error::message(format!(
                "PAC API {kind} {operation:?} uses bitwise-composed domain {domain:?} outside its register {}.{}",
                composed.peripheral, composed.register
            )));
        }
        Ok(())
    }

    pub fn operation_count(&self) -> usize {
        self.interrupt_snapshots.len()
            + self.full_register_writes.len()
            + self.full_register_reads.len()
            + self.field_reads.len()
            + self.field_snapshot_reads.len()
            + self.fixed_register_writes.len()
            + self.fixed_register_images.len()
            + self.fixed_register_sequences.len()
            + self.w1c_register_snapshots.len()
            + self.register_image_reads.len()
            + self.register_image_writes.len()
            + self.partition_image_reads.len()
            + self.zero_based_field_writes.len()
            + self.sampled_bit_zero_writes.len()
            + self.zero_register_writes.len()
            + self.masked_register_modifies.len()
            + self.field_or_modifies.len()
            + self.field_replace_modifies.len()
            + self.field_argument_modifies.len()
            + self.indexed_bit_set_modifies.len()
    }

    pub fn domain_count(&self) -> usize {
        self.flag_domains.len()
            + self.enum_domains.len()
            + self.bounded_domains.len()
            + self.opaque_domains.len()
            + self.bitwise_composed_domains.len()
            + self.indirect_register_field_domains.len()
    }

    pub fn ownership_partition_count(&self) -> usize {
        self.ownership_partitions.len()
    }

    pub fn source_ids(&self) -> BTreeSet<&str> {
        self.flag_domains
            .iter()
            .flat_map(|domain| &domain.values)
            .flat_map(|value| &value.sources)
            .map(String::as_str)
            .chain(
                self.enum_domains
                    .iter()
                    .flat_map(|domain| &domain.values)
                    .flat_map(|value| &value.sources)
                    .map(String::as_str),
            )
            .chain(
                self.bounded_domains
                    .iter()
                    .flat_map(|domain| &domain.sources)
                    .map(String::as_str),
            )
            .chain(
                self.opaque_domains
                    .iter()
                    .flat_map(|domain| &domain.sources)
                    .map(String::as_str),
            )
            .chain(
                self.bitwise_composed_domains
                    .iter()
                    .flat_map(|domain| &domain.sources)
                    .map(String::as_str),
            )
            .chain(
                self.indirect_register_field_domains
                    .iter()
                    .flat_map(|domain| &domain.fields)
                    .flat_map(|field| &field.sources)
                    .map(String::as_str),
            )
            .chain(
                self.interrupt_snapshots
                    .iter()
                    .map(Operation::sources)
                    .chain(self.full_register_writes.iter().map(Operation::sources))
                    .chain(self.full_register_reads.iter().map(Operation::sources))
                    .chain(self.field_reads.iter().map(Operation::sources))
                    .chain(self.field_snapshot_reads.iter().map(Operation::sources))
                    .chain(self.fixed_register_writes.iter().map(Operation::sources))
                    .chain(self.fixed_register_images.iter().map(Operation::sources))
                    .chain(self.fixed_register_sequences.iter().map(Operation::sources))
                    .chain(self.w1c_register_snapshots.iter().map(Operation::sources))
                    .chain(self.register_image_reads.iter().map(Operation::sources))
                    .chain(self.register_image_writes.iter().map(Operation::sources))
                    .chain(
                        self.partition_image_reads
                            .iter()
                            .map(|operation| operation.sources.as_slice()),
                    )
                    .chain(self.zero_based_field_writes.iter().map(Operation::sources))
                    .chain(self.sampled_bit_zero_writes.iter().map(Operation::sources))
                    .chain(self.zero_register_writes.iter().map(Operation::sources))
                    .chain(self.masked_register_modifies.iter().map(Operation::sources))
                    .chain(self.field_or_modifies.iter().map(Operation::sources))
                    .chain(self.field_replace_modifies.iter().map(Operation::sources))
                    .chain(self.field_argument_modifies.iter().map(Operation::sources))
                    .chain(self.indexed_bit_set_modifies.iter().map(Operation::sources))
                    .flatten()
                    .map(String::as_str),
            )
            .collect()
    }

    /// Peripherals targeted by the pack's register transactions.
    ///
    /// Partition image reads name an ownership partition instead of a
    /// peripheral and are not listed.
    pub fn operation_peripherals(&self) -> BTreeSet<&str> {
        let mut peripherals = BTreeSet::new();
        macro_rules! collect {
            ($($operations:ident),* $(,)?) => {
                $(peripherals.extend(self.$operations.iter().map(Operation::peripheral));)*
            };
        }
        collect!(
            interrupt_snapshots,
            full_register_writes,
            full_register_reads,
            field_reads,
            field_snapshot_reads,
            fixed_register_writes,
            fixed_register_images,
            fixed_register_sequences,
            w1c_register_snapshots,
            register_image_reads,
            register_image_writes,
            zero_based_field_writes,
            sampled_bit_zero_writes,
            zero_register_writes,
            masked_register_modifies,
            field_or_modifies,
            field_replace_modifies,
            field_argument_modifies,
            indexed_bit_set_modifies,
        );
        peripherals
    }

    fn validate_domains(&self) -> Result<()> {
        let mut domain_names = BTreeSet::new();
        for domain in &self.flag_domains {
            validate_domain_header("flag", &domain.name, &domain.description, &mut domain_names)?;
            if domain.description.trim().is_empty() || domain.values.is_empty() {
                return Err(Error::message(format!(
                    "PAC API flag domain {:?} requires a description and at least one value",
                    domain.name
                )));
            }
            let mut value_names = BTreeSet::new();
            let mut values = BTreeSet::new();
            for value in &domain.values {
                if !is_upper_snake_case(&value.name) {
                    return Err(Error::message(format!(
                        "PAC API flag value {}::{:?} is not UPPER_SNAKE_CASE",
                        domain.name, value.name
                    )));
                }
                if !value_names.insert(value.name.as_str()) || !values.insert(value.value) {
                    return Err(Error::message(format!(
                        "PAC API flag domain {:?} repeats a name or numeric value",
                        domain.name
                    )));
                }
                if value.description.trim().is_empty() || value.sources.is_empty() {
                    return Err(Error::message(format!(
                        "PAC API flag value {}::{} requires a description and evidence sources",
                        domain.name, value.name
                    )));
                }
                let mut sources = BTreeSet::new();
                if value
                    .sources
                    .iter()
                    .any(|source| source.is_empty() || !sources.insert(source))
                {
                    return Err(Error::message(format!(
                        "PAC API flag value {}::{} has an empty or duplicate source",
                        domain.name, value.name
                    )));
                }
            }
        }
        for domain in &self.enum_domains {
            validate_domain_header("enum", &domain.name, &domain.description, &mut domain_names)?;
            if domain.values.is_empty() {
                return Err(Error::message(format!(
                    "PAC API enum domain {:?} requires at least one value",
                    domain.name
                )));
            }
            let mut value_names = BTreeSet::new();
            let mut values = BTreeSet::new();
            for value in &domain.values {
                if !is_upper_camel_case(&value.name) {
                    return Err(Error::message(format!(
                        "PAC API enum value {}::{:?} is not UpperCamelCase",
                        domain.name, value.name
                    )));
                }
                if !value_names.insert(value.name.as_str()) || !values.insert(value.value) {
                    return Err(Error::message(format!(
                        "PAC API enum domain {:?} repeats a name or numeric value",
                        domain.name
                    )));
                }
                validate_domain_evidence(
                    "enum value",
                    &format!("{}::{}", domain.name, value.name),
                    &value.description,
                    &value.sources,
                )?;
            }
        }
        for domain in &self.bounded_domains {
            validate_domain_header(
                "bounded",
                &domain.name,
                &domain.description,
                &mut domain_names,
            )?;
            if domain.min > domain.max {
                return Err(Error::message(format!(
                    "PAC API bounded domain {:?} has min greater than max",
                    domain.name
                )));
            }
            validate_sources("bounded domain", &domain.name, &domain.sources)?;
        }
        for domain in &self.opaque_domains {
            validate_domain_header(
                "opaque",
                &domain.name,
                &domain.description,
                &mut domain_names,
            )?;
            validate_component("peripheral", &domain.name, &domain.peripheral)?;
            validate_component("register", &domain.name, &domain.register)?;
            validate_sources("opaque domain", &domain.name, &domain.sources)?;
        }
        for domain in &self.bitwise_composed_domains {
            validate_domain_header(
                "bitwise-composed",
                &domain.name,
                &domain.description,
                &mut domain_names,
            )?;
            validate_component("peripheral", &domain.name, &domain.peripheral)?;
            validate_component("register", &domain.name, &domain.register)?;
            if domain.arguments.is_empty() {
                return Err(Error::message(format!(
                    "PAC API bitwise-composed domain {:?} requires at least one argument",
                    domain.name
                )));
            }
            if domain.output_mask == 0 {
                return Err(Error::message(format!(
                    "PAC API bitwise-composed domain {:?} requires a non-zero output mask",
                    domain.name
                )));
            }
            let mut argument_names = BTreeSet::new();
            for argument in &domain.arguments {
                if !is_lower_snake_case(&argument.name) || is_rust_keyword(&argument.name) {
                    return Err(Error::message(format!(
                        "PAC API bitwise-composed domain {:?} argument {:?} is not an available lower snake case name",
                        domain.name, argument.name
                    )));
                }
                if !argument_names.insert(argument.name.as_str()) {
                    return Err(Error::message(format!(
                        "PAC API bitwise-composed domain {:?} repeats argument {:?}",
                        domain.name, argument.name
                    )));
                }
                if argument.right_shift >= 32 || argument.left_shift >= 32 {
                    return Err(Error::message(format!(
                        "PAC API bitwise-composed domain {:?} argument {:?} shifts outside u32",
                        domain.name, argument.name
                    )));
                }
                if argument.retain_mask == 0 {
                    return Err(Error::message(format!(
                        "PAC API bitwise-composed domain {:?} argument {:?} retains no bits",
                        domain.name, argument.name
                    )));
                }
            }
            validate_sources("bitwise-composed domain", &domain.name, &domain.sources)?;
        }
        Ok(())
    }

    fn validate_indirect_register_field_domains(
        &self,
        existing_type_names: &BTreeSet<&str>,
    ) -> Result<()> {
        let mut type_names = existing_type_names.clone();
        let mut module_names = BTreeSet::new();
        for domain in &self.indirect_register_field_domains {
            if !is_upper_camel_case(&domain.name) || is_rust_keyword(&domain.name) {
                return Err(Error::message(format!(
                    "PAC API indirect-register field domain name {:?} is not an available UpperCamelCase Rust type",
                    domain.name
                )));
            }
            if !type_names.insert(domain.name.as_str()) {
                return Err(Error::message(format!(
                    "PAC API repeats or collides with indirect-register field domain {:?}",
                    domain.name
                )));
            }
            if !is_lower_snake_case(&domain.module)
                || is_rust_keyword(&domain.module)
                || !module_names.insert(domain.module.as_str())
            {
                return Err(Error::message(format!(
                    "PAC API indirect-register field domain {} has invalid or duplicate module {:?}",
                    domain.name, domain.module
                )));
            }
            if domain.description.trim().is_empty() || domain.fields.is_empty() {
                return Err(Error::message(format!(
                    "PAC API indirect-register field domain {:?} requires a description and at least one field",
                    domain.name
                )));
            }
            if !matches!(domain.value_bits, 8 | 16 | 32) {
                return Err(Error::message(format!(
                    "PAC API indirect-register field domain {:?} value-bits must be 8, 16, or 32",
                    domain.name
                )));
            }
            let mut field_names = BTreeSet::new();
            for field in &domain.fields {
                if !is_upper_snake_case(&field.name) || !field_names.insert(field.name.as_str()) {
                    return Err(Error::message(format!(
                        "PAC API indirect-register field {}::{:?} is not a unique UPPER_SNAKE_CASE name",
                        domain.name, field.name
                    )));
                }
                if field.bit_width == 0
                    || u16::from(field.bit_offset) + u16::from(field.bit_width)
                        > u16::from(domain.value_bits)
                {
                    return Err(Error::message(format!(
                        "PAC API indirect-register field {}::{} lies outside its {}-bit register value",
                        domain.name, field.name, domain.value_bits
                    )));
                }
                validate_domain_evidence(
                    "indirect-register field",
                    &format!("{}::{}", domain.name, field.name),
                    &field.description,
                    &field.sources,
                )?;
            }
        }
        Ok(())
    }

    fn domain_names(&self) -> BTreeSet<&str> {
        self.flag_domains
            .iter()
            .map(|domain| domain.name.as_str())
            .chain(self.enum_domains.iter().map(|domain| domain.name.as_str()))
            .chain(
                self.bounded_domains
                    .iter()
                    .map(|domain| domain.name.as_str()),
            )
            .chain(
                self.opaque_domains
                    .iter()
                    .map(|domain| domain.name.as_str()),
            )
            .chain(
                self.bitwise_composed_domains
                    .iter()
                    .map(|domain| domain.name.as_str()),
            )
            .collect()
    }
}

fn validate_domain_header<'a>(
    kind: &str,
    name: &'a str,
    description: &str,
    names: &mut BTreeSet<&'a str>,
) -> Result<()> {
    if !is_upper_camel_case(name) {
        return Err(Error::message(format!(
            "PAC API {kind} domain name {name:?} is not UpperCamelCase"
        )));
    }
    if !names.insert(name) {
        return Err(Error::message(format!(
            "PAC API contains duplicate domain {name:?}"
        )));
    }
    if description.trim().is_empty() {
        return Err(Error::message(format!(
            "PAC API {kind} domain {name:?} requires a description"
        )));
    }
    Ok(())
}

fn validate_domain_evidence(
    kind: &str,
    name: &str,
    description: &str,
    sources: &[String],
) -> Result<()> {
    if description.trim().is_empty() {
        return Err(Error::message(format!(
            "PAC API {kind} {name} requires a description"
        )));
    }
    validate_sources(kind, name, sources)
}

fn validate_sources(kind: &str, name: &str, sources: &[String]) -> Result<()> {
    if sources.is_empty() {
        return Err(Error::message(format!(
            "PAC API {kind} {name} requires evidence sources"
        )));
    }
    let mut unique = BTreeSet::new();
    if sources
        .iter()
        .any(|source| source.is_empty() || !unique.insert(source))
    {
        return Err(Error::message(format!(
            "PAC API {kind} {name} has an empty or duplicate source"
        )));
    }
    Ok(())
}

trait Operation {
    fn name(&self) -> &str;
    fn peripheral(&self) -> &str;
    fn register(&self) -> Option<&str>;
    fn sources(&self) -> &[String];
}

macro_rules! impl_register_operation {
    ($type:ty) => {
        impl Operation for $type {
            fn name(&self) -> &str {
                &self.name
            }
            fn peripheral(&self) -> &str {
                &self.peripheral
            }
            fn register(&self) -> Option<&str> {
                Some(&self.register)
            }
            fn sources(&self) -> &[String] {
                &self.sources
            }
        }
    };
}

impl Operation for InterruptSnapshot {
    fn name(&self) -> &str {
        &self.name
    }
    fn peripheral(&self) -> &str {
        &self.peripheral
    }
    fn register(&self) -> Option<&str> {
        None
    }
    fn sources(&self) -> &[String] {
        &self.sources
    }
}

impl_register_operation!(FullRegisterWrite);
impl_register_operation!(FullRegisterRead);
impl_register_operation!(FieldRead);
impl_register_operation!(FieldSnapshotRead);
impl_register_operation!(FixedRegisterWrite);
impl_register_operation!(FixedRegisterImage);
impl Operation for FixedRegisterSequence {
    fn name(&self) -> &str {
        &self.name
    }
    fn peripheral(&self) -> &str {
        &self.peripheral
    }
    fn register(&self) -> Option<&str> {
        None
    }
    fn sources(&self) -> &[String] {
        &self.sources
    }
}
impl_register_operation!(W1cRegisterSnapshot);
impl_register_operation!(RegisterImageRead);
impl_register_operation!(RegisterImageWrite);
impl_register_operation!(ZeroBasedFieldWrite);
impl_register_operation!(SampledBitZeroWrite);
impl_register_operation!(ZeroRegisterWrite);
impl_register_operation!(MaskedRegisterModify);
impl_register_operation!(FieldOrModify);
impl_register_operation!(FieldReplaceModify);
impl_register_operation!(FieldArgumentModify);
impl_register_operation!(IndexedBitSetModify);

fn validate_operations<T: Operation>(kind: &str, operations: &[T]) -> Result<()> {
    let mut names = BTreeSet::new();
    for operation in operations {
        if !is_lower_snake_case(operation.name()) {
            return Err(Error::message(format!(
                "PAC API {kind} name {:?} is not lower snake case",
                operation.name()
            )));
        }
        if !names.insert(operation.name()) {
            return Err(Error::message(format!(
                "PAC API contains duplicate {kind} name {:?}",
                operation.name()
            )));
        }
        validate_component("peripheral", operation.name(), operation.peripheral())?;
        if let Some(register) = operation.register() {
            validate_component("register", operation.name(), register)?;
        }
        if operation.sources().is_empty() {
            return Err(Error::message(format!(
                "PAC API operation {:?} has no sources",
                operation.name()
            )));
        }
        let mut sources = BTreeSet::new();
        for source in operation.sources() {
            if source.is_empty() || !sources.insert(source) {
                return Err(Error::message(format!(
                    "PAC API operation {:?} has an empty or duplicate source",
                    operation.name()
                )));
            }
        }
    }
    Ok(())
}

fn validate_component(kind: &str, operation: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(Error::message(format!(
            "PAC API operation {operation:?} has invalid {kind} {value:?}"
        )));
    }
    Ok(())
}

fn is_lower_snake_case(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn is_upper_snake_case(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn is_upper_camel_case(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && !value.contains('_')
}

fn is_svd_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

fn is_rust_keyword(value: &str) -> bool {
    matches!(
        value,
        "Self"
            | "abstract"
            | "as"
            | "async"
            | "await"
            | "become"
            | "box"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "do"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "final"
            | "fn"
            | "for"
            | "gen"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "macro"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "override"
            | "priv"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "unsafe"
            | "unsized"
            | "use"
            | "virtual"
            | "where"
            | "while"
            | "yield"
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn empty_pack() -> PacApiPack {
        PacApiPack {
            schema: 5,
            options: PacApiOptions::default(),
            ownership_partitions: Vec::new(),
            flag_domains: Vec::new(),
            enum_domains: Vec::new(),
            bounded_domains: Vec::new(),
            opaque_domains: Vec::new(),
            bitwise_composed_domains: Vec::new(),
            indirect_register_field_domains: Vec::new(),
            interrupt_snapshots: Vec::new(),
            full_register_writes: Vec::new(),
            full_register_reads: Vec::new(),
            field_reads: Vec::new(),
            field_snapshot_reads: Vec::new(),
            fixed_register_writes: Vec::new(),
            fixed_register_images: Vec::new(),
            fixed_register_sequences: Vec::new(),
            w1c_register_snapshots: Vec::new(),
            register_image_reads: Vec::new(),
            register_image_writes: Vec::new(),
            partition_image_reads: Vec::new(),
            zero_based_field_writes: Vec::new(),
            sampled_bit_zero_writes: Vec::new(),
            zero_register_writes: Vec::new(),
            masked_register_modifies: Vec::new(),
            field_or_modifies: Vec::new(),
            field_replace_modifies: Vec::new(),
            field_argument_modifies: Vec::new(),
            indexed_bit_set_modifies: Vec::new(),
        }
    }

    #[test]
    fn validates_closed_flag_domains() {
        let mut pack = empty_pack();
        pack.flag_domains.push(FlagDomain {
            name: "InterruptMask".to_owned(),
            description: "Reviewed writable interrupt bits.".to_owned(),
            values: vec![FlagValue {
                name: "RX_READY".to_owned(),
                value: 1 << 3,
                description: "Enable the receive-ready interrupt.".to_owned(),
                sources: vec!["VENDOR_IRQ_ENABLE".to_owned()],
            }],
        });
        assert_eq!(pack.domain_count(), 1);
        assert!(pack.validate().is_ok());
        assert_eq!(pack.source_ids(), BTreeSet::from(["VENDOR_IRQ_ENABLE"]));
    }

    #[test]
    fn validates_indirect_register_field_domains() {
        let mut pack = empty_pack();
        pack.indirect_register_field_domains
            .push(IndirectRegisterFieldDomain {
                name: "AnalogField".to_owned(),
                module: "analog_fields".to_owned(),
                description: "Reviewed fields on an indirect analog bus.".to_owned(),
                value_bits: 8,
                register_image: false,
                fields: vec![IndirectRegisterField {
                    name: "GAIN".to_owned(),
                    description: "Analog gain field.".to_owned(),
                    bank: 0x67,
                    register: 3,
                    bit_offset: 2,
                    bit_width: 3,
                    sources: vec!["VENDOR_ANALOG_GAIN".to_owned()],
                }],
            });

        assert_eq!(pack.domain_count(), 1);
        assert!(pack.validate().is_ok());
        assert_eq!(pack.source_ids(), BTreeSet::from(["VENDOR_ANALOG_GAIN"]));

        pack.indirect_register_field_domains[0].fields[0].bit_width = 7;
        assert!(pack.validate().is_err());

        let mut colliding_pack = empty_pack();
        colliding_pack.flag_domains.push(FlagDomain {
            name: "AnalogField".to_owned(),
            description: "Reviewed writable values.".to_owned(),
            values: vec![FlagValue {
                name: "ENABLED".to_owned(),
                value: 1,
                description: "Enable the reviewed behavior.".to_owned(),
                sources: vec!["VENDOR_ENABLE".to_owned()],
            }],
        });
        colliding_pack.indirect_register_field_domains =
            pack.indirect_register_field_domains.clone();
        colliding_pack.indirect_register_field_domains[0].fields[0].bit_width = 3;
        assert!(colliding_pack.validate().is_err());
    }

    #[test]
    fn an_indirect_register_image_lists_each_occupied_register_once_in_order() {
        let field = |name: &str, bank, register, bit_offset| IndirectRegisterField {
            name: name.to_owned(),
            description: "Analog field.".to_owned(),
            bank,
            register,
            bit_offset,
            bit_width: 1,
            sources: vec!["VENDOR_ANALOG".to_owned()],
        };
        let domain = IndirectRegisterFieldDomain {
            name: "AnalogField".to_owned(),
            module: "analog_fields".to_owned(),
            description: "Reviewed fields on an indirect analog bus.".to_owned(),
            value_bits: 8,
            register_image: true,
            fields: vec![
                field("HIGH", 0x67, 3, 7),
                field("OTHER", 0x61, 9, 0),
                field("LOW", 0x67, 3, 0),
            ],
        };
        assert_eq!(domain.image_registers(), [(0x61, 9), (0x67, 3)]);
    }

    #[test]
    fn accepts_a_partitioned_masked_transaction() {
        let mut pack = empty_pack();
        pack.opaque_domains.push(OpaqueDomain {
            name: "CommandInput".to_owned(),
            description: "Reviewed command input image.".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "COMMAND".to_owned(),
            sources: vec!["REVIEW".to_owned()],
        });
        pack.masked_register_modifies.push(MaskedRegisterModify {
            name: "publish_command".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "COMMAND".to_owned(),
            domain: "CommandInput".to_owned(),
            preserve_mask: 0xffff_0000,
            input_mask: 0x0000_fff0,
            set_mask: 0x0000_000f,
            exposure: PacApiExposure::Facade,
            sources: vec!["REVIEW".to_owned()],
        });
        assert_eq!(pack.operation_count(), 1);
        assert!(pack.validate().is_ok());
    }

    #[test]
    fn bitwise_composed_domain_rejects_invalid_argument_projection() {
        let mut pack = empty_pack();
        pack.bitwise_composed_domains.push(BitwiseComposedDomain {
            name: "CommandInput".to_owned(),
            description: "Reviewed command composed from typed arguments.".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "COMMAND".to_owned(),
            arguments: vec![
                BitwiseComposedDomainArgument {
                    name: "selector".to_owned(),
                    value_type: BitwiseComposedValueType::U8,
                    right_shift: 0,
                    retain_mask: u32::MAX,
                    left_shift: 2,
                },
                BitwiseComposedDomainArgument {
                    name: "value".to_owned(),
                    value_type: BitwiseComposedValueType::U16,
                    right_shift: 0,
                    retain_mask: u32::MAX,
                    left_shift: 6,
                },
            ],
            fixed_value: 0,
            output_mask: 0x0001_fffc,
            sources: vec!["REVIEW".to_owned()],
        });
        assert!(pack.validate().is_ok());

        pack.bitwise_composed_domains[0].arguments[1].right_shift = 32;
        assert!(pack.validate().is_err());

        pack.bitwise_composed_domains[0].arguments[1].right_shift = 0;
        pack.bitwise_composed_domains[0].arguments[1].retain_mask = 0;
        assert!(pack.validate().is_err());
    }

    #[test]
    fn field_or_modify_requires_a_zero_based_bounded_domain() {
        let mut pack = empty_pack();
        pack.bounded_domains.push(BoundedDomain {
            name: "FieldInput".to_owned(),
            description: "Reviewed field-local OR input.".to_owned(),
            min: 0,
            max: 0x000f_ffff,
            sources: vec!["REVIEW".to_owned()],
        });
        pack.field_or_modifies.push(FieldOrModify {
            name: "publish_pointer".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "POINTER".to_owned(),
            fields: vec![FieldOrProjection {
                field: "COMPRESSED_POINTER".to_owned(),
                source_bit_offset: 0,
            }],
            domain: Some("FieldInput".to_owned()),
            value: None,
            exposure: PacApiExposure::Facade,
            sources: vec!["REVIEW".to_owned()],
        });

        assert!(pack.validate().is_ok());
        pack.bounded_domains[0].min = 1;
        assert!(pack.validate().is_err());
    }

    #[test]
    fn field_replace_modify_requires_one_zero_based_input_mode() {
        let mut pack = empty_pack();
        pack.bounded_domains.push(BoundedDomain {
            name: "FieldInput".to_owned(),
            description: "Reviewed field replacement input.".to_owned(),
            min: 0,
            max: 0xff,
            sources: vec!["REVIEW".to_owned()],
        });
        pack.field_replace_modifies.push(FieldReplaceModify {
            name: "publish_field".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "CONTROL".to_owned(),
            fields: vec![FieldOrProjection {
                field: "VALUE".to_owned(),
                source_bit_offset: 0,
            }],
            domain: Some("FieldInput".to_owned()),
            value: None,
            input_transform: None,
            exposure: PacApiExposure::Facade,
            sources: vec!["REVIEW".to_owned()],
        });

        assert!(pack.validate().is_ok());
        pack.field_replace_modifies[0].value = Some(1);
        assert!(pack.validate().is_err());
        pack.field_replace_modifies[0].value = None;
        pack.bounded_domains[0].min = 1;
        assert!(pack.validate().is_err());
    }

    #[test]
    fn field_replace_modify_accepts_a_closed_enum_domain() {
        let mut pack = empty_pack();
        pack.enum_domains.push(EnumDomain {
            name: "ReviewedMode".to_owned(),
            description: "Only instruction-proven hardware modes.".to_owned(),
            values: vec![
                EnumValue {
                    name: "ModeTwo".to_owned(),
                    value: 2,
                    description: "First reviewed mode.".to_owned(),
                    sources: vec!["REVIEW".to_owned()],
                },
                EnumValue {
                    name: "ModeFive".to_owned(),
                    value: 5,
                    description: "Second reviewed mode.".to_owned(),
                    sources: vec!["REVIEW".to_owned()],
                },
            ],
        });
        pack.field_replace_modifies.push(FieldReplaceModify {
            name: "publish_mode".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "CONTROL".to_owned(),
            fields: vec![FieldOrProjection {
                field: "MODE".to_owned(),
                source_bit_offset: 0,
            }],
            domain: Some("ReviewedMode".to_owned()),
            value: None,
            input_transform: None,
            exposure: PacApiExposure::Facade,
            sources: vec!["REVIEW".to_owned()],
        });

        assert!(pack.validate().is_ok());
    }

    #[test]
    fn field_argument_modify_requires_distinct_typed_arguments() {
        let mut pack = empty_pack();
        pack.bounded_domains.push(BoundedDomain {
            name: "Divider".to_owned(),
            description: "Reviewed divider field value.".to_owned(),
            min: 0,
            max: 0x0fff,
            sources: vec!["REVIEW".to_owned()],
        });
        pack.field_argument_modifies.push(FieldArgumentModify {
            name: "configure_clock".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "CONTROL".to_owned(),
            arguments: vec![
                FieldModifyArgument {
                    name: "enabled".to_owned(),
                    field: "ENABLE".to_owned(),
                    domain: None,
                },
                FieldModifyArgument {
                    name: "divider".to_owned(),
                    field: "DIVIDER".to_owned(),
                    domain: Some("Divider".to_owned()),
                },
            ],
            exposure: PacApiExposure::Facade,
            sources: vec!["REVIEW".to_owned()],
        });

        assert!(pack.validate().is_ok());
        pack.field_argument_modifies[0].arguments[1].name = "enabled".to_owned();
        assert!(pack.validate().is_err());
    }

    #[test]
    fn register_image_read_requires_a_complete_value_domain() {
        let mut pack = empty_pack();
        pack.opaque_domains.push(OpaqueDomain {
            name: "SavedImage".to_owned(),
            description: "Register-specific saved image.".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "CONTROL".to_owned(),
            sources: vec!["REVIEW".to_owned()],
        });
        pack.register_image_reads.push(RegisterImageRead {
            name: "save_control".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "CONTROL".to_owned(),
            domain: "SavedImage".to_owned(),
            exposure: PacApiExposure::Facade,
            sources: vec!["REVIEW".to_owned()],
        });

        assert!(pack.validate().is_ok());
        pack.register_image_reads[0].register = "OTHER".to_owned();
        assert!(pack.validate().is_err());
    }

    #[test]
    fn indexed_bit_set_requires_a_machine_bounded_index() {
        let mut pack = empty_pack();
        pack.bounded_domains.push(BoundedDomain {
            name: "RequestIndex".to_owned(),
            description: "Reviewed request-bit index.".to_owned(),
            min: 0,
            max: 31,
            sources: vec!["REVIEW".to_owned()],
        });
        pack.indexed_bit_set_modifies.push(IndexedBitSetModify {
            name: "request_event".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "REQUESTS".to_owned(),
            field: "REQUESTS_UNKNOWN".to_owned(),
            domain: "RequestIndex".to_owned(),
            exposure: PacApiExposure::Facade,
            sources: vec!["REVIEW".to_owned()],
        });

        assert!(pack.validate().is_ok());
        pack.bounded_domains[0].max = 32;
        assert!(pack.validate().is_err());
    }

    #[test]
    fn indexed_bit_set_accepts_a_closed_enum_index() {
        let mut pack = empty_pack();
        pack.enum_domains.push(EnumDomain {
            name: "RequestIndex".to_owned(),
            description: "Reviewed request-bit selector.".to_owned(),
            values: vec![EnumValue {
                name: "Beacon".to_owned(),
                value: 0,
                description: "Request beacon handling.".to_owned(),
                sources: vec!["REVIEW".to_owned()],
            }],
        });
        pack.indexed_bit_set_modifies.push(IndexedBitSetModify {
            name: "request_event".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "REQUESTS".to_owned(),
            field: "REQUESTS_UNKNOWN".to_owned(),
            domain: "RequestIndex".to_owned(),
            exposure: PacApiExposure::Facade,
            sources: vec!["REVIEW".to_owned()],
        });

        assert!(pack.validate().is_ok());
        pack.enum_domains[0].values[0].value = 32;
        assert!(pack.validate().is_err());
    }

    #[test]
    fn rejects_duplicate_ownership_and_incomplete_masks() {
        let mut pack = empty_pack();
        pack.ownership_partitions.push(OwnershipPartition {
            name: "RadioPeripherals".to_owned(),
            member: "radio".to_owned(),
            description: "Unique radio register owner.".to_owned(),
            peripherals: vec!["RADIO".to_owned(), "RADIO".to_owned()],
        });
        assert!(pack.validate().is_err());
        pack.ownership_partitions.clear();
        pack.opaque_domains.push(OpaqueDomain {
            name: "CommandInput".to_owned(),
            description: "Reviewed command input image.".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "COMMAND".to_owned(),
            sources: vec!["REVIEW".to_owned()],
        });
        pack.masked_register_modifies.push(MaskedRegisterModify {
            name: "publish_command".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "COMMAND".to_owned(),
            domain: "CommandInput".to_owned(),
            preserve_mask: 0xffff_0000,
            input_mask: 0x0000_fff0,
            set_mask: 0,
            exposure: PacApiExposure::Facade,
            sources: vec!["REVIEW".to_owned()],
        });
        assert!(pack.validate().is_err());
    }

    #[test]
    fn rejects_obsolete_schema_without_compatibility() {
        let mut pack = empty_pack();
        pack.schema = 3;
        assert!(
            pack.validate()
                .unwrap_err()
                .to_string()
                .contains("requires schema = 5")
        );
    }

    #[test]
    fn rejects_removed_inferred_ownership_option() {
        let input = r#"schema = 5

[options]
peripheral-ownership = true
"#;
        assert!(toml_edit::de::from_str::<PacApiPack>(input).is_err());
    }

    #[test]
    fn w1c_register_snapshot_is_affine_and_evidenced() {
        let mut pack = empty_pack();
        pack.w1c_register_snapshots.push(W1cRegisterSnapshot {
            name: "event_status".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "EVENT_STATUS".to_owned(),
            sources: vec!["PUBLIC_EVENT_STATUS_W1C".to_owned()],
        });

        assert!(pack.validate().is_ok());
        assert_eq!(pack.operation_count(), 1);
        assert_eq!(
            pack.source_ids(),
            BTreeSet::from(["PUBLIC_EVENT_STATUS_W1C"])
        );
    }

    #[test]
    fn rejects_rust_keywords_in_generated_ownership_identifiers() {
        let mut pack = empty_pack();
        pack.ownership_partitions.push(OwnershipPartition {
            name: "Self".to_owned(),
            member: "radio".to_owned(),
            description: "Invalid generated owner.".to_owned(),
            peripherals: vec!["RADIO".to_owned()],
        });
        assert!(pack.validate().is_err());

        pack.ownership_partitions[0].name = "RadioPeripherals".to_owned();
        pack.ownership_partitions[0].member = "type".to_owned();
        assert!(pack.validate().is_err());
    }

    #[test]
    fn typed_register_write_requires_a_declared_domain() {
        let mut pack = empty_pack();
        pack.full_register_writes.push(FullRegisterWrite {
            name: "write_interrupt_mask".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "ENABLE".to_owned(),
            field: "EVENTS".to_owned(),
            domain: "InterruptMask".to_owned(),
            exposure: PacApiExposure::Facade,
            sources: vec!["VENDOR_IRQ".to_owned()],
        });
        assert!(
            pack.validate()
                .unwrap_err()
                .to_string()
                .contains("unknown domain")
        );
    }
}
