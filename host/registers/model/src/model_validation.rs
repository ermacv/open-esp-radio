//! Hardware-model invariants not enforced by `svd-rs` strict validation.

use std::collections::BTreeSet;

use svd_rs::{
    Device, FieldInfo, RegisterCluster, RegisterInfo, RegisterProperties, Usage, WriteConstraint,
};

use crate::{Error, Result};

pub(super) fn validate_device(device: &Device) -> Result<()> {
    for peripheral in &device.peripherals {
        if let Some(children) = &peripheral.registers {
            let properties = merge_properties(
                device.default_register_properties,
                peripheral.default_register_properties,
            );
            validate_children(&peripheral.name, children, properties)?;
        }
    }
    Ok(())
}

fn validate_children(
    parent: &str,
    children: &[RegisterCluster],
    inherited: RegisterProperties,
) -> Result<()> {
    let mut names = BTreeSet::new();
    for child in children {
        let name = match child {
            RegisterCluster::Register(register) => register.name.as_str(),
            RegisterCluster::Cluster(cluster) => cluster.name.as_str(),
        };
        if !names.insert(name) {
            return Err(Error::message(format!(
                "duplicate register/cluster name {name:?} in model scope {parent}"
            )));
        }
        match child {
            RegisterCluster::Register(register) => {
                let properties = merge_properties(inherited, register.properties);
                validate_register(parent, register, properties)?;
            }
            RegisterCluster::Cluster(cluster) => {
                let properties = merge_properties(inherited, cluster.default_register_properties);
                validate_children(
                    &format!("{parent}.{}", cluster.name),
                    &cluster.children,
                    properties,
                )?;
            }
        }
    }
    Ok(())
}

fn validate_register(
    parent: &str,
    register: &RegisterInfo,
    properties: RegisterProperties,
) -> Result<()> {
    let identity = format!("{parent}.{}", register.name);
    let size_bits = properties
        .size
        .ok_or_else(|| Error::message(format!("register {identity} has no inherited size")))?;
    if !(1..=128).contains(&size_bits) {
        return Err(Error::message(format!(
            "register {identity} has unsupported size {size_bits}"
        )));
    }
    let mut names = BTreeSet::new();
    let mut occupied = 0_u128;
    for field in register.fields.iter().flatten() {
        if !names.insert(field.name.as_str()) {
            return Err(Error::message(format!(
                "register {identity} contains duplicate field name {:?}",
                field.name
            )));
        }
        let offset = field.bit_offset();
        let width = field.bit_width();
        if width == 0 || offset.checked_add(width).is_none_or(|end| end > size_bits) {
            return Err(Error::message(format!(
                "register {identity} field {} has invalid bit range {offset}+{width} for a {size_bits}-bit register",
                field.name
            )));
        }
        let mask = if width == 128 {
            u128::MAX
        } else {
            ((1_u128 << width) - 1) << offset
        };
        if occupied & mask != 0 {
            return Err(Error::message(format!(
                "register {identity} field {} overlaps another field",
                field.name
            )));
        }
        occupied |= mask;
        validate_field_semantics(&identity, field)?;
    }
    Ok(())
}

fn validate_field_semantics(register: &str, field: &FieldInfo) -> Result<()> {
    let identity = format!("{register}.{}", field.name);
    for values in &field.enumerated_values {
        let mut names = BTreeSet::new();
        let mut exact_values = BTreeSet::new();
        let mut defaults = 0;
        for value in &values.values {
            if !names.insert(value.name.as_str()) {
                return Err(Error::message(format!(
                    "field {identity} contains duplicate enumerated name {:?}",
                    value.name
                )));
            }
            if value.is_default() {
                defaults += 1;
            }
            if let Some(value) = value.value
                && !exact_values.insert(value)
            {
                return Err(Error::message(format!(
                    "field {identity} contains duplicate enumerated value {value}"
                )));
            }
        }
        if defaults > 1 {
            return Err(Error::message(format!(
                "field {identity} contains more than one default enumerated value"
            )));
        }
    }

    match field.write_constraint {
        Some(WriteConstraint::UseEnumeratedValues(false)) => Err(Error::message(format!(
            "field {identity} has non-operative useEnumeratedValues=false"
        ))),
        Some(WriteConstraint::UseEnumeratedValues(true))
            if !field.enumerated_values.iter().any(|values| {
                matches!(values.usage, None | Some(Usage::Write | Usage::ReadWrite))
            }) =>
        {
            Err(Error::message(format!(
                "field {identity} requires enumerated writes but defines no write enumeration"
            )))
        }
        Some(WriteConstraint::WriteAsRead(false)) => Err(Error::message(format!(
            "field {identity} has non-operative writeAsRead=false"
        ))),
        _ => Ok(()),
    }
}

const fn merge_properties(
    parent: RegisterProperties,
    child: RegisterProperties,
) -> RegisterProperties {
    let mut properties = parent;
    if child.size.is_some() {
        properties.size = child.size;
    }
    if child.access.is_some() {
        properties.access = child.access;
    }
    if child.protection.is_some() {
        properties.protection = child.protection;
    }
    if child.reset_value.is_some() {
        properties.reset_value = child.reset_value;
    }
    if child.reset_mask.is_some() {
        properties.reset_mask = child.reset_mask;
    }
    properties
}

#[cfg(test)]
mod tests {
    use super::*;
    use svd_rs::{EnumeratedValue, EnumeratedValues, ValidateLevel};

    fn value(name: &str, value: u64) -> EnumeratedValue {
        EnumeratedValue::builder()
            .name(name.to_owned())
            .value(Some(value))
            .build(ValidateLevel::Disabled)
            .unwrap()
    }

    fn field(values: Vec<EnumeratedValue>, usage: Usage, constraint: WriteConstraint) -> FieldInfo {
        let values = EnumeratedValues::builder()
            .usage(Some(usage))
            .values(values)
            .build(ValidateLevel::Disabled)
            .unwrap();
        FieldInfo::builder()
            .name("MODE".to_owned())
            .bit_offset(0)
            .bit_width(2)
            .enumerated_values(vec![values])
            .write_constraint(Some(constraint))
            .build(ValidateLevel::Disabled)
            .unwrap()
    }

    #[test]
    fn rejects_ambiguous_enumerations_and_nonoperative_constraints() {
        let duplicate = field(
            vec![value("OFF", 0), value("ALSO_OFF", 0)],
            Usage::Write,
            WriteConstraint::UseEnumeratedValues(true),
        );
        assert!(
            validate_field_semantics("RADIO.CONTROL", &duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate enumerated value")
        );

        let read_only = field(
            vec![value("OFF", 0)],
            Usage::Read,
            WriteConstraint::UseEnumeratedValues(true),
        );
        assert!(
            validate_field_semantics("RADIO.CONTROL", &read_only)
                .unwrap_err()
                .to_string()
                .contains("defines no write enumeration")
        );

        let disabled = field(
            vec![value("OFF", 0)],
            Usage::Write,
            WriteConstraint::UseEnumeratedValues(false),
        );
        assert!(
            validate_field_semantics("RADIO.CONTROL", &disabled)
                .unwrap_err()
                .to_string()
                .contains("non-operative")
        );
    }
}
