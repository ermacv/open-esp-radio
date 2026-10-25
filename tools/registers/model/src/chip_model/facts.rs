//! Intrinsic field facts, checked against the included layouts and root regions.

use std::collections::{BTreeMap, BTreeSet};

use super::block_type::{Context, mask, parse_bits};
use super::{BlockType, Error, ErrorKind, Field, FieldClass, MemoryLayout, RamRegion};

pub(super) fn check(
    context: Context<'_>,
    types: &BTreeMap<String, BlockType>,
    layouts: &[MemoryLayout],
    regions: &[RamRegion],
) -> Result<(), Error> {
    for block in types.values() {
        let context = Context {
            path: &block.path,
            ..context
        };
        // The fixed address prefix each high field supplies, by field.
        let mut prefixes = BTreeMap::new();
        for (key, register) in &block.templates {
            for field in &register.fields {
                if field.pointer.is_some() {
                    check_pointer(context, block, key, field, layouts, regions, &mut prefixes)?;
                }
            }
        }
        check_pairs(context, block)?;
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "one pointer fact and the facts it is checked against"
)]
fn check_pointer<'b>(
    context: Context<'_>,
    block: &'b BlockType,
    key: &str,
    field: &'b Field,
    layouts: &[MemoryLayout],
    regions: &[RamRegion],
    prefixes: &mut BTreeMap<&'b str, u64>,
) -> Result<(), Error> {
    let Some(pointer) = &field.pointer else {
        return Ok(());
    };
    let entity = format!("{}.{key}.{}", block.name(), field.name);
    let fail = |reason: String| context.error(&entity, ErrorKind::Pointer(reason));
    context.sources(&entity, &pointer.sources)?;
    if !matches!(
        field.class,
        FieldClass::Plain | FieldClass::WriteOnly | FieldClass::ReadOnly
    ) || !field.values.is_empty()
        || field.write_range.is_some()
        || field.value_range.is_some()
        || field.write_values_only
    {
        return Err(fail(
            "requires a plain, write-only or read-only field without a value domain".into(),
        ));
    }
    let (_, width) = parse_bits(&field.bits).map_err(|kind| context.error(&entity, kind))?;
    let encoding = &pointer.encoding;
    if encoding.width != width
        || encoding.shift >= 32
        || encoding
            .width
            .checked_add(encoding.shift)
            .is_none_or(|bits| bits > 32)
        || (encoding.low_bits && (width == 32 || encoding.shift != 0))
        || (encoding.high_field.is_some() && !encoding.low_bits)
    {
        return Err(fail("encoding must fill the field, fit a 32-bit address, and use high-field only with unshifted low-bits".into()));
    }
    let region = regions
        .iter()
        .find(|region| region.name == pointer.placement)
        .ok_or_else(|| {
            fail(format!(
                "placement {} is no RAM region of the root",
                pointer.placement
            ))
        })?;
    let end = region.base + region.size - 1; // Root ranges were checked before facts.
    let covered = width + encoding.shift;
    if end > u64::from(u32::MAX) || (region.base >> covered) != (end >> covered) {
        return Err(fail(
            "placement crosses the encoding's fixed high-address prefix".into(),
        ));
    }
    if let Some(name) = &pointer.points_to {
        let layout = layouts
            .iter()
            .find(|layout| layout.name == *name)
            .ok_or_else(|| fail(format!("points-to layout {name} is not included")))?;
        if !matches!(
            layout.kind,
            crate::LayoutKind::Words | crate::LayoutKind::Array
        ) || layout.placement != [pointer.placement.clone()]
        {
            return Err(fail(format!(
                "layout {name} must be a word or array layout placed in the pointer's region alone"
            )));
        }
        let alignment = 1u32 << encoding.shift;
        if pointer.member_offset >= layout.size
            || !pointer.member_offset.is_multiple_of(alignment)
            || !layout.align.is_multiple_of(alignment)
        {
            return Err(fail("member-offset must lie inside the target and target alignment must preserve the encoding's omitted low bits".into()));
        }
    } else if pointer.member_offset != 0 || encoding.shift != 0 {
        return Err(fail(
            "raw buffer pointers have no member-offset or omitted low bits".into(),
        ));
    }
    if let Some(reference) = &encoding.high_field {
        let high = reference
            .rsplit_once('.')
            .and_then(|(register, name)| {
                block
                    .templates
                    .get(register)?
                    .fields
                    .iter()
                    .find(|field| field.name == name)
            })
            .ok_or_else(|| fail(format!("high-field {reference} is no field of this type")))?;
        let (_, high_width) =
            parse_bits(&high.bits).map_err(|kind| context.error(&entity, kind))?;
        if high_width != 32 - width
            || high.pointer.is_some()
            || (field.class.written() && high.class != FieldClass::Plain)
            || !high.class.readable()
        {
            return Err(fail("high-field must supply the remaining address bits and be readable (plain for a writable pointer)".into()));
        }
        // Its one value is the placement's prefix: no domain of its own.
        if !high.values.is_empty()
            || high.value_range.is_some()
            || high.write_range.is_some()
            || high.write_values_only
        {
            return Err(fail(format!(
                "high-field {reference} takes the placement's address prefix alone and has no value domain"
            )));
        }
        let prefix = region.base >> width;
        if *prefixes.entry(reference.as_str()).or_insert(prefix) != prefix {
            return Err(fail(format!(
                "high-field {reference} supplies another pointer's different prefix"
            )));
        }
    }
    Ok(())
}

fn check_pairs(context: Context<'_>, block: &BlockType) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    for pair in &block.file.interrupt_pairs {
        let entity = format!("{}.{} -> {}", block.name(), pair.status, pair.clear);
        let fail = |reason: String| context.error(&entity, ErrorKind::InterruptPair(reason));
        context.sources(&entity, &pair.sources)?;
        if !seen.insert(&pair.status) {
            return Err(fail(
                "a status register has exactly one clear register".into(),
            ));
        }
        let status = block
            .templates
            .get(&pair.status)
            .ok_or_else(|| fail(format!("unknown status register {}", pair.status)))?;
        let clear = block
            .templates
            .get(&pair.clear)
            .ok_or_else(|| fail(format!("unknown clear register {}", pair.clear)))?;
        let bits = |fields: &[Field]| -> Result<u64, Error> {
            fields.iter().try_fold(0, |bits, field| {
                let (low, width) =
                    parse_bits(&field.bits).map_err(|kind| context.error(&entity, kind))?;
                Ok(bits | mask(low, width))
            })
        };
        let shape = |register: &super::Register| {
            (
                register
                    .width
                    .unwrap_or_else(|| block.space().default_width()),
                register.dim.map(|dim| (dim.count, dim.stride)),
            )
        };
        if shape(status) != shape(clear)
            || status.fields.is_empty()
            || status.fifo.is_some()
            || status.data_port.is_some()
            || status.read_side_effect.is_some()
            || status
                .fields
                .iter()
                .any(|field| !field.class.readable() || field.class == FieldClass::ReadClears)
            || clear.fifo.is_some()
            || clear.data_port.is_some()
            || clear.read_side_effect.is_some()
            || clear.fields.iter().any(|field| {
                !matches!(
                    field.class,
                    FieldClass::WriteOneToClear | FieldClass::WriteTrigger
                )
            })
            || bits(&status.fields)? & !bits(&clear.fields)? != 0
        {
            return Err(fail("status must read without side effects, its lanes must be W1C/trigger lanes of the clear word, and both must repeat alike".into()));
        }
    }
    Ok(())
}
