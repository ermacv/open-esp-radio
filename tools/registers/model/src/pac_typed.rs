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
//! command register is written only by its starts, each refusing while a
//! self-clearing bit of the register still reads one; a policy-declared
//! FIFO port is only popped and pushed,
//! and a command port's data words are read and stored whole. A policy pointer field is set only
//! from the image of a typed DMA reference; a status-to-clear snapshot
//! acknowledges exactly what it sampled; a read capability gives up a
//! block's writes for a reader; a division gives each owner its words,
//! array elements and bits of shared write-trigger words; a field whose
//! only permitted value is zero is zero in every write. A policy-declared
//! shared block has only `&self` methods (reads, trigger strobes and
//! whole-word stores) and is `Sync` at its fixed address.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use svd_rs::{
    Access, Device, FieldInfo, MaybeArray, ModifiedWriteValues, Peripheral, RegisterCluster,
    RegisterInfo, WriteConstraint,
};

use crate::{
    Error, FieldOrModify, FullRegisterValue, InterruptSnapshot, PacApiPack, PointerField,
    ReadCapability, RegisterDivision, RegisterPointerEncoding, RegisterSides, Result,
    SharedPeripheral,
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
        pack.command_registers.clear();
        pack.data_ports.clear();
        pack.read_capabilities.clear();
        pack.register_divisions.clear();
        pack.shared_blocks.clear();
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
        let partitions = pack
            .ownership_partitions
            .iter()
            .map(|partition| partition.name.clone())
            .collect::<BTreeSet<_>>();
        pack.partition_image_reads
            .retain(|image| partitions.contains(&image.partition));
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
            .chain(
                self.command_registers
                    .iter()
                    .map(|command| command.peripheral.as_str()),
            )
            .chain(self.data_ports.iter().map(|port| port.peripheral.as_str()))
            .chain(
                self.interrupt_snapshots
                    .iter()
                    .map(|snapshot| snapshot.peripheral.as_str()),
            )
            .chain(
                self.pointer_fields
                    .iter()
                    .map(|pointer| pointer.peripheral.as_str()),
            )
            .chain(
                self.read_capabilities
                    .iter()
                    .map(|capability| capability.peripheral.as_str()),
            )
            .chain(
                self.register_divisions
                    .iter()
                    .map(|division| division.peripheral.as_str()),
            )
            .find(|peripheral| libraries.contains_key(*peripheral))
        {
            return Err(Error::message(format!(
                "PAC API register sides, write-trigger field, FIFO port, snapshot, pointer field, read capability or division of {peripheral} change a block whose layout a register library renders; declare it in the library's API"
            )));
        }
        let device = svd_parser::parse(svd).map_err(|error| Error::message(error.to_string()))?;
        let mut output = String::from(
            "// Typed register blocks over `oer-mmio`, rendered by `cargo registers`\n\
             // from the published SVD and the API policy; no `unsafe`, no raw write.\n\n",
        );
        output.push_str(&self.render_typed_device(&device, &typed, libraries)?);
        Ok(output)
    }

    /// The typed blocks of `typed` peripherals of `device` and their
    /// partitions, after the operations were validated.
    pub(crate) fn render_typed_device(
        &self,
        device: &Device,
        typed: &BTreeSet<&str>,
        libraries: &BTreeMap<String, SharedPeripheral>,
    ) -> Result<String> {
        let mut output = String::new();
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
        // The blocks of the partitions an image reads: each reads its words.
        let imaged = self
            .partition_image_reads
            .iter()
            .filter_map(|image| {
                self.ownership_partitions
                    .iter()
                    .find(|partition| partition.name == image.partition)
            })
            .flat_map(|partition| &partition.peripherals)
            .filter(|peripheral| typed.contains(peripheral.as_str()))
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let mut blocks = BTreeMap::new();
        for &name in typed {
            let peripheral = device
                .peripherals
                .iter()
                .find(|peripheral| peripheral.name == name)
                .ok_or_else(|| {
                    Error::message(format!("typed peripheral {name:?} is not in the SVD"))
                })?;
            if let Some(library) = libraries.get(name) {
                if imaged.contains(name) {
                    return Err(Error::message(format!(
                        "a partition image reads {name}, whose layout a register library renders"
                    )));
                }
                let (module, owner) = render_placed(&mut output, peripheral, library)?;
                blocks.insert(name, (module, owner));
                continue;
            }
            let mut block =
                TypedBlock::new(&device, peripheral, self, Some(peripheral.base_address))?;
            block.imaged = imaged.contains(name);
            block.render(&mut output, self)?;
            blocks.insert(name, (block.module.clone(), block.owner.clone()));
        }
        output.push_str(&self.render_typed_partitions(&device, &blocks)?);
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
            || !self.read_capabilities.is_empty()
            || !self.register_divisions.is_empty()
            || !self.pointer_fields.is_empty()
        {
            return Err(Error::message(
                "a register library's API declares no ownership partition, register sides, interrupt source, read capability, register division or pointer field: they belong to a chip",
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
    fn render_typed_partitions(
        &self,
        device: &Device,
        blocks: &BTreeMap<&str, (String, String)>,
    ) -> Result<String> {
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
                let mut own = Vec::new();
                for peripheral in &partition.peripherals {
                    let (module, owner) = blocks.get(peripheral.as_str()).ok_or_else(|| {
                        Error::message(format!(
                            "ownership partition {} mixes typed peripherals and {peripheral}, which is not typed",
                            partition.name
                        ))
                    })?;
                    let _ = writeln!(fields, "pub {module}: super::{module}::{owner},");
                    let _ = writeln!(values, "{module}: super::{module}::{owner}::new({module}),");
                    own.push(module.clone());
                }
                push_doc(&mut output, &partition.description, "");
                let _ = writeln!(output, "pub struct {} {{\n{fields}}}\n", partition.name);
                output.push_str(&self.render_typed_partition_images(device, partition, blocks)?);
                if partition.handoff {
                    let (claims, pattern) = claim_tuple(&own);
                    let _ = writeln!(
                        output,
                        "/// The device registers of [`{name}`]'s blocks, which its platform\n\
                         /// handoff claims with one `oer_mmio::assume_owned`; the PAC root\n\
                         /// does not claim them.\n\
                         pub type {name}Claims = {claims};\n\n\
                         impl {name} {{\n\
                         /// Give each claimed block to its owner.\n\
                         #[inline]\n\
                         pub fn from_claims(claims: {name}Claims) -> Self {{\n\
                         let {pattern} = claims;\n\
                         Self {{\n{values}}}\n\
                         }}\n\
                         }}\n",
                        name = partition.name
                    );
                    continue;
                }
                ordered.extend(own);
                push_doc(&mut members, &partition.description, "");
                let _ = writeln!(members, "pub {}: {},", partition.member, partition.name);
                let _ = writeln!(
                    built,
                    "{}: {} {{\n{values}}},",
                    partition.member, partition.name
                );
            }
            if ordered.is_empty() {
                // Only platform handoffs claim the typed blocks: the PAC
                // root claims none.
                output.push_str("}\n");
            } else {
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

    /// The images of a typed partition: each reads every readable register
    /// of its blocks once, by image index, through the block's
    /// `read_image_word`.
    fn render_typed_partition_images(
        &self,
        device: &Device,
        partition: &crate::OwnershipPartition,
        blocks: &BTreeMap<&str, (String, String)>,
    ) -> Result<String> {
        let mut output = String::new();
        for image in self
            .partition_image_reads
            .iter()
            .filter(|image| image.partition == partition.name)
        {
            let registers = self.partition_registers(device, &partition.name)?;
            let mut arms = String::new();
            for (index, register) in registers.iter().enumerate() {
                let (module, _) = blocks.get(register.peripheral.as_str()).ok_or_else(|| {
                    Error::message(format!(
                        "partition image {} reads {}, which is not typed",
                        image.name, register.peripheral
                    ))
                })?;
                let base = device
                    .peripherals
                    .iter()
                    .find(|peripheral| peripheral.name == register.peripheral)
                    .map_or(0, |peripheral| peripheral.base_address);
                let _ = writeln!(
                    arms,
                    "{index} => self.{module}.read_image_word(0x{:x}),",
                    u64::from(register.address) - base
                );
            }
            let length = format!("{}_LEN", image.name.to_ascii_uppercase());
            let _ = writeln!(
                output,
                "impl {} {{\n\
                 /// The registers [`Self::{name}`] reads.\n\
                 pub const {length}: usize = {count};\n\n\
                 /// Read register `index` of the partition image: every readable\n\
                 /// register of the partition once, ascending by address; `None`\n\
                 /// past its end.\n\
                 ///\n\
                 /// SOURCE: {}.\n\
                 pub fn {name}(&self, index: usize) -> Option<u32> {{\n\
                 match index {{\n{arms}_ => None,\n}}\n\
                 }}\n\
                 }}\n",
                partition.name,
                image.sources.join(", "),
                name = image.name,
                count = registers.len(),
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
            sampled_bit_zero_writes,
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
    pub(crate) fn validate_typed_operations(&self, typed: &BTreeSet<&str>) -> Result<()> {
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
            w1c_register_snapshots => "a typed W1C register samples and acknowledges itself",
            field_reads => "use the typed register reader",
            field_snapshot_reads => "use the typed register reader",
            fixed_register_writes => "the typed backend renders no fixed variant write yet",
            register_image_reads => "the typed backend renders no register image read yet",
            register_image_writes => "the typed backend renders no register image write yet",
            zero_register_writes => "a store of zero is the fixed image zero",
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
            .chain(
                self.command_registers
                    .iter()
                    .map(|command| command.peripheral.as_str()),
            )
            .chain(self.data_ports.iter().map(|port| port.peripheral.as_str()))
            .chain(
                self.read_capabilities
                    .iter()
                    .map(|capability| capability.peripheral.as_str()),
            )
            .chain(
                self.register_divisions
                    .iter()
                    .map(|division| division.peripheral.as_str()),
            )
            .chain(
                self.shared_blocks
                    .iter()
                    .map(|block| block.peripheral.as_str()),
            )
            .find(|peripheral| !typed.contains(peripheral))
        {
            return Err(Error::message(format!(
                "PAC API register sides, interrupt source, write-trigger field, FIFO port, read capability, register division or shared block of {peripheral} name a peripheral that is not typed"
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

#[derive(Clone)]
struct Enumeration {
    name: String,
    variants: Vec<(String, u64, Option<String>)>,
    complete: bool,
}

/// What a bit of a command register is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandBit {
    /// Started by writing one; the hardware clears it when done.
    Command,
    /// Written with each start, by the start's argument.
    Qualifier,
    /// A self-clearing bit no start sets: read, zero in every start.
    Held,
}

#[derive(Clone)]
struct TypedField<'a> {
    info: &'a FieldInfo,
    method: String,
    offset: u32,
    width: u32,
    readable: bool,
    writable: bool,
    /// A DMA address field: no integer writer.
    pointer: bool,
    /// A policy pointer field: written only from the image of a typed
    /// reference.
    image: Option<&'a PointerField>,
    /// A write-trigger field: only its explicit trigger writes one to it.
    trigger: bool,
    /// A bit of a command register: a command, a qualifier or a held bit.
    command: Option<CommandBit>,
    /// A field whose one permitted value is zero: zero in every write.
    zero: bool,
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
        self.writable && !self.pointer && !self.trigger && self.command.is_none() && !self.zero
    }

    /// Whether a writer changes the field: by a setter or, for a policy
    /// pointer field, by the image of a typed reference.
    fn writes(&self) -> bool {
        self.has_setter() || self.image.is_some()
    }

    /// Whether a reader returns the field: a trigger reads back nothing the
    /// register keeps, and a constant-zero field holds nothing.
    fn has_getter(&self) -> bool {
        self.readable && !self.trigger && !self.zero
    }

    /// The type of a policy pointer field's image, in its register module.
    fn image_type(&self) -> String {
        format!("{}Image", type_binding_name(&self.info.name))
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

#[derive(Clone)]
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
    /// A command port's data words: each read and stored whole.
    port: bool,
    /// The clear register of a status-to-clear snapshot: written only by
    /// the snapshot's acknowledgement.
    acknowledged: bool,
    /// A register of a shared block: read, strobed and stored through `&self`.
    shared: bool,
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
        !self.w1c
            && !self.fifo
            && !self.port
            && !self.acknowledged
            && !self.shared
            && self.fields.iter().any(TypedField::writes)
    }

    /// The bits of the register's constant-zero fields.
    fn zeros(&self) -> u32 {
        self.fields
            .iter()
            .filter(|field| field.zero)
            .fold(0, |mask, field| mask | field.mask())
    }

    /// The bits of the register's DMA address fields.
    fn pointers(&self) -> u32 {
        self.fields
            .iter()
            .filter(|field| field.pointer || field.image.is_some())
            .fold(0, |mask, field| mask | field.mask())
    }

    /// The declared fields a fixed image may set, as a constant expression.
    fn settable_fields(&self) -> String {
        if self.zeros() == 0 {
            format!("{}::FIELDS", self.method)
        } else {
            format!("({0}::FIELDS & !{0}::ZERO)", self.method)
        }
    }

    /// The one field a shared block's register is stored whole by.
    fn stored_field(&self) -> Option<&TypedField<'_>> {
        match self.fields.as_slice() {
            [field] if self.shared && field.has_setter() => Some(field),
            _ => None,
        }
    }

    /// The bits of the register's write-trigger fields.
    fn triggers(&self) -> u32 {
        self.fields
            .iter()
            .filter(|field| field.trigger)
            .fold(0, |mask, field| mask | field.mask())
    }

    /// The bits of the register's commands.
    fn commands(&self) -> u32 {
        self.fields
            .iter()
            .filter(|field| field.command == Some(CommandBit::Command))
            .fold(0, |mask, field| mask | field.mask())
    }

    /// The self-clearing bits of a command register: a start refuses while
    /// any reads one, and writes zero to all but its own.
    fn busy(&self) -> u32 {
        self.fields
            .iter()
            .filter(|field| field.command.is_some())
            .fold(0, |mask, field| mask | field.mask())
    }

    /// The bits a write other than a field's own writes zero to, as a
    /// constant expression: write triggers, self-clearing command bits and
    /// constant-zero fields.
    fn modify_zero(&self) -> Option<String> {
        let masks = [
            (self.triggers(), "TRIGGERS"),
            (self.busy(), "BUSY"),
            (self.zeros(), "ZERO"),
        ]
        .into_iter()
        .filter(|(mask, _)| *mask != 0)
        .map(|(_, name)| format!("{}::{name}", self.method))
        .collect::<Vec<_>>();
        match masks.as_slice() {
            [] => None,
            [mask] => Some(mask.clone()),
            masks => Some(format!("({})", masks.join(" | "))),
        }
    }

    /// The read a read-modify-write starts from: every write-trigger and
    /// command field zero, so writing the image back starts no hardware
    /// action again.
    fn modify_read_call(&self) -> String {
        match self.modify_zero() {
            None => self.read_call(),
            Some(zero) => format!("{} & !{zero}", self.read_call()),
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
struct DeclaredKinds<'k, 'p> {
    /// The `(register, field)` DMA address fields.
    pointers: &'k BTreeSet<(&'p str, &'p str)>,
    /// The `(register, field)` policy pointer fields.
    images: &'k BTreeMap<(&'p str, &'p str), &'p PointerField>,
    /// The `(register, field)` write-trigger fields.
    triggers: &'k BTreeSet<(&'p str, &'p str)>,
    /// The `(register, field)` bits of command registers.
    commands: &'k BTreeMap<(&'p str, &'p str), CommandBit>,
    /// Whether the register is a FIFO port.
    fifo: bool,
    /// Whether the register holds a command port's data words.
    port: bool,
    /// Whether the register is the clear register of a snapshot.
    acknowledged: bool,
    /// Whether the register belongs to a shared block.
    shared: bool,
}

/// The owner type suffix of a block's interrupt-handler side.
const IRQ_SIDE: &str = "Irq";
/// The owner type suffix of a block's task side.
const TASK_SIDE: &str = "Task";
/// The owner type suffix of the registers a read capability leaves.
const REST: &str = "Rest";

/// How one owner reaches a register.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Reach {
    Owned,
    Observed,
    /// The owner's bits of a write-trigger word it shares with the other
    /// parts of a division.
    Trigger(u32),
}

/// One owner type of a block: the whole owner, a side, a part, a reader.
struct View {
    name: String,
    inner: String,
    reach: BTreeMap<String, Reach>,
    /// The arrays the owner has only some elements of: their first element
    /// and count.
    elements: BTreeMap<String, (u64, u64)>,
    /// The owner's type documentation; the whole owner's is rendered apart.
    doc: String,
    /// The owner is `Copy`: a reader every copy of which only reads.
    copy: bool,
}

impl View {
    /// `register` as the owner reaches it: an array it has only some
    /// elements of starts at the first of them and counts only them.
    fn register<'r, 'a>(&self, register: &'r TypedRegister<'a>) -> Cow<'r, TypedRegister<'a>> {
        match (self.elements.get(&register.info.name), register.array) {
            (Some(&(first, count)), Some((stride, _))) => {
                let mut part = register.clone();
                part.offset = register.offset + stride * first;
                part.array = Some((stride, count));
                Cow::Owned(part)
            }
            _ => Cow::Borrowed(register),
        }
    }

    /// The element `index` of `register` as the owner indexes it, if it
    /// has that element.
    fn element(&self, register: &TypedRegister<'_>, index: u64) -> Option<u64> {
        let (first, count) = match (self.elements.get(&register.info.name), register.array) {
            (Some(&elements), _) => elements,
            (None, Some((_, count))) => (0, count),
            (None, None) => return None,
        };
        index
            .checked_sub(first)
            .filter(|relative| *relative < count)
    }

    fn owns(&self, register: &str) -> bool {
        self.reach.get(register) == Some(&Reach::Owned)
    }

    fn reads(&self, register: &str) -> bool {
        matches!(
            self.reach.get(register),
            Some(Reach::Owned | Reach::Observed)
        )
    }

    /// The owner's bits of a shared trigger word; zero if it has none.
    fn trigger_bits(&self, register: &str) -> u32 {
        match self.reach.get(register) {
            Some(Reach::Trigger(mask)) => *mask,
            _ => 0,
        }
    }

    /// Whether the owner may write `value` whole to `register`: it owns the
    /// register, or `value` sets only its bits of a shared trigger word.
    fn writes_image(&self, register: &str, value: u32) -> bool {
        self.owns(register) || {
            let mask = self.trigger_bits(register);
            mask != 0 && value & !mask == 0
        }
    }
}

struct TypedBlock<'a> {
    peripheral: &'a Peripheral,
    module: String,
    owner: String,
    /// The chip's base address; a register library's block has none.
    base: Option<u64>,
    registers: Vec<TypedRegister<'a>>,
    split: Option<&'a RegisterSides>,
    /// The status-to-clear snapshots of the block.
    snapshots: Vec<&'a InterruptSnapshot>,
    capability: Option<&'a ReadCapability>,
    division: Option<&'a RegisterDivision>,
    /// A partition image reads the block: its owner reads any readable word.
    imaged: bool,
    shared: Option<&'a crate::SharedBlock>,
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
        let commands = policy
            .command_registers
            .iter()
            .filter(|command| command.peripheral == name)
            .flat_map(|command| {
                let register = command.register.as_str();
                command
                    .commands
                    .iter()
                    .map(move |field| ((register, field.as_str()), CommandBit::Command))
                    .chain(
                        command
                            .qualifiers
                            .iter()
                            .map(move |field| ((register, field.as_str()), CommandBit::Qualifier)),
                    )
                    .chain(
                        command
                            .held
                            .iter()
                            .map(move |field| ((register, field.as_str()), CommandBit::Held)),
                    )
            })
            .collect::<BTreeMap<_, _>>();
        let ports = policy
            .data_ports
            .iter()
            .filter(|port| port.peripheral == name)
            .map(|port| port.register.as_str())
            .collect::<BTreeSet<_>>();
        let images = policy
            .pointer_fields
            .iter()
            .filter(|pointer| pointer.peripheral == name)
            .map(|pointer| ((pointer.register.as_str(), pointer.field.as_str()), pointer))
            .collect::<BTreeMap<_, _>>();
        let snapshots = policy
            .interrupt_snapshots
            .iter()
            .filter(|snapshot| snapshot.peripheral == name)
            .collect::<Vec<_>>();
        let cleared = snapshots
            .iter()
            .map(|snapshot| snapshot.clear_register.as_str())
            .collect::<BTreeSet<_>>();
        let shared = policy
            .shared_blocks
            .iter()
            .find(|block| block.peripheral == name);
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
                images: &images,
                triggers: &triggers,
                commands: &commands,
                fifo: fifos.contains(info.name.as_str()),
                port: ports.contains(info.name.as_str()),
                acknowledged: cleared.contains(info.name.as_str()),
                shared: shared.is_some(),
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
        if let Some((register, field)) = commands.keys().find(|(register, field)| {
            !registers.iter().any(|typed| {
                typed.info.name == *register
                    && typed.fields.iter().any(|known| known.info.name == *field)
            })
        }) {
            return Err(Error::message(format!(
                "command field {name}.{register}.{field} is not a field of the block"
            )));
        }
        if let Some(register) = ports
            .iter()
            .find(|register| !registers.iter().any(|typed| typed.info.name == **register))
        {
            return Err(Error::message(format!(
                "data port {name}.{register} is not a register of the block"
            )));
        }
        if let Some((operation, register)) = policy
            .typed_register_operations(name)
            .find(|(_, register)| ports.contains(register))
        {
            return Err(Error::message(format!(
                "PAC API {operation:?} names data port {name}.{register}, which is only read and stored whole"
            )));
        }
        if let Some((operation, register)) = policy
            .typed_register_operations(name)
            .find(|(_, register)| commands.keys().any(|(command, _)| command == register))
        {
            return Err(Error::message(format!(
                "PAC API {operation:?} names command register {name}.{register}, which only its starts write"
            )));
        }
        if let Some((register, field)) = images.keys().find(|(register, field)| {
            !registers.iter().any(|typed| {
                typed.info.name == *register
                    && typed.fields.iter().any(|known| known.info.name == *field)
            })
        }) {
            return Err(Error::message(format!(
                "pointer field {name}.{register}.{field} is not a field of the block"
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
        // A reviewed fixed image is the one other store of a clear register:
        // the vendor's acknowledgement of named sources, never a writer of
        // arbitrary bits. No sample captured those events, so the image
        // names its proof: the enable register it masks them in first, or
        // why its caller consumes the event itself.
        let mut fixed_images = BTreeSet::new();
        for image in policy
            .fixed_register_images
            .iter()
            .filter(|image| image.peripheral == name)
        {
            let on_clear = cleared.contains(image.register.as_str());
            match (on_clear, &image.masks, &image.consumed_event) {
                (true, Some(_), None) | (true, None, Some(_)) => {}
                (true, None, None) => {
                    return Err(Error::message(format!(
                        "fixed-register-image {:?} acknowledges events of {name}.{} that no sample captured: it masks them first or names the caller that consumes them",
                        image.name, image.register
                    )));
                }
                (true, Some(_), Some(_)) => {
                    return Err(Error::message(format!(
                        "fixed-register-image {:?} names one proof: masks or consumed-event",
                        image.name
                    )));
                }
                (false, None, None) => {}
                (false, _, _) => {
                    return Err(Error::message(format!(
                        "fixed-register-image {:?} names an acknowledgement proof but {name}.{} is no snapshot's clear register",
                        image.name, image.register
                    )));
                }
            }
            if let Some(masks) = &image.masks {
                let enable = registers
                    .iter()
                    .find(|register| register.info.name == *masks);
                if cleared.contains(masks.as_str())
                    || fifos.contains(&masks.as_str())
                    || !enable.is_some_and(|enable| enable.array.is_none())
                {
                    return Err(Error::message(format!(
                        "fixed-register-image {:?} masks {name}.{masks}, which is no plain enable register of the block",
                        image.name
                    )));
                }
            }
            fixed_images.insert(image.name.as_str());
        }
        if let Some((operation, register)) =
            policy
                .typed_register_operations(name)
                .find(|(operation, register)| {
                    cleared.contains(register) && !fixed_images.contains(operation)
                })
        {
            return Err(Error::message(format!(
                "PAC API {operation:?} writes {name}.{register}, which only its snapshot's acknowledgement writes"
            )));
        }
        if let Some((register, field)) = triggers
            .iter()
            .chain(images.keys())
            .chain(commands.keys())
            .find(|(register, _)| cleared.contains(register))
        {
            return Err(Error::message(format!(
                "write-trigger, command or pointer field {name}.{register}.{field} writes a register only its snapshot's acknowledgement writes"
            )));
        }
        let split = policy
            .register_sides
            .iter()
            .find(|sides| sides.peripheral == name);
        let capability = policy
            .read_capabilities
            .iter()
            .find(|capability| capability.peripheral == name);
        let division = policy
            .register_divisions
            .iter()
            .find(|division| division.peripheral == name);
        if usize::from(split.is_some())
            + usize::from(capability.is_some())
            + usize::from(division.is_some())
            > 1
        {
            return Err(Error::message(format!(
                "typed peripheral {name} takes at most one of register sides, a read capability and a register division"
            )));
        }
        if shared.is_some() {
            if split.is_some() || capability.is_some() || division.is_some() {
                return Err(Error::message(format!(
                    "shared block {name} has one owner: no register sides, read capability or register division"
                )));
            }
            if let Some((operation, _)) = policy.typed_register_operations(name).next() {
                return Err(Error::message(format!(
                    "PAC API {operation:?} is on shared block {name}, which has only reads, strobes and stores"
                )));
            }
        }
        let block = Self {
            peripheral,
            module: crate::pac_api_render::member_binding_name(name),
            owner: type_binding_name(name),
            base,
            registers,
            split,
            snapshots,
            capability,
            division,
            imaged: false,
            shared,
        };
        block.validate_split()?;
        block.validate_snapshots()?;
        block.validate_capability()?;
        block.validate_division(policy)?;
        Ok(block)
    }

    /// Whether `register` is read without a side effect.
    fn reads_freely(register: &TypedRegister<'_>) -> bool {
        register.readable && !register.clears_on_read && !register.fifo
    }

    /// The bits a snapshot keeps and acknowledges: the status register's
    /// declared fields that the clear register declares too.
    fn snapshot_mask(&self, snapshot: &InterruptSnapshot) -> Result<u32> {
        let status = self.register(&snapshot.status_register)?;
        let clear = self.register(&snapshot.clear_register)?;
        Ok(status.declared() & clear.declared())
    }

    fn validate_snapshots(&self) -> Result<()> {
        let name = &self.peripheral.name;
        for snapshot in &self.snapshots {
            let status = self.register(&snapshot.status_register)?;
            let clear = self.register(&snapshot.clear_register)?;
            let refuse = |reason: &str| {
                Err(Error::message(format!(
                    "interrupt snapshot {:?} of {name}: {reason}",
                    snapshot.name
                )))
            };
            if !Self::reads_freely(status) || status.w1c || status.array.is_some() {
                return refuse(
                    "its status register is one word read without a side effect and acknowledged by its clear register",
                );
            }
            let clears = clear.fields.iter().all(|field| {
                field.info.modified_write_values == Some(ModifiedWriteValues::OneToClear)
                    || (field.info.modified_write_values.is_none()
                        && clear.info.modified_write_values
                            == Some(ModifiedWriteValues::OneToClear))
            });
            if !clear.writable
                || clear.array.is_some()
                || clear.fifo
                || clear.fields.is_empty()
                || !clears
            {
                return refuse("its clear register is one word of write-one-to-clear fields");
            }
            let mask = self.snapshot_mask(snapshot)?;
            if mask == 0 {
                return refuse("its clear register acknowledges no field of its status register");
            }
            if let Some(field) = status
                .fields
                .iter()
                .find(|field| field.mask() & mask != 0 && field.mask() & !mask != 0)
            {
                return refuse(&format!(
                    "status field {} is acknowledged only in part",
                    field.info.name
                ));
            }
        }
        Ok(())
    }

    fn validate_capability(&self) -> Result<()> {
        let Some(capability) = self.capability else {
            return Ok(());
        };
        let mut seen = BTreeSet::new();
        for name in &capability.registers {
            let register = self.register(name)?;
            if !seen.insert(name) || !Self::reads_freely(register) {
                return Err(Error::message(format!(
                    "read capability {} of {} names {name} twice or reads it with a side effect",
                    capability.name, self.peripheral.name
                )));
            }
        }
        Ok(())
    }

    fn validate_division(&self, policy: &PacApiPack) -> Result<()> {
        let Some(division) = self.division else {
            return Ok(());
        };
        let name = &self.peripheral.name;
        let refuse = |part: &str, reason: String| {
            Err(Error::message(format!(
                "register division of {name}, part {part}: {reason}"
            )))
        };
        let triggers = policy
            .write_trigger_fields
            .iter()
            .filter(|trigger| &trigger.peripheral == name)
            .map(|trigger| (trigger.register.as_str(), trigger.field.as_str()))
            .collect::<BTreeSet<_>>();
        // Each whole register, array element and trigger bit has one part.
        let mut owned = BTreeMap::<&str, &str>::new();
        let mut elements = BTreeMap::<(&str, u64), &str>::new();
        let mut bits = BTreeMap::<&str, u32>::new();
        for part in &division.parts {
            for register in &part.registers {
                self.register(register)?;
                if owned.insert(register, &part.name).is_some() {
                    return refuse(&part.name, format!("{register} has another owner"));
                }
            }
            for range in &part.elements {
                let register = self.register(&range.register)?;
                let Some((_, count)) = register.array else {
                    return refuse(&part.name, format!("{} is no array", range.register));
                };
                let (first, length) = (u64::from(range.first), u64::from(range.count));
                if length == 0 || first + length > count {
                    return refuse(
                        &part.name,
                        format!("elements of {} lie outside the array", range.register),
                    );
                }
                for element in first..first + length {
                    if elements
                        .insert((range.register.as_str(), element), &part.name)
                        .is_some()
                    {
                        return refuse(
                            &part.name,
                            format!("element {element} of {} has another owner", range.register),
                        );
                    }
                }
            }
            for register in &part.observes {
                if !Self::reads_freely(self.register(register)?) {
                    return refuse(
                        &part.name,
                        format!("it observes {register}, which has no side-effect-free read"),
                    );
                }
            }
            for trigger in &part.triggers {
                let register = self.register(&trigger.register)?;
                if register.readable || !register.writable || register.array.is_some() {
                    return refuse(
                        &part.name,
                        format!("{} is no write-only trigger word", trigger.register),
                    );
                }
                let mut mask = 0;
                for field in &trigger.fields {
                    let typed = self::field(register, field)?;
                    if !triggers.contains(&(trigger.register.as_str(), field.as_str()))
                        || typed.width != 1
                    {
                        return refuse(
                            &part.name,
                            format!(
                                "{}.{field} is no declared write-trigger field of one bit",
                                trigger.register
                            ),
                        );
                    }
                    mask |= typed.mask();
                }
                let shared = bits.entry(&trigger.register).or_default();
                if trigger.fields.is_empty() || *shared & mask != 0 {
                    return refuse(
                        &part.name,
                        format!(
                            "its bits of {} are none or another part's",
                            trigger.register
                        ),
                    );
                }
                *shared |= mask;
            }
        }
        for register in &self.registers {
            let whole = owned.contains_key(register.info.name.as_str());
            let split = elements.keys().any(|(name, _)| *name == register.info.name);
            let shared = bits.contains_key(register.info.name.as_str());
            if usize::from(whole) + usize::from(split) + usize::from(shared) > 1 {
                return Err(Error::message(format!(
                    "register division of {name} gives {} whole, by element or by trigger bit at once",
                    register.info.name
                )));
            }
        }
        Ok(())
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
            elements: BTreeMap::new(),
            doc: String::new(),
            copy: false,
        }];
        let side = format!(
            "/// One owner of a split `{}` block.\n\
             #[must_use = \"a split owner must be joined to recover the block\"]\n",
            self.peripheral.name
        );
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
                elements: BTreeMap::new(),
                doc: side.clone(),
                copy: false,
            });
            views.push(View {
                name: format!("{}{}", self.owner, IRQ_SIDE),
                inner: format!("oer_mmio::Part<P, BlockWords, {}Words>", IRQ_SIDE),
                reach: part,
                elements: BTreeMap::new(),
                doc: side,
                copy: false,
            });
        }
        if let Some(capability) = self.capability {
            let read = capability
                .registers
                .iter()
                .map(|register| (register.clone(), Reach::Observed))
                .collect::<BTreeMap<_, _>>();
            if read.len() != self.registers.len() {
                views.push(View {
                    name: format!("{}{REST}", self.owner),
                    inner: format!("oer_mmio::Part<P, BlockWords, {REST}Words>"),
                    reach: self
                        .registers
                        .iter()
                        .filter(|register| !read.contains_key(&register.info.name))
                        .map(|register| (register.info.name.clone(), Reach::Owned))
                        .collect(),
                    elements: BTreeMap::new(),
                    doc: format!(
                        "/// The `{}` registers but those [`{}{}`] reads: their one\n\
                         /// writer once the reader is split off.\n\
                         #[must_use = \"dropping a register owner permanently loses its register authority\"]\n",
                        self.peripheral.name, self.owner, capability.name
                    ),
                    copy: false,
                });
            }
            let mut doc = format!(
                "/// A read-only owner of `{}` registers: {}\n///\n",
                self.peripheral.name,
                clean(&capability.description).replace('\n', " ")
            );
            if capability.copy {
                doc.push_str(
                    "/// It is `Copy`: every copy only reads, and nothing writes the\n\
                     /// registers it reads any more.\n///\n",
                );
            }
            let _ = writeln!(doc, "/// SOURCE: {}.", capability.sources.join(", "));
            views.push(View {
                name: format!("{}{}", self.owner, capability.name),
                inner: format!("oer_mmio::Reader<P, BlockWords, {}Words>", capability.name),
                reach: read,
                elements: BTreeMap::new(),
                doc,
                copy: capability.copy,
            });
        }
        if let Some(division) = self.division {
            for part in &division.parts {
                let mut reach = BTreeMap::new();
                let mut elements = BTreeMap::new();
                for register in &part.observes {
                    reach.insert(register.clone(), Reach::Observed);
                }
                for register in &part.registers {
                    reach.insert(register.clone(), Reach::Owned);
                }
                for range in &part.elements {
                    reach.insert(range.register.clone(), Reach::Owned);
                    elements.insert(
                        range.register.clone(),
                        (u64::from(range.first), u64::from(range.count)),
                    );
                }
                for trigger in &part.triggers {
                    let mask = self
                        .register(&trigger.register)
                        .map(|register| {
                            register
                                .fields
                                .iter()
                                .filter(|field| trigger.fields.contains(&field.info.name))
                                .fold(0, |mask, field| mask | field.mask())
                        })
                        .unwrap_or_default();
                    reach.insert(trigger.register.clone(), Reach::Trigger(mask));
                }
                views.push(View {
                    name: format!("{}{}", self.owner, part.name),
                    inner: format!("oer_mmio::Part<P, BlockWords, {}Words>", part.name),
                    reach,
                    elements,
                    doc: format!(
                        "/// One owner of a divided `{}` block, which nothing joins again.\n",
                        self.peripheral.name
                    ),
                    copy: false,
                });
            }
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
             {shared}\
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
            shared = self.shared.map_or_else(String::new, |shared| {
                format!(
                    "/// A shared block: {}\n\
                     ///\n\
                     /// Every method takes `&self`; at its fixed address the owner is\n\
                     /// `Sync`, so every execution context reaches the one owner.\n\
                     ///\n\
                     /// SOURCE: {}.\n\
                     impl oer_mmio::SharedRegisters for BlockWords {{}}\n\n",
                    clean(&shared.description).replace('\n', " "),
                    shared.sources.join(", ")
                )
            }),
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
            if view.name == *owner {
                self.render_dividers(&mut body);
                self.render_image_word(&mut body)?;
            }
            for register in &self.registers {
                if let Some(reach) = view.reach.get(&register.info.name) {
                    view.register(register).render_methods(&mut body, *reach);
                }
            }
            self.render_snapshot_methods(&mut body, view)?;
            self.render_transactions(&mut body, &mut asserts, policy, view)?;
            if view.name != *owner {
                let _ = writeln!(
                    output,
                    "{}pub struct {}<P: oer_mmio::Placement{default}>({});\n",
                    view.doc, view.name, view.inner
                );
                if view.copy {
                    let _ = writeln!(
                        output,
                        "impl<P: oer_mmio::Placement> Clone for {0}<P> {{\n\
                         fn clone(&self) -> Self {{\n*self\n}}\n}}\n\n\
                         impl<P: oer_mmio::Placement> Copy for {0}<P> {{}}\n",
                        view.name
                    );
                }
            }
            let _ = writeln!(
                output,
                "impl<P: oer_mmio::Placement> {}<P> {{\n{body}}}\n",
                view.name
            );
        }
        if let Some(source) = policy
            .interrupt_sources
            .iter()
            .find(|source| source.peripheral == self.peripheral.name)
        {
            // The handler holds the interrupt side of a divided block, or
            // the whole block.
            let holder = match self.split {
                Some(_) => format!("{owner}{IRQ_SIDE}"),
                None => owner.clone(),
            };
            let _ = writeln!(
                output,
                "impl<P: oer_mmio::Placement> oer_mmio::InterruptSource for {holder}<P> {{\n\
                 const SOURCE: u16 = {};\n\
                 }}\n",
                source.source
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
        self.render_part_words(output, &views);
        self.render_snapshot_types(output)?;
        for register in &self.registers {
            register.render_module(output);
        }
        output.push_str(&asserts);
        output.push_str("}\n\n");
        Ok(())
    }

    /// The whole owner's methods that give its registers to a read
    /// capability or to the parts of a division.
    fn render_dividers(&self, body: &mut String) {
        let owner = &self.owner;
        if let Some(capability) = self.capability {
            let reader = format!("{owner}{}", capability.name);
            let method = snake(&capability.name);
            if capability.registers.len() == self.registers.len() {
                let _ = writeln!(
                    body,
                    "/// Give up every write: the block becomes [`{reader}`], which only\n\
                     /// reads.\n\
                     pub fn into_{method}(self) -> {reader}<P> {{\n\
                     {reader}(self.0.into_reader::<{}Words>())\n\
                     }}\n",
                    capability.name
                );
            } else {
                let _ = writeln!(
                    body,
                    "/// Give up the writes of the registers [`{reader}`] reads: split\n\
                     /// the block into it and [`{owner}{REST}`], which owns every other\n\
                     /// register.\n\
                     pub fn split_{method}(self) -> ({owner}{REST}<P>, {reader}<P>) {{\n\
                     let (rest, reader) = self.0.split::<{REST}Words, {}Words>();\n\
                     ({owner}{REST}(rest), {reader}(reader.into_reader()))\n\
                     }}\n",
                    capability.name
                );
            }
        }
        if let Some(division) = self.division {
            let types = division
                .parts
                .iter()
                .map(|part| format!("{owner}{}<P>", part.name))
                .collect::<Vec<_>>()
                .join(", ");
            let words = division
                .parts
                .iter()
                .map(|part| format!("{}Words", part.name))
                .collect::<Vec<_>>()
                .join(", ");
            let bindings = division
                .parts
                .iter()
                .map(|part| snake(&part.name))
                .collect::<Vec<_>>();
            let built = division
                .parts
                .iter()
                .zip(&bindings)
                .map(|(part, binding)| format!("{owner}{}({binding})", part.name))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(
                body,
                "/// Divide the block between its owners: {}\n\
                 ///\n\
                 /// Nothing joins them again.\n\
                 ///\n\
                 /// SOURCE: {}.\n\
                 pub fn divide(self) -> ({types}) {{\n\
                 let ({}) = self.0.divide::<({words})>();\n\
                 ({built})\n\
                 }}\n",
                clean(&division.description).replace('\n', " "),
                division.sources.join(", "),
                bindings.join(", "),
            );
        }
    }

    /// The whole owner's read of any readable word, for a partition image.
    fn render_image_word(&self, body: &mut String) -> Result<()> {
        if !self.imaged {
            return Ok(());
        }
        let mut arms = String::new();
        for register in self.registers.iter().filter(|register| register.readable) {
            if !Self::reads_freely(register) {
                return Err(Error::message(format!(
                    "a partition image reads every readable register of {}, and {} changes when read",
                    self.peripheral.name, register.info.name
                )));
            }
            for word in register.words() {
                let _ = writeln!(arms, "0x{word:x} => self.0.read::<0x{word:x}>(),");
            }
        }
        let _ = writeln!(
            body,
            "/// One read of the readable word at byte `offset`, for its partition's\n\
             /// image: every readable register reads without a side effect; `None`\n\
             /// for any other offset.\n\
             pub fn read_image_word(&self, offset: usize) -> Option<u32> {{\n\
             Some(match offset {{\n{arms}_ => return None,\n}})\n\
             }}\n"
        );
        Ok(())
    }

    /// The word lists of a read capability, the registers it leaves and the
    /// parts of a division.
    fn render_part_words(&self, output: &mut String, views: &[View]) {
        if self.capability.is_none() && self.division.is_none() {
            return;
        }
        for view in views.iter().skip(1) {
            let suffix = view
                .name
                .strip_prefix(self.owner.as_str())
                .unwrap_or_default();
            let mut owned = Vec::new();
            let mut observed = Vec::new();
            let mut triggers = Vec::new();
            for register in &self.registers {
                let words = view.register(register).words();
                match view.reach.get(&register.info.name) {
                    // A reader's registers are its own: no owner writes them.
                    Some(Reach::Owned) => owned.extend(words),
                    Some(Reach::Observed) if self.capability.is_some() => {
                        owned.extend(words);
                    }
                    Some(Reach::Observed) => observed.extend(words),
                    Some(Reach::Trigger(mask)) => {
                        triggers.push(format!("(0x{:x}, 0x{mask:08x})", register.offset));
                    }
                    None => {}
                }
            }
            owned.sort_unstable();
            observed.sort_unstable();
            let list = |words: &[u64]| {
                words
                    .iter()
                    .map(|word| format!("0x{word:x}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let triggers = if triggers.is_empty() {
                String::new()
            } else {
                format!(
                    "const TRIGGERS: &'static [(usize, u32)] = &[{}];\n",
                    triggers.join(", ")
                )
            };
            let _ = writeln!(
                output,
                "/// The words [`{}`] owns, observes and shares.\n\
                 pub struct {suffix}Words;\n\n\
                 impl oer_mmio::Words for {suffix}Words {{\n\
                 const OWNED: &'static [usize] = &[{}];\n\
                 const OBSERVED: &'static [usize] = &[{}];\n\
                 {triggers}\
                 }}\n",
                view.name,
                list(&owned),
                list(&observed),
            );
        }
    }

    /// The affine snapshot type of each status-to-clear protocol.
    fn render_snapshot_types(&self, output: &mut String) -> Result<()> {
        for snapshot in &self.snapshots {
            let status = self.register(&snapshot.status_register)?;
            let mask = self.snapshot_mask(snapshot)?;
            let name = format!("{}Snapshot", type_binding_name(&snapshot.name));
            let _ = writeln!(
                output,
                "/// One sample of the `{}` events `{}` acknowledges, to\n\
                 /// acknowledge once.\n\
                 ///\n\
                 /// SOURCE: {}.\n\
                 #[must_use = \"a sampled event set must be acknowledged\"]\n\
                 #[derive(Debug, Eq, PartialEq)]\n\
                 pub struct {name}(u32);\n\n\
                 impl {name} {{\n\
                 /// The bits the snapshot keeps: the events the clear register\n\
                 /// acknowledges.\n\
                 pub const EVENTS: u32 = 0x{mask:08x};\n\n\
                 /// Whether no event was sampled.\n\
                 pub const fn is_clear(&self) -> bool {{\nself.0 == 0\n}}\n",
                snapshot.status_register,
                snapshot.clear_register,
                snapshot.sources.join(", ")
            );
            for field in status
                .fields
                .iter()
                .filter(|field| field.mask() & !mask == 0 && field.has_getter())
            {
                render_getter(output, field, "&self");
                let _ = writeln!(
                    output,
                    "/// The snapshot without `{0}` unless `keep`: it is then not acknowledged.\n\
                     pub const fn retain_{1}(self, keep: bool) -> Self {{\n\
                     if keep {{ self }} else {{ Self({2}) }}\n}}\n",
                    field.info.name,
                    field.method.trim_start_matches("r#"),
                    without_field(field.mask())
                );
            }
            output.push_str("}\n\n");
        }
        Ok(())
    }

    /// Each snapshot's sample and acknowledgement, on an owner that owns its
    /// clear register and reads its status register.
    fn render_snapshot_methods(&self, body: &mut String, view: &View) -> Result<()> {
        for snapshot in &self.snapshots {
            if !view.owns(&snapshot.clear_register) || !view.reads(&snapshot.status_register) {
                continue;
            }
            let status = self.register(&snapshot.status_register)?;
            let clear = self.register(&snapshot.clear_register)?;
            let name = format!("{}Snapshot", type_binding_name(&snapshot.name));
            let _ = writeln!(
                body,
                "/// Sample `{}` once, keeping the events `{}` acknowledges.\n\
                 #[inline]\npub fn sample_{method}(&self) -> {name} {{\n\
                 {name}(self.0.{} & {name}::EVENTS)\n}}\n\n\
                 /// Acknowledge exactly the sampled events: one to each bit of `{}`\n\
                 /// the snapshot holds and zero to every other bit.\n\
                 #[inline]\npub fn acknowledge_{method}(&mut self, snapshot: {name}) {{\n\
                 self.0.{};\n}}\n",
                snapshot.status_register,
                snapshot.clear_register,
                status.read_call(),
                snapshot.clear_register,
                clear.write_call("snapshot.0"),
                method = snapshot.name,
            );
        }
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
        let first_view = view.name == self.owner;
        // The register as this owner reaches it.
        let reached = |register: &str| -> Result<Cow<'_, TypedRegister<'a>>> {
            Ok(view.register(self.register(register)?))
        };
        for operation in policy
            .full_register_writes
            .iter()
            .filter(|operation| operation.peripheral == name && view.owns(&operation.register))
        {
            let register = reached(&operation.register)?;
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
                || register.fields.iter().any(|field| field.image.is_some())
            {
                return Err(Error::message(format!(
                    "typed full-register-write {:?} needs one register with one 32-bit field that is no pointer field",
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
        for operation in policy
            .full_register_reads
            .iter()
            .filter(|operation| operation.peripheral == name && view.reads(&operation.register))
        {
            let register = reached(&operation.register)?;
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
        for operation in policy.fixed_register_images.iter().filter(|operation| {
            operation.peripheral == name && view.writes_image(&operation.register, operation.value)
        }) {
            let register = reached(&operation.register)?;
            if operation.whole_word && operation.sources.is_empty() {
                return Err(Error::message(format!(
                    "typed fixed-register-image {:?} writes a whole word without the vendor evidence for it",
                    operation.name
                )));
            }
            if operation.value & register.pointers() != 0 {
                return Err(Error::message(format!(
                    "typed fixed-register-image {:?} sets a DMA address field, which only a typed reference sets",
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
                } else if let Some(masks) = &operation.masks {
                    format!(
                        "Mask the sources of `0x{:08x}` in `{masks}`, keeping every other bit, then acknowledge them",
                        operation.value
                    )
                } else {
                    format!("Write the fixed image `0x{:08x}`", operation.value)
                },
                &operation.register,
                &operation.sources,
            );
            let mask = view.trigger_bits(&operation.register);
            let write = if mask == 0 {
                register.write_call(&format!("0x{:08x}", operation.value))
            } else {
                format!(
                    "write_trigger::<0x{:x}>(0x{:08x})",
                    register.offset, operation.value
                )
            };
            // An acknowledgement of events no sample captured: their enable
            // bits are cleared first, so no enabled event is lost to it.
            let masking = match &operation.masks {
                None => String::new(),
                Some(masks) => {
                    if !view.owns(masks) {
                        return Err(Error::message(format!(
                            "typed fixed-register-image {:?} masks {name}.{masks}, which its owner does not own",
                            operation.name
                        )));
                    }
                    let enable = reached(masks)?;
                    if first_view {
                        let _ = writeln!(
                            asserts,
                            "const _: () = assert!(oer_mmio::check::within(0x{:08x}, {}), \"{} masks declared enable fields only\");",
                            operation.value,
                            enable.settable_fields(),
                            operation.name
                        );
                    }
                    format!(
                        "let bits = self.0.{};\nself.0.{};\n",
                        enable.modify_read_call(),
                        enable.write_call(&format!("bits & !0x{:08x}", operation.value))
                    )
                }
            };
            if let Some(reason) = &operation.consumed_event {
                let _ = writeln!(body, "///\n/// The caller consumes these events: {reason}");
            }
            let _ = writeln!(
                body,
                "#[inline]\npub fn {}(&mut self, {}) {{\n{masking}self.0.{write};\n}}\n",
                operation.name,
                register.index_parameter(),
            );
            if first_view && !operation.whole_word {
                let _ = writeln!(
                    asserts,
                    "const _: () = assert!(oer_mmio::check::within(0x{:08x}, {}), \"{} writes declared fields only\");",
                    operation.value,
                    register.settable_fields(),
                    operation.name
                );
            }
            if mask != 0 {
                let _ = writeln!(
                    asserts,
                    "const _: () = assert!(oer_mmio::check::within(0x{:08x}, 0x{mask:08x}), \"{}'s {} sets its own trigger bits only\");",
                    operation.value, view.name, operation.name
                );
            }
        }
        for operation in policy
            .fixed_register_sequences
            .iter()
            .filter(|operation| operation.peripheral == name)
        {
            let mut writes = String::new();
            let mut checks = String::new();
            let mut reachable = true;
            for step in &operation.steps {
                let whole = self.register(&step.register)?;
                if !whole.writable || whole.w1c || whole.acknowledged {
                    return Err(Error::message(format!(
                        "typed fixed-register-sequence {:?} writes {}, which takes no fixed image",
                        operation.name, step.register
                    )));
                }
                if step.value & whole.pointers() != 0 {
                    return Err(Error::message(format!(
                        "typed fixed-register-sequence {:?} sets a DMA address field of {}, which only a typed reference sets",
                        operation.name, step.register
                    )));
                }
                let mask = view.trigger_bits(&step.register);
                if mask != 0 && step.value & !mask == 0 {
                    let _ = writeln!(
                        writes,
                        "self.0.write_trigger::<0x{:x}>(0x{:08x});",
                        whole.offset, step.value
                    );
                    let _ = writeln!(
                        checks,
                        "const _: () = assert!(oer_mmio::check::within(0x{:08x}, 0x{mask:08x}), \"{}'s {} sets its own trigger bits only\");",
                        step.value, view.name, operation.name
                    );
                    continue;
                }
                if !view.owns(&step.register) {
                    reachable = false;
                    break;
                }
                let register = view.register(whole);
                let call = match (whole.array, step.index) {
                    (None, None) => register.write_call(&format!("0x{:08x}", step.value)),
                    (Some((_, count)), Some(index)) if (index as u64) < count => {
                        let Some(element) = view.element(whole, index as u64) else {
                            reachable = false;
                            break;
                        };
                        let (stride, count) = register.array.unwrap_or((0, 1));
                        format!(
                            "write_indexed::<0x{:x}, 0x{stride:x}, {count}>(oer_mmio::Index::constant::<{element}>(), 0x{:08x})",
                            register.offset, step.value
                        )
                    }
                    _ => {
                        return Err(Error::message(format!(
                            "typed fixed-register-sequence {:?} indexes {} outside its array or not at all",
                            operation.name, step.register
                        )));
                    }
                };
                let _ = writeln!(writes, "self.0.{call};");
                if first_view && !step.whole_word {
                    let _ = writeln!(
                        checks,
                        "const _: () = assert!(oer_mmio::check::within(0x{:08x}, {}), \"{} writes declared fields only\");",
                        step.value,
                        whole.settable_fields(),
                        operation.name
                    );
                }
            }
            if !reachable {
                continue;
            }
            if operation.steps.iter().any(|step| step.whole_word) && operation.sources.is_empty() {
                return Err(Error::message(format!(
                    "typed fixed-register-sequence {:?} writes a whole word without the vendor evidence for it",
                    operation.name
                )));
            }
            asserts.push_str(&checks);
            let registers = operation
                .steps
                .iter()
                .map(|step| step.register.as_str())
                .collect::<Vec<_>>()
                .join("`, `");
            transaction_doc(
                body,
                if operation.steps.iter().any(|step| step.whole_word) {
                    "Write the reviewed fixed images in order, the vendor's whole-word ones with their undeclared bits"
                } else {
                    "Write the reviewed fixed images in order"
                },
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
            .filter(|operation| operation.peripheral == name && view.owns(&operation.register))
        {
            let register = reached(&operation.register)?;
            if (operation.input_mask | operation.set_mask) & register.pointers() != 0 {
                return Err(Error::message(format!(
                    "typed masked-register-modify {:?} changes a DMA address field, which only a typed reference sets",
                    operation.name
                )));
            }
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
                     oer_mmio::check::partition(0x{p:08x}, 0x{i:08x}, 0x{s:08x}, {m}),\n\
                     \"{n} partitions the word and changes declared fields only\"\n\
                     );",
                    p = operation.preserve_mask,
                    i = operation.input_mask,
                    s = operation.set_mask,
                    m = register.settable_fields(),
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
            .filter(|(_, operation)| operation.peripheral == name && view.owns(&operation.register))
        {
            let register = reached(&operation.register)?;
            let (parameter, input, image) =
                projected_image(&register, operation, &flag_or_enum, !clear)?;
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
                    "const _: () = assert!(oer_mmio::check::within({image}, {}), \"{} changes declared fields only\");",
                    register.settable_fields(),
                    operation.name
                );
            }
        }
        for operation in policy
            .field_replace_modifies
            .iter()
            .filter(|operation| operation.peripheral == name && view.owns(&operation.register))
        {
            let register = reached(&operation.register)?;
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
            let (parameter, input, image) =
                projected_image(&register, &projection, &flag_or_enum, true)?;
            let mask = operation
                .fields
                .iter()
                .map(|projection| field(&register, &projection.field).map(TypedField::mask))
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
                    "const _: () = assert!(oer_mmio::check::within({image}, 0x{mask:08x}) && oer_mmio::check::within(0x{mask:08x}, {}), \"{} replaces declared fields only\");",
                    register.settable_fields(),
                    operation.name
                );
            }
        }
        for operation in policy
            .zero_based_field_writes
            .iter()
            .filter(|operation| operation.peripheral == name && view.owns(&operation.register))
        {
            let register = reached(&operation.register)?;
            let mut parameters = String::new();
            let mut image = Vec::new();
            for field_name in &operation.fields {
                let selected = field(&register, field_name)?;
                if !selected.writes() {
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
                let (parameter, bits) = match selected.image {
                    Some(_) => (
                        format!("{}::{}", register.method, selected.image_type()),
                        format!("{argument}.0"),
                    ),
                    None => {
                        let write_type = selected.write_type();
                        (
                            qualified(&write_type, &register),
                            write_type.bits().replace("value", &argument),
                        )
                    }
                };
                let _ = write!(parameters, "{argument}: {parameter}, ");
                image.push(if selected.offset == 0 {
                    bits
                } else if operation.fields.len() == 1 {
                    // The one term is the whole argument: no parentheses.
                    format!("{bits} << {}", selected.offset)
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
        for operation in policy
            .sampled_bit_zero_writes
            .iter()
            .filter(|operation| operation.peripheral == name && view.owns(&operation.register))
        {
            let register = reached(&operation.register)?;
            let selected = field(&register, &operation.field)?;
            if !TypedBlock::reads_freely(&register)
                || !register.writable
                || register.w1c
                || register.acknowledged
                || !selected.has_setter()
            {
                return Err(Error::message(format!(
                    "typed sampled-bit-zero-write {:?} needs a field with a setter in a register read without a side effect",
                    operation.name
                )));
            }
            transaction_doc(
                body,
                &format!(
                    "Read the register once and write back `{}` as read, zero in every other bit",
                    operation.field
                ),
                &operation.register,
                &operation.sources,
            );
            let _ = writeln!(
                body,
                "#[inline]\npub fn {}(&mut self, {}) {{\nlet bits = self.0.{};\nself.0.{};\n}}\n",
                operation.name,
                register.index_parameter(),
                register.read_call(),
                register.write_call(&format!("bits & 0x{:08x}", selected.mask()))
            );
        }
        Ok(())
    }
}

/// A snapshot's bits without the field of `mask`: no AND at all when the
/// field is the whole word.
fn without_field(mask: u32) -> String {
    if mask == u32::MAX {
        "0".to_owned()
    } else {
        format!("self.0 & !0x{mask:08x}")
    }
}

/// The logical input of a field-local transaction, projected into its
/// fields: the parameter it takes, the statement binding its `u32` input
/// and the register image expression over that input.
fn projected_image(
    register: &TypedRegister<'_>,
    operation: &FieldOrModify,
    flag_or_enum: &BTreeSet<&str>,
    takes_image: bool,
) -> Result<(String, String, String)> {
    // A policy pointer field is set only from its image of a typed DMA
    // reference, which the transaction then takes, or to zero.
    if let ([projection], None, None) = (
        operation.fields.as_slice(),
        &operation.domain,
        operation.value,
    ) {
        let selected = field(register, &projection.field)?;
        if takes_image && selected.image.is_some() && projection.source_bit_offset == 0 {
            let bits = if selected.offset == 0 {
                "value.0".to_owned()
            } else {
                format!("(value.0 << {})", selected.offset)
            };
            return Ok((
                format!("value: {}::{}", register.method, selected.image_type()),
                String::new(),
                bits,
            ));
        }
    }
    for projection in &operation.fields {
        let selected = field(register, &projection.field)?;
        let cleared = selected.image.is_some() && operation.value == Some(0);
        if !cleared && (selected.pointer || selected.image.is_some() || selected.zero) {
            return Err(Error::message(format!(
                "typed field transaction {:?} changes {}, which only a typed DMA reference or zero sets",
                operation.name, projection.field
            )));
        }
    }
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
        kinds: &DeclaredKinds<'_, 'a>,
    ) -> Result<Self> {
        let readable = access.can_read();
        let writable = access.can_write();
        let clears_on_read = info.read_action.is_some();
        let fifo = kinds.fifo;
        let port = kinds.port;
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
        if let Some(trigger) = fields.iter().find(|field| {
            field.trigger
                && (!field.writable || (field.width != 1 && kinds.shared) || w1c || fifo || port)
        }) {
            return Err(Error::message(format!(
                "typed write-trigger field {peripheral}.{}.{} must be a writable field of an ordinary register, one bit in a shared block",
                info.name, trigger.info.name
            )));
        }
        if let Some(command) = fields.iter().find(|field| {
            field.command.is_some()
                && (field.width != 1
                    || !field.readable
                    || !field.writable
                    || field.trigger
                    || field.pointer
                    || field.image.is_some()
                    || w1c
                    || fifo
                    || port
                    || kinds.shared
                    || kinds.acknowledged
                    || clears_on_read)
        }) {
            return Err(Error::message(format!(
                "typed command field {peripheral}.{}.{} must be one readable and writable bit of an ordinary register",
                info.name, command.info.name
            )));
        }
        if fields.iter().any(|field| field.command.is_some())
            && let Some(other) = fields
                .iter()
                .find(|field| field.command.is_none() && (field.writes() || field.trigger))
        {
            return Err(Error::message(format!(
                "typed command register {peripheral}.{} is written only by its starts, but its field {} has a setter or a trigger",
                info.name, other.info.name
            )));
        }
        if port
            && (w1c
                || fifo
                || kinds.shared
                || kinds.acknowledged
                || clears_on_read
                || !matches!(
                    fields.as_slice(),
                    [field] if field.readable && field.has_setter() && field.image.is_none()
                ))
        {
            return Err(Error::message(format!(
                "typed data port {peripheral}.{} must be one register or array with one readable and writable field",
                info.name
            )));
        }
        if fifo
            && (array.is_some()
                || w1c
                || fields.len() != 1
                || fields
                    .iter()
                    .any(|field| field.pointer || field.image.is_some()))
        {
            return Err(Error::message(format!(
                "typed FIFO port {peripheral}.{} must be one register with one field that is no pointer field",
                info.name
            )));
        }
        if kinds.shared {
            let strobes = fields.iter().all(|field| field.trigger || !field.writable);
            let stored = matches!(fields.as_slice(), [field] if field.has_setter());
            if clears_on_read
                || fifo
                || w1c
                || fields.iter().any(|field| field.pointer)
                || (writable && (array.is_some() || !(strobes || stored)))
            {
                return Err(Error::message(format!(
                    "shared block register {peripheral}.{} must be a side-effect-free read, a write-trigger strobe or one field stored whole",
                    info.name
                )));
            }
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
            port,
            acknowledged: kinds.acknowledged,
            shared: kinds.shared,
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
        if let Reach::Trigger(mask) = reach {
            self.render_shared_triggers(body, mask);
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
            self.render_busy(body);
        }
        if reach != Reach::Owned || self.acknowledged {
            return;
        }
        if self.shared {
            self.render_shared_writes(body);
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
        self.render_commands(body);
        if self.port {
            self.render_port_write(body);
        }
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
            // A one-bit trigger writes one; a wider one writes its value.
            let (what, parameter, bits) = if field.width == 1 {
                (
                    "one".to_owned(),
                    String::new(),
                    format!("0x{:08x}", field.mask()),
                )
            } else {
                let write_type = field.write_type();
                let bits = write_type.bits();
                (
                    "`value`".to_owned(),
                    format!("value: {}", qualified(&write_type, self)),
                    if field.offset == 0 {
                        bits.to_owned()
                    } else {
                        format!("({bits} << {})", field.offset)
                    },
                )
            };
            let image = if self.readable {
                format!("(self.0.{}) | {bits}", self.modify_read_call())
            } else {
                // Written alone: the shift needs no parentheses.
                bits.strip_prefix('(')
                    .and_then(|bits| bits.strip_suffix(')'))
                    .map_or(bits.clone(), str::to_owned)
            };
            let _ = writeln!(
                body,
                "/// Write {what} to the write-trigger field `{}` of `{name}`, starting its\n\
                 /// hardware action once; {}.\n\
                 #[inline]\npub fn trigger_{module}_{}(&mut self, {index}{parameter}) {{\n\
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

    /// Whether each command reads one: it still runs.
    fn render_busy(&self, body: &mut String) {
        let module = &self.method;
        let index = self.index_parameter();
        let name = &self.info.name;
        for field in self
            .fields
            .iter()
            .filter(|field| field.command == Some(CommandBit::Command))
        {
            let _ = writeln!(
                body,
                "/// Whether the command `{}` of `{name}` still runs: the hardware\n\
                 /// clears the field when the command is done.\n\
                 #[inline]\npub fn {module}_{}_busy(&self, {index}) -> bool {{\n\
                 (self.0.{} & 0x{:08x}) != 0\n}}\n",
                field.info.name,
                field.method.trim_start_matches("r#"),
                self.read_call(),
                field.mask()
            );
        }
    }

    /// The start of each command: the one write of the register, refused
    /// while any self-clearing bit reads one, so it writes zero only over
    /// zero. It sets the command and each qualifier its argument sets, and
    /// keeps every other field read.
    fn render_commands(&self, body: &mut String) {
        let module = &self.method;
        let index = self.index_parameter();
        let name = &self.info.name;
        let Some(zero) = self.modify_zero().filter(|_| self.commands() != 0) else {
            return;
        };
        let qualifiers = self
            .fields
            .iter()
            .filter(|field| field.command == Some(CommandBit::Qualifier))
            .collect::<Vec<_>>();
        let parameters = qualifiers
            .iter()
            .map(|field| format!("{}: bool, ", field.method))
            .collect::<String>();
        let qualified = qualifiers
            .iter()
            .map(|field| {
                format!(
                    " | if {} {{ 0x{:08x} }} else {{ 0 }}",
                    field.method,
                    field.mask()
                )
            })
            .collect::<String>();
        let named = qualifiers
            .iter()
            .map(|field| format!("`{}`", field.info.name))
            .collect::<Vec<_>>()
            .join(", ");
        for field in self
            .fields
            .iter()
            .filter(|field| field.command == Some(CommandBit::Command))
        {
            let method = field.method.trim_start_matches("r#");
            let _ = writeln!(
                body,
                "/// Start the command `{}` of `{name}`: one write sets it{}; every\n\
                 /// other field keeps the value read, every other self-clearing bit is\n\
                 /// zero. It refuses, writing nothing, while any self-clearing bit of\n\
                 /// the register reads one (`BUSY`).\n\
                 #[inline]\npub fn start_{module}_{method}(&mut self, {index}{parameters}) -> Result<(), oer_mmio::CommandBusy> {{\n\
                 let value = self.0.{};\n\
                 if value & {module}::BUSY != 0 {{\n\
                 return Err(oer_mmio::CommandBusy {{ running: value & {module}::BUSY }});\n}}\n\
                 self.0.{};\nOk(())\n}}\n",
                field.info.name,
                if named.is_empty() {
                    String::new()
                } else {
                    format!(" with {named} as given")
                },
                self.read_call(),
                self.write_call(&format!(
                    "(value & !{zero}) | 0x{:08x}{qualified}",
                    field.mask()
                ))
            );
        }
    }

    /// A command port's data word stored whole: its one field, zero in
    /// every other bit, nothing read back.
    fn render_port_write(&self, body: &mut String) {
        let module = &self.method;
        let index = self.index_parameter();
        let name = &self.info.name;
        let Some(field) = self.fields.first() else {
            return;
        };
        let write_type = field.write_type();
        let bits = write_type.bits();
        let image = if field.offset == 0 {
            bits.to_owned()
        } else {
            format!("({bits} << {})", field.offset)
        };
        let _ = writeln!(
            body,
            "/// Store the data word `{name}` whole: its one field `{}`, zero in every\n\
             /// other bit; the hardware also writes it, so it is never modified.\n\
             #[inline]\npub fn write_{module}(&mut self, {index}value: {}) {{\n\
             self.0.{};\n}}\n",
            field.info.name,
            qualified(&write_type, self),
            self.write_call(&image)
        );
    }

    /// The triggers of a division part's own bits of a shared write-only
    /// trigger word: each stores its field alone, zero in every other bit.
    fn render_shared_triggers(&self, body: &mut String, mask: u32) {
        let module = &self.method;
        let name = &self.info.name;
        for field in self
            .fields
            .iter()
            .filter(|field| field.trigger && field.mask() & !mask == 0)
        {
            let _ = writeln!(
                body,
                "/// Write one to the write-trigger field `{}` of the shared word `{name}`,\n\
                 /// starting its hardware action once; zero in every other bit, which\n\
                 /// leaves the other owners' fields alone.\n\
                 #[inline]\npub fn trigger_{module}_{}(&mut self) {{\n\
                 self.0.write_trigger::<0x{:x}>(0x{:08x});\n}}\n",
                field.info.name,
                field.method.trim_start_matches("r#"),
                self.offset,
                field.mask()
            );
        }
    }

    /// A shared block register's writes through `&self`: each trigger
    /// strobed alone, every other bit zero, or the one field stored whole
    /// from zero. Neither reads the register back.
    fn render_shared_writes(&self, body: &mut String) {
        let module = &self.method;
        let name = &self.info.name;
        for field in self.fields.iter().filter(|field| field.trigger) {
            let _ = writeln!(
                body,
                "/// Strobe the write-trigger field `{}` of `{name}` alone: one there,\n\
                 /// zero in every other bit, nothing read back.\n\
                 #[inline]\npub fn trigger_{module}_{}(&self) {{\n\
                 self.0.write_shared::<0x{:x}>(0x{:08x});\n}}\n",
                field.info.name,
                field.method.trim_start_matches("r#"),
                self.offset,
                field.mask()
            );
        }
        if let Some(field) = self.stored_field() {
            let write_type = field.write_type();
            let bits = write_type.bits();
            let image = if field.offset == 0 {
                bits.to_owned()
            } else {
                format!("({bits} << {})", field.offset)
            };
            let _ = writeln!(
                body,
                "/// Store `{name}` whole: its one field `{}`, zero in every other\n\
                 /// bit, nothing read back.\n\
                 #[inline]\npub fn store_{module}(&self, value: {}) {{\n\
                 self.0.write_shared::<0x{:x}>({image});\n}}\n",
                field.info.name,
                qualified(&write_type, self),
                self.offset
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
        if self.commands() != 0 {
            let _ = writeln!(
                output,
                "/// The bits of the commands: each set only by its start.\n\
                 pub const COMMANDS: u32 = 0x{:08x};\n",
                self.commands()
            );
        }
        if self.busy() != 0 {
            let _ = writeln!(
                output,
                "/// The self-clearing bits: a start refuses while any reads one.\n\
                 pub const BUSY: u32 = 0x{:08x};\n",
                self.busy()
            );
        }
        if self.zeros() != 0 {
            let _ = writeln!(
                output,
                "/// The bits of the fields whose one permitted value is zero: zero in\n\
                 /// every write.\n\
                 pub const ZERO: u32 = 0x{:08x};\n",
                self.zeros()
            );
        }
        for field in &self.fields {
            if let Some(pointer) = field.image {
                render_image(output, field, pointer);
            }
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
        if self.w1c && !self.acknowledged {
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
                     if keep {{ self }} else {{ Self({2}) }}\n}}\n",
                    field.info.name,
                    field.method.trim_start_matches("r#"),
                    without_field(field.mask())
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
            for field in self.fields.iter().filter(|field| field.writes()) {
                let (parameter, bits, what) = match field.image {
                    Some(_) => (
                        field.image_type(),
                        "value.0",
                        format!("the image of a reference, `{}`", field.image_type()),
                    ),
                    None => {
                        let write_type = field.write_type();
                        (write_type.rust(), write_type.bits(), String::new())
                    }
                };
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
                    "/// Set `{}`{}.\n#[inline]\npub fn set_{}(&mut self, value: {parameter}) -> &mut Self {{\n{assignment}\nself\n}}\n",
                    field.info.name,
                    if what.is_empty() {
                        String::new()
                    } else {
                        format!(" to {what}")
                    },
                    field.method.trim_start_matches("r#"),
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
        kinds: &DeclaredKinds<'_, 'a>,
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
        let zero = match info.write_constraint {
            Some(WriteConstraint::Range(range)) if range.min == 0 && range.max == 0 => true,
            Some(WriteConstraint::Range(range))
                if range.min != 0 || range.max != u64::from(u32::MAX >> (32 - width)) =>
            {
                return Err(Error::message(format!(
                    "typed field {peripheral}.{}.{} has a range narrower than its width but zero alone; the typed backend renders no bounded field yet",
                    register.name, info.name
                )));
            }
            _ => false,
        };
        let enumeration = enumeration(peripheral, register, info, width)?;
        let key = (register.name.as_str(), info.name.as_str());
        let image = kinds.images.get(&key).copied();
        if let Some(pointer) = image {
            let fits = match pointer.encoding {
                None => width == 32,
                Some(RegisterPointerEncoding::BleCompressed) => width == 20,
                Some(RegisterPointerEncoding::LowBits) => width < 32,
            };
            if !fits
                || !access.can_write()
                || zero
                || enumeration.is_some()
                || kinds.pointers.contains(&key)
            {
                return Err(Error::message(format!(
                    "typed pointer field {peripheral}.{}.{} must be a writable field of its encoding's width, without values or another address write",
                    register.name, info.name
                )));
            }
        }
        Ok(Self {
            info,
            method: field_method(&info.name),
            offset,
            width,
            readable: access.can_read(),
            writable: access.can_write(),
            pointer: kinds.pointers.contains(&key),
            image,
            trigger: kinds.triggers.contains(&key),
            command: kinds.commands.get(&key).copied(),
            zero,
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

/// A policy pointer field's image type: made only from a typed reference
/// (or null), in the hardware's encoding of the address.
fn render_image(output: &mut String, field: &TypedField<'_>, pointer: &PointerField) {
    let image = field.image_type();
    let target = crate::layouts::camel(&pointer.points_to);
    let window = "<super::super::layouts::DmaWindow as oer_mmio::DmaWindow>::RANGE";
    let offset = pointer.member_offset;
    let address = if offset == 0 {
        "reference.bits()".to_owned()
    } else {
        format!("reference.bits().wrapping_add(0x{offset:x})")
    };
    // The address of a member of a referenced layout stays inside the
    // address space, so adding its offset never wraps.
    let plus = if offset == 0 {
        String::new()
    } else {
        format!(" + 0x{offset:x}")
    };
    let mut checks = if offset == 0 {
        String::new()
    } else {
        format!(
            "const _: () = assert!(\n\
             {window}.end as u64{plus} <= 1 << 32,\n\
             \"a member of every layout in the DMA window lies inside the address space\"\n\
             );\n"
        )
    };
    let (result, body, encoding) = match pointer.encoding {
        None => ("Self", format!("Self({address})"), "the address"),
        Some(RegisterPointerEncoding::LowBits) => {
            let width = field.width;
            let _ = writeln!(
                checks,
                "const _: () = assert!(\n\
                 ({window}.start as u64{plus}) >> {width} == ({window}.end as u64 - 1{plus}) >> {width},\n\
                 \"every address of the DMA window shares the high bits the hardware supplies\"\n\
                 );"
            );
            (
                "Self",
                format!("Self({address} & 0x{:08x})", u32::MAX >> (32 - width)),
                "the address's low bits; the hardware supplies the high bits, which every address of the DMA window shares",
            )
        }
        Some(RegisterPointerEncoding::BleCompressed) => (
            "Option<Self>",
            format!(
                "let address = {address};\n\
                 let index = (address >> 2) & 0x000f_ffff;\n\
                 if address & 0xffc0_0003 != 0x2f00_0000 || index == 0 {{\nNone\n}} else {{\nSome(Self(index))\n}}"
            ),
            "the nonzero word index of an aligned address with the `0x2f` SRAM prefix; `None` if the address has no such index",
        ),
    };
    let member = if offset == 0 {
        String::new()
    } else {
        format!(", at its member `0x{offset:x}` bytes in")
    };
    let _ = writeln!(
        output,
        "/// The `{}` field's image of a reference to a `{}`: {encoding}.\n\
         ///\n\
         /// SOURCE: {}.\n\
         #[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
         pub struct {image}(pub(super) u32);\n\n\
         impl {image} {{\n\
         /// The null reference: zero.\n\
         pub const NULL: Self = Self(0);\n\n\
         /// The image of `reference`{member}.\n\
         #[inline]\n\
         pub const fn new(reference: super::super::layouts::DmaRef<super::super::layouts::{target}>) -> {result} {{\n\
         {body}\n\
         }}\n\n\
         /// The image as the field holds it.\n\
         pub const fn bits(self) -> u32 {{\nself.0\n}}\n\
         }}\n\n\
         {checks}",
        field.info.name,
        pointer.points_to,
        pointer.sources.join(", "),
    );
}

/// The snake-case method name of an upper-camel-case type suffix.
fn snake(name: &str) -> String {
    let mut snake = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() && index > 0 {
            snake.push('_');
        }
        snake.push(character.to_ascii_lowercase());
    }
    snake
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
