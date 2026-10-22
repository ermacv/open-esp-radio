//! Qualified DWARF names; generic arguments are retained verbatim from rustc.
//! DIE offsets are used only to follow references, never as comparison keys.
use super::TYPE_MIN_BYTES;
use crate::Result;
use oer_analysis_elf::{
    Elf,
    dwarf::{DebugSections, gimli},
};
use std::collections::{BTreeMap, BTreeSet};

type Reader<'a> = oer_analysis_elf::dwarf::SliceReader<'a>;
#[derive(Default)]
struct Type {
    name: Option<String>,
    size: Option<u64>,
    array: bool,
    composite: bool,
    target: Option<usize>,
    dimensions: Vec<u64>,
}

pub(super) fn read(file: &Elf<'_>) -> Result<BTreeMap<String, u64>> {
    let sections = DebugSections::load(file)?;
    let dwarf = sections.dwarf()?;
    let mut types = BTreeMap::<usize, Type>::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let mut scopes = Vec::<(isize, String)>::new();
        let mut arrays = Vec::<(isize, usize)>::new();
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs()? {
            let depth = entry.depth();
            scopes.retain(|(d, _)| *d < depth);
            arrays.retain(|(d, _)| *d < depth);
            let Some(offset) = entry
                .offset()
                .to_debug_info_offset(&unit.header)
                .map(|o| o.0)
            else {
                return Err("sizes: unsupported type-unit DWARF".into());
            };
            let name = entry
                .attr_value(gimli::DW_AT_name)
                .map(|v| dwarf.attr_string(&unit, v))
                .transpose()?
                .map(|r| r.to_string().map(|s| s.to_owned()))
                .transpose()?;
            let tag = entry.tag();
            let composite = matches!(
                tag,
                gimli::DW_TAG_structure_type
                    | gimli::DW_TAG_union_type
                    | gimli::DW_TAG_enumeration_type
                    | gimli::DW_TAG_class_type
            );
            let array = tag == gimli::DW_TAG_array_type;
            let qualified = name.as_ref().map(|name| {
                scopes
                    .iter()
                    .map(|(_, n)| n.as_str())
                    .chain([name.as_str()])
                    .collect::<Vec<_>>()
                    .join("::")
            });
            let size = entry
                .attr_value(gimli::DW_AT_byte_size)
                .and_then(|v| v.udata_value())
                .or_else(|| {
                    matches!(
                        tag,
                        gimli::DW_TAG_pointer_type
                            | gimli::DW_TAG_reference_type
                            | gimli::DW_TAG_rvalue_reference_type
                    )
                    .then_some(u64::from(unit.header.address_size()))
                });
            let target = entry
                .attr_value(gimli::DW_AT_type)
                .and_then(|v| reference(&unit, v));
            if composite || array || size.is_some() || target.is_some() {
                types.insert(
                    offset,
                    Type {
                        name: qualified,
                        size,
                        array,
                        composite,
                        target,
                        dimensions: Vec::new(),
                    },
                );
            }
            if tag == gimli::DW_TAG_subrange_type
                && let Some(&(_, owner)) = arrays.last()
            {
                let count = entry
                    .attr_value(gimli::DW_AT_count)
                    .and_then(|v| v.udata_value())
                    .or_else(|| {
                        let upper = entry.attr_value(gimli::DW_AT_upper_bound)?.udata_value()?;
                        let lower = entry
                            .attr_value(gimli::DW_AT_lower_bound)
                            .and_then(|v| v.udata_value())
                            .unwrap_or(0);
                        upper.checked_sub(lower)?.checked_add(1)
                    })
                    .ok_or("sizes: array has no constant length")?;
                types
                    .get_mut(&owner)
                    .expect("array owner recorded")
                    .dimensions
                    .push(count);
            }
            if array {
                arrays.push((depth, offset));
            }
            if (composite || matches!(tag, gimli::DW_TAG_namespace | gimli::DW_TAG_subprogram))
                && let Some(name) = name
            {
                scopes.push((depth, name));
            }
        }
    }
    let mut out = BTreeMap::new();
    for (&offset, ty) in &types {
        if !ty.composite && !ty.array {
            continue;
        }
        if let Some((name, size)) = layout(offset, &types, &mut BTreeSet::new())?
            && size >= TYPE_MIN_BYTES
        {
            if let Some(previous) = out.insert(name.clone(), size)
                && previous != size
            {
                return Err(format!(
                    "sizes: unstable DWARF type key `{name}`: {previous} and {size} bytes"
                )
                .into());
            }
        }
    }
    Ok(out)
}

fn reference(
    unit: &gimli::Unit<Reader<'_>>,
    value: gimli::AttributeValue<Reader<'_>>,
) -> Option<usize> {
    match value {
        gimli::AttributeValue::UnitRef(o) => o.to_debug_info_offset(&unit.header).map(|o| o.0),
        gimli::AttributeValue::DebugInfoRef(o) => Some(o.0),
        _ => None,
    }
}

fn layout(
    offset: usize,
    types: &BTreeMap<usize, Type>,
    visiting: &mut BTreeSet<usize>,
) -> Result<Option<(String, u64)>> {
    if !visiting.insert(offset) {
        return Err("sizes: cyclic DWARF layout".into());
    }
    let result = (|| {
        let Some(ty) = types.get(&offset) else {
            return Ok(None);
        };
        if ty.array {
            let Some(target) = ty.target else {
                return Err("sizes: array has no element type".into());
            };
            let Some((mut name, mut size)) = layout(target, types, visiting)? else {
                return Err("sizes: array element has no stable layout".into());
            };
            for count in ty.dimensions.iter().rev() {
                size = size
                    .checked_mul(*count)
                    .ok_or("sizes: array size overflow")?;
                name = format!("[{name}; {count}]");
            }
            return Ok(Some((name, size)));
        }
        match (&ty.name, ty.size) {
            (Some(name), Some(size)) => Ok(Some((name.clone(), size))),
            (None, Some(size)) if ty.composite && size >= TYPE_MIN_BYTES => Err(format!(
                "sizes: unnamed composite type at DIE {offset:#x}, {size} bytes"
            )
            .into()),
            _ => Ok(None),
        }
    })();
    visiting.remove(&offset);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn namespaces_generics_futures_and_arrays_keep_their_identity() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("fixture.rs");
        let object = directory.path().join("fixture.o");
        std::fs::write(&source, r#"
mod alpha { pub struct State(pub [u8; 256]); }
mod beta { pub struct State(pub [u8; 320]); }
struct Generic<T>(T);
async fn future<const N: usize>() { let value = [1u8; N]; core::future::pending::<()>().await; core::hint::black_box(value); }
#[unsafe(no_mangle)] pub fn keep() {
    core::hint::black_box(Generic(alpha::State([0;256])));
    core::hint::black_box(Generic(beta::State([0;320])));
    core::hint::black_box(future::<256>());
    core::hint::black_box(future::<320>());
}
fn main() { keep(); }
"#).unwrap();
        let output = oer_os_toolchain::command(oer_os_toolchain::Tool::Rustc)
            .unwrap()
            .args(["--edition=2024", "-Cdebuginfo=2", "-Copt-level=1"])
            .arg(&source)
            .arg("-o")
            .arg(&object)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = std::fs::read(object).unwrap();
        let file = Elf::parse(&bytes).unwrap();
        let types = read(&file).unwrap();
        for (name, bytes) in [
            ("fixture::alpha::State", 256),
            ("fixture::beta::State", 320),
            ("fixture::Generic<fixture::alpha::State>", 256),
            ("fixture::Generic<fixture::beta::State>", 320),
            ("[u8; 256]", 256),
            ("[u8; 320]", 320),
        ] {
            assert_eq!(types.get(name), Some(&bytes), "{name}: {types:?}");
        }
        for argument in ["256", "320"] {
            assert!(
                types
                    .keys()
                    .any(|name| name.contains("async_fn_env") && name.contains(argument)),
                "{types:?}"
            );
        }
    }
}
