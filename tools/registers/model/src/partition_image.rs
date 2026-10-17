//! Read-only images of an ownership partition's readable registers.
//!
//! One enumeration defines the image order: the partition's readable
//! registers, array elements expanded, once per address and ascending by
//! address. The raw PAC reads them by index through their generated
//! accessors, and host cross-checks name the same indices from the same
//! enumeration, so both sides agree on what each index observes without the
//! facade exposing any address.

use std::collections::BTreeMap;

use svd_rs::{Access, Device, MaybeArray, RegisterCluster};

use crate::{Error, PacApiPack, Result, pac_api_svd};

/// One readable register of an ownership partition in image order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionRegister {
    pub peripheral: String,
    /// Register name as published, with any dimension placeholder.
    pub register: String,
    /// Element of a register array.
    pub element: Option<usize>,
    pub address: u32,
}

impl PacApiPack {
    /// The readable registers of ownership partition `partition` in `svd`,
    /// in image order.
    pub fn partition_readable_registers(
        &self,
        svd: &str,
        partition: &str,
    ) -> Result<Vec<PartitionRegister>> {
        let device = svd_parser::parse(svd).map_err(|error| Error::message(error.to_string()))?;
        self.partition_registers(&device, partition)
    }

    pub(crate) fn partition_registers(
        &self,
        device: &Device,
        partition: &str,
    ) -> Result<Vec<PartitionRegister>> {
        let members = &self
            .ownership_partitions
            .iter()
            .find(|candidate| candidate.name == partition)
            .ok_or_else(|| Error::message(format!("unknown ownership partition {partition:?}")))?
            .peripherals;
        let mut registers = BTreeMap::new();
        for name in members {
            let peripheral = device
                .peripherals
                .iter()
                .find(|candidate| &candidate.name == name)
                .ok_or_else(|| {
                    Error::message(format!(
                        "partition {partition} names unknown peripheral {name}"
                    ))
                })?;
            let base = u32::try_from(peripheral.base_address).map_err(|_| {
                Error::message(format!("peripheral {name} lies outside the 32-bit space"))
            })?;
            for child in peripheral.registers.as_deref().unwrap_or_default() {
                let RegisterCluster::Register(register) = child else {
                    return Err(Error::message(format!(
                        "partition image of {partition} does not support cluster {name}.{}",
                        child.name()
                    )));
                };
                let properties = pac_api_svd::merged_properties(device, peripheral, register);
                if !properties.access.unwrap_or(Access::ReadWrite).can_read() {
                    continue;
                }
                let elements: Vec<(Option<usize>, u32)> = match register {
                    MaybeArray::Single(info) => vec![(None, info.address_offset)],
                    MaybeArray::Array(info, dimension) => (0..dimension.dim)
                        .map(|element| {
                            (
                                Some(element as usize),
                                info.address_offset + element * dimension.dim_increment,
                            )
                        })
                        .collect(),
                };
                for (element, offset) in elements {
                    registers
                        .entry(base + offset)
                        .or_insert_with(|| PartitionRegister {
                            peripheral: name.clone(),
                            register: register.name.clone(),
                            element,
                            address: base + offset,
                        });
                }
            }
        }
        Ok(registers.into_values().collect())
    }

    /// Raw-PAC modules reading every declared partition image by index.
    pub(crate) fn render_partition_image_reads(&self, device: &Device) -> Result<String> {
        if self.partition_image_reads.is_empty() {
            return Ok(String::new());
        }
        let mut output = String::from(
            "\n/// Safe, SVD-declared read-only observations of every readable register of\n\
             /// an ownership partition, by image index.\n\
             pub mod partition_image_read {\n",
        );
        for operation in &self.partition_image_reads {
            let registers = self.partition_registers(device, &operation.partition)?;
            let mut arms = String::new();
            for (index, register) in registers.iter().enumerate() {
                let element = register
                    .element
                    .map_or(String::new(), |element| element.to_string());
                arms.push_str(&format!(
                    "            {index} => registers.{}.{}({element}).read().bits(),\n",
                    crate::pac_api_render::member_binding_name(&register.peripheral),
                    crate::pac_api_render::member_binding_name(&register.register),
                ));
            }
            output.push_str(&format!(
                "\n    /// Registers of `{partition}` that [`{name}`] observes.\n\
                 pub const {length}: usize = {count};\n\n\
                 /// Read register `index` of the `{partition}` image; `None` past its end.\n\
                 pub fn {name}(registers: &crate::peripheral_ownership::{partition}, index: usize) -> Option<u32> {{\n\
                 \x20       Some(match index {{\n{arms}\
                 \x20           _ => return None,\n\
                 \x20       }})\n\
                 \x20   }}\n",
                partition = operation.partition,
                name = operation.name,
                length = image_length_name(&operation.name),
                count = registers.len(),
            ));
        }
        output.push_str("}\n");
        Ok(output)
    }

    /// Facade bridges of the partition images the policy exposes.
    pub(crate) fn render_partition_image_bridges(&self) -> String {
        let mut output = String::new();
        for operation in &self.partition_image_reads {
            if !operation.exposure.exposes_facade() {
                continue;
            }
            let length = image_length_name(&operation.name);
            output.push_str(&format!(
                "/// Registers the reviewed `{name}` partition image observes.\n\
                 pub(crate) const {length}: usize = crate::svd::partition_image_read::{length};\n\n\
                 /// Typed bridge for the reviewed `{name}` partition image.\n\
                 #[inline]\n\
                 pub(crate) fn {name}(registers: &crate::svd::peripheral_ownership::{partition}, index: usize) -> Option<u32> {{\n\
                 \x20   crate::svd::partition_image_read::{name}(registers, index)\n\
                 }}\n\n",
                name = operation.name,
                partition = operation.partition,
            ));
        }
        output
    }
}

/// Name of the length constant of image `name`.
fn image_length_name(name: &str) -> String {
    format!("{}_LEN", name.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OwnershipPartition, PacApiExposure, PartitionImageRead};

    const SVD: &str = r#"<?xml version="1.0"?>
<device><name>T</name><addressUnitBits>8</addressUnitBits><width>32</width>
<size>32</size><access>read-write</access><resetValue>0</resetValue><resetMask>0xffffffff</resetMask>
<peripherals>
<peripheral><name>A</name><baseAddress>0x1000</baseAddress><registers>
<register><name>RO</name><addressOffset>0x8</addressOffset><access>read-only</access></register>
<register><name>WO</name><addressOffset>0x4</addressOffset><access>write-only</access></register>
<register><name>RW</name><addressOffset>0x0</addressOffset></register>
<register><dim>2</dim><dimIncrement>0x4</dimIncrement><name>ARR%s</name><addressOffset>0x10</addressOffset></register>
</registers></peripheral>
<peripheral><name>B</name><baseAddress>0x1000</baseAddress><registers>
<register><name>ALIAS</name><addressOffset>0x0</addressOffset></register>
</registers></peripheral>
</peripherals></device>"#;

    fn pack() -> PacApiPack {
        let mut pack = crate::pac_api::tests::empty_pack();
        pack.ownership_partitions.push(OwnershipPartition {
            name: "Owned".into(),
            member: "owned".into(),
            description: "test partition".into(),
            peripherals: vec!["A".into(), "B".into()],
        });
        pack.partition_image_reads.push(PartitionImageRead {
            name: "owned_image".into(),
            partition: "Owned".into(),
            exposure: PacApiExposure::Facade,
            sources: vec!["TEST".into()],
        });
        pack
    }

    #[test]
    fn readable_registers_are_ordered_by_address_once_each() {
        let registers = pack().partition_readable_registers(SVD, "Owned").unwrap();
        let listed: Vec<_> = registers
            .iter()
            .map(|r| (r.register.as_str(), r.element, r.address))
            .collect();
        assert_eq!(
            listed,
            [
                ("RW", None, 0x1000),
                ("RO", None, 0x1008),
                ("ARR%s", Some(0), 0x1010),
                ("ARR%s", Some(1), 0x1014),
            ]
        );
    }

    #[test]
    fn the_raw_image_reads_each_register_by_index() {
        let device = svd_parser::parse(SVD).unwrap();
        let rendered = pack().render_partition_image_reads(&device).unwrap();
        assert!(rendered.contains("pub const OWNED_IMAGE_LEN: usize = 4;"));
        assert!(rendered.contains("0 => registers.a.rw().read().bits(),"));
        assert!(rendered.contains("3 => registers.a.arr(1).read().bits(),"));
        assert!(
            pack()
                .render_partition_image_bridges()
                .contains("pub(crate) fn owned_image(")
        );
    }
}
