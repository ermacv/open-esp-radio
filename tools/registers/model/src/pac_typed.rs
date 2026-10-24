//! Typed register blocks over `oer-mmio`: the generator's own renderer for
//! the peripherals a PAC API policy lists in `typed-peripherals`.
//!
//! The rendered code holds no `unsafe` and no raw write. A block is an owner
//! type around an `oer_mmio::Block` (or one `Part` of a declared split); a
//! register has typed field readers and writers: `bool` for one bit, a
//! value enum where the model enumerates the field's values, the exact
//! native integer for 8, 16 and 32 bits and `oer_mmio::Bits<WIDTH>`
//! otherwise. Reads and read-modify-writes keep every other bit, reserved and
//! undeclared ones included; a write-only register is written from zero; a
//! write-one-to-clear register is sampled into an affine snapshot and
//! acknowledged with exactly that snapshot. A register array takes an
//! `oer_mmio::Index`. A DMA address field has no generic writer: only its
//! policy transaction, which takes a typed DMA reference, writes it. The
//! policy's transactions on the block become methods of the owners that
//! own their register, with their masks checked by `const` assertions.
//! A policy-declared write-trigger field is zero in every read-modify-write
//! of its register and one only in its explicit trigger; a policy-declared
//! FIFO port is only popped and pushed.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use svd_rs::{
    Access, Device, FieldInfo, MaybeArray, ModifiedWriteValues, Peripheral, RegisterCluster,
    RegisterInfo, WriteConstraint,
};

use crate::{
    Error, FieldOrModify, FullRegisterValue, PacApiPack, RegisterSides, Result, SharedPeripheral,
    pac_api_render::{domain_value_expression, push_doc, type_binding_name},
};

impl PacApiPack {
    /// The peripherals rendered as typed register blocks.
    pub fn typed_peripherals(&self) -> BTreeSet<&str> {
        self.options
            .typed_peripherals
            .iter()
            .map(String::as_str)
            .collect()
    }

    /// The policy without the transactions, pointer fields, partition
    /// members and splits of typed peripherals: what the facade renders
    /// bridges for. Every value domain stays; both backends name them.
    pub fn without_typed_operations(&self) -> Self {
        let typed = self
            .options
            .typed_peripherals
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut pack = self.clone();
        pack.options.typed_peripherals.clear();
        pack.register_sides.clear();
        pack.interrupt_sources.clear();
        pack.write_trigger_fields.clear();
        pack.fifo_ports.clear();
        macro_rules! retain {
            ($($operations:ident),* $(,)?) => {
                $(pack.$operations.retain(|operation| !typed.contains(&operation.peripheral));)*
            };
        }
        retain!(
            interrupt_snapshots,
            full_register_writes,
            pointer_fields,
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
            field_clear_modifies,
            field_replace_modifies,
            field_argument_modifies,
            indexed_bit_set_modifies,
        );
        for partition in &mut pack.ownership_partitions {
            partition
                .peripherals
                .retain(|peripheral| !typed.contains(peripheral));
        }
        pack.ownership_partitions
            .retain(|partition| !partition.peripherals.is_empty());
        pack
    }

    /// The policy the svd2rust backend renders: no operation and no domain
    /// bound to a typed peripheral, which its SVD no longer holds.
    pub fn without_typed(&self) -> Self {
        let typed = self.typed_peripherals();
        let mut pack = self.without_typed_operations();
        pack.opaque_domains
            .retain(|domain| !typed.contains(domain.peripheral.as_str()));
        pack.bitwise_composed_domains
            .retain(|domain| !typed.contains(domain.peripheral.as_str()));
        pack
    }

    /// Render every typed peripheral of a chip's `svd` with its
    /// transactions, for the PAC API module (`generated.rs`), whose root
    /// holds the value domains and the `layouts` module the transactions
    /// name. A peripheral whose layout a register library owns (`libraries`,
    /// by peripheral) is that library's block placed at the chip's address:
    /// the library renders its accessors and transactions once.
    pub fn render_typed_rust(
        &self,
        svd: &str,
        libraries: &BTreeMap<String, SharedPeripheral>,
    ) -> Result<String> {
        let typed = self.typed_peripherals();
        if let Some(peripheral) = libraries
            .keys()
            .find(|peripheral| !typed.contains(peripheral.as_str()))
        {
            return Err(Error::message(format!(
                "{peripheral}'s layout comes from a register library, whose blocks are typed: list it in typed-peripherals"
            )));
        }
        if typed.is_empty() {
            return Ok(String::new());
        }
        self.validate_typed_operations(&typed)?;
        if let Some(peripheral) = self
            .register_sides
            .iter()
            .map(|sides| sides.peripheral.as_str())
            .chain(
                self.write_trigger_fields
                    .iter()
                    .map(|trigger| trigger.peripheral.as_str()),
            )
            .chain(self.fifo_ports.iter().map(|port| port.peripheral.as_str()))
            .find(|peripheral| libraries.contains_key(*peripheral))
        {
            return Err(Error::message(format!(
                "PAC API register sides, write-trigger field or FIFO port of {peripheral} change a block whose layout a register library renders; declare it in the library's API"
            )));
        }
        let device = svd_parser::parse(svd).map_err(|error| Error::message(error.to_string()))?;
        let mut output = String::from(
            "// Typed register blocks over `oer-mmio`, rendered by `cargo registers`\n\
             // from the published SVD and the API policy; no `unsafe`, no raw write.\n\n",
        );
        if let Some(name) = self.options.unexposed_peripherals.iter().find(|name| {
            !device
                .peripherals
                .iter()
                .any(|peripheral| &peripheral.name == *name)
        }) {
            return Err(Error::message(format!(
                "unexposed peripheral {name:?} is not in the SVD"
            )));
        }
        let mut blocks = BTreeMap::new();
        for name in typed {
            let peripheral = device
                .peripherals
                .iter()
                .find(|peripheral| peripheral.name == name)
                .ok_or_else(|| {
                    Error::message(format!("typed peripheral {name:?} is not in the SVD"))
                })?;
            if let Some(library) = libraries.get(name) {
                let (module, owner) = render_placed(&mut output, peripheral, library)?;
                blocks.insert(name, (module, owner));
                continue;
            }
            let block = TypedBlock::new(&device, peripheral, self, Some(peripheral.base_address))?;
            block.render(&mut output, self)?;
            blocks.insert(name, (block.module.clone(), block.owner.clone()));
        }
        output.push_str(&self.render_typed_partitions(&blocks)?);
        Ok(output)
    }

    /// Render every peripheral of a register library's layout `svd` as a
    /// typed block without an address, with its transactions: each chip
    /// that places the layout claims the block at its own address. The
    /// library has no partitions, no claim and no interrupt source; it
    /// splits no block.
    pub fn render_library_typed_rust(&self, svd: &str) -> Result<String> {
        if !self.options.typed_peripherals.is_empty()
            || !self.options.unexposed_peripherals.is_empty()
        {
            return Err(Error::message(
                "a register library types every layout it owns: its API lists neither typed nor unexposed peripherals",
            ));
        }
        if !self.ownership_partitions.is_empty()
            || !self.register_sides.is_empty()
            || !self.interrupt_sources.is_empty()
        {
            return Err(Error::message(
                "a register library's API declares no ownership partition, register sides or interrupt source: they belong to a chip",
            ));
        }
        let device = svd_parser::parse(svd).map_err(|error| Error::message(error.to_string()))?;
        let mut pack = self.clone();
        pack.options.typed_peripherals = device
            .peripherals
            .iter()
            .map(|peripheral| peripheral.name.clone())
            .collect();
        pack.validate_typed_operations(&pack.typed_peripherals())?;
        let mut output = String::from(
            "// Typed register blocks over `oer-mmio` without an address, rendered by\n\
             // `cargo registers` from the library's layouts and API policy; no\n\
             // `unsafe`, no raw write. Each chip places them at its own address.\n\n",
        );
        for peripheral in &device.peripherals {
            TypedBlock::new(&device, peripheral, &pack, None)?.render(&mut output, &pack)?;
        }
        Ok(output)
    }

    /// The ownership partitions of the typed blocks, the claim of every
    /// typed block the PAC root makes at once, and the compile-time check
    /// that no two blocks list the same word.
    fn render_typed_partitions(&self, blocks: &BTreeMap<&str, (String, String)>) -> Result<String> {
        let partitions = self
            .ownership_partitions
            .iter()
            .filter(|partition| {
                partition
                    .peripherals
                    .iter()
                    .any(|peripheral| blocks.contains_key(peripheral.as_str()))
            })
            .collect::<Vec<_>>();
        let mut output = String::new();
        let mut ordered = Vec::new();
        if !partitions.is_empty() {
            output.push_str(
                "/// The ownership partitions of the typed register blocks.\n\
                 pub mod peripheral_ownership {\n",
            );
            let mut members = String::new();
            let mut built = String::new();
            for partition in &partitions {
                let mut fields = String::new();
                let mut values = String::new();
                for peripheral in &partition.peripherals {
                    let (module, owner) = blocks.get(peripheral.as_str()).ok_or_else(|| {
                        Error::message(format!(
                            "ownership partition {} mixes typed peripherals and {peripheral}, which is not typed",
                            partition.name
                        ))
                    })?;
                    let _ = writeln!(fields, "pub {module}: super::{module}::{owner},");
                    let _ = writeln!(values, "{module}: super::{module}::{owner}::new({module}),");
                    ordered.push(module.clone());
                }
                push_doc(&mut output, &partition.description, "");
                let _ = writeln!(output, "pub struct {} {{\n{fields}}}\n", partition.name);
                push_doc(&mut members, &partition.description, "");
                let _ = writeln!(members, "pub {}: {},", partition.member, partition.name);
                let _ = writeln!(
                    built,
                    "{}: {} {{\n{values}}},",
                    partition.member, partition.name
                );
            }
            let (claims, pattern) = claim_tuple(&ordered);
            let _ = writeln!(
                output,
                "/// Every typed partition.\n\
                 pub struct PeripheralPartitions {{\n{members}}}\n\n\
                 /// The device registers of every typed block, which the PAC root\n\
                 /// takes with one `oer_mmio::assume_owned`.\n\
                 pub type Claims = {claims};\n\n\
                 /// Give each claimed block to its owner in its partition.\n\
                 #[inline]\n\
                 pub fn partition(claims: Claims) -> PeripheralPartitions {{\n\
                 let {pattern} = claims;\n\
                 PeripheralPartitions {{\n{built}}}\n\
                 }}\n\
                 }}\n"
            );
        }
        if !blocks.is_empty() {
            let list = blocks
                .values()
                .map(|(module, _)| {
                    format!(
                        "({module}::BASE, <{module}::BlockWords as oer_mmio::Registers>::WORDS)"
                    )
                })
                .collect::<Vec<_>>()
                .join(",\n");
            let _ = writeln!(
                output,
                "const _: () = assert!(\n\
                 oer_mmio::check::disjoint_blocks(&[\n{list}\n]),\n\
                 \"no two typed blocks list the same word\"\n\
                 );\n"
            );
        }
        Ok(output)
    }

    /// The name and register of every transaction the typed backend renders
    /// on `peripheral`.
    fn typed_register_operations<'s>(
        &'s self,
        peripheral: &'s str,
    ) -> impl Iterator<Item = (&'s str, &'s str)> + 's {
        let mut operations = Vec::new();
        macro_rules! collect {
            ($($operations:ident),* $(,)?) => {
                $(operations.extend(
                    self.$operations
                        .iter()
                        .filter(|operation| operation.peripheral == peripheral)
                        .map(|operation| (operation.name.as_str(), operation.register.as_str())),
                );)*
            };
        }
        collect!(
            full_register_writes,
            full_register_reads,
            fixed_register_images,
            masked_register_modifies,
            field_or_modifies,
            field_clear_modifies,
            field_replace_modifies,
            zero_based_field_writes,
        );
        for operation in self
            .fixed_register_sequences
            .iter()
            .filter(|operation| operation.peripheral == peripheral)
        {
            operations.extend(
                operation
                    .steps
                    .iter()
                    .map(|step| (operation.name.as_str(), step.register.as_str())),
            );
        }
        operations.into_iter()
    }

    /// Typed peripherals take only the transactions the typed backend
    /// renders; the affine W1C snapshot is every W1C register's own API.
    fn validate_typed_operations(&self, typed: &BTreeSet<&str>) -> Result<()> {
        macro_rules! refuse {
            ($($operations:ident => $reason:literal),* $(,)?) => {
                $(if let Some(operation) = self.$operations.iter().find(|operation| typed.contains(operation.peripheral.as_str())) {
                    return Err(Error::message(format!(
                        "PAC API {} {:?} is on typed peripheral {}: {}",
                        stringify!($operations), operation.name, operation.peripheral, $reason
                    )));
                })*
            };
        }
        refuse!(
            interrupt_snapshots => "a typed W1C register samples and acknowledges itself",
            w1c_register_snapshots => "a typed W1C register samples and acknowledges itself",
            pointer_fields => "the typed backend renders no composed pointer image yet",
            field_reads => "use the typed register reader",
            field_snapshot_reads => "use the typed register reader",
            fixed_register_writes => "the typed backend renders no fixed variant write yet",
            register_image_reads => "the typed backend renders no register image read yet",
            register_image_writes => "the typed backend renders no register image write yet",
            sampled_bit_zero_writes => "the typed backend renders no sampled-bit write yet",
            zero_register_writes => "the typed backend renders no zero write yet",
            field_argument_modifies => "the typed backend renders no multi-argument modify yet",
            indexed_bit_set_modifies => "the typed backend renders no indexed bit set yet",
        );
        if let Some(peripheral) = self
            .register_sides
            .iter()
            .map(|sides| sides.peripheral.as_str())
            .chain(
                self.interrupt_sources
                    .iter()
                    .map(|source| source.peripheral.as_str()),
            )
            .chain(
                self.write_trigger_fields
                    .iter()
                    .map(|trigger| trigger.peripheral.as_str()),
            )
            .chain(self.fifo_ports.iter().map(|port| port.peripheral.as_str()))
            .find(|peripheral| !typed.contains(peripheral))
        {
            return Err(Error::message(format!(
                "PAC API register sides, interrupt source, write-trigger field or FIFO port of {peripheral} name a peripheral that is not typed"
            )));
        }
        Ok(())
    }
}

/// One rendered field type.
#[derive(Clone)]
enum FieldType {
    Bool,
    Native(&'static str),
    Bits(u32),
    Enum(String),
}

impl FieldType {
    fn rust(&self) -> String {
        match self {
            Self::Bool => "bool".to_owned(),
            Self::Native(native) => (*native).to_owned(),
            Self::Bits(width) => format!("oer_mmio::Bits<{width}>"),
            Self::Enum(name) => name.clone(),
        }
    }

    fn bits(&self) -> &'static str {
        match self {
            Self::Bool => "u32::from(value)",
            Self::Native("u32") => "value",
            Self::Native(_) => "u32::from(value)",
            Self::Bits(_) => "value.get()",
            Self::Enum(_) => "value.bits()",
        }
    }
}

struct Enumeration {
    name: String,
    variants: Vec<(String, u64, Option<String>)>,
    complete: bool,
}

struct TypedField<'a> {
    info: &'a FieldInfo,
    method: String,
    offset: u32,
    width: u32,
    readable: bool,
    writable: bool,
    pointer: bool,
    /// A write-trigger field: only its explicit trigger writes one to it.
    trigger: bool,
    enumeration: Option<Enumeration>,
}

impl TypedField<'_> {
    fn mask(&self) -> u32 {
        (u32::MAX >> (32 - self.width)) << self.offset
    }

    /// The type a reader returns: the writer's type without the value
    /// enum, so a value read back is written again without a conversion.
    fn read_type(&self) -> FieldType {
        match self.width {
            1 => FieldType::Bool,
            8 => FieldType::Native("u8"),
            16 => FieldType::Native("u16"),
            32 => FieldType::Native("u32"),
            width => FieldType::Bits(width),
        }
    }

    fn write_type(&self) -> FieldType {
        if let Some(enumeration) = &self.enumeration {
            return FieldType::Enum(enumeration.name.clone());
        }
        match self.width {
            1 => FieldType::Bool,
            8 => FieldType::Native("u8"),
            16 => FieldType::Native("u16"),
            32 => FieldType::Native("u32"),
            width => FieldType::Bits(width),
        }
    }

    fn has_setter(&self) -> bool {
        self.writable && !self.pointer && !self.trigger
    }

    /// Whether a reader returns the field: a trigger reads back nothing the
    /// register keeps.
    fn has_getter(&self) -> bool {
        self.readable && !self.trigger
    }

    /// The field's value in `bits`, the register image.
    fn read_expression(&self, bits: &str) -> String {
        if self.width == 1 {
            return format!("({bits} & 0x{:08x}) != 0", self.mask());
        }
        let shifted = if self.offset == 0 {
            bits.to_owned()
        } else {
            format!("({bits} >> {})", self.offset)
        };
        match self.read_type() {
            FieldType::Native("u32") => shifted,
            FieldType::Native(native) => format!("({shifted}) as {native}"),
            _ => format!(
                "oer_mmio::Bits::truncate({})",
                shifted.trim_start_matches('(').trim_end_matches(')')
            ),
        }
    }
}

struct TypedRegister<'a> {
    info: &'a RegisterInfo,
    method: String,
    offset: u64,
    array: Option<(u64, u64)>,
    readable: bool,
    writable: bool,
    clears_on_read: bool,
    w1c: bool,
    /// A FIFO port: popped and pushed only.
    fifo: bool,
    fields: Vec<TypedField<'a>>,
}

impl TypedRegister<'_> {
    fn words(&self) -> Vec<u64> {
        match self.array {
            None => vec![self.offset],
            Some((stride, count)) => (0..count)
                .map(|index| self.offset + stride * index)
                .collect(),
        }
    }

    fn declared(&self) -> u32 {
        self.fields
            .iter()
            .fold(0, |mask, field| mask | field.mask())
    }

    fn has_setters(&self) -> bool {
        !self.w1c && !self.fifo && self.fields.iter().any(TypedField::has_setter)
    }

    /// The bits of the register's write-trigger fields.
    fn triggers(&self) -> u32 {
        self.fields
            .iter()
            .filter(|field| field.trigger)
            .fold(0, |mask, field| mask | field.mask())
    }

    /// The read a read-modify-write starts from: every write-trigger field
    /// zero, so writing the image back starts no hardware action.
    fn modify_read_call(&self) -> String {
        if self.triggers() == 0 {
            self.read_call()
        } else {
            format!("{} & !{}::TRIGGERS", self.read_call(), self.method)
        }
    }

    fn index_parameter(&self) -> String {
        self.array.map_or_else(String::new, |(_, count)| {
            format!("index: oer_mmio::Index<{count}>, ")
        })
    }

    fn read_call(&self) -> String {
        match self.array {
            None => format!("read::<0x{:x}>()", self.offset),
            Some((stride, count)) => format!(
                "read_indexed::<0x{:x}, 0x{stride:x}, {count}>(index)",
                self.offset
            ),
        }
    }

    fn write_call(&self, value: &str) -> String {
        match self.array {
            None => format!("write::<0x{:x}>({value})", self.offset),
            Some((stride, count)) => format!(
                "write_indexed::<0x{:x}, 0x{stride:x}, {count}>(index, {value})",
                self.offset
            ),
        }
    }
}

/// What the policy declares about one register beyond the SVD.
struct DeclaredKinds<'p> {
    /// The `(register, field)` DMA address fields.
    pointers: &'p BTreeSet<(&'p str, &'p str)>,
    /// The `(register, field)` write-trigger fields.
    triggers: &'p BTreeSet<(&'p str, &'p str)>,
    /// Whether the register is a FIFO port.
    fifo: bool,
}

/// The owner type suffix of a block's interrupt-handler side.
const IRQ_SIDE: &str = "Irq";
/// The owner type suffix of a block's task side.
const TASK_SIDE: &str = "Task";

/// How one owner reaches a register.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Reach {
    Owned,
    Observed,
}

struct View {
    name: String,
    inner: String,
    reach: BTreeMap<String, Reach>,
}

struct TypedBlock<'a> {
    peripheral: &'a Peripheral,
    module: String,
    owner: String,
    /// The chip's base address; a register library's block has none.
    base: Option<u64>,
    registers: Vec<TypedRegister<'a>>,
    split: Option<&'a RegisterSides>,
}

impl<'a> TypedBlock<'a> {
    fn new(
        device: &Device,
        peripheral: &'a Peripheral,
        policy: &'a PacApiPack,
        base: Option<u64>,
    ) -> Result<Self> {
        let name = peripheral.name.as_str();
        if matches!(peripheral, MaybeArray::Array(..)) || peripheral.derived_from.is_some() {
            return Err(Error::message(format!(
                "typed peripheral {name} must be one plain SVD peripheral"
            )));
        }
        let pointers = policy
            .full_register_writes
            .iter()
            .filter(|operation| operation.peripheral == name)
            .filter(|operation| !matches!(operation.value(), Ok(FullRegisterValue::Domain(_))))
            .map(|operation| (operation.register.as_str(), operation.field.as_str()))
            .collect::<BTreeSet<_>>();
        let triggers = policy
            .write_trigger_fields
            .iter()
            .filter(|trigger| trigger.peripheral == name)
            .map(|trigger| (trigger.register.as_str(), trigger.field.as_str()))
            .collect::<BTreeSet<_>>();
        let fifos = policy
            .fifo_ports
            .iter()
            .filter(|port| port.peripheral == name)
            .map(|port| port.register.as_str())
            .collect::<BTreeSet<_>>();
        let mut registers = Vec::new();
        for child in peripheral.registers.as_deref().unwrap_or_default() {
            let RegisterCluster::Register(register) = child else {
                return Err(Error::message(format!(
                    "typed peripheral {name} has a cluster; the typed backend renders registers only"
                )));
            };
            let properties = crate::pac_api_svd::merged_properties(device, peripheral, register);
            if properties.size != Some(32) {
                return Err(Error::message(format!(
                    "typed register {name}.{} is not 32 bits wide",
                    register.name
                )));
            }
            let access = properties.access.unwrap_or(Access::ReadWrite);
            let (info, array) = match register {
                MaybeArray::Single(info) => (info, None),
                MaybeArray::Array(info, dimension) => (
                    info,
                    Some((u64::from(dimension.dim_increment), u64::from(dimension.dim))),
                ),
            };
            let kinds = DeclaredKinds {
                pointers: &pointers,
                triggers: &triggers,
                fifo: fifos.contains(info.name.as_str()),
            };
            registers.push(TypedRegister::new(name, info, access, array, &kinds)?);
        }
        if let Some((register, field)) = triggers.iter().find(|(register, field)| {
            !registers.iter().any(|typed| {
                typed.info.name == *register
                    && typed.fields.iter().any(|known| known.info.name == *field)
            })
        }) {
            return Err(Error::message(format!(
                "write-trigger field {name}.{register}.{field} is not a field of the block"
            )));
        }
        if let Some(register) = fifos
            .iter()
            .find(|register| !registers.iter().any(|typed| typed.info.name == **register))
        {
            return Err(Error::message(format!(
                "FIFO port {name}.{register} is not a register of the block"
            )));
        }
        if let Some((operation, register)) = policy
            .typed_register_operations(name)
            .find(|(_, register)| fifos.contains(register))
        {
            return Err(Error::message(format!(
                "PAC API {operation:?} names FIFO port {name}.{register}, which is only popped and pushed"
            )));
        }
        registers.sort_by_key(|register| register.offset);
        let mut words = BTreeSet::new();
        for register in &registers {
            for word in register.words() {
                if word % 4 != 0 || !words.insert(word) {
                    return Err(Error::message(format!(
                        "typed register {name}.{} overlaps another word or is unaligned",
                        register.info.name
                    )));
                }
            }
        }
        if registers.is_empty() {
            return Err(Error::message(format!(
                "typed peripheral {name} has no register"
            )));
        }
        let split = policy
            .register_sides
            .iter()
            .find(|sides| sides.peripheral == name);
        let block = Self {
            peripheral,
            module: crate::pac_api_render::member_binding_name(name),
            owner: type_binding_name(name),
            base,
            registers,
            split,
        };
        block.validate_split()?;
        Ok(block)
    }

    /// Every declared word of the block, ascending.
    fn words(&self) -> Vec<u64> {
        let mut words = self
            .registers
            .iter()
            .flat_map(TypedRegister::words)
            .collect::<Vec<_>>();
        words.sort_unstable();
        words
    }

    fn register(&self, name: &str) -> Result<&TypedRegister<'a>> {
        self.registers
            .iter()
            .find(|register| register.info.name == name)
            .ok_or_else(|| {
                Error::message(format!("{} has no register {name:?}", self.peripheral.name))
            })
    }

    fn validate_split(&self) -> Result<()> {
        let Some(split) = self.split else {
            return Ok(());
        };
        let mut seen = BTreeSet::new();
        for name in split.irq.iter().chain(&split.irq_observes) {
            self.register(name)?;
            if !seen.insert(name) {
                return Err(Error::message(format!(
                    "register split of {} names {name} twice",
                    self.peripheral.name
                )));
            }
        }
        for name in &split.irq_observes {
            let register = self.register(name)?;
            if !register.readable || register.clears_on_read || register.fifo {
                return Err(Error::message(format!(
                    "register split of {} observes {name}, which has no side-effect-free read",
                    self.peripheral.name
                )));
            }
        }
        Ok(())
    }

    fn views(&self) -> Vec<View> {
        let all = self
            .registers
            .iter()
            .map(|register| (register.info.name.clone(), Reach::Owned))
            .collect();
        let mut views = vec![View {
            name: self.owner.clone(),
            inner: "oer_mmio::Block<P, BlockWords>".to_owned(),
            reach: all,
        }];
        if let Some(split) = self.split {
            let part = self
                .registers
                .iter()
                .filter_map(|register| {
                    let name = &register.info.name;
                    if split.irq.contains(name) {
                        Some((name.clone(), Reach::Owned))
                    } else if split.irq_observes.contains(name) {
                        Some((name.clone(), Reach::Observed))
                    } else {
                        None
                    }
                })
                .collect();
            let rest = self
                .registers
                .iter()
                .filter(|register| !split.irq.contains(&register.info.name))
                .map(|register| (register.info.name.clone(), Reach::Owned))
                .collect();
            views.push(View {
                name: format!("{}{}", self.owner, TASK_SIDE),
                inner: format!("oer_mmio::Part<P, BlockWords, {}Words>", TASK_SIDE),
                reach: rest,
            });
            views.push(View {
                name: format!("{}{}", self.owner, IRQ_SIDE),
                inner: format!("oer_mmio::Part<P, BlockWords, {}Words>", IRQ_SIDE),
                reach: part,
            });
        }
        views
    }

    fn render(&self, output: &mut String, policy: &PacApiPack) -> Result<()> {
        let owner = &self.owner;
        // A chip's owner defaults to its device registers; a library's
        // owner names its placement, which each chip supplies.
        let default = self.base.map_or_else(String::new, |base| {
            format!(" = oer_mmio::Fixed<0x{base:x}>")
        });
        let fixed = if self.base.is_some() {
            ""
        } else {
            "<oer_mmio::Fixed<BASE>>"
        };
        let size = self.words().last().map_or(0, |last| last + 4);
        let words = self
            .words()
            .iter()
            .map(|word| format!("0x{word:x}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(output, "/// The `{}` register block.", self.peripheral.name);
        if let Some(description) = &self.peripheral.description {
            output.push_str("///\n");
            push_doc(output, &clean(description), "");
        }
        let (fixed_join, host_join) = match self.split {
            Some(_) => {
                let rest = format!("{owner}{}", TASK_SIDE);
                let part = format!("{owner}{}", IRQ_SIDE);
                (
                    format!(
                        "\n/// The block again, from the two owners of its one split.\n\
                         pub fn join(rest: {rest}{fixed}, part: {part}{fixed}) -> Self {{\n\
                         Self(oer_mmio::Block::join(rest.0, part.0))\n\
                         }}\n"
                    ),
                    format!(
                        "\n/// The block again, from the two owners of one split; owners of\n\
                         /// two blocks are refused and returned.\n\
                         pub fn try_join(\n\
                         rest: {rest}<oer_mmio::Host<'a>>,\n\
                         part: {part}<oer_mmio::Host<'a>>,\n\
                         ) -> Result<Self, oer_mmio::NotOneSplit<{rest}<oer_mmio::Host<'a>>, {part}<oer_mmio::Host<'a>>>> {{\n\
                         oer_mmio::Block::try_join(rest.0, part.0)\n\
                         .map(Self)\n\
                         .map_err(|refused| oer_mmio::NotOneSplit {{\n\
                         first: {rest}(refused.first),\n\
                         second: {part}(refused.second),\n\
                         }})\n\
                         }}\n"
                    ),
                )
            }
            None => (String::new(), String::new()),
        };
        let source = policy
            .interrupt_sources
            .iter()
            .find(|source| source.peripheral == self.peripheral.name)
            .map_or_else(String::new, |source| {
                format!(
                    "/// The interrupt-matrix source of the block's interrupt.\n\
                     ///\n\
                     /// SOURCE: {}.\n\
                     pub const SOURCE: u16 = {};\n\n",
                    source.sources.join(", "),
                    source.source
                )
            });
        let (placement, origin, fixed_impl) = match self.base {
            Some(base) => (
                format!(
                    "/// The block's base address.\n\
                     pub const BASE: usize = 0x{base:x};\n"
                ),
                "[`BASE`]",
                format!(
                    "/// The block's device registers, as the PAC root claims them.\n\
                     pub type Claim = oer_mmio::Block<oer_mmio::Fixed<0x{base:x}>, BlockWords>;\n\n"
                ),
            ),
            None => (String::new(), "the block's base", String::new()),
        };
        let constructor = match self.base {
            Some(_) => format!(
                "impl {owner} {{\n\
                 /// The owner of the block's device registers.\n\
                 pub const fn new(block: Claim) -> Self {{\n\
                 Self(block)\n\
                 }}\n\
                 {fixed_join}\
                 }}\n\n"
            ),
            None => format!(
                "impl<const BASE: usize> {owner}<oer_mmio::Fixed<BASE>> {{\n\
                 /// The owner of the block's device registers at `BASE`, as a chip's\n\
                 /// PAC root claims them.\n\
                 pub const fn new(block: oer_mmio::Block<oer_mmio::Fixed<BASE>, BlockWords>) -> Self {{\n\
                 Self(block)\n\
                 }}\n\
                 {fixed_join}\
                 }}\n\n"
            ),
        };
        let _ = writeln!(
            output,
            "pub mod {module} {{\n\
             {source}\
             {placement}\
             /// The bytes from {origin} to the end of the block's last word: the\n\
             /// host memory a test block needs.\n\
             pub const SIZE: usize = 0x{size:x};\n\n\
             /// The block's declared words, as byte offsets from {origin}: its\n\
             /// whole register authority.\n\
             pub struct BlockWords;\n\n\
             impl oer_mmio::Registers for BlockWords {{\n\
             const WORDS: &'static [usize] = &[{words}];\n\
             }}\n\n\
             {fixed_impl}\
             /// The exclusive owner of the `{name}` registers.\n\
             #[must_use = \"dropping a register owner permanently loses its register authority\"]\n\
             pub struct {owner}<P: oer_mmio::Placement{default}>(oer_mmio::Block<P, BlockWords>);\n\n\
             {constructor}\
             impl<'a> {owner}<oer_mmio::Host<'a>> {{\n\
             /// The block over host memory, for tests of its accessors.\n\
             pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {{\n\
             oer_mmio::Block::host(words).map(Self)\n\
             }}\n\
             {host_join}\
             }}\n",
            module = self.module,
            name = self.peripheral.name,
        );
        let views = self.views();
        let mut asserts = String::new();
        for view in &views {
            let mut body = String::new();
            if view.name == *owner && self.split.is_some() {
                let rest = format!("{owner}{}", TASK_SIDE);
                let part = format!("{owner}{}", IRQ_SIDE);
                let _ = writeln!(
                    body,
                    "/// Divide the block: [`{rest}`] owns every register but those [`{part}`] owns.\n\
                     pub fn split(self) -> ({rest}<P>, {part}<P>) {{\n\
                     let (rest, part) = self.0.split::<{}Words, {}Words>();\n\
                     ({rest}(rest), {part}(part))\n\
                     }}\n",
                    TASK_SIDE, IRQ_SIDE,
                );
            }
            for register in &self.registers {
                if let Some(reach) = view.reach.get(&register.info.name) {
                    register.render_methods(&mut body, *reach);
                }
            }
            self.render_transactions(&mut body, &mut asserts, policy, view)?;
            if view.name != *owner {
                let _ = writeln!(
                    output,
                    "/// One owner of a split `{}` block.\n\
                     #[must_use = \"a split owner must be joined to recover the block\"]\n\
                     pub struct {}<P: oer_mmio::Placement{default}>({});\n",
                    self.peripheral.name, view.name, view.inner
                );
            }
            let _ = writeln!(
                output,
                "impl<P: oer_mmio::Placement> {}<P> {{\n{body}}}\n",
                view.name
            );
        }
        if let Some(split) = self.split {
            for view in views.iter().skip(1) {
                let suffix = view.name.strip_prefix(owner.as_str()).unwrap_or_default();
                let words = |reach: Reach| {
                    self.registers
                        .iter()
                        .filter(|register| view.reach.get(&register.info.name) == Some(&reach))
                        .flat_map(TypedRegister::words)
                        .map(|word| format!("0x{word:x}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let _ = writeln!(
                    output,
                    "/// The words [`{}`] owns and observes.\n\
                     pub struct {suffix}Words;\n\n\
                     impl oer_mmio::Words for {suffix}Words {{\n\
                     const OWNED: &'static [usize] = &[{}];\n\
                     const OBSERVED: &'static [usize] = &[{}];\n\
                     }}\n",
                    view.name,
                    words(Reach::Owned),
                    words(Reach::Observed),
                );
            }
            let _ = writeln!(
                output,
                "// SOURCE: {}. {}\n\
                 const _: () = assert!(\n\
                 oer_mmio::check::sides_cover(\n\
                 <BlockWords as oer_mmio::Registers>::WORDS,\n\
                 <{TASK_SIDE}Words as oer_mmio::Words>::OWNED,\n\
                 <{IRQ_SIDE}Words as oer_mmio::Words>::OWNED,\n\
                 ),\n\
                 \"every word of the block belongs to exactly one side\"\n\
                 );\n",
                split.sources.join(", "),
                clean(&split.description).replace('\n', " ")
            );
        }
        for register in &self.registers {
            register.render_module(output);
        }
        output.push_str(&asserts);
        output.push_str("}\n\n");
        Ok(())
    }

    fn render_transactions(
        &self,
        body: &mut String,
        asserts: &mut String,
        policy: &PacApiPack,
        view: &View,
    ) -> Result<()> {
        let name = self.peripheral.name.as_str();
        let flag_or_enum = policy
            .flag_domains
            .iter()
            .map(|domain| domain.name.as_str())
            .chain(
                policy
                    .enum_domains
                    .iter()
                    .map(|domain| domain.name.as_str()),
            )
            .collect::<BTreeSet<_>>();
        let owned = |register: &str| view.reach.get(register) == Some(&Reach::Owned);
        let first_view = view.name == self.owner;
        for operation in policy
            .full_register_writes
            .iter()
            .filter(|operation| operation.peripheral == name && owned(&operation.register))
        {
            let register = self.register(&operation.register)?;
            let (parameter, value) = match operation.value()? {
                FullRegisterValue::Domain(domain) => (
                    format!("super::{domain}"),
                    domain_value_expression(domain, &flag_or_enum).to_owned(),
                ),
                FullRegisterValue::PointsTo(layout) => (
                    format!(
                        "super::layouts::DmaRef<super::layouts::{}>",
                        crate::layouts::camel(layout)
                    ),
                    "value.bits()".to_owned(),
                ),
                FullRegisterValue::Buffer => (
                    "super::layouts::DmaWindowAddress".to_owned(),
                    "value.get()".to_owned(),
                ),
            };
            if register.array.is_some()
                || register.fields.len() != 1
                || register.declared() != u32::MAX
            {
                return Err(Error::message(format!(
                    "typed full-register-write {:?} needs one register with one 32-bit field",
                    operation.name
                )));
            }
            transaction_doc(
                body,
                "Write the whole register",
                &operation.register,
                &operation.sources,
            );
            let _ = writeln!(
                body,
                "#[inline]\npub fn {}(&mut self, value: {parameter}) {{\nself.0.{};\n}}\n",
                operation.name,
                register.write_call(&value)
            );
        }
        for operation in policy.full_register_reads.iter().filter(|operation| {
            operation.peripheral == name && view.reach.contains_key(&operation.register)
        }) {
            let register = self.register(&operation.register)?;
            if !policy
                .opaque_domains
                .iter()
                .any(|domain| domain.name == operation.domain)
                || register.array.is_some()
            {
                return Err(Error::message(format!(
                    "typed full-register-read {:?} needs one register and an opaque domain",
                    operation.name
                )));
            }
            transaction_doc(
                body,
                "Read the whole register",
                &operation.register,
                &operation.sources,
            );
            let _ = writeln!(
                body,
                "#[inline]\npub fn {}(&self) -> super::{} {{\nsuper::{}::new(self.0.{})\n}}\n",
                operation.name,
                operation.domain,
                operation.domain,
                register.read_call()
            );
        }
        for operation in policy
            .fixed_register_images
            .iter()
            .filter(|operation| operation.peripheral == name && owned(&operation.register))
        {
            let register = self.register(&operation.register)?;
            if operation.whole_word && operation.sources.is_empty() {
                return Err(Error::message(format!(
                    "typed fixed-register-image {:?} writes a whole word without the vendor evidence for it",
                    operation.name
                )));
            }
            transaction_doc(
                body,
                &if operation.whole_word {
                    format!(
                        "Write the vendor's whole-word image `0x{:08x}`, undeclared bits included",
                        operation.value
                    )
                } else {
                    format!("Write the fixed image `0x{:08x}`", operation.value)
                },
                &operation.register,
                &operation.sources,
            );
            let _ = writeln!(
                body,
                "#[inline]\npub fn {}(&mut self, {}) {{\nself.0.{};\n}}\n",
                operation.name,
                register.index_parameter(),
                register.write_call(&format!("0x{:08x}", operation.value))
            );
            if first_view && !operation.whole_word {
                let _ = writeln!(
                    asserts,
                    "const _: () = assert!(oer_mmio::check::within(0x{:08x}, {}::FIELDS), \"{} writes declared fields only\");",
                    operation.value, register.method, operation.name
                );
            }
        }
        for operation in policy.fixed_register_sequences.iter().filter(|operation| {
            operation.peripheral == name && operation.steps.iter().all(|step| owned(&step.register))
        }) {
            let mut writes = String::new();
            for step in &operation.steps {
                let register = self.register(&step.register)?;
                let index = match (register.array, step.index) {
                    (None, None) => String::new(),
                    (Some((_, count)), Some(index)) if (index as u64) < count => {
                        format!("oer_mmio::Index::constant::<{index}>()")
                    }
                    _ => {
                        return Err(Error::message(format!(
                            "typed fixed-register-sequence {:?} indexes {} outside its array or not at all",
                            operation.name, step.register
                        )));
                    }
                };
                if !register.writable || register.w1c {
                    return Err(Error::message(format!(
                        "typed fixed-register-sequence {:?} writes {}, which takes no fixed image",
                        operation.name, step.register
                    )));
                }
                let call = match register.array {
                    None => register.write_call(&format!("0x{:08x}", step.value)),
                    Some((stride, count)) => format!(
                        "write_indexed::<0x{:x}, 0x{stride:x}, {count}>({index}, 0x{:08x})",
                        register.offset, step.value
                    ),
                };
                let _ = writeln!(writes, "self.0.{call};");
                if first_view {
                    let _ = writeln!(
                        asserts,
                        "const _: () = assert!(oer_mmio::check::within(0x{:08x}, {}::FIELDS), \"{} writes declared fields only\");",
                        step.value, register.method, operation.name
                    );
                }
            }
            let registers = operation
                .steps
                .iter()
                .map(|step| step.register.as_str())
                .collect::<Vec<_>>()
                .join("`, `");
            transaction_doc(
                body,
                "Write the reviewed fixed images in order",
                &registers,
                &operation.sources,
            );
            let _ = writeln!(
                body,
                "#[inline]\npub fn {}(&mut self) {{\n{writes}}}\n",
                operation.name
            );
        }
        for operation in policy
            .masked_register_modifies
            .iter()
            .filter(|operation| operation.peripheral == name && owned(&operation.register))
        {
            let register = self.register(&operation.register)?;
            let value = domain_value_expression(&operation.domain, &flag_or_enum);
            let mut image = format!(
                "(bits & 0x{:08x}) | ({value} & 0x{:08x})",
                operation.preserve_mask, operation.input_mask
            );
            if operation.set_mask != 0 {
                let _ = write!(image, " | 0x{:08x}", operation.set_mask);
            }
            transaction_doc(
                body,
                &format!(
                    "Keep `0x{:08x}`, take the value into `0x{:08x}` and set `0x{:08x}`",
                    operation.preserve_mask, operation.input_mask, operation.set_mask
                ),
                &operation.register,
                &operation.sources,
            );
            let _ = writeln!(
                body,
                "#[inline]\npub fn {}(&mut self, {}value: super::{}) {{\nlet bits = self.0.{};\nself.0.{};\n}}\n",
                operation.name,
                register.index_parameter(),
                operation.domain,
                register.modify_read_call(),
                register.write_call(&image)
            );
            if first_view {
                let _ = writeln!(
                    asserts,
                    "const _: () = assert!(\n\
                     oer_mmio::check::partition(0x{p:08x}, 0x{i:08x}, 0x{s:08x}, {m}::FIELDS),\n\
                     \"{n} partitions the word and changes declared fields only\"\n\
                     );",
                    p = operation.preserve_mask,
                    i = operation.input_mask,
                    s = operation.set_mask,
                    m = register.method,
                    n = operation.name
                );
            }
        }
        for (clear, operation) in policy
            .field_or_modifies
            .iter()
            .map(|operation| (false, operation))
            .chain(
                policy
                    .field_clear_modifies
                    .iter()
                    .map(|operation| (true, operation)),
            )
            .filter(|(_, operation)| operation.peripheral == name && owned(&operation.register))
        {
            let register = self.register(&operation.register)?;
            let (parameter, input, image) = projected_image(register, operation, &flag_or_enum)?;
            let update = if clear {
                format!("bits & !({image})")
            } else {
                format!("bits | {image}")
            };
            let verb = if clear { "Clear" } else { "Set" };
            transaction_doc(
                body,
                &format!("{verb} one reviewed image in its fields, keeping every other bit"),
                &operation.register,
                &operation.sources,
            );
            let _ = writeln!(
                body,
                "#[inline]\npub fn {}(&mut self, {}{parameter}) {{\n{input}let bits = self.0.{};\nself.0.{};\n}}\n",
                operation.name,
                register.index_parameter(),
                register.modify_read_call(),
                register.write_call(&update)
            );
            if first_view && operation.value.is_some() {
                let _ = writeln!(
                    asserts,
                    "const _: () = assert!(oer_mmio::check::within({image}, {}::FIELDS), \"{} changes declared fields only\");",
                    register.method, operation.name
                );
            }
        }
        for operation in policy
            .field_replace_modifies
            .iter()
            .filter(|operation| operation.peripheral == name && owned(&operation.register))
        {
            let register = self.register(&operation.register)?;
            let projection = FieldOrModify {
                name: operation.name.clone(),
                peripheral: operation.peripheral.clone(),
                register: operation.register.clone(),
                fields: operation.fields.clone(),
                domain: operation.domain.clone(),
                value: operation.value,
                exposure: operation.exposure,
                sources: operation.sources.clone(),
            };
            let (parameter, input, image) = projected_image(register, &projection, &flag_or_enum)?;
            let mask = operation
                .fields
                .iter()
                .map(|projection| field(register, &projection.field).map(TypedField::mask))
                .try_fold(0, |mask, field| field.map(|field| mask | field))?;
            let transform = operation
                .input_transform
                .map_or_else(String::new, |transform| {
                    let arithmetic = if transform.wrapping_add != 0 {
                        format!("input.wrapping_add(0x{:08x})", transform.wrapping_add)
                    } else {
                        format!("input.wrapping_sub(0x{:08x})", transform.wrapping_subtract)
                    };
                    let bounded = transform
                        .clamp_maximum
                        .map_or(arithmetic.clone(), |maximum| {
                            format!("{arithmetic}.min(0x{maximum:08x})")
                        });
                    format!("let input = {bounded} & 0x{:08x};\n", transform.retain_mask)
                });
            transaction_doc(
                body,
                "Replace the listed fields from one reviewed image, keeping every other bit",
                &operation.register,
                &operation.sources,
            );
            let _ = writeln!(
                body,
                "#[inline]\npub fn {}(&mut self, {}{parameter}) {{\n{input}{transform}let bits = self.0.{};\nself.0.{};\n}}\n",
                operation.name,
                register.index_parameter(),
                register.modify_read_call(),
                register.write_call(&if image == "0x00000000" {
                    format!("bits & !0x{mask:08x}")
                } else {
                    format!("(bits & !0x{mask:08x}) | {image}")
                })
            );
            if first_view && operation.value.is_some() {
                let _ = writeln!(
                    asserts,
                    "const _: () = assert!(oer_mmio::check::within({image}, 0x{mask:08x}) && oer_mmio::check::within(0x{mask:08x}, {}::FIELDS), \"{} replaces declared fields only\");",
                    register.method, operation.name
                );
            }
        }
        for operation in policy
            .zero_based_field_writes
            .iter()
            .filter(|operation| operation.peripheral == name && owned(&operation.register))
        {
            let register = self.register(&operation.register)?;
            let mut parameters = String::new();
            let mut image = Vec::new();
            for field_name in &operation.fields {
                let selected = field(register, field_name)?;
                if !selected.has_setter() {
                    return Err(Error::message(format!(
                        "typed zero-based-field-write {:?} writes {field_name}, which has no writer",
                        operation.name
                    )));
                }
                let argument = if operation.fields.len() == 1 {
                    "value".to_owned()
                } else {
                    format!("{}_value", selected.method.trim_start_matches("r#"))
                };
                let write_type = selected.write_type();
                let _ = write!(
                    parameters,
                    "{argument}: {}, ",
                    qualified(&write_type, register)
                );
                let bits = write_type.bits().replace("value", &argument);
                image.push(if selected.offset == 0 {
                    bits
                } else {
                    format!("({bits} << {})", selected.offset)
                });
            }
            transaction_doc(
                body,
                "Write the listed fields and zero to every other bit",
                &operation.register,
                &operation.sources,
            );
            let lint = if operation.fields.len() + usize::from(register.array.is_some()) > 6 {
                "#[allow(clippy::too_many_arguments, reason = \"one typed argument per field\")]\n"
            } else {
                ""
            };
            let _ = writeln!(
                body,
                "{lint}#[inline]\npub fn {}(&mut self, {}{parameters}) {{\nself.0.{};\n}}\n",
                operation.name,
                register.index_parameter(),
                register.write_call(&image.join(" | "))
            );
        }
        Ok(())
    }
}

/// The logical input of a field-local transaction, projected into its
/// fields: the parameter it takes, the statement binding its `u32` input
/// and the register image expression over that input.
fn projected_image(
    register: &TypedRegister<'_>,
    operation: &FieldOrModify,
    flag_or_enum: &BTreeSet<&str>,
) -> Result<(String, String, String)> {
    match (&operation.domain, operation.value) {
        (None, Some(value)) => {
            let mut image = 0_u32;
            for projection in &operation.fields {
                let selected = field(register, &projection.field)?;
                let mask = u32::MAX >> (32 - selected.width);
                let field_value = value
                    .checked_shr(u32::from(projection.source_bit_offset))
                    .unwrap_or(0)
                    & mask;
                image |= field_value << selected.offset;
            }
            Ok((String::new(), String::new(), format!("0x{image:08x}")))
        }
        (Some(domain), None) => {
            let mut terms = Vec::new();
            for projection in &operation.fields {
                let selected = field(register, &projection.field)?;
                let mask = u32::MAX >> (32 - selected.width);
                let source = if projection.source_bit_offset == 0 {
                    "input".to_owned()
                } else {
                    format!("(input >> {})", projection.source_bit_offset)
                };
                let projected = format!("({source} & 0x{mask:08x})");
                terms.push(if selected.offset == 0 {
                    projected
                } else {
                    format!("({projected} << {})", selected.offset)
                });
            }
            Ok((
                format!("value: super::{domain}"),
                format!(
                    "let input = {};\n",
                    domain_value_expression(domain, flag_or_enum)
                ),
                format!("({})", terms.join(" | ")),
            ))
        }
        _ => Err(Error::message(format!(
            "typed field transaction {:?} takes exactly one of domain and value",
            operation.name
        ))),
    }
}

/// A register library's block placed at a chip's address: the library's
/// module with the chip's base, claim and owner at that base. Returns the
/// module and owner names, as a chip block's.
fn render_placed(
    output: &mut String,
    peripheral: &Peripheral,
    library: &SharedPeripheral,
) -> Result<(String, String)> {
    let name = peripheral.name.as_str();
    if matches!(peripheral, MaybeArray::Array(..)) || peripheral.derived_from.is_some() {
        return Err(Error::message(format!(
            "typed peripheral {name} must be one plain SVD peripheral"
        )));
    }
    let module = crate::pac_api_render::member_binding_name(name);
    let owner = type_binding_name(name);
    let base = peripheral.base_address;
    let path = format!("{}::{module}", library.crate_name);
    let _ = writeln!(
        output,
        "/// The `{name}` register block at this chip's address: its layout,\n\
         /// accessors and transactions are the `{}` register library's\n\
         /// [`{path}`].\n\
         pub mod {module} {{\n\
         pub use {path}::*;\n\n\
         /// The block's base address.\n\
         pub const BASE: usize = 0x{base:x};\n\n\
         /// The block's device registers, as the PAC root claims them.\n\
         pub type Claim = oer_mmio::Block<oer_mmio::Fixed<0x{base:x}>, BlockWords>;\n\n\
         /// The exclusive owner of the `{name}` registers, at this chip's address\n\
         /// unless placed in host memory.\n\
         pub type {owner}<P = oer_mmio::Fixed<0x{base:x}>> = {path}::{owner}<P>;\n\
         }}\n",
        library.library
    );
    Ok((module, owner))
}

/// The claim type of `modules`' blocks as nested tuples of at most eight
/// (the arity `oer_mmio::Claim` takes), and the pattern that binds each
/// block to its module's name.
fn claim_tuple(modules: &[String]) -> (String, String) {
    if let [module] = modules {
        return (format!("super::{module}::Claim"), module.clone());
    }
    let groups = if modules.len() <= 8 {
        modules.iter().map(std::slice::from_ref).collect::<Vec<_>>()
    } else {
        let size = modules.len().div_ceil(8);
        modules.chunks(size).collect()
    };
    let (types, patterns): (Vec<_>, Vec<_>) = groups.into_iter().map(claim_tuple).unzip();
    (
        format!("({},)", types.join(", ")),
        format!("({},)", patterns.join(", ")),
    )
}

fn field<'r, 'a>(register: &'r TypedRegister<'a>, name: &str) -> Result<&'r TypedField<'a>> {
    register
        .fields
        .iter()
        .find(|field| field.info.name == name)
        .ok_or_else(|| {
            Error::message(format!(
                "register {} has no field {name:?}",
                register.info.name
            ))
        })
}

fn qualified(field_type: &FieldType, register: &TypedRegister<'_>) -> String {
    match field_type {
        FieldType::Enum(name) => format!("{}::{name}", register.method),
        other => other.rust(),
    }
}

fn transaction_doc(body: &mut String, summary: &str, register: &str, sources: &[String]) {
    let _ = writeln!(
        body,
        "/// {summary} (`{register}`).\n///\n/// SOURCE: {}.",
        sources.join(", ")
    );
}

impl<'a> TypedRegister<'a> {
    fn new(
        peripheral: &str,
        info: &'a RegisterInfo,
        access: Access,
        array: Option<(u64, u64)>,
        kinds: &DeclaredKinds<'_>,
    ) -> Result<Self> {
        let readable = access.can_read();
        let writable = access.can_write();
        let clears_on_read = info.read_action.is_some();
        let fifo = kinds.fifo;
        let mut fields = Vec::new();
        for field in info.fields.as_deref().unwrap_or_default() {
            let MaybeArray::Single(field) = field else {
                return Err(Error::message(format!(
                    "typed register {peripheral}.{} has a field array",
                    info.name
                )));
            };
            fields.push(TypedField::new(peripheral, info, field, access, kinds)?);
        }
        let w1c_fields = fields
            .iter()
            .filter(|field| {
                field.info.modified_write_values == Some(ModifiedWriteValues::OneToClear)
                    || (field.info.modified_write_values.is_none()
                        && info.modified_write_values == Some(ModifiedWriteValues::OneToClear))
            })
            .count();
        // A write-only register is always written from zero, so writing a
        // one acknowledges exactly the bits the writer sets: it needs no
        // sample.
        let w1c = w1c_fields > 0 && readable;
        if w1c && (w1c_fields != fields.len() || !writable) {
            return Err(Error::message(format!(
                "typed register {peripheral}.{} mixes write-one-to-clear and other fields",
                info.name
            )));
        }
        if let Some(modified) = info.modified_write_values
            && !matches!(
                modified,
                ModifiedWriteValues::OneToClear | ModifiedWriteValues::Modify
            )
        {
            return Err(Error::message(format!(
                "typed register {peripheral}.{} has unsupported modified write values",
                info.name
            )));
        }
        if clears_on_read && writable && !fifo {
            return Err(Error::message(format!(
                "typed register {peripheral}.{} is written and changes when read",
                info.name
            )));
        }
        if readable
            && writable
            && !fifo
            && fields
                .iter()
                .any(|field| field.writable && !field.readable && !field.trigger)
        {
            return Err(Error::message(format!(
                "typed register {peripheral}.{} has a write-only field a read-modify-write would replay",
                info.name
            )));
        }
        if let Some(trigger) = fields.iter().find(|field| field.trigger)
            && (trigger.width != 1 || !trigger.writable || w1c || fifo)
        {
            return Err(Error::message(format!(
                "typed write-trigger field {peripheral}.{}.{} must be one writable bit of an ordinary register",
                info.name, trigger.info.name
            )));
        }
        if fifo
            && (array.is_some()
                || w1c
                || fields.len() != 1
                || fields.iter().any(|field| field.pointer))
        {
            return Err(Error::message(format!(
                "typed FIFO port {peripheral}.{} must be one register with one field",
                info.name
            )));
        }
        let mut declared = 0_u32;
        for field in &fields {
            if declared & field.mask() != 0 {
                return Err(Error::message(format!(
                    "typed register {peripheral}.{} has overlapping fields",
                    info.name
                )));
            }
            declared |= field.mask();
        }
        Ok(Self {
            info,
            method: crate::pac_api_render::member_binding_name(&info.name),
            offset: u64::from(info.address_offset),
            array,
            readable,
            writable,
            clears_on_read,
            w1c,
            fifo,
            fields,
        })
    }

    fn render_methods(&self, body: &mut String, reach: Reach) {
        let module = &self.method;
        let index = self.index_parameter();
        let read = self.read_call();
        let name = &self.info.name;
        if self.fifo {
            self.render_fifo_methods(body, reach);
            return;
        }
        if self.readable {
            let receiver = if self.clears_on_read {
                "&mut self"
            } else {
                "&self"
            };
            let _ = writeln!(
                body,
                "/// Read `{name}`.\n#[inline]\npub fn read_{module}({receiver}, {index}) -> {module}::R {{\n{module}::R(self.0.{read})\n}}\n"
            );
        }
        if reach != Reach::Owned {
            return;
        }
        if self.w1c {
            let _ = writeln!(
                body,
                "/// Sample `{name}` once, keeping its declared fields, to acknowledge.\n\
                 #[inline]\npub fn sample_{module}(&self, {index}) -> {module}::Snapshot {{\n\
                 {module}::Snapshot(self.0.{read} & {module}::FIELDS)\n}}\n\n\
                 /// Acknowledge exactly the sampled set: write one to each bit the\n\
                 /// snapshot holds and zero to every other bit.\n\
                 #[inline]\npub fn acknowledge_{module}(&mut self, {index}snapshot: {module}::Snapshot) {{\n\
                 self.0.{};\n}}\n",
                self.write_call("snapshot.0")
            );
            return;
        }
        self.render_triggers(body);
        if !self.has_setters() {
            return;
        }
        if self.readable {
            let _ = writeln!(
                body,
                "/// Read `{name}`, change fields and write it back; every other bit keeps\n\
                 /// the value read{}.\n\
                 #[inline]\npub fn modify_{module}(&mut self, {index}change: impl FnOnce(&mut {module}::W)) {{\n\
                 let mut value = {module}::W(self.0.{});\n\
                 change(&mut value);\n\
                 self.0.{};\n}}\n",
                if self.triggers() == 0 {
                    ""
                } else {
                    " and every write-trigger field is zero"
                },
                self.modify_read_call(),
                self.write_call("value.0")
            );
        } else {
            let _ = writeln!(
                body,
                "/// Write `{name}` from zero: the fields set, zero in every other bit.\n\
                 #[inline]\npub fn write_{module}(&mut self, {index}change: impl FnOnce(&mut {module}::W)) {{\n\
                 let mut value = {module}::W(0);\n\
                 change(&mut value);\n\
                 self.0.{};\n}}\n",
                self.write_call("value.0")
            );
        }
    }

    /// The explicit trigger of each write-trigger field: the one write that
    /// sets it, keeping every other field read and zero in every other
    /// trigger field.
    fn render_triggers(&self, body: &mut String) {
        let module = &self.method;
        let index = self.index_parameter();
        let name = &self.info.name;
        for field in self.fields.iter().filter(|field| field.trigger) {
            let image = if self.readable {
                format!(
                    "(self.0.{}) | 0x{:08x}",
                    self.modify_read_call(),
                    field.mask()
                )
            } else {
                format!("0x{:08x}", field.mask())
            };
            let _ = writeln!(
                body,
                "/// Write one to the write-trigger field `{}` of `{name}`, starting its\n\
                 /// hardware action once; {}.\n\
                 #[inline]\npub fn trigger_{module}_{}(&mut self, {index}) {{\n\
                 self.0.{};\n}}\n",
                field.info.name,
                if self.readable {
                    "every other field keeps the value read, every other trigger is zero"
                } else {
                    "every other bit is zero"
                },
                field.method.trim_start_matches("r#"),
                self.write_call(&image)
            );
        }
    }

    /// A FIFO port's `pop` (a read through `&mut`, which takes an element
    /// out) and `push` (a write from zero, which puts one in).
    fn render_fifo_methods(&self, body: &mut String, reach: Reach) {
        let module = &self.method;
        let name = &self.info.name;
        let Some(field) = self.fields.first() else {
            return;
        };
        if reach != Reach::Owned {
            return;
        }
        if field.readable {
            let _ = writeln!(
                body,
                "/// Take the next element out of the FIFO port `{name}`.\n\
                 #[inline]\npub fn pop_{module}(&mut self) -> {} {{\n\
                 let value = self.0.{};\n{}\n}}\n",
                field.read_type().rust(),
                self.read_call(),
                field.read_expression("value")
            );
        }
        if field.writable {
            let write_type = field.write_type();
            let bits = write_type.bits();
            let image = if field.offset == 0 {
                bits.to_owned()
            } else {
                format!("({bits} << {})", field.offset)
            };
            let _ = writeln!(
                body,
                "/// Put one element into the FIFO port `{name}`; every other bit is zero.\n\
                 #[inline]\npub fn push_{module}(&mut self, value: {}) {{\n\
                 self.0.{};\n}}\n",
                qualified(&write_type, self),
                self.write_call(&image)
            );
        }
    }

    fn render_module(&self, output: &mut String) {
        let module = &self.method;
        let _ = writeln!(output, "/// The `{}` register.", self.info.name);
        if let Some(description) = &self.info.description {
            output.push_str("///\n");
            push_doc(output, &clean(description), "");
        }
        let _ = writeln!(
            output,
            "pub mod {module} {{\n\
             /// The bits of the register's declared fields.\n\
             pub const FIELDS: u32 = 0x{:08x};\n\
             /// The register's byte offset in its block.\n\
             pub const OFFSET: usize = 0x{:x};\n",
            self.declared(),
            self.offset
        );
        if self.triggers() != 0 {
            let _ = writeln!(
                output,
                "/// The bits of the write-trigger fields: zero in every\n\
                 /// read-modify-write, one only in a field's explicit trigger.\n\
                 pub const TRIGGERS: u32 = 0x{:08x};\n",
                self.triggers()
            );
        }
        if let Some((stride, count)) = self.array {
            let _ = writeln!(
                output,
                "/// The byte distance between two elements of the array.\n\
                 pub const STRIDE: usize = 0x{stride:x};\n\
                 /// The array's element count.\n\
                 pub const COUNT: usize = {count};\n"
            );
        }
        for field in &self.fields {
            if let Some(enumeration) = &field.enumeration {
                render_enumeration(output, field, enumeration);
            }
        }
        if self.readable && !self.fifo {
            output.push_str(
                "/// One observation of the register.\n\
                 #[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
                 pub struct R(pub(super) u32);\n\nimpl R {\n",
            );
            for field in self.fields.iter().filter(|field| field.has_getter()) {
                render_getter(output, field, "self");
            }
            output.push_str("}\n\n");
        }
        if self.w1c {
            output.push_str(
                "/// One sample of the register's asserted fields, to acknowledge once.\n\
                 #[must_use = \"a sampled write-one-to-clear set must be acknowledged\"]\n\
                 #[derive(Debug, Eq, PartialEq)]\n\
                 pub struct Snapshot(pub(super) u32);\n\nimpl Snapshot {\n\
                 /// Whether no declared field was asserted.\n\
                 pub const fn is_clear(&self) -> bool {\nself.0 == 0\n}\n\n",
            );
            for field in &self.fields {
                render_getter(output, field, "&self");
                let _ = writeln!(
                    output,
                    "/// The snapshot without `{0}` unless `keep`: it is then not acknowledged.\n\
                     pub const fn retain_{1}(self, keep: bool) -> Self {{\n\
                     if keep {{ self }} else {{ Self(self.0 & !0x{2:08x}) }}\n}}\n",
                    field.info.name,
                    field.method.trim_start_matches("r#"),
                    field.mask()
                );
            }
            output.push_str("}\n\n");
        }
        if self.has_setters() {
            output.push_str(if self.readable {
                "/// The register's value while it is changed.\n\
                 pub struct W(pub(super) u32);\n\nimpl W {\n"
            } else {
                "/// The value written to the register.\n\
                 pub struct W(pub(super) u32);\n\nimpl W {\n"
            });
            if self.readable {
                for field in self.fields.iter().filter(|field| field.has_getter()) {
                    render_getter(output, field, "&self");
                }
            }
            for field in self.fields.iter().filter(|field| field.has_setter()) {
                let write_type = field.write_type();
                let bits = write_type.bits();
                let value = if field.offset == 0 {
                    bits.to_owned()
                } else {
                    format!("({bits} << {})", field.offset)
                };
                let assignment = if field.width == 32 {
                    format!("self.0 = {value};")
                } else {
                    format!("self.0 = (self.0 & !0x{:08x}) | {value};", field.mask())
                };
                let _ = writeln!(
                    output,
                    "/// Set `{}`.\n#[inline]\npub fn set_{}(&mut self, value: {}) -> &mut Self {{\n{assignment}\nself\n}}\n",
                    field.info.name,
                    field.method.trim_start_matches("r#"),
                    write_type.rust(),
                );
            }
            output.push_str("}\n\n");
        }
        output.push_str("}\n\n");
    }
}

fn render_getter(output: &mut String, field: &TypedField<'_>, receiver: &str) {
    if let Some(description) = &field.info.description {
        push_doc(output, &clean(description), "");
    } else {
        let _ = writeln!(output, "/// The `{}` field.", field.info.name);
    }
    let _ = writeln!(
        output,
        "#[inline]\npub const fn {}({receiver}) -> {} {{\n{}\n}}\n",
        field.method,
        field.read_type().rust(),
        field.read_expression("self.0")
    );
    if let Some(enumeration) = &field.enumeration {
        let shifted = if field.offset == 0 {
            "self.0".to_owned()
        } else {
            format!("(self.0 >> {})", field.offset)
        };
        let (result, body) = if enumeration.complete {
            (
                enumeration.name.clone(),
                format!(
                    "{}::from_field({})",
                    enumeration.name,
                    shifted.trim_start_matches('(').trim_end_matches(')')
                ),
            )
        } else {
            (
                format!("Option<{}>", enumeration.name),
                if field.width == 32 {
                    format!("{}::from_bits(self.0)", enumeration.name)
                } else {
                    format!(
                        "{}::from_bits({shifted} & 0x{:x})",
                        enumeration.name,
                        u32::MAX >> (32 - field.width)
                    )
                },
            )
        };
        let _ = writeln!(
            output,
            "/// The `{}` field as its declared value{}.\n#[inline]\npub const fn {}_variant({receiver}) -> {result} {{\n{body}\n}}\n",
            field.info.name,
            if enumeration.complete {
                ""
            } else {
                ", if it is one"
            },
            field.method.trim_start_matches("r#"),
        );
    }
}

fn render_enumeration(output: &mut String, field: &TypedField<'_>, enumeration: &Enumeration) {
    let name = &enumeration.name;
    let _ = writeln!(
        output,
        "/// The declared values of the `{}` field.\n\
         #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\n\
         #[allow(clippy::enum_variant_names, reason = \"the variants are the model's value names\")]\n\
         pub enum {name} {{",
        field.info.name
    );
    for (variant, value, description) in &enumeration.variants {
        if let Some(description) = description {
            push_doc(output, &clean(description), "");
        }
        let _ = writeln!(output, "{variant} = 0x{value:x},");
    }
    let _ = writeln!(output, "}}\n\nimpl {name} {{");
    let _ = writeln!(
        output,
        "/// The value's field bits.\n#[inline]\npub const fn bits(self) -> u32 {{\nself as u32\n}}\n"
    );
    let mask = u32::MAX >> (32 - field.width);
    if enumeration.complete
        && let Some((last, rest)) = enumeration.variants.split_last()
    {
        let arms = rest
            .iter()
            .map(|(variant, value, _)| format!("0x{value:x} => Self::{variant},"))
            .collect::<String>();
        let _ = writeln!(
            output,
            "/// The value of the field in the low bits of `bits`; the field has\n\
             /// exactly the declared values.\n\
             #[inline]\npub const fn from_field(bits: u32) -> Self {{\n\
             match bits & 0x{mask:x} {{\n{arms}_ => Self::{},\n}}\n}}\n",
            last.0
        );
    } else {
        let arms = enumeration
            .variants
            .iter()
            .map(|(variant, value, _)| format!("0x{value:x} => Some(Self::{variant}),"))
            .collect::<String>();
        let _ = writeln!(
            output,
            "/// The declared value `bits`, if it is one.\n\
             #[inline]\npub const fn from_bits(bits: u32) -> Option<Self> {{\n\
             match bits {{\n{arms}_ => None,\n}}\n}}\n"
        );
    }
    output.push_str("}\n\n");
}

impl<'a> TypedField<'a> {
    fn new(
        peripheral: &str,
        register: &RegisterInfo,
        info: &'a FieldInfo,
        register_access: Access,
        kinds: &DeclaredKinds<'_>,
    ) -> Result<Self> {
        let access = info.access.unwrap_or(register_access);
        let width = info.bit_width();
        let offset = info.bit_offset();
        if width == 0 || width > 32 || offset + width > 32 {
            return Err(Error::message(format!(
                "typed field {peripheral}.{}.{} does not fit its register",
                register.name, info.name
            )));
        }
        if info.read_action.is_some() {
            return Err(Error::message(format!(
                "typed field {peripheral}.{}.{} changes when read; the typed backend renders that per register",
                register.name, info.name
            )));
        }
        if let Some(WriteConstraint::Range(range)) = info.write_constraint
            && (range.min != 0 || range.max != u64::from(u32::MAX >> (32 - width)))
        {
            return Err(Error::message(format!(
                "typed field {peripheral}.{}.{} has a range narrower than its width; the typed backend renders no bounded field yet",
                register.name, info.name
            )));
        }
        let enumeration = enumeration(peripheral, register, info, width)?;
        Ok(Self {
            info,
            method: field_method(&info.name),
            offset,
            width,
            readable: access.can_read(),
            writable: access.can_write(),
            pointer: kinds
                .pointers
                .contains(&(register.name.as_str(), info.name.as_str())),
            trigger: kinds
                .triggers
                .contains(&(register.name.as_str(), info.name.as_str())),
            enumeration,
        })
    }
}

fn enumeration(
    peripheral: &str,
    register: &RegisterInfo,
    info: &FieldInfo,
    width: u32,
) -> Result<Option<Enumeration>> {
    let sets = &info.enumerated_values;
    let Some(set) = sets.first() else {
        return Ok(None);
    };
    if sets.len() > 1 {
        return Err(Error::message(format!(
            "typed field {peripheral}.{}.{} has separate read and write enumerations",
            register.name, info.name
        )));
    }
    if set.derived_from.is_some() {
        return Err(Error::message(format!(
            "typed field {peripheral}.{}.{} derives its enumeration",
            register.name, info.name
        )));
    }
    let mut seen = BTreeSet::new();
    let mut variants = Vec::new();
    for value in &set.values {
        let Some(bits) = value.value else {
            return Err(Error::message(format!(
                "typed field {peripheral}.{}.{} has a default enumerated value",
                register.name, info.name
            )));
        };
        if bits > u64::from(u32::MAX >> (32 - width)) || !seen.insert(bits) {
            return Err(Error::message(format!(
                "typed field {peripheral}.{}.{} value {} is outside the field or repeated",
                register.name, info.name, value.name
            )));
        }
        let mut variant = type_binding_name(&value.name);
        if variant.starts_with(|character: char| character.is_ascii_digit()) {
            variant.insert(0, 'V');
        }
        variants.push((variant, bits, value.description.clone()));
    }
    let complete = width < 64 && variants.len() as u64 == 1_u64 << width;
    Ok(Some(Enumeration {
        name: type_binding_name(&info.name),
        variants,
        complete,
    }))
}

fn field_method(name: &str) -> String {
    let method = crate::pac_api_render::member_binding_name(name);
    const KEYWORDS: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "static", "struct", "super", "trait", "true", "type",
        "unsafe", "use", "where", "while", "gen",
    ];
    if KEYWORDS.contains(&method.as_str()) {
        format!("r#{method}")
    } else {
        method
    }
}

/// An SVD description as doc lines: each line trimmed, blank ones dropped.
fn clean(description: &str) -> String {
    description
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
