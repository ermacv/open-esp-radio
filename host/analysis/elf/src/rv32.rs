//! The RV32 relocation types: the one table of what each type patches and
//! what its target is to the instruction or word it patches.
//!
//! Source: the RISC-V ELF psABI, "Relocations"
//! (<https://github.com/riscv-non-isa/riscv-elf-psabi-doc/blob/master/riscv-elf.adoc#relocations>).
//! Every reader of RV32 relocations (fingerprints, the stack analysis, the
//! lifter, the vendor report) asks this table; none keeps its own list.
//!
//! [`mask`] clears exactly the bits a relocation patches, so two objects
//! built from the same source compare equal before link-time values are
//! known. A type the table does not know masks its whole 32-bit word: a
//! fingerprint then never depends on a field it cannot name.

pub use object::elf::{
    R_RISCV_32, R_RISCV_32_PCREL, R_RISCV_64, R_RISCV_ADD8, R_RISCV_ADD16, R_RISCV_ADD32,
    R_RISCV_ADD64, R_RISCV_ALIGN, R_RISCV_BRANCH, R_RISCV_CALL, R_RISCV_CALL_PLT, R_RISCV_COPY,
    R_RISCV_GOT_HI20, R_RISCV_GOT32_PCREL, R_RISCV_GPREL_I, R_RISCV_GPREL_S, R_RISCV_HI20,
    R_RISCV_IRELATIVE, R_RISCV_JAL, R_RISCV_JUMP_SLOT, R_RISCV_LO12_I, R_RISCV_LO12_S,
    R_RISCV_NONE, R_RISCV_PCREL_HI20, R_RISCV_PCREL_LO12_I, R_RISCV_PCREL_LO12_S, R_RISCV_PLT32,
    R_RISCV_RELATIVE, R_RISCV_RELAX, R_RISCV_RVC_BRANCH, R_RISCV_RVC_JUMP, R_RISCV_RVC_LUI,
    R_RISCV_SET_ULEB128, R_RISCV_SET6, R_RISCV_SET8, R_RISCV_SET16, R_RISCV_SET32,
    R_RISCV_SUB_ULEB128, R_RISCV_SUB6, R_RISCV_SUB8, R_RISCV_SUB16, R_RISCV_SUB32, R_RISCV_SUB64,
    R_RISCV_TLS_DTPMOD32, R_RISCV_TLS_DTPMOD64, R_RISCV_TLS_DTPREL32, R_RISCV_TLS_DTPREL64,
    R_RISCV_TLS_GD_HI20, R_RISCV_TLS_GOT_HI20, R_RISCV_TLS_TPREL32, R_RISCV_TLS_TPREL64,
    R_RISCV_TLSDESC, R_RISCV_TLSDESC_ADD_LO12, R_RISCV_TLSDESC_CALL, R_RISCV_TLSDESC_HI20,
    R_RISCV_TLSDESC_LOAD_LO12, R_RISCV_TPREL_ADD, R_RISCV_TPREL_HI20, R_RISCV_TPREL_I,
    R_RISCV_TPREL_LO12_I, R_RISCV_TPREL_LO12_S, R_RISCV_TPREL_S,
};

/// The bits of the location a relocation patches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Field {
    /// Nothing: a hint to the linker.
    None,
    /// The U-type immediate of a 32-bit instruction, bits 31:12.
    Upper,
    /// The I-type immediate, bits 31:20.
    Immediate,
    /// The S-type immediate, bits 31:25 and 11:7.
    Store,
    /// The B-type immediate, bits 31:25 and 11:7.
    Branch,
    /// The J-type immediate, bits 31:12.
    Jump,
    /// An `auipc` and the `jalr` after it: the U-type immediate of the first
    /// word and the I-type immediate of the second.
    Call,
    /// The CB-type immediate of a 16-bit instruction.
    CompressedBranch,
    /// The CJ-type immediate of a 16-bit instruction.
    CompressedJump,
    /// The CI-type immediate of `c.lui`.
    CompressedUpper,
    /// The low six bits of a byte.
    Low6,
    /// A little-endian datum of this many bytes.
    Data(u8),
    /// A ULEB128 value: the seven value bits of each of its bytes.
    Uleb128,
    /// A type the table does not know: its whole 32-bit word.
    Unknown,
}

/// What a relocation's target is to the code or data it patches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// Patches nothing (`NONE`, `RELAX`, `ALIGN`).
    Hint,
    /// An `auipc`+`jalr` call of the target.
    Call,
    /// A `jal` to the target.
    Jump,
    /// A conditional branch, `c.j` or `c.jal` to the target.
    Branch,
    /// The upper 20 bits of the target's absolute address (`HI20`).
    AbsoluteHigh,
    /// The low 12 bits of the target's absolute address (`LO12_I`, `LO12_S`).
    AbsoluteLow,
    /// The upper bits of the PC-relative offset of the target (`PCREL_HI20`).
    PcRelativeHigh,
    /// The low bits completing the `PCREL_HI20` at the labelled instruction
    /// the target names.
    PcRelativeLow,
    /// The target's absolute address in a data word (`R_RISCV_32`).
    Word,
    /// Every other use: GOT and TLS forms, `c.lui`, link-time arithmetic,
    /// dynamic relocations and unknown types.
    Other,
}

/// One row of the table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Kind {
    pub name: &'static str,
    pub field: Field,
    pub role: Role,
}

/// The table row of `r_type`.
pub fn kind(r_type: u32) -> Kind {
    use Field as F;
    use Role as R;
    let (name, field, role) = match r_type {
        R_RISCV_NONE => ("NONE", F::None, R::Hint),
        R_RISCV_32 => ("32", F::Data(4), R::Word),
        R_RISCV_64 => ("64", F::Data(8), R::Other),
        R_RISCV_RELATIVE => ("RELATIVE", F::Data(4), R::Other),
        R_RISCV_COPY => ("COPY", F::None, R::Other),
        R_RISCV_JUMP_SLOT => ("JUMP_SLOT", F::Data(4), R::Other),
        R_RISCV_TLS_DTPMOD32 => ("TLS_DTPMOD32", F::Data(4), R::Other),
        R_RISCV_TLS_DTPMOD64 => ("TLS_DTPMOD64", F::Data(8), R::Other),
        R_RISCV_TLS_DTPREL32 => ("TLS_DTPREL32", F::Data(4), R::Other),
        R_RISCV_TLS_DTPREL64 => ("TLS_DTPREL64", F::Data(8), R::Other),
        R_RISCV_TLS_TPREL32 => ("TLS_TPREL32", F::Data(4), R::Other),
        R_RISCV_TLS_TPREL64 => ("TLS_TPREL64", F::Data(8), R::Other),
        R_RISCV_TLSDESC => ("TLSDESC", F::Data(4), R::Other),
        R_RISCV_BRANCH => ("BRANCH", F::Branch, R::Branch),
        R_RISCV_JAL => ("JAL", F::Jump, R::Jump),
        R_RISCV_CALL => ("CALL", F::Call, R::Call),
        R_RISCV_CALL_PLT => ("CALL_PLT", F::Call, R::Call),
        R_RISCV_GOT_HI20 => ("GOT_HI20", F::Upper, R::Other),
        R_RISCV_TLS_GOT_HI20 => ("TLS_GOT_HI20", F::Upper, R::Other),
        R_RISCV_TLS_GD_HI20 => ("TLS_GD_HI20", F::Upper, R::Other),
        R_RISCV_PCREL_HI20 => ("PCREL_HI20", F::Upper, R::PcRelativeHigh),
        R_RISCV_PCREL_LO12_I => ("PCREL_LO12_I", F::Immediate, R::PcRelativeLow),
        R_RISCV_PCREL_LO12_S => ("PCREL_LO12_S", F::Store, R::PcRelativeLow),
        R_RISCV_HI20 => ("HI20", F::Upper, R::AbsoluteHigh),
        R_RISCV_LO12_I => ("LO12_I", F::Immediate, R::AbsoluteLow),
        R_RISCV_LO12_S => ("LO12_S", F::Store, R::AbsoluteLow),
        R_RISCV_TPREL_HI20 => ("TPREL_HI20", F::Upper, R::Other),
        R_RISCV_TPREL_LO12_I => ("TPREL_LO12_I", F::Immediate, R::Other),
        R_RISCV_TPREL_LO12_S => ("TPREL_LO12_S", F::Store, R::Other),
        R_RISCV_TPREL_ADD => ("TPREL_ADD", F::None, R::Other),
        R_RISCV_ADD8 => ("ADD8", F::Data(1), R::Other),
        R_RISCV_ADD16 => ("ADD16", F::Data(2), R::Other),
        R_RISCV_ADD32 => ("ADD32", F::Data(4), R::Other),
        R_RISCV_ADD64 => ("ADD64", F::Data(8), R::Other),
        R_RISCV_SUB8 => ("SUB8", F::Data(1), R::Other),
        R_RISCV_SUB16 => ("SUB16", F::Data(2), R::Other),
        R_RISCV_SUB32 => ("SUB32", F::Data(4), R::Other),
        R_RISCV_SUB64 => ("SUB64", F::Data(8), R::Other),
        R_RISCV_GOT32_PCREL => ("GOT32_PCREL", F::Data(4), R::Other),
        R_RISCV_ALIGN => ("ALIGN", F::None, R::Hint),
        R_RISCV_RVC_BRANCH => ("RVC_BRANCH", F::CompressedBranch, R::Branch),
        R_RISCV_RVC_JUMP => ("RVC_JUMP", F::CompressedJump, R::Branch),
        R_RISCV_RVC_LUI => ("RVC_LUI", F::CompressedUpper, R::Other),
        R_RISCV_GPREL_I => ("GPREL_I", F::Immediate, R::Other),
        R_RISCV_GPREL_S => ("GPREL_S", F::Store, R::Other),
        R_RISCV_TPREL_I => ("TPREL_I", F::Immediate, R::Other),
        R_RISCV_TPREL_S => ("TPREL_S", F::Store, R::Other),
        R_RISCV_RELAX => ("RELAX", F::None, R::Hint),
        R_RISCV_SUB6 => ("SUB6", F::Low6, R::Other),
        R_RISCV_SET6 => ("SET6", F::Low6, R::Other),
        R_RISCV_SET8 => ("SET8", F::Data(1), R::Other),
        R_RISCV_SET16 => ("SET16", F::Data(2), R::Other),
        R_RISCV_SET32 => ("SET32", F::Data(4), R::Other),
        R_RISCV_32_PCREL => ("32_PCREL", F::Data(4), R::Other),
        R_RISCV_IRELATIVE => ("IRELATIVE", F::Data(4), R::Other),
        R_RISCV_PLT32 => ("PLT32", F::Data(4), R::Other),
        R_RISCV_SET_ULEB128 => ("SET_ULEB128", F::Uleb128, R::Other),
        R_RISCV_SUB_ULEB128 => ("SUB_ULEB128", F::Uleb128, R::Other),
        R_RISCV_TLSDESC_HI20 => ("TLSDESC_HI20", F::Upper, R::Other),
        R_RISCV_TLSDESC_LOAD_LO12 => ("TLSDESC_LOAD_LO12", F::Immediate, R::Other),
        R_RISCV_TLSDESC_ADD_LO12 => ("TLSDESC_ADD_LO12", F::Immediate, R::Other),
        R_RISCV_TLSDESC_CALL => ("TLSDESC_CALL", F::None, R::Other),
        _ => ("unknown", F::Unknown, R::Other),
    };
    Kind { name, field, role }
}

/// Whether a relocation of `r_type` may enter another function: an
/// `auipc`+`jalr` call or a `jal`.
pub fn enters(r_type: u32) -> bool {
    matches!(kind(r_type).role, Role::Call | Role::Jump)
}

/// Whether a relocation of `r_type` transfers control to its target.
pub fn transfers(r_type: u32) -> bool {
    matches!(kind(r_type).role, Role::Call | Role::Jump | Role::Branch)
}

/// The patched bits of `field` as (byte offset, byte width, mask) parts,
/// relative to the relocation offset. `Uleb128` is variable and has none.
pub fn parts(field: Field) -> &'static [(usize, usize, u32)] {
    match field {
        Field::None | Field::Uleb128 => &[],
        Field::Upper | Field::Jump => &[(0, 4, 0xffff_f000)],
        Field::Immediate => &[(0, 4, 0xfff0_0000)],
        Field::Store | Field::Branch => &[(0, 4, 0xfe00_0f80)],
        Field::Call => &[(0, 4, 0xffff_f000), (4, 4, 0xfff0_0000)],
        Field::CompressedBranch => &[(0, 2, 0x1c7c)],
        Field::CompressedJump => &[(0, 2, 0x1ffc)],
        Field::CompressedUpper => &[(0, 2, 0x107c)],
        Field::Low6 => &[(0, 1, 0x3f)],
        Field::Data(1) => &[(0, 1, 0xff)],
        Field::Data(2) => &[(0, 2, 0xffff)],
        Field::Data(8) => &[(0, 4, u32::MAX), (4, 4, u32::MAX)],
        Field::Data(_) | Field::Unknown => &[(0, 4, u32::MAX)],
    }
}

/// Clear in `bytes` the bits a relocation of `r_type` at `offset` patches.
/// A part that does not lie wholly inside `bytes` is left alone.
pub fn mask(bytes: &mut [u8], offset: usize, r_type: u32) {
    let field = kind(r_type).field;
    if field == Field::Uleb128 {
        for byte in bytes.iter_mut().skip(offset) {
            let more = *byte & 0x80 != 0;
            *byte &= 0x80;
            if !more {
                break;
            }
        }
        return;
    }
    for &(at, width, mask) in parts(field) {
        let Some(start) = offset.checked_add(at) else {
            continue;
        };
        let Some(location) = bytes.get_mut(start..start + width) else {
            continue;
        };
        let mut word = [0u8; 4];
        word[..width].copy_from_slice(location);
        let value = u32::from_le_bytes(word) & !mask;
        location.copy_from_slice(&value.to_le_bytes()[..width]);
    }
}

/// The floating-point calling convention an RV32 file header declares.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatAbi {
    /// `EF_RISCV_FLOAT_ABI_SOFT`.
    Soft,
    /// `EF_RISCV_FLOAT_ABI_SINGLE`.
    Single,
    /// `EF_RISCV_FLOAT_ABI_DOUBLE`.
    Double,
    /// `EF_RISCV_FLOAT_ABI_QUAD`.
    Quad,
}

/// What an RV32 file header's `e_flags` declare: the floating-point
/// calling convention and whether the code is RV32E.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeaderFlags {
    pub float_abi: FloatAbi,
    /// `EF_RISCV_RVE`: the reduced integer register file.
    pub rve: bool,
}

/// Decode an RV32 file header's `e_flags`.
pub fn header_flags(e_flags: u32) -> HeaderFlags {
    use object::elf::{
        EF_RISCV_FLOAT_ABI, EF_RISCV_FLOAT_ABI_DOUBLE, EF_RISCV_FLOAT_ABI_SINGLE,
        EF_RISCV_FLOAT_ABI_SOFT, EF_RISCV_RVE,
    };
    HeaderFlags {
        float_abi: match e_flags & EF_RISCV_FLOAT_ABI {
            EF_RISCV_FLOAT_ABI_SOFT => FloatAbi::Soft,
            EF_RISCV_FLOAT_ABI_SINGLE => FloatAbi::Single,
            EF_RISCV_FLOAT_ABI_DOUBLE => FloatAbi::Double,
            _ => FloatAbi::Quad,
        },
        rve: e_flags & EF_RISCV_RVE != 0,
    }
}
