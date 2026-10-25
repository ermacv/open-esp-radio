//! The policy items a field accessor cannot express, rendered on the owner
//! of the part the policy gives them to:
//!
//! - a fixed image marked `whole-word` (the vendor's store of a whole word,
//!   undeclared and read-only bits included);
//! - a fixed image acknowledging write-one-to-clear events with its proof:
//!   the enable word it masks first (`masks`) or the event the caller
//!   consumed (`consumed-event`);
//! - an interrupt snapshot: a status word sampled into an affine token and
//!   acknowledged, exactly as sampled, in its clear word;
//! - a write from zero storing sourced `whole-word-bits` beside its fields.
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
    pac_api_render::push_doc,
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
    for image in file.fixed_register_images.iter().filter(|image| {
        (image.whole_word || image.masks.is_some() || image.consumed_event.is_some())
            && mine(&image.name)
    }) {
        let entity = image.name.as_str();
        let target = element(entity, plan, owner, model, &image.register)?;
        if target
            .template
            .fields
            .iter()
            .any(|field| field.pointer && field.mask() & image.value != 0)
        {
            return Err(Error::new(entity, "a fixed image sets an address field"));
        }
        let value = format!("0x{:08x}", image.value);
        let write = target.write(entity, owner, &value, image.value)?;
        let mut masking = String::new();
        let summary = if image.whole_word {
            format!(
                "Write the vendor's whole-word image `0x{:08x}`, undeclared bits included",
                image.value
            )
        } else if let Some(masks) = &image.masks {
            let enable = element(entity, plan, owner, model, masks)?;
            if !enable.template.modifiable() {
                return Err(Error::new(
                    entity,
                    format!("its enable word {masks} is not read and written back"),
                ));
            }
            let zero = enable.template.write_zero();
            let read = enable.read(entity, owner)?;
            let cleared = enable.write(
                entity,
                owner,
                &format!("bits & 0x{:08x}", !(zero | image.value)),
                u32::MAX,
            )?;
            let _ = writeln!(masking, "let bits = {read};\n{cleared};");
            format!(
                "Mask the sources of `0x{:08x}` in `{masks}`, keeping every other bit, then acknowledge them",
                image.value
            )
        } else {
            format!("Acknowledge the consumed events `0x{:08x}`", image.value)
        };
        doc(body, &summary, &image.register, &image.sources);
        if let Some(reason) = &image.consumed_event {
            let _ = writeln!(body, "///\n/// The caller consumed these events: {reason}");
        }
        let _ = writeln!(
            body,
            "#[inline]\npub fn {}(&mut self) {{\n{masking}{write};\n}}\n",
            names::ident(&image.name)
        );
    }
    for snapshot in file
        .interrupt_snapshots
        .iter()
        .filter(|snapshot| mine(&snapshot.name))
    {
        render_snapshot(body, output, plan, owner, model, snapshot)?;
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
                "{parameter}: {}, ",
                field.write_type(Some(&target.template.module()))
            );
            let _ = write!(
                image,
                " | {}",
                field.positioned().replace("value", &parameter)
            );
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
            "#[inline]\npub fn {}(&mut self, {parameters}) {{\n{call};\n}}\n",
            names::ident(&write.name)
        );
    }
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
         pub const fn is_clear(&self) -> bool {{\nself.0 == 0\n}}\n",
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
             if keep {{ self }} else {{ Self(self.0 & 0x{:08x}) }}\n}}\n",
            field.field.name,
            field.method(),
            field.native(),
            field.read_expression("self.0"),
            field.field.name,
            field.stem,
            !field.mask()
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
    let write = clear.write(
        entity,
        owner,
        &format!("snapshot.0{consumed_bits}"),
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
         #[inline]\npub fn acknowledge_{method}(&mut self, snapshot: {token}{consumed_parameter}) {{\n{write};\n}}\n",
        if consumed.is_empty() {
            ""
        } else {
            " and the consumed one"
        },
        snapshot.clear,
    );
    Ok(())
}
