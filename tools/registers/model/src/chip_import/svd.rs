//! The vendor SVD as the import reads it: one peripheral's registers as
//! single elements, each with its own fields, and the array a register
//! element came from.

use svd_rs::{Access, Device, MaybeArray, ModifiedWriteValues, RegisterCluster};

use super::ImportError;

/// The repeated register a single element was expanded from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SvdArray {
    /// The template name, with one `%s`.
    pub(super) name: String,
    pub(super) index: u32,
    pub(super) count: u32,
    pub(super) stride: u32,
    pub(super) description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SvdField {
    pub(super) name: String,
    pub(super) low: u32,
    pub(super) width: u32,
    pub(super) access: Option<Access>,
    pub(super) modified: Option<ModifiedWriteValues>,
    pub(super) description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SvdRegister {
    pub(super) name: String,
    pub(super) offset: u32,
    pub(super) width: u32,
    pub(super) reset: Option<(u64, u64)>,
    pub(super) description: Option<String>,
    /// The array the element belongs to, when its elements are named by
    /// their index from zero (`%s` without a list).
    pub(super) array: Option<SvdArray>,
    pub(super) fields: Vec<SvdField>,
}

#[derive(Clone, Debug)]
pub(super) struct SvdBlock {
    pub(super) base: u64,
    /// The size of the peripheral's address block.
    pub(super) size: Option<u64>,
    pub(super) registers: Vec<SvdRegister>,
    pub(super) interrupts: Vec<(String, u32)>,
}

pub(super) fn parse(text: &str) -> Result<Device, ImportError> {
    svd_parser::parse(text).map_err(|error| ImportError::new("vendor SVD", error.to_string()))
}

fn clean(text: Option<&String>) -> Option<String> {
    let joined = text?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!joined.is_empty()).then_some(joined)
}

/// The peripheral `name` of `device`, its registers taken from the
/// peripheral it derives from when it has none of its own.
pub(super) fn block(device: &Device, name: &str) -> Result<SvdBlock, ImportError> {
    let context = format!("vendor SVD peripheral {name}");
    let peripheral = device
        .peripherals
        .iter()
        .find(|peripheral| peripheral.name == name)
        .ok_or_else(|| ImportError::new(&context, "the SVD has no such peripheral"))?;
    let mut source = peripheral;
    let mut depth = 0;
    while source.registers.is_none() {
        let Some(base) = &source.derived_from else {
            break;
        };
        depth += 1;
        if depth > 8 {
            return Err(ImportError::new(&context, "derivedFrom does not end"));
        }
        source = device
            .peripherals
            .iter()
            .find(|peripheral| &peripheral.name == base)
            .ok_or_else(|| {
                ImportError::new(&context, format!("derives from {base}, which the SVD lacks"))
            })?;
    }
    let default_size = source
        .default_register_properties
        .size
        .or(device.default_register_properties.size)
        .unwrap_or(32);
    let default_value = source
        .default_register_properties
        .reset_value
        .or(device.default_register_properties.reset_value);
    let default_mask = source
        .default_register_properties
        .reset_mask
        .or(device.default_register_properties.reset_mask);
    let mut registers = Vec::new();
    for child in source.registers.as_deref().unwrap_or_default() {
        let RegisterCluster::Register(register) = child else {
            return Err(ImportError::new(
                &context,
                "holds a cluster, which the import does not read",
            ));
        };
        let width = register.properties.size.unwrap_or(default_size);
        let value = register.properties.reset_value.or(default_value);
        let mask = register.properties.reset_mask.or(default_mask);
        let reset = value.map(|value| (value, mask.unwrap_or(u64::MAX)));
        let fields = fields(&context, register.fields.as_deref().unwrap_or_default())?;
        let description = clean(register.description.as_ref());
        let offset = register.address_offset;
        match register {
            MaybeArray::Single(info) => registers.push(SvdRegister {
                name: info.name.clone(),
                offset,
                width,
                reset,
                description,
                array: None,
                fields,
            }),
            MaybeArray::Array(info, dimension) => {
                let names = svd_rs::array::names(info, dimension).collect::<Vec<_>>();
                let indexed = info.name.matches("%s").count() == 1
                    && !info.name.contains("[%s]")
                    && dimension.dim_array_index.is_none()
                    && names
                        .iter()
                        .enumerate()
                        .all(|(index, name)| *name == info.name.replace("%s", &index.to_string()));
                for (index, name) in names.into_iter().enumerate() {
                    let index = u32::try_from(index)
                        .map_err(|_| ImportError::new(&context, "an array is too long"))?;
                    let element = index
                        .checked_mul(dimension.dim_increment)
                        .and_then(|step| offset.checked_add(step))
                        .ok_or_else(|| {
                            ImportError::new(&context, format!("{name} lies beyond 4 GiB"))
                        })?;
                    registers.push(SvdRegister {
                        name,
                        offset: element,
                        width,
                        reset,
                        description: description.clone(),
                        array: indexed.then(|| SvdArray {
                            name: info.name.clone(),
                            index,
                            count: dimension.dim,
                            stride: dimension.dim_increment,
                            description: description.clone(),
                        }),
                        fields: fields.clone(),
                    });
                }
            }
        }
    }
    let size = peripheral
        .address_block
        .as_deref()
        .or(source.address_block.as_deref())
        .and_then(|blocks| {
            blocks
                .iter()
                .map(|block| u64::from(block.offset) + u64::from(block.size))
                .max()
        });
    Ok(SvdBlock {
        base: peripheral.base_address,
        size,
        registers,
        interrupts: peripheral
            .interrupt
            .iter()
            .map(|interrupt| (interrupt.name.clone(), interrupt.value))
            .collect(),
    })
}

/// Every field element of a register.
fn fields(context: &str, fields: &[svd_rs::Field]) -> Result<Vec<SvdField>, ImportError> {
    let mut out = Vec::new();
    for field in fields {
        let info = match field {
            MaybeArray::Single(info) | MaybeArray::Array(info, _) => info,
        };
        let make = |name: String, low: u32| SvdField {
            name,
            low,
            width: info.bit_range.width,
            access: info.access,
            modified: info.modified_write_values,
            description: clean(info.description.as_ref()),
        };
        match field {
            MaybeArray::Single(info) => out.push(make(info.name.clone(), info.bit_range.offset)),
            MaybeArray::Array(info, dimension) => {
                for (index, name) in svd_rs::array::names(info, dimension).enumerate() {
                    let step = u32::try_from(index)
                        .ok()
                        .and_then(|index| index.checked_mul(dimension.dim_increment));
                    let low = step
                        .and_then(|step| info.bit_range.offset.checked_add(step))
                        .ok_or_else(|| {
                            ImportError::new(context, format!("field {name} lies beyond bit 2^32"))
                        })?;
                    out.push(make(name, low));
                }
            }
        }
    }
    Ok(out)
}
