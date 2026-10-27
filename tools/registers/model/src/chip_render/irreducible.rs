//! The policy items a field accessor cannot express, rendered on the owner
//! of the part the policy gives them to:
//!
//! - a fixed image marked `whole-word` (the vendor's store of a whole word,
//!   undeclared and read-only bits included), and each such step of a fixed
//!   sequence, one method per image; the HAL orders the steps;
//! - a fixed image acknowledging write-one-to-clear events with its proof:
//!   the enable word it masks first (`masks`) or the event the caller
//!   consumed (`consumed-event`);
//! - a fixed configuration of a readable hardware-updated word that has no
//!   generic modify accessor, preserving undeclared and read-only lanes;
//! - an interrupt snapshot: a status word sampled into an affine token and
//!   acknowledged, exactly as sampled, in its clear word;
//! - a write from zero storing sourced `whole-word-bits` beside its fields;
//! - a field-or-modify of a pointer field: the vendor's OR of a typed
//!   reference's image into a fresh read;
//! - a register image read: one side-effect-free word, whole and undecoded;
//! - a partition image read: on each part of the partition, a read-only
//!   diagnostic image of the part's readable, side-effect-free words, raw.
//! - a sourced zero publication to a read-observed word, without a load.
//!
//! Every other transaction of the policy is a sequence of field accessors
//! and belongs to the HAL.

use std::fmt::Write as _;

use super::{
    Error,
    block::{FieldPlan, Template, TypePlan},
    names,
    owners::Owner,
};
use crate::{
    chip_model::{ChipModel, FieldClass, Location},
    chip_policy::ChipPolicy,
    chip_render::names::push_doc,
};

/// How the owner reaches one element.
struct Reached<'t, 'm> {
    template: &'t Template<'m>,
    offset: u64,
}

fn element<'t, 'm>(
    entity: &str,
    plan: &'t TypePlan<'m>,
    owner: &Owner<'_>,
    model: &ChipModel,
    identity: &str,
) -> Result<Reached<'t, 'm>, Error> {
    let register = model
        .register(identity)
        .ok_or_else(|| Error::new(entity, format!("{identity} is no register element")))?;
    let Location::Mmio { address } = register.location else {
        return Err(Error::new(
            entity,
            format!("{identity} is no MMIO register"),
        ));
    };
    let offset = address.wrapping_sub(owner.base);
    let (template, _) = plan
        .element(offset)
        .ok_or_else(|| Error::new(entity, format!("{identity} is no element of its type")))?;
    Ok(Reached { template, offset })
}

impl Reached<'_, '_> {
    fn read(&self, entity: &str, owner: &Owner<'_>) -> Result<String, Error> {
        if owner.words.owned.contains(&self.offset) || owner.words.observed.contains(&self.offset) {
            Ok(format!("self.0.read::<0x{:x}>()", self.offset))
        } else {
            Err(Error::new(
                entity,
                format!("{} reads no word at 0x{:x}", owner.name(), self.offset),
            ))
        }
    }

    fn write(
        &self,
        entity: &str,
        owner: &Owner<'_>,
        value: &str,
        bits: u32,
    ) -> Result<String, Error> {
        if owner.part.read_only {
            return Err(Error::new(entity, "a read-only part writes nothing"));
        }
        if owner.words.owned.contains(&self.offset) {
            return Ok(format!("self.0.write::<0x{:x}>({value})", self.offset));
        }
        let mask = owner.trigger_mask(self.template, self.offset);
        if mask != 0 && bits & !mask == 0 {
            return Ok(if owner.division.is_some() {
                format!("self.0.write_trigger::<0x{:x}>({value})", self.offset)
            } else {
                format!("self.0.write::<0x{:x}>({value})", self.offset)
            });
        }
        Err(Error::new(
            entity,
            format!(
                "{} writes no word at 0x{:x} with these bits",
                owner.name(),
                self.offset
            ),
        ))
    }
}

fn doc(body: &mut String, summary: &str, register: &str, sources: &[String]) {
    let _ = writeln!(
        body,
        "/// {summary} (`{register}`).\n///\n/// SOURCE: {}.",
        sources.join(", ")
    );
}

pub(super) fn render(
    body: &mut String,
    output: &mut String,
    plan: &TypePlan<'_>,
    owner: &Owner<'_>,
    model: &ChipModel,
    policy: &ChipPolicy,
) -> Result<(), Error> {
    let file = policy.file();
    let mine = |name: &str| policy.owner(name) == Some(owner.id);
    for zero in file
        .zero_register_writes
        .iter()
        .filter(|zero| mine(&zero.name))
    {
        let entity = zero.name.as_str();
        if !model.register(&zero.register).is_some_and(|register| {
            register
                .fields
                .iter()
                .any(|field| field.class == FieldClass::ReadObserved)
        }) {
            continue;
        }
        let target = element(entity, plan, owner, model, &zero.register)?;
        doc(
            body,
            "Publish the sourced complete zero word; its hardware effect is unqualified",
            &zero.register,
            &zero.sources,
        );
        let write = target.write(entity, owner, "0", 0)?;
        let _ = writeln!(
            body,
            "#[inline]\npub fn {}(&mut self) {{\n{write};\n}}\n",
            names::ident(entity)
        );
    }
    for image in file
        .fixed_register_images
        .iter()
        .filter(|image| mine(&image.name))
    {
        let entity = image.name.as_str();
        let target = element(entity, plan, owner, model, &image.register)?;
        let configuration = !image.whole_word
            && image.masks.is_none()
            && image.consumed_event.is_none()
            && target.template.readable()
            && !target.template.modifiable()
            && target.template.fields.iter().any(|field| {
                matches!(
                    field.class,
                    FieldClass::HardwareWritten | FieldClass::SelfClearing
                )
            });
        if !(configuration
            || image.whole_word
            || image.masks.is_some()
            || image.consumed_event.is_some())
        {
            continue;
        }
        if target
            .template
            .fields
            .iter()
            .any(|field| field.pointer.is_some() && field.mask() & image.value != 0)
        {
            return Err(Error::new(entity, "a fixed image sets an address field"));
        }
        let declared = target
            .template
            .fields
            .iter()
            .fold(0, |bits, field| bits | field.mask());
        let preserve_w1c_reserved = !image.whole_word
            && declared != u32::MAX
            && !target.template.fields.is_empty()
            && target
                .template
                .fields
                .iter()
                .all(|field| field.class == FieldClass::WriteOneToClear);
        let constant = format!("{}_IMAGE", names::snake(entity).to_ascii_uppercase());
        doc(
            output,
            "The fixed policy image, before preservation of other lanes",
            &image.register,
            &image.sources,
        );
        let _ = writeln!(
            output,
            "pub const {constant}: u32 = 0x{:08x};\n",
            image.value
        );
        let value = if preserve_w1c_reserved {
            // The fixed W1C set does not come from a status sample. Read
            // only to preserve undeclared bits, as affine ack does.
            let read = target.read(entity, owner)?;
            format!("({read} & 0x{:08x}) | {constant}", !declared)
        } else if configuration {
            let mask = target
                .template
                .fields
                .iter()
                .filter(|field| {
                    !matches!(
                        field.class,
                        FieldClass::ReadOnly | FieldClass::DebugOnly | FieldClass::Unknown
                    )
                })
                .fold(0, |mask, field| mask | field.mask());
            let read = target.read(entity, owner)?;
            format!("({read} & 0x{:08x}) | {constant}", !mask)
        } else {
            constant
        };
        let write = target.write(entity, owner, &value, image.value)?;
        // A masks proof disables the acknowledged sources first, whether or
        // not the image is a whole word.
        let mut masking = String::new();
        if let Some(masks) = &image.masks {
            let enable = element(entity, plan, owner, model, masks)?;
            if !enable.template.modifiable() {
                return Err(Error::new(
                    entity,
                    format!("its enable word {masks} is not read and written back"),
                ));
            }
            let zero = enable.template.write_zero() | image.value;
            if zero == u32::MAX {
                let cleared = enable.write(entity, owner, "0", u32::MAX)?;
                let _ = writeln!(masking, "{cleared};");
            } else {
                let read = enable.read(entity, owner)?;
                let cleared = enable.write(
                    entity,
                    owner,
                    &super::block::without("bits", zero),
                    u32::MAX,
                )?;
                let _ = writeln!(masking, "let bits = {read};\n{cleared};");
            }
        }
        let summary = match (image.whole_word, &image.masks) {
            (true, None) => format!(
                "Write the vendor's whole-word image `0x{:08x}`, undeclared bits included",
                image.value
            ),
            (true, Some(masks)) => format!(
                "Mask the sources of `0x{:08x}` in `{masks}`, keeping every other bit, then write the vendor's whole-word image, undeclared bits included",
                image.value
            ),
            (false, Some(masks)) => format!(
                "Mask the sources of `0x{:08x}` in `{masks}`, keeping every other bit, then acknowledge them",
                image.value
            ),
            (false, None) if configuration => format!(
                "Apply the fixed configuration `0x{:08x}`, preserving undeclared and read-only bits",
                image.value
            ),
            (false, None) => format!("Acknowledge the consumed events `0x{:08x}`", image.value),
        };
        doc(body, &summary, &image.register, &image.sources);
        if let Some(reason) = &image.consumed_event {
            body.push_str("///\n/// The caller consumed these events:\n");
            push_doc(body, reason, "");
        }
        let _ = writeln!(
            body,
            "#[inline]\npub fn {}(&mut self) {{\n{masking}{write};\n}}\n",
            names::ident(&image.name)
        );
    }
    for modify in file
        .field_or_modifies
        .iter()
        .filter(|modify| modify.domain.is_none() && modify.value.is_none() && mine(&modify.name))
    {
        render_pointer_or(body, plan, owner, model, modify)?;
    }
    for sequence in file
        .fixed_register_sequences
        .iter()
        .filter(|sequence| mine(&sequence.name))
    {
        for step in sequence.steps.iter().filter(|step| step.whole_word) {
            let entity = sequence.name.as_str();
            let target = element(entity, plan, owner, model, &step.register)?;
            if target
                .template
                .fields
                .iter()
                .any(|field| field.pointer.is_some() && field.mask() & step.value != 0)
            {
                return Err(Error::new(entity, "a fixed image sets an address field"));
            }
            let write =
                target.write(entity, owner, &format!("0x{:08x}", step.value), step.value)?;
            let element = step.register.rsplit('.').next().unwrap_or_default();
            doc(
                body,
                &format!(
                    "Write the vendor's whole-word image `0x{:08x}`, undeclared bits included: one step of `{}`, which the HAL orders",
                    step.value, sequence.name
                ),
                &step.register,
                &sequence.sources,
            );
            let _ = writeln!(
                body,
                "#[inline]\npub fn {}(&mut self) {{\n{write};\n}}\n",
                names::ident(&format!("{}_{}", sequence.name, names::snake(element)))
            );
        }
    }
    for snapshot in file
        .interrupt_snapshots
        .iter()
        .filter(|snapshot| mine(&snapshot.name))
    {
        render_snapshot(body, output, plan, owner, model, snapshot)?;
    }
    for read in file
        .register_image_reads
        .iter()
        .filter(|read| mine(&read.name))
    {
        let target = element(&read.name, plan, owner, model, &read.register)?;
        let call = target.read(&read.name, owner)?;
        doc(
            body,
            "Read the complete word without decoding or acknowledging it, including undeclared and reserved bits",
            &read.register,
            &read.sources,
        );
        let _ = writeln!(
            body,
            "#[inline]\npub fn {}(&self) -> u32 {{\n{call}\n}}\n",
            names::ident(&read.name)
        );
    }
    for read in file
        .partition_image_reads
        .iter()
        .filter(|read| read.partition == owner.partition)
    {
        render_partition_image(body, plan, owner, read);
    }
    for write in file
        .zero_based_field_writes
        .iter()
        .filter(|write| write.whole_word_bits.is_some() && mine(&write.name))
    {
        let entity = write.name.as_str();
        let Some(bits) = &write.whole_word_bits else {
            continue;
        };
        let Some((register, _)) = write
            .fields
            .first()
            .and_then(|field| field.rsplit_once('.'))
        else {
            return Err(Error::new(entity, "it names no field"));
        };
        let target = element(entity, plan, owner, model, register)?;
        let mut parameters = String::new();
        let mut image = format!("0x{:08x}", bits.value | target.template.reserved_reset);
        let mut mask = bits.value;
        for reference in &write.fields {
            let name = reference.rsplit('.').next().unwrap_or_default();
            let field = target
                .template
                .fields
                .iter()
                .find(|field| field.field.name == name)
                .filter(|field| field.setter())
                .ok_or_else(|| {
                    Error::new(entity, format!("{reference} is no field a write sets"))
                })?;
            let parameter = field.method();
            let _ = write!(
                parameters,
                ", {parameter}: {}",
                field.write_type(Some(&target.template.module()))
            );
            let _ = write!(image, " | {}", field.positioned_of(&parameter));
            mask |= field.mask();
        }
        let call = target.write(entity, owner, &image, mask)?;
        doc(
            body,
            &format!(
                "Write the fields from zero beside the vendor's fixed ones `0x{:08x}` (SOURCE: {})",
                bits.value,
                bits.sources.join(", ")
            ),
            register,
            &write.sources,
        );
        let _ = writeln!(
            body,
            "#[inline]\npub fn {}(&mut self{parameters}) {{\n{call};\n}}\n",
            names::ident(&write.name)
        );
    }
    Ok(())
}

/// The part's share of a partition image read: each word the part holds
/// whose read has a value and changes nothing, ascending by offset, read
/// whole (undeclared and reserved lanes included) as a diagnostic of the
/// hardware, with no field decoding. A word the part only observes is its
/// owner's.
fn render_partition_image(
    body: &mut String,
    plan: &TypePlan<'_>,
    owner: &Owner<'_>,
    read: &crate::chip_policy::PartitionImageRead,
) {
    let offsets = owner
        .words
        .owned
        .iter()
        .copied()
        .filter(|offset| {
            plan.element(*offset)
                .is_some_and(|(template, _)| template.readable())
        })
        .collect::<Vec<_>>();
    let constant = names::ident(&read.name).to_uppercase();
    let list = offsets
        .iter()
        .map(|offset| format!("0x{offset:x}"))
        .collect::<Vec<_>>()
        .join(", ");
    let arms = offsets
        .iter()
        .enumerate()
        .map(|(index, offset)| format!("{index} => Some(self.0.read::<0x{offset:x}>()),\n"))
        .collect::<String>();
    let sources = read.sources.join(", ");
    let _ = writeln!(
        body,
        "/// The block offsets of the part's words in the `{name}` diagnostic\n\
         /// image, ascending.\n\
         pub const {constant}_OFFSETS: [u32; {len}] = [{list}];\n\n\
         /// Read word `index` of the part's `{name}` diagnostic image: the\n\
         /// readable words without a read side effect, at\n\
         /// [`Self::{constant}_OFFSETS`], whole and undecoded; `None` past\n\
         /// its end.\n///\n/// SOURCE: {sources}.\n\
         pub fn {method}(&self, index: usize) -> Option<u32> {{\n\
         match index {{\n{arms}_ => None,\n}}\n}}\n",
        name = read.name,
        len = offsets.len(),
        method = names::ident(&read.name),
    );
}

/// A field-or-modify of one pointer field from input bit zero, with
/// neither domain nor value (the policy proves it takes a typed
/// reference's image): the vendor's OR of that image into a fresh read,
/// which a field accessor, replacing the field, cannot express.
fn render_pointer_or(
    body: &mut String,
    plan: &TypePlan<'_>,
    owner: &Owner<'_>,
    model: &ChipModel,
    modify: &crate::chip_policy::FieldBitModify,
) -> Result<(), Error> {
    let entity = modify.name.as_str();
    let [projection] = modify.fields.as_slice() else {
        return Err(Error::new(
            entity,
            "it ORs an address into more than one field",
        ));
    };
    let Some((register, name)) = projection.field.rsplit_once('.') else {
        return Err(Error::new(entity, "it names no field"));
    };
    let target = element(entity, plan, owner, model, register)?;
    let field = target
        .template
        .fields
        .iter()
        .find(|field| field.field.name == name)
        .filter(|field| field.pointer.is_some() && field.setter())
        .ok_or_else(|| {
            Error::new(
                entity,
                format!("{} is no pointer field a write sets", projection.field),
            )
        })?;
    if !target.template.modifiable() {
        return Err(Error::new(
            entity,
            format!("{register} is not read and written back"),
        ));
    }
    let read = target.read(entity, owner)?;
    let zero = target.template.write_zero();
    let kept = if zero == 0 {
        read
    } else {
        format!("{read} & 0x{:08x}", !zero)
    };
    let write = target.write(
        entity,
        owner,
        &format!("{kept} | {}", field.positioned()),
        field.mask(),
    )?;
    doc(
        body,
        &format!(
            "OR the image of `value` into `{}` of a fresh read; every other bit keeps\n/// the value read, every write-trigger field is zero",
            field.field.name
        ),
        register,
        &modify.sources,
    );
    let _ = writeln!(
        body,
        "#[inline]\npub fn {}(&mut self, value: {}) {{\n{write};\n}}\n",
        names::ident(&modify.name),
        field.write_type(Some(&target.template.module()))
    );
    Ok(())
}

fn render_snapshot(
    body: &mut String,
    output: &mut String,
    plan: &TypePlan<'_>,
    owner: &Owner<'_>,
    model: &ChipModel,
    snapshot: &crate::chip_policy::InterruptSnapshot,
) -> Result<(), Error> {
    let entity = snapshot.name.as_str();
    let status = element(entity, plan, owner, model, &snapshot.status)?;
    let clear = element(entity, plan, owner, model, &snapshot.clear)?;
    let readable = status
        .template
        .fields
        .iter()
        .filter(|field| field.getter())
        .fold(0, |mask, field| mask | field.mask());
    let events = clear
        .template
        .fields
        .iter()
        .filter(|field| {
            matches!(
                field.class,
                FieldClass::WriteOneToClear | FieldClass::WriteTrigger
            ) && field.mask() & !readable == 0
        })
        .fold(0, |mask, field| mask | field.mask());
    // A snapshot naming its events samples those alone.
    let events = if snapshot.events.is_empty() {
        events
    } else {
        events
            & status
                .template
                .fields
                .iter()
                .filter(|field| {
                    snapshot.events.iter().any(|reference| {
                        reference.rsplit('.').next() == Some(field.field.name.as_str())
                    })
                })
                .fold(0, |mask, field| mask | field.mask())
    };
    let consumed = snapshot
        .consumed_fields
        .iter()
        .map(|reference| {
            let name = reference.rsplit('.').next().unwrap_or_default();
            clear
                .template
                .fields
                .iter()
                .find(|field| field.field.name == name && field.width == 1)
                .ok_or_else(|| {
                    Error::new(
                        entity,
                        format!("{reference} is no one-bit field of its clear word"),
                    )
                })
        })
        .collect::<Result<Vec<&FieldPlan<'_>>, Error>>()?;
    let consumed_mask = consumed.iter().fold(0, |mask, field| mask | field.mask());
    let camel = names::camel(&snapshot.name);
    let token = format!("{camel}Snapshot");
    let _ = writeln!(
        output,
        "/// One sample of the `{}` events `{}` acknowledges, to acknowledge once.\n\
         ///\n/// SOURCE: {}.\n\
         #[must_use = \"a sampled event set must be acknowledged\"]\n\
         #[derive(Debug, Eq, PartialEq)]\n\
         pub struct {token}(u32);\n\n\
         impl {token} {{\n\
         /// The bits the snapshot keeps: the events the clear word acknowledges.\n\
         pub const EVENTS: u32 = 0x{events:08x};\n\n\
         /// Whether no event was sampled.\n\
         pub const fn is_clear(&self) -> bool {{\nself.0 == 0\n}}\n\n\
         /// The sampled event bits: a value of the sample, which grants nothing.\n\
         pub const fn bits(&self) -> u32 {{\nself.0\n}}\n",
        snapshot.status,
        snapshot.clear,
        snapshot.sources.join(", ")
    );
    for field in status
        .template
        .fields
        .iter()
        .filter(|field| field.getter() && field.mask() & !events == 0)
    {
        let _ = writeln!(
            output,
            "/// The `{}` field.\n#[inline]\npub const fn {}(&self) -> {} {{\n{}\n}}\n\n\
             /// The snapshot without `{}` unless `keep`: it is then not acknowledged.\n\
             pub const fn retain_{}(self, keep: bool) -> Self {{\n\
             if keep {{ self }} else {{ Self({}) }}\n}}\n",
            field.field.name,
            field.method(),
            field.native(),
            field.read_expression("self.0"),
            field.field.name,
            field.stem,
            super::block::without("self.0", field.mask())
        );
    }
    output.push_str("}\n\n");
    let mut consumed_parameter = String::new();
    let mut consumed_bits = String::new();
    if !consumed.is_empty() {
        let kind = format!("{camel}Consumed");
        let _ = writeln!(
            output,
            "/// The consumed event an acknowledgement of [`{token}`] also writes."
        );
        if let Some(reason) = &snapshot.consumed_event {
            output.push_str("///\n");
            push_doc(output, reason, "");
        }
        let _ = writeln!(
            output,
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub enum {kind} {{"
        );
        for field in &consumed {
            let _ = writeln!(
                output,
                "/// `{}`.\n{} = 0x{:08x},",
                field.field.name,
                names::camel(&field.stem),
                field.mask()
            );
        }
        let _ = writeln!(
            output,
            "}}\n\nimpl {kind} {{\n/// The field's bit.\n#[inline]\npub const fn bits(self) -> u32 {{\nself as u32\n}}\n}}\n"
        );
        consumed_parameter = format!(", consumed: Option<{kind}>");
        consumed_bits =
            " | match consumed {\nSome(consumed) => consumed.bits(),\nNone => 0,\n}".to_owned();
    }
    let read = status.read(entity, owner)?;
    // Every clear lane the acknowledgement leaves takes zero.
    if let Some(field) = clear
        .template
        .fields
        .iter()
        .find(|field| field.mask() & (events | consumed_mask) == 0 && !field.zero_admitted())
    {
        return Err(Error::new(
            entity,
            format!(
                "its acknowledgement writes zero to {}, which its write constraint forbids",
                field.field.name
            ),
        ));
    }
    // The bits of no field keep their value: read back from a clear word
    // whose every field reads, at their reset value otherwise.
    let replayable = clear.template.readable()
        && clear.template.declared() != u32::MAX
        && clear
            .template
            .fields
            .iter()
            .all(|field| field.class.readable());
    let (kept, keep) = if replayable {
        (
            format!(
                "let kept = {} & 0x{:08x};\n",
                clear.read(entity, owner)?,
                !clear.template.declared()
            ),
            "kept | ".to_owned(),
        )
    } else if clear.template.zero_image() != 0 {
        (
            String::new(),
            format!("0x{:08x} | ", clear.template.zero_image()),
        )
    } else {
        (String::new(), String::new())
    };
    let write = clear.write(
        entity,
        owner,
        &format!("{keep}snapshot.0{consumed_bits}"),
        events | consumed_mask,
    )?;
    doc(
        body,
        &format!(
            "Sample `{}` once, keeping the events `{}` acknowledges",
            snapshot.status, snapshot.clear
        ),
        &snapshot.status,
        &snapshot.sources,
    );
    let method = names::snake(&snapshot.name);
    let _ = writeln!(
        body,
        "#[inline]\npub fn sample_{method}(&self) -> {token} {{\n{token}({read} & {token}::EVENTS)\n}}\n\n\
         /// Acknowledge exactly the sampled events{}: one to each such bit of\n\
         /// `{}`, zero to every other bit.\n\
         #[inline]\npub fn acknowledge_{method}(&mut self, snapshot: {token}{consumed_parameter}) {{\n{kept}{write};\n}}\n",
        if consumed.is_empty() {
            ""
        } else {
            " and the consumed one"
        },
        snapshot.clear,
    );
    Ok(())
}
