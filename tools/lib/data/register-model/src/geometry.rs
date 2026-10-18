//! Lossless expanded geometry for research queries, independent of PAC exposure.

use super::*;

#[derive(Clone, Debug, Serialize)]
pub struct FieldGeometry {
    pub name: String,
    pub offset: u32,
    pub width: u32,
    /// Complete declaration, including enums, constraints and side effects.
    pub declaration: svd_rs::FieldInfo,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegisterGeometry {
    pub address: u64,
    pub width: Option<u32>,
    pub name: String,
    pub template: String,
    pub declaration: RegisterInfo,
    pub fields: Vec<FieldGeometry>,
}

impl RegisterModel {
    /// Expanded declarations with inherited properties and every field.
    pub fn register_geometry(&self) -> Result<Vec<RegisterGeometry>> {
        device_geometry(&self.device)
    }
}

/// Expand SVD inheritance, clusters, arrays and field declarations. The caller
/// retains the original XML as evidence, including unsupported extensions.
pub fn svd_geometry(xml: &str) -> Result<Vec<RegisterGeometry>> {
    let device = svd_parser::parse_with_config(xml, &svd_parser::Config::default().expand(true))
        .map_err(|error| Error::message(error.to_string()))?;
    device_geometry(&device)
}

fn device_geometry(device: &Device) -> Result<Vec<RegisterGeometry>> {
    let mut output = Vec::new();
    for peripheral in &device.peripherals {
        let template = peripheral.name.clone();
        let instances: Vec<_> = match peripheral {
            MaybeArray::Single(info) => vec![info.clone()],
            MaybeArray::Array(info, dim) => svd_rs::peripheral::expand(info, dim).collect(),
        };
        for instance in instances {
            collect(
                &mut output,
                instance.base_address,
                merge_properties(
                    device.default_register_properties,
                    instance.default_register_properties,
                ),
                &instance.name,
                &template,
                instance.registers.as_deref().unwrap_or_default(),
            )?;
        }
    }
    output.sort_by_key(|register| (register.address, register.width, register.name.clone()));
    Ok(output)
}

fn collect(
    output: &mut Vec<RegisterGeometry>,
    base: u64,
    inherited: RegisterProperties,
    name: &str,
    template: &str,
    children: &[RegisterCluster],
) -> Result<()> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let instances: Vec<_> = match register {
                    MaybeArray::Single(info) => vec![info.clone()],
                    MaybeArray::Array(info, dim) => svd_rs::register::expand(info, dim).collect(),
                };
                for mut instance in instances {
                    instance.properties = merge_properties(inherited, instance.properties);
                    let width = instance.properties.size;
                    let address = base
                        .checked_add(u64::from(instance.address_offset))
                        .ok_or_else(|| Error::message("register geometry address overflow"))?;
                    let mut fields = Vec::new();
                    for field in instance.fields.iter().flatten() {
                        let instances: Vec<_> = match field {
                            MaybeArray::Single(info) => vec![info.clone()],
                            MaybeArray::Array(info, dim) => {
                                svd_rs::field::expand(info, dim).collect()
                            }
                        };
                        for field in instances {
                            fields.push(FieldGeometry {
                                name: field.name.clone(),
                                offset: field.bit_offset(),
                                width: field.bit_width(),
                                declaration: field,
                            });
                        }
                    }
                    output.push(RegisterGeometry {
                        address,
                        width,
                        name: format!("{name}.{}", instance.name),
                        template: format!("{template}.{}", register.name),
                        declaration: instance,
                        fields,
                    });
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let instances: Vec<_> = match cluster {
                    MaybeArray::Single(info) => vec![info.clone()],
                    MaybeArray::Array(info, dim) => svd_rs::cluster::expand(info, dim).collect(),
                };
                for instance in instances {
                    let base = base
                        .checked_add(u64::from(instance.address_offset))
                        .ok_or_else(|| Error::message("cluster geometry address overflow"))?;
                    collect(
                        output,
                        base,
                        merge_properties(inherited, instance.default_register_properties),
                        &format!("{name}.{}", instance.name),
                        &format!("{template}.{}", cluster.name),
                        &instance.children,
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct PeripheralRegion {
    pub name: String,
    pub start: u64,
    pub end_exclusive: u64,
    pub declaration: svd_rs::AddressBlock,
}

fn regions(device: &Device) -> Result<Vec<PeripheralRegion>> {
    let mut output = Vec::new();
    for peripheral in &device.peripherals {
        let instances: Vec<_> = match peripheral {
            MaybeArray::Single(info) => vec![info.clone()],
            MaybeArray::Array(info, dim) => svd_rs::peripheral::expand(info, dim).collect(),
        };
        for instance in instances {
            for block in instance.address_block.iter().flatten() {
                if block.usage != AddressBlockUsage::Registers {
                    continue;
                }
                let start = instance
                    .base_address
                    .checked_add(u64::from(block.offset))
                    .ok_or_else(|| Error::message("peripheral range overflow"))?;
                let end_exclusive = start
                    .checked_add(u64::from(block.size))
                    .ok_or_else(|| Error::message("peripheral range overflow"))?;
                output.push(PeripheralRegion {
                    name: instance.name.clone(),
                    start,
                    end_exclusive,
                    declaration: block.clone(),
                });
            }
        }
    }
    Ok(output)
}

impl RegisterModel {
    pub fn peripheral_regions(&self) -> Result<Vec<PeripheralRegion>> {
        regions(&self.device)
    }
}

pub fn svd_regions(xml: &str) -> Result<Vec<PeripheralRegion>> {
    let device = svd_parser::parse_with_config(xml, &svd_parser::Config::default().expand(true))
        .map_err(|error| Error::message(error.to_string()))?;
    regions(&device)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svd_clusters_arrays_and_inherited_fields_preserve_complete_declarations() {
        let xml = r#"<device schemaVersion="1.3"><name>TEST</name><version>1</version><description>fixture</description><addressUnitBits>8</addressUnitBits><width>32</width><size>32</size><peripherals><peripheral><name>DEV</name><baseAddress>4096</baseAddress><registers><cluster><dim>2</dim><dimIncrement>16</dimIncrement><dimIndex>A,C</dimIndex><name>CHANNEL%s</name><addressOffset>0</addressOffset><register><name>FIRST</name><description>kept description</description><addressOffset>0</addressOffset><fields><field><name>MODE</name><bitOffset>2</bitOffset><bitWidth>2</bitWidth><enumeratedValues><enumeratedValue><name>ON</name><description>enum meaning</description><value>1</value></enumeratedValue></enumeratedValues></field></fields></register><register derivedFrom="FIRST"><name>SECOND</name><addressOffset>4</addressOffset></register></cluster></registers></peripheral></peripherals></device>"#;
        let registers = svd_geometry(xml).unwrap();
        assert_eq!(registers.len(), 4);
        assert!(
            registers
                .iter()
                .all(|register| register.width == Some(32) && register.fields.len() == 1)
        );
        assert!(
            registers
                .iter()
                .any(|register| register.name.contains("CHANNELC"))
        );
        assert!(registers.iter().all(|register| {
            register.fields[0].declaration.enumerated_values[0].values[0].name == "ON"
        }));
        assert!(registers.iter().all(
            |register| register.declaration.description.as_deref() == Some("kept description")
        ));
    }
}
