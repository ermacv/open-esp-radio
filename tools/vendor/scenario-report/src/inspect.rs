//! Standalone inspection of a chip's pinned vendor code.
//!
//! Every archive member and ELF image of the chip's artifact manifest is
//! decoded with Blobray's RISC-V decoder. A forward pass over each function
//! folds constants and follows the bits of a loaded word through masks, so a
//! store reports which bits it clears, sets to a constant or takes from a
//! value the pass does not know, such as an argument. Two queries use it:
//! [`xref`] lists every read and write to an address range, and [`show`]
//! prints one function, from every artifact that defines it, with its folded
//! constants, relocation targets and string literals.
//!
//! The pass is intraprocedural and forgets what it knows where control can
//! enter from elsewhere, so an access through a pointer computed in another
//! function, or after a join, is not attributed.
use oer_analysis_elf::rv32::Role;
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::{
    DecodedOp, FunctionDecoder, FunctionSemantics, InstructionFlow, IntegerOp, MemoryKind, Operand,
    SemanticOp,
};
use oer_vendor_scenario::harness::{Result, invalid};
use oer_vendor_scenario::registers::Registers;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// Leading bytes of a Unix `ar` archive.
const ARCHIVE_MAGIC: &[u8] = b"!<arch>\n";
/// Artifact kinds the manifest pins as code: static archives and ELF images.
const CODE_EXTENSIONS: [&str; 2] = [".a", ".elf"];
/// Longest string literal a reference shows.
const STRING_LIMIT: usize = 160;
/// Width of a full word access, in bytes.
const WORD_BYTES: u8 = 4;
/// All bits of a word.
const ALL: u32 = u32::MAX;
/// Architectural registers, the hard-wired zero, the stack pointer and the
/// ones a call preserves.
const REGISTERS: usize = 32;
pub(crate) const ZERO: u8 = 0;
const CALLEE_SAVED: [u8; 13] = [2, 8, 9, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27];
/// Smallest instruction, the step of the undecodable-bytes fallback.
const PARCEL: usize = 2;
/// The argument registers `a0` to `a7`.
const ARGUMENTS: std::ops::Range<u8> = 10..18;
/// Most loads a followed pointer may take from its root.
const DEPTH: usize = 4;
/// Flags, width, precision and length characters between `%` and a
/// printf conversion.
const CONVERSION_MODIFIERS: &str = "-+ #0123456789.*lhzjtqL";
/// Most passes the dataflow takes to reach its fixed point.
const PASSES: usize = 16;

/// One vendor function of a pinned artifact.
pub struct Function {
    /// `artifact` for an ELF image, `artifact[member]` for an archive member.
    pub origin: String,
    pub name: String,
    /// Address of the first byte: linked for an ELF image, relative to its
    /// section for an archive member.
    pub entry: u32,
    pub code: Vec<u8>,
    /// Relocations of the function's code, by offset from its entry.
    references: BTreeMap<u32, Reference>,
    /// Whether the function belongs to a linked image, whose calls name
    /// their targets by address rather than by relocation.
    linked: bool,
}

/// What a relocation at one instruction refers to.
#[derive(Clone, Debug)]
struct Reference {
    /// ELF relocation type.
    kind: u32,
    target: String,
    addend: i64,
    /// The NUL-terminated string the target addresses, when it is one.
    literal: Option<String>,
}

impl Reference {
    fn is_call(&self) -> bool {
        matches!(
            oer_analysis_elf::rv32::kind(self.kind).role,
            Role::Call | Role::Jump
        )
    }

    /// Whether the relocation supplies the upper bits of an address.
    fn is_upper(&self) -> bool {
        matches!(
            oer_analysis_elf::rv32::kind(self.kind).role,
            Role::AbsoluteHigh | Role::PcRelativeHigh
        )
    }

    /// Whether the relocation supplies the low bits of a symbol's address.
    fn is_lower_absolute(&self) -> bool {
        matches!(
            oer_analysis_elf::rv32::kind(self.kind).role,
            Role::AbsoluteLow
        )
    }

    /// Whether the relocation completes the address its paired `auipc`
    /// started.
    fn is_lower_pc_relative(&self) -> bool {
        matches!(
            oer_analysis_elf::rv32::kind(self.kind).role,
            Role::PcRelativeLow
        )
    }

    /// The symbol address the relocation names.
    fn address(&self) -> Value {
        Value::Address {
            base: Place::Symbol(self.target.clone()),
            offset: self.addend as i32,
            literal: self.literal.clone(),
        }
    }

    fn describe(&self) -> String {
        let mut text = self.target.clone();
        if self.addend != 0 {
            let _ = write!(text, "{:+#x}", self.addend);
        }
        if let Some(literal) = &self.literal {
            let _ = write!(text, " {literal:?}");
        }
        text
    }
}

/// The code functions of every pinned artifact of the installed chip.
pub struct Corpus {
    pub functions: Vec<Function>,
    /// Function names of the linked images by address, for calls resolved
    /// by address: an image calls into the ROM and back.
    names: BTreeMap<u32, String>,
    registers: Registers,
    /// Code artifacts the manifest pins but this checkout does not hold, or
    /// holds as a firmware build of another recipe.
    pub missing: Vec<String>,
}

impl Corpus {
    /// Load every code artifact of the chip's manifest below the repository
    /// `root`, failing on one whose bytes differ from its pinned digest (for
    /// a firmware output, that of its build of the current recipe).
    pub fn load(root: &Path) -> Result<Self> {
        let mut corpus = Self {
            functions: vec![],
            names: BTreeMap::new(),
            registers: Registers::load(&root.join(oer_vendor_scenario::chip().registers))?,
            missing: vec![],
        };
        for artifact in &oer_vendor_scenario::artifacts::manifest().artifact {
            if !CODE_EXTENSIONS
                .iter()
                .any(|extension| artifact.path.ends_with(extension))
            {
                continue;
            }
            let path = oer_vendor_scenario::artifacts::path(root, &artifact.id);
            let Ok(bytes) = std::fs::read(&path) else {
                corpus.missing.push(artifact.id.clone());
                continue;
            };
            // A firmware output whose build does not follow its current
            // recipe has no digest to match: it is not held as pinned.
            let Ok(pin) = oer_vendor_scenario::artifacts::sha256(&artifact.id) else {
                corpus.missing.push(artifact.id.clone());
                continue;
            };
            let digest = format!("{:x}", Sha256::digest(&bytes));
            if digest != pin {
                return Err(invalid(format!(
                    "{} at {} has sha256 {digest}, not the pinned {pin}",
                    artifact.id,
                    path.display(),
                )));
            }
            corpus.add_artifact(&artifact.id, &bytes)?;
        }
        Ok(corpus)
    }

    /// A corpus of the given objects, for tests: (origin, ELF or archive).
    pub fn of(objects: &[(&str, &[u8])], registers: Registers) -> Result<Self> {
        let mut corpus = Self {
            functions: vec![],
            names: BTreeMap::new(),
            registers,
            missing: vec![],
        };
        for (origin, bytes) in objects {
            corpus.add_artifact(origin, bytes)?;
        }
        Ok(corpus)
    }

    fn add_artifact(&mut self, id: &str, bytes: &[u8]) -> Result<()> {
        if !bytes.starts_with(ARCHIVE_MAGIC) {
            self.add_object(id, bytes, true);
            return Ok(());
        }
        for (name, data) in oer_analysis_elf::members(bytes)? {
            self.add_object(&format!("{id}[{name}]"), data, false);
        }
        Ok(())
    }

    /// Add the text symbols of one ELF object with their relocations; bytes
    /// that are not an ELF object, such as an archive symbol table, add
    /// nothing.
    fn add_object(&mut self, origin: &str, bytes: &[u8], linked: bool) {
        let Ok(file) = oer_analysis_elf::Elf::parse(bytes) else {
            return;
        };
        let mut references: BTreeMap<(usize, u64), Reference> = BTreeMap::new();
        for section in file.sections() {
            for relocation in file.relocations(section.index).into_iter().flatten() {
                if oer_analysis_elf::rv32::kind(relocation.r_type).role == Role::Hint {
                    continue;
                }
                let oer_analysis_elf::Target::Symbol(index) = relocation.target else {
                    continue;
                };
                let Ok(symbol) = file.symbol_at(index) else {
                    continue;
                };
                let target_section = symbol.section.and_then(|s| file.section(s).ok());
                let name = match symbol.kind {
                    oer_analysis_elf::SymbolKind::Section => target_section
                        .as_ref()
                        .map(|s| s.name.to_owned())
                        .unwrap_or_default(),
                    _ => symbol.name.to_owned(),
                };
                let literal = target_section.as_ref().and_then(|s| {
                    literal(s, symbol.address.wrapping_add_signed(relocation.addend))
                });
                references.insert(
                    (section.index, relocation.at),
                    Reference {
                        kind: relocation.r_type,
                        target: name,
                        addend: relocation.addend,
                        literal,
                    },
                );
            }
        }
        for symbol in file.symbols() {
            let Some(index) = symbol.section else {
                continue;
            };
            if symbol.kind != oer_analysis_elf::SymbolKind::Text
                || symbol.size == 0
                || !symbol.defined
            {
                continue;
            }
            let Ok(section) = file.section(index) else {
                continue;
            };
            let start = symbol.address - section.address;
            let Some(code) = section
                .data
                .get(start as usize..(start + symbol.size) as usize)
            else {
                continue;
            };
            let Ok(entry) = u32::try_from(symbol.address) else {
                continue;
            };
            let references = references
                .range((index, start)..(index, start + symbol.size))
                .map(|((_, offset), reference)| ((offset - start) as u32, reference.clone()))
                .collect();
            if linked {
                self.names
                    .entry(entry)
                    .or_insert_with(|| symbol.name.to_owned());
            }
            self.functions.push(Function {
                origin: origin.to_owned(),
                name: symbol.name.to_owned(),
                entry,
                code: code.to_vec(),
                references,
                linked,
            });
        }
    }

    /// The decoded, annotated instructions of `function`.
    pub(crate) fn walk(&self, function: &Function) -> Vec<Step> {
        let mut decoded = vec![];
        let mut offset = 0usize;
        while offset + PARCEL <= function.code.len() {
            match RiscvDecoder.decode(&function.code[offset..]) {
                Some(op) => {
                    let length = usize::from(op.length);
                    decoded.push((offset, length, Some(op)));
                    offset += length;
                }
                None => {
                    decoded.push((offset, PARCEL, None));
                    offset += PARCEL;
                }
            }
        }
        let index: BTreeMap<usize, usize> = decoded
            .iter()
            .enumerate()
            .map(|(index, (at, ..))| (*at, index))
            .collect();
        // The instructions control reaches next from each one, and whether
        // it gets there through a call.
        let successors: Vec<Vec<usize>> = decoded
            .iter()
            .enumerate()
            .map(|(position, (at, _, op))| {
                let next = position + 1;
                let target = |displacement: i32| {
                    usize::try_from(*at as i64 + i64::from(displacement))
                        .ok()
                        .and_then(|target| index.get(&target).copied())
                };
                let mut to = vec![];
                match op.as_ref().map(|op| op.flow) {
                    None
                    | Some(InstructionFlow::Next | InstructionFlow::Trap)
                    | Some(InstructionFlow::Jump { link: true, .. })
                    | Some(InstructionFlow::Indirect { link: true, .. }) => to.push(next),
                    Some(InstructionFlow::Branch { displacement }) => {
                        to.push(next);
                        to.extend(target(displacement));
                    }
                    Some(InstructionFlow::Jump {
                        displacement,
                        link: false,
                    }) => to.extend(target(displacement)),
                    Some(InstructionFlow::Indirect { link: false, .. })
                    | Some(InstructionFlow::Return { .. })
                    | Some(InstructionFlow::Stop) => {}
                }
                to.retain(|successor| *successor < decoded.len());
                to
            })
            .collect();
        // Forward dataflow to a fixed point: the state entering an
        // instruction is the merge of the states leaving its predecessors.
        let mut entering: Vec<Option<State>> = vec![None; decoded.len()];
        if let Some(first) = entering.first_mut() {
            *first = Some(State::entry());
        }
        for _ in 0..PASSES {
            let mut changed = false;
            for position in 0..decoded.len() {
                let Some(state) = entering[position].clone() else {
                    continue;
                };
                let (leaving, _) = self.execute(function, &decoded[position], state);
                for successor in &successors[position] {
                    let merged = match &entering[*successor] {
                        Some(existing) => existing.merge(&leaving),
                        None => leaving.clone(),
                    };
                    if entering[*successor].as_ref() != Some(&merged) {
                        entering[*successor] = Some(merged);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        decoded
            .iter()
            .zip(entering)
            .map(|(instruction, state)| {
                self.execute(function, instruction, state.unwrap_or_else(State::new))
                    .1
            })
            .collect()
    }

    /// One instruction from `state`: the state leaving it and what it shows.
    fn execute(
        &self,
        function: &Function,
        (at, length, op): &(usize, usize, Option<DecodedOp>),
        mut state: State,
    ) -> (State, Step) {
        let at = *at;
        let Some(op) = op else {
            return (
                State::new(),
                Step {
                    offset: at as u32,
                    text: ".half".into(),
                    notes: vec![],
                    access: None,
                    print: None,
                },
            );
        };
        let raw = &function.code[at..at + length];
        let reference = function.references.get(&(at as u32));
        let address = function.entry.wrapping_add(at as u32);
        let mut notes = vec![];
        if let Some(reference) = reference {
            notes.push(reference.describe());
        }
        let call = matches!(
            op.flow,
            InstructionFlow::Jump { link: true, .. } | InstructionFlow::Indirect { link: true, .. }
        );
        // A linked image names its callees by address; an archive member
        // names them through the call's relocation, noted above.
        if call && function.linked && !reference.is_some_and(|r| r.is_call()) {
            let target = match op.flow {
                InstructionFlow::Jump { displacement, .. } => {
                    Some(address.wrapping_add_signed(displacement))
                }
                InstructionFlow::Indirect { base, offset, .. } => match state.get(base) {
                    Value::Constant(value) => Some(value.wrapping_add_signed(offset)),
                    _ => None,
                },
                _ => None,
            };
            let callee = target.map(|target| {
                self.names
                    .get(&target)
                    .cloned()
                    .unwrap_or_else(|| format!("{target:#010x}"))
            });
            if let Some(name) = &callee {
                notes.push(format!("<{name}>"));
            }
        }
        let lifted = RiscvDecoder.lift(raw);
        let access = state.step(lifted, address, reference, &mut notes, &self.registers);
        // A tail call jumps through a register without returning and passes
        // its arguments like any call.
        let tail = matches!(op.flow, InstructionFlow::Indirect { link: false, .. });
        let print = if call || tail { state.print() } else { None };
        if call {
            state.call();
        }
        (
            state,
            Step {
                offset: at as u32,
                text: op.text.clone(),
                notes,
                access,
                print,
            },
        )
    }
}

/// The NUL-terminated printable string at `address` of `section`, if any.
fn literal(section: &oer_analysis_elf::Section<'_>, address: u64) -> Option<String> {
    let start = usize::try_from(address.checked_sub(section.address)?).ok()?;
    let data = section.data;
    let bytes = data.get(start..)?;
    let end = bytes.iter().position(|b| *b == 0)?;
    let text = std::str::from_utf8(&bytes[..end]).ok()?;
    if text.is_empty()
        || !text
            .chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\t')
    {
        return None;
    }
    Some(text.chars().take(STRING_LIMIT).collect())
}

/// One decoded instruction and what the pass learned from it.
pub(crate) struct Step {
    pub(crate) offset: u32,
    text: String,
    notes: Vec<String>,
    access: Option<Access>,
    print: Option<Print>,
}

impl Step {
    /// The instruction with what the pass learned, as `show` prints it.
    pub(crate) fn display(&self) -> String {
        if self.notes.is_empty() {
            self.text.clone()
        } else {
            format!("{}    ; {}", self.text, self.notes.join(", "))
        }
    }
}

/// A call whose argument addresses a format string: the format and, for
/// each conversion, the location bits the following argument carries.
struct Print {
    format: String,
    arguments: Vec<Option<Extracted>>,
}

/// `(word & bits) >> shift` of the word at `location`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Extracted {
    pub location: Location,
    pub bits: u32,
    pub shift: u32,
}

/// A pointer the pass follows symbolically: an argument register's value at
/// entry, a symbol's address, or the word read from a location.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Place {
    Argument(u8),
    Symbol(String),
    Loaded(Box<Location>),
}

impl Place {
    /// How many loads lead to this pointer.
    fn depth(&self) -> usize {
        match self {
            Place::Argument(_) | Place::Symbol(_) => 0,
            Place::Loaded(location) => 1 + location.depth(),
        }
    }
}

impl std::fmt::Display for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Place::Argument(index) => write!(f, "a{index}"),
            Place::Symbol(name) => write!(f, "&{name}"),
            Place::Loaded(location) => write!(f, "{location}"),
        }
    }
}

/// A memory location an access resolves to.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Location {
    Absolute(u32),
    /// `offset` bytes past the pointer `Place`.
    Field(Place, i32),
}

impl Location {
    fn depth(&self) -> usize {
        match self {
            Location::Absolute(_) => 0,
            Location::Field(place, _) => place.depth(),
        }
    }

    /// The field offsets from the root pointer to this location, in order.
    pub fn offsets(&self) -> Vec<i32> {
        match self {
            Location::Absolute(_) => vec![],
            Location::Field(place, offset) => {
                let mut offsets = match place {
                    Place::Loaded(location) => location.offsets(),
                    _ => vec![],
                };
                offsets.push(*offset);
                offsets
            }
        }
    }
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Location::Absolute(address) => write!(f, "{address:#010x}"),
            Location::Field(place, offset) if *offset < 0 => {
                write!(f, "{place}->-{:#x}", offset.unsigned_abs())
            }
            Location::Field(place, offset) => write!(f, "{place}->{offset:#x}"),
        }
    }
}

/// A load or store to a location the pass resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Access {
    pub location: Location,
    pub width: u8,
    pub effect: Effect,
}

/// What an access does to its location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    Read,
    /// A store of the value read from the same location with `clear` bits
    /// forced to zero, `set` bits forced to one and `value` bits taken from a
    /// value the pass does not know; every other bit keeps its old value.
    Modify {
        clear: u32,
        set: u32,
        value: u32,
    },
    /// A store of a constant.
    Constant(u32),
    /// A store of a value the pass does not know, nonzero only in `bits`.
    Unknown {
        bits: u32,
    },
}

/// A register's value where the pass knows something about it.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Value {
    Constant(u32),
    /// The pointer `base` plus `offset` bytes, and the string it addresses
    /// when a relocation names one.
    Address {
        base: Place,
        offset: i32,
        literal: Option<String>,
    },
    /// `(word & bits) >> shift` of the word read from `location`.
    Extract {
        location: Location,
        bits: u32,
        shift: u32,
    },
    /// The `width`-byte value read from `location` with only its `keep`
    /// bits, `set` bits forced to one and `value` bits from a value the pass
    /// does not know.
    Word {
        location: Location,
        width: u8,
        keep: u32,
        set: u32,
        value: u32,
    },
    /// A value the pass does not know, nonzero only in `bits`.
    Unknown {
        bits: u32,
    },
}

const UNKNOWN: Value = Value::Unknown { bits: ALL };

impl Value {
    /// The value as a pointer, when it is one the pass can follow.
    fn pointer(&self) -> Option<(Place, i32)> {
        match self {
            Value::Address { base, offset, .. } => Some((base.clone(), *offset)),
            Value::Word {
                location,
                width: WORD_BYTES,
                keep: ALL,
                set: 0,
                value: 0,
            } if location.depth() < DEPTH => Some((Place::Loaded(Box::new(location.clone())), 0)),
            _ => None,
        }
    }

    /// The bits of a location the value carries, when it is a word read
    /// from one, a field of it, or a masked word.
    fn extracted(&self) -> Option<Extracted> {
        match self {
            Value::Word {
                location,
                keep,
                set: 0,
                value: 0,
                ..
            } => Some(Extracted {
                location: location.clone(),
                bits: *keep,
                shift: 0,
            }),
            Value::Extract {
                location,
                bits,
                shift,
            } => Some(Extracted {
                location: location.clone(),
                bits: *bits,
                shift: *shift,
            }),
            _ => None,
        }
    }

    /// What two paths agree on.
    fn merge(&self, other: &Value) -> Value {
        match (self, other) {
            _ if self == other => self.clone(),
            (Value::Unknown { bits: a }, Value::Unknown { bits: b }) => {
                Value::Unknown { bits: a | b }
            }
            (Value::Unknown { bits }, Value::Constant(value))
            | (Value::Constant(value), Value::Unknown { bits }) => {
                Value::Unknown { bits: bits | value }
            }
            _ => UNKNOWN,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct State([Value; REGISTERS]);

impl State {
    fn new() -> Self {
        Self([UNKNOWN; REGISTERS])
    }

    /// The state at a function's entry: each argument register holds its
    /// argument.
    fn entry() -> Self {
        let mut state = Self::new();
        for (index, register) in ARGUMENTS.enumerate() {
            state.0[usize::from(register)] = Value::Address {
                base: Place::Argument(index as u8),
                offset: 0,
                literal: None,
            };
        }
        state
    }

    fn merge(&self, other: &State) -> State {
        let mut merged = self.clone();
        for (value, other) in merged.0.iter_mut().zip(&other.0) {
            *value = value.merge(other);
        }
        merged
    }

    fn get(&self, register: u8) -> Value {
        if register == ZERO {
            Value::Constant(0)
        } else {
            self.0[usize::from(register)].clone()
        }
    }

    fn operand(&self, operand: Operand) -> Value {
        match operand {
            Operand::Immediate(value) => Value::Constant(value),
            Operand::Register(register) => self.get(register),
        }
    }

    fn set(&mut self, register: u8, value: Value) {
        if register != ZERO {
            self.0[usize::from(register)] = value;
        }
    }

    /// The print a call makes: the first argument register addressing a
    /// string with conversions, and the arguments after it.
    fn print(&self) -> Option<Print> {
        let registers: Vec<u8> = ARGUMENTS.collect();
        let (index, format) =
            registers.iter().enumerate().find_map(|(index, register)| {
                match self.get(*register) {
                    Value::Address {
                        literal: Some(text),
                        ..
                    } if !conversions(&text).is_empty() => Some((index, text)),
                    _ => None,
                }
            })?;
        let arguments = registers[index + 1..]
            .iter()
            .take(conversions(&format).len())
            .map(|register| self.get(*register).extracted())
            .collect();
        Some(Print { format, arguments })
    }

    /// A call leaves only the callee-saved registers known.
    fn call(&mut self) {
        for (register, value) in self.0.iter_mut().enumerate() {
            if !CALLEE_SAVED.contains(&(register as u8)) {
                *value = UNKNOWN;
            }
        }
    }

    /// Advance over one lifted instruction at `address`; the access it makes.
    /// A relocated upper immediate is the address of its symbol, and a
    /// relocated low immediate completes the address its base holds.
    fn step(
        &mut self,
        op: SemanticOp,
        address: u32,
        reference: Option<&Reference>,
        notes: &mut Vec<String>,
        registers: &Registers,
    ) -> Option<Access> {
        match op {
            SemanticOp::Upper {
                dest,
                value,
                pc_relative,
            } => {
                let result = match reference {
                    Some(reference) if reference.is_upper() => reference.address(),
                    Some(_) => UNKNOWN,
                    None if pc_relative => Value::Constant(address.wrapping_add(value)),
                    None => Value::Constant(value),
                };
                if let Value::Constant(value) = result {
                    notes.push(format!("= {value:#x}"));
                }
                self.set(dest, result);
                None
            }
            SemanticOp::Integer {
                op,
                dest,
                left,
                right,
            } => {
                let result = match reference {
                    Some(reference) if reference.is_lower_absolute() => reference.address(),
                    Some(reference) if reference.is_lower_pc_relative() => {
                        match self.operand(left) {
                            address @ Value::Address { .. } => address,
                            _ => UNKNOWN,
                        }
                    }
                    Some(_) => UNKNOWN,
                    None => {
                        if let Some(note) =
                            selected_bits(op, &self.operand(left), &self.operand(right), registers)
                        {
                            notes.push(note);
                        }
                        combine(op, self.operand(left), self.operand(right))
                    }
                };
                if let Value::Constant(value) = result
                    && matches!(right, Operand::Immediate(_))
                {
                    notes.push(format!("= {value:#x}"));
                }
                self.set(dest, result);
                None
            }
            SemanticOp::Memory {
                kind,
                base,
                displacement,
                width,
                dest,
                source,
                ..
            } => {
                let base = self.get(base);
                // A relocated low part replaces the displacement: an
                // absolute one names the symbol, a pc-relative one completes
                // the address its base already holds.
                let location = match (&base, reference) {
                    (_, Some(reference)) if reference.is_lower_absolute() => reference
                        .address()
                        .pointer()
                        .map(|(place, offset)| Location::Field(place, offset)),
                    (_, Some(reference)) if reference.is_lower_pc_relative() => base
                        .pointer()
                        .map(|(place, offset)| Location::Field(place, offset)),
                    (_, Some(_)) => None,
                    (Value::Constant(base), None) => {
                        Some(Location::Absolute(base.wrapping_add_signed(displacement)))
                    }
                    (_, None) => base
                        .pointer()
                        .map(|(place, offset)| Location::Field(place, offset + displacement)),
                };
                match &location {
                    Some(Location::Absolute(target)) => {
                        notes.push(format!("[{}]", registers.name(*target)))
                    }
                    Some(location @ Location::Field(..)) => notes.push(format!("[{location}]")),
                    None => {}
                }
                let access = location.clone().map(|location| {
                    let effect = match kind {
                        MemoryKind::Load | MemoryKind::FloatLoad => Effect::Read,
                        _ => match source.map(|s| self.get(s)) {
                            Some(Value::Word {
                                location: read,
                                width: read_width,
                                keep,
                                set,
                                value,
                            }) if read == location && read_width == width => {
                                let bits = width_mask(width);
                                Effect::Modify {
                                    clear: !(keep | set | value) & bits,
                                    set: set & bits,
                                    value: value & bits,
                                }
                            }
                            Some(Value::Constant(value)) => {
                                Effect::Constant(value & width_mask(width))
                            }
                            Some(Value::Unknown { bits }) => Effect::Unknown {
                                bits: bits & width_mask(width),
                            },
                            _ => Effect::Unknown {
                                bits: width_mask(width),
                            },
                        },
                    };
                    Access {
                        location,
                        width,
                        effect,
                    }
                });
                if let Some(dest) = dest {
                    let loaded = match location {
                        Some(location) if kind == MemoryKind::Load => Value::Word {
                            location,
                            width,
                            keep: width_mask(width),
                            set: 0,
                            value: 0,
                        },
                        _ => UNKNOWN,
                    };
                    self.set(dest, loaded);
                }
                access
            }
            SemanticOp::Link { dest } | SemanticOp::Opaque { dest } => {
                self.set(dest, UNKNOWN);
                None
            }
            SemanticOp::Unsupported => {
                *self = Self::new();
                None
            }
            SemanticOp::Fence { .. } | SemanticOp::None => None,
        }
    }
}

/// The register fields a mask or bit operation on a register value
/// selects, named by the published bindings.
fn selected_bits(
    op: IntegerOp,
    left: &Value,
    right: &Value,
    registers: &Registers,
) -> Option<String> {
    let (address, shift) = match left {
        Value::Word {
            location: Location::Absolute(address),
            ..
        } => (*address, 0),
        Value::Extract {
            location: Location::Absolute(address),
            shift,
            ..
        } => (*address, *shift),
        _ => return None,
    };
    let Value::Constant(operand) = right else {
        return None;
    };
    let mask = match op {
        IntegerOp::And | IntegerOp::Or | IntegerOp::Xor | IntegerOp::AndNot => *operand,
        IntegerOp::BitSet | IntegerOp::BitClear | IntegerOp::BitInvert | IntegerOp::BitExtract => {
            1 << (operand % u32::BITS)
        }
        _ => return None,
    };
    let word = address & !(u32::from(WORD_BYTES) - 1);
    let shift = shift + (address - word) * u8::BITS;
    Some(registers.bits(word, mask.checked_shl(shift).unwrap_or(0)))
}

/// The bits a `width`-byte access covers.
fn width_mask(width: u8) -> u32 {
    ALL.checked_shr(u32::BITS.saturating_sub(u32::from(width) * u8::BITS))
        .unwrap_or(ALL)
}

/// `op` over two values, keeping what the pass can say about the result.
fn combine(op: IntegerOp, left: Value, right: Value) -> Value {
    use IntegerOp::*;
    use Value::{Address, Constant, Extract, Unknown, Word};
    let word = |value: &Value| matches!(value, Word { .. });
    match (op, &left, &right) {
        (_, Constant(a), Constant(b)) => return Constant(op.evaluate(*a, *b)),
        (Add | Or | Xor | Sub | Shl | Shr | Sar, _, Constant(0)) => return left,
        (Add | Or | Xor, Constant(0), _) => return right,
        _ => {}
    }
    // Pointer arithmetic keeps the pointer.
    match (op, left.pointer(), right.pointer(), &left, &right) {
        (Add, Some((base, offset)), _, _, Constant(more))
        | (Add, _, Some((base, offset)), Constant(more), _) => {
            return Address {
                base,
                offset: offset.wrapping_add(*more as i32),
                literal: None,
            };
        }
        (Sub, Some((base, offset)), _, _, Constant(less)) => {
            return Address {
                base,
                offset: offset.wrapping_sub(*less as i32),
                literal: None,
            };
        }
        _ => {}
    }
    // A shift or mask of a word read from a location extracts a field.
    match (op, &left, &right) {
        (
            Shr | Sar,
            Word {
                location,
                keep,
                set: 0,
                value: 0,
                ..
            },
            Constant(shift),
        ) if *shift < u32::BITS => {
            return Extract {
                location: location.clone(),
                bits: keep & (ALL << shift),
                shift: *shift,
            };
        }
        (
            Shr | Sar,
            Extract {
                location,
                bits,
                shift,
            },
            Constant(more),
        ) if shift + more < u32::BITS => {
            let shift = shift + more;
            return Extract {
                location: location.clone(),
                bits: bits & (ALL << shift),
                shift,
            };
        }
        (
            And,
            Extract {
                location,
                bits,
                shift,
            },
            Constant(mask),
        )
        | (
            And,
            Constant(mask),
            Extract {
                location,
                bits,
                shift,
            },
        ) => {
            return Extract {
                location: location.clone(),
                bits: bits & mask.checked_shl(*shift).unwrap_or(0),
                shift: *shift,
            };
        }
        _ => {}
    }
    // Any other arithmetic on a pointer or a field is on a value the pass
    // does not know beyond the field's width.
    let unknown = |value: Value| match value {
        Address { .. } => UNKNOWN,
        Extract { bits, shift, .. } => Unknown {
            bits: bits >> shift,
        },
        other => other,
    };
    let (left, right) = (unknown(left), unknown(right));
    match (op, &left, &right) {
        (And, _, Constant(mask)) | (And, Constant(mask), _) if word(&left) || word(&right) => {
            let mask = *mask;
            modify(
                if word(&left) {
                    left.clone()
                } else {
                    right.clone()
                },
                |keep, set, value| (keep & mask, set & mask, value & mask),
            )
        }
        (AndNot, Word { .. }, Constant(mask)) => {
            let mask = *mask;
            modify(left.clone(), |keep, set, value| {
                (keep & !mask, set & !mask, value & !mask)
            })
        }
        (Or, _, Constant(bits)) | (Or, Constant(bits), _) if word(&left) || word(&right) => {
            let bits = *bits;
            modify(
                if word(&left) {
                    left.clone()
                } else {
                    right.clone()
                },
                |keep, set, value| (keep & !bits, set | bits, value & !bits),
            )
        }
        (BitSet, Word { .. }, Constant(bit)) => {
            let bits = 1u32 << (bit % u32::BITS);
            modify(left.clone(), |keep, set, value| {
                (keep & !bits, set | bits, value & !bits)
            })
        }
        (BitClear, Word { .. }, Constant(bit)) => {
            let bits = 1u32 << (bit % u32::BITS);
            modify(left.clone(), |keep, set, value| {
                (keep & !bits, set & !bits, value & !bits)
            })
        }
        (Or, Word { .. }, Unknown { bits }) | (Or, Unknown { bits }, Word { .. }) => {
            let bits = *bits;
            modify(
                if word(&left) {
                    left.clone()
                } else {
                    right.clone()
                },
                |keep, set, value| (keep & !bits, set & !bits, value | bits),
            )
        }
        (And, Unknown { bits }, Constant(mask)) | (And, Constant(mask), Unknown { bits }) => {
            Unknown { bits: bits & mask }
        }
        (Or, Unknown { bits }, Constant(more)) | (Or, Constant(more), Unknown { bits }) => {
            Unknown { bits: bits | more }
        }
        (Or, Unknown { bits: a }, Unknown { bits: b }) => Unknown { bits: a | b },
        (And, Unknown { bits: a }, Unknown { bits: b }) => Unknown { bits: a & b },
        (Shl, Unknown { bits }, Constant(shift)) => Unknown {
            bits: bits.checked_shl(*shift).unwrap_or(0),
        },
        (Shr, Unknown { bits }, Constant(shift)) => Unknown {
            bits: bits.checked_shr(*shift).unwrap_or(0),
        },
        // An extracted field is a value like any other from here on.
        (Shl, Word { .. }, Constant(shift)) => Unknown {
            bits: ALL.checked_shl(*shift).unwrap_or(0),
        },
        (Shr, Word { .. }, Constant(shift)) => Unknown {
            bits: ALL.checked_shr(*shift).unwrap_or(0),
        },
        _ => UNKNOWN,
    }
}

fn modify(word: Value, change: impl FnOnce(u32, u32, u32) -> (u32, u32, u32)) -> Value {
    match word {
        Value::Word {
            location,
            width,
            keep,
            set,
            value,
        } => {
            let (keep, set, value) = change(keep, set, value);
            Value::Word {
                location,
                width,
                keep,
                set,
                value,
            }
        }
        other => other,
    }
}

/// The address range an `xref` of one address covers: its word.
pub const WORD: u32 = WORD_BYTES as u32;

/// `text` as an address: hexadecimal with a `0x` prefix, or decimal.
pub fn parse_address(text: &str) -> std::result::Result<u32, String> {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(&hex.replace('_', ""), 16),
        None => text.replace('_', "").parse(),
    };
    parsed.map_err(|error| format!("{text}: {error}"))
}

/// The installed chip's corpus from this checkout. A pinned code artifact
/// the checkout does not hold is reported and skipped.
pub fn load() -> Result<Corpus> {
    let corpus = Corpus::load(&oer_os_process::built_root())?;
    for id in &corpus.missing {
        eprintln!("{id}: not in this checkout, skipped");
    }
    Ok(corpus)
}

/// Every resolved access of the corpus to `start..end`, one line each:
/// address and register, R or W, origin, function and offset, and for a
/// store the bits it changes named by the published fields.
pub fn xref(corpus: &Corpus, start: u32, end: u32) -> String {
    report(
        corpus,
        |location| matches!(location, Location::Absolute(address) if (start..end).contains(address)),
    )
}

/// Every access of the corpus to a field reached through pointers whose
/// last offsets are `offsets`, from any argument or symbol: `[0x34, 0]`
/// matches the word at offset 0 of the pointer stored at offset 0x34.
pub fn fields(corpus: &Corpus, offsets: &[i32]) -> String {
    report(corpus, |location| {
        matches!(location, Location::Field(..)) && location.offsets().ends_with(offsets)
    })
}

/// One line for each access to a location `wanted` selects.
fn report(corpus: &Corpus, wanted: impl Fn(&Location) -> bool) -> String {
    let mut lines = BTreeSet::new();
    for function in &corpus.functions {
        for step in corpus.walk(function) {
            let Some(access) = step.access else {
                continue;
            };
            if !wanted(&access.location) {
                continue;
            }
            let (name, bits): (String, Box<dyn Fn(u32) -> String>) = match &access.location {
                Location::Absolute(address) => {
                    let word = address & !(u32::from(WORD_BYTES) - 1);
                    (
                        corpus.registers.name(*address),
                        Box::new(move |mask| corpus.registers.bits(word, mask)),
                    )
                }
                location => (location.to_string(), Box::new(bit_list)),
            };
            let (mode, detail) = match access.effect {
                Effect::Read => ("R", String::new()),
                Effect::Modify { clear, set, value } => {
                    let mut parts = vec![];
                    if clear != 0 {
                        parts.push(format!("clears {}", bits(clear)));
                    }
                    if set != 0 {
                        parts.push(format!("sets {}", bits(set)));
                    }
                    if value != 0 {
                        parts.push(format!("from a value {}", bits(value)));
                    }
                    if parts.is_empty() {
                        parts.push("rewrites the value read".into());
                    }
                    ("W", parts.join("; "))
                }
                Effect::Constant(value) => ("W", format!("= {value:#x}")),
                Effect::Unknown { bits: mask } => ("W", format!("a value {}", bits(mask))),
            };
            let width = if access.width == WORD_BYTES {
                String::new()
            } else {
                format!(" {}-byte", access.width)
            };
            lines.insert((
                access.location.clone(),
                function.origin.clone(),
                function.name.clone(),
                step.offset,
                format!(
                    "{name} {mode}{width} {}::{}+{:#x}  {detail}",
                    function.origin, function.name, step.offset
                ),
            ));
        }
    }
    let mut text = String::new();
    for (.., line) in lines {
        let _ = writeln!(text, "{}", line.trim_end());
    }
    text
}

/// Every print of the corpus that passes bits of a location in
/// `start..end` to a conversion: the register bits, the shift, the format
/// text up to that conversion and the call site.
pub fn prints(corpus: &Corpus, start: u32, end: u32) -> String {
    let mut lines = BTreeSet::new();
    for function in &corpus.functions {
        for step in corpus.walk(function) {
            let Some(print) = step.print else {
                continue;
            };
            let conversions = conversions(&print.format);
            for (argument, conversion) in print.arguments.iter().zip(&conversions) {
                let Some(Extracted {
                    location: Location::Absolute(address),
                    bits,
                    shift,
                }) = argument
                else {
                    continue;
                };
                if !(start..end).contains(address) {
                    continue;
                }
                let word = address & !(u32::from(WORD_BYTES) - 1);
                lines.insert((
                    *address,
                    function.origin.clone(),
                    function.name.clone(),
                    step.offset,
                    format!(
                        "{} {} >> {shift} -> {conversion:?} {}::{}+{:#x}",
                        corpus.registers.name(*address),
                        corpus.registers.bits(word, *bits),
                        function.origin,
                        function.name,
                        step.offset
                    ),
                ));
            }
        }
    }
    let mut text = String::new();
    for (.., line) in lines {
        let _ = writeln!(text, "{line}");
    }
    text
}

/// The conversions of a printf format, each with the text since the
/// previous one: `"a=%d b=%x"` is `["a=%d", " b=%x"]`.
fn conversions(format: &str) -> Vec<String> {
    let mut conversions = vec![];
    let mut current = String::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        if c != '%' {
            continue;
        }
        if chars.peek() == Some(&'%') {
            current.extend(chars.next());
            continue;
        }
        while let Some(&c) = chars.peek() {
            if !CONVERSION_MODIFIERS.contains(c) {
                break;
            }
            current.push(c);
            chars.next();
        }
        if let Some(c) = chars.next() {
            current.push(c);
            if c.is_ascii_alphabetic() {
                conversions.push(std::mem::take(&mut current));
            }
        }
    }
    conversions
}

/// The set bits of `mask` by number.
fn bit_list(mask: u32) -> String {
    let bits: Vec<String> = (0..u32::BITS)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| bit.to_string())
        .collect();
    format!("bits {}", bits.join(","))
}

/// `text` as a field offset: hexadecimal with a `0x` prefix, or decimal,
/// either with an optional leading `-`.
pub fn parse_offset(text: &str) -> std::result::Result<i32, String> {
    let (negative, magnitude) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let value = parse_address(magnitude)?;
    let value = i32::try_from(value).map_err(|error| format!("{text}: {error}"))?;
    Ok(if negative { -value } else { value })
}

/// Every definition of `name` in the corpus, annotated.
pub fn show(corpus: &Corpus, name: &str) -> Option<String> {
    let mut text = String::new();
    for function in corpus.functions.iter().filter(|f| f.name == name) {
        let _ = writeln!(
            text,
            "== {}::{} at {:#x}, {:#x} bytes",
            function.origin,
            function.name,
            function.entry,
            function.code.len()
        );
        for step in corpus.walk(function) {
            let _ = writeln!(text, "  +{:04x}  {}", step.offset, step.display());
        }
    }
    (!text.is_empty()).then_some(text)
}

/// The Blobray executable `cargo vendor-check scenario` passes to every
/// command; inspection reads the pinned artifacts itself and ignores it.
#[derive(clap::Args)]
pub struct Runner {
    #[arg(long, hide = true)]
    binary: Option<std::path::PathBuf>,
}

/// The reviewer commands a scenario binary offers beside its scenarios.
#[derive(clap::Subcommand)]
pub enum Command {
    /// Every read and write of the pinned vendor code to the addresses from
    /// `start` up to `end`, one word when `end` is omitted, with the bits each
    /// store clears, sets or takes from a computed value.
    Xref {
        #[arg(value_parser = parse_address)]
        start: u32,
        #[arg(value_parser = parse_address)]
        end: Option<u32>,
        #[command(flatten)]
        runner: Runner,
    },
    /// One pinned vendor function, annotated, from every artifact defining it.
    Show {
        function: String,
        #[command(flatten)]
        runner: Runner,
    },
    /// Every read and write of the pinned vendor code to a structure field
    /// reached through pointers whose last offsets are `offsets`, from any
    /// argument or symbol: `0x34 0` is the word at offset 0 of the pointer
    /// stored at offset 0x34.
    Fields {
        #[arg(required = true, allow_hyphen_values = true, value_parser = parse_offset)]
        offsets: Vec<i32>,
        #[command(flatten)]
        runner: Runner,
    },
    /// Every print of the pinned vendor code that passes bits of the
    /// addresses from `start` up to `end` (one word when omitted) to a
    /// format conversion, with the format text up to that conversion.
    Prints {
        #[arg(value_parser = parse_address)]
        start: u32,
        #[arg(value_parser = parse_address)]
        end: Option<u32>,
        #[command(flatten)]
        runner: Runner,
    },
}

/// Print what `command` asks for about the installed chip's pinned code.
pub fn run(command: Command) -> Result<()> {
    let corpus = load()?;
    let text = match command {
        Command::Xref { start, end, .. } => {
            xref(&corpus, start, end.unwrap_or(start.saturating_add(WORD)))
        }
        Command::Prints { start, end, .. } => {
            prints(&corpus, start, end.unwrap_or(start.saturating_add(WORD)))
        }
        Command::Fields { offsets, .. } => fields(&corpus, &offsets),
        Command::Show { function, .. } => show(&corpus, &function)
            .ok_or_else(|| invalid(format!("no pinned artifact defines {function}")))?,
    };
    print!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests;
