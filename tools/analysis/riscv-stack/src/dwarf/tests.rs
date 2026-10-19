use super::locations::Place;
use super::*;
use object::write::Object;
use object::{Architecture, BinaryFormat, Endianness, SectionKind};
use oer_analysis_riscv_model::{Callee, WorkingMemory};

/// Tiny DWARF4 fixtures. Attribute values are already encoded in their
/// declared forms; the production reader still parses every section/DIE.
struct Fixture {
    info: Vec<u8>,
    abbrev: Vec<u8>,
    codes: u64,
    loc: Vec<u8>,
    ranges: Vec<u8>,
}
fn uleb(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        out.push(byte | if value == 0 { 0 } else { 0x80 });
        if value == 0 {
            break;
        }
    }
}
type Attr = (gimli::DwAt, gimli::DwForm, Vec<u8>);
fn number(name: gimli::DwAt, form: gimli::DwForm, value: u32) -> Attr {
    (name, form, value.to_le_bytes().to_vec())
}
fn reference_attr(name: gimli::DwAt, value: usize) -> Attr {
    number(name, gimli::DW_FORM_ref4, value as u32)
}
fn expr(bytes: &[u8]) -> Attr {
    let mut data = Vec::new();
    uleb(&mut data, bytes.len() as u64);
    data.extend_from_slice(bytes);
    (gimli::DW_AT_location, gimli::DW_FORM_exprloc, data)
}
fn string(name: gimli::DwAt, value: &str) -> Attr {
    let mut bytes = value.as_bytes().to_vec();
    bytes.push(0);
    (name, gimli::DW_FORM_string, bytes)
}
fn flag(name: gimli::DwAt, value: bool) -> Attr {
    (name, gimli::DW_FORM_flag, vec![u8::from(value)])
}
fn range(start: u32, end: u32) -> [Attr; 2] {
    [
        number(gimli::DW_AT_low_pc, gimli::DW_FORM_addr, start),
        number(gimli::DW_AT_high_pc, gimli::DW_FORM_data4, end - start),
    ]
}
impl Fixture {
    fn new() -> Self {
        let mut info = vec![0; 4];
        info.extend_from_slice(&4_u16.to_le_bytes());
        info.extend_from_slice(&0_u32.to_le_bytes());
        info.push(4);
        let mut fixture = Self {
            info,
            abbrev: Vec::new(),
            codes: 0,
            loc: Vec::new(),
            ranges: Vec::new(),
        };
        fixture.die(gimli::DW_TAG_compile_unit, true, &range(0, 0x10000));
        fixture
    }
    fn die(&mut self, tag: gimli::DwTag, children: bool, attrs: &[Attr]) -> usize {
        self.codes += 1;
        uleb(&mut self.abbrev, self.codes);
        uleb(&mut self.abbrev, u64::from(tag.0));
        self.abbrev.push(u8::from(children));
        for (name, form, _) in attrs {
            uleb(&mut self.abbrev, u64::from(name.0));
            uleb(&mut self.abbrev, u64::from(form.0));
        }
        self.abbrev.extend_from_slice(&[0, 0]);
        let offset = self.info.len();
        uleb(&mut self.info, self.codes);
        for (_, _, data) in attrs {
            self.info.extend_from_slice(data);
        }
        offset
    }
    fn end(&mut self) {
        self.info.push(0);
    }
    fn ty(&mut self, name: &str) -> usize {
        self.die(
            gimli::DW_TAG_base_type,
            false,
            &[
                string(gimli::DW_AT_name, name),
                number(gimli::DW_AT_byte_size, gimli::DW_FORM_data4, 4),
            ],
        )
    }
    fn function(&mut self, start: u32, end: u32) -> usize {
        self.die(gimli::DW_TAG_subprogram, true, &range(start, end))
    }
    fn finish(mut self) -> Rc<[u8]> {
        self.end();
        let length = (self.info.len() - 4) as u32;
        self.info[..4].copy_from_slice(&length.to_le_bytes());
        self.abbrev.push(0);
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        for (name, bytes) in [
            (".debug_info", self.info),
            (".debug_abbrev", self.abbrev),
            (".debug_loc", self.loc),
            (".debug_ranges", self.ranges),
        ] {
            let section =
                object.add_section(Vec::new(), name.as_bytes().to_vec(), SectionKind::Debug);
            object.append_section_data(section, &bytes, 1);
        }
        Rc::from(object.write().unwrap())
    }
}
fn read_fixture(elf: &Rc<[u8]>, mode: DebugMetadata) -> (Dwarf, TypeFacts) {
    read(elf, mode, &mut crate::tests::ledger(), &mut || Ok(())).unwrap()
}

#[test]
fn common_pass_collects_register_parameters_stack_locals_and_pointer_values() {
    let mut fixture = Fixture::new();
    let ty = fixture.ty("word");
    fixture.function(100, 200);
    let parameter = fixture.die(
        gimli::DW_TAG_formal_parameter,
        false,
        &[reference_attr(gimli::DW_AT_type, ty), expr(&[0x5a])],
    ); // reg10
    let local = fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[reference_attr(gimli::DW_AT_type, ty), expr(&[0x72, 36])],
    ); // breg2+36
    let pointer = fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[
            reference_attr(gimli::DW_AT_type, ty),
            expr(&[0x73, 8, 0x9f]),
        ],
    ); // breg3+8; stack_value
    fixture.end();
    let elf = fixture.finish();
    let (_, standard) = read_fixture(&elf, DebugMetadata::Standard);
    let (_, mut typed) = read_fixture(&elf, DebugMetadata::TypedLocations);
    assert!(standard.locations.is_none());
    let locals = typed.locations.take().unwrap();
    assert_eq!(
        format!("{standard:?}"),
        format!("{typed:?}"),
        "common type/signature/global facts are identical"
    );
    for (register, owner, place) in [
        (10, parameter, Place::Register(10)),
        (2, local, Place::Memory(2, 36)),
        (3, pointer, Place::Pointer(3, 8)),
    ] {
        let location = locals.registers[&register][0];
        assert_eq!(
            (location.start, location.end, location.owner),
            (100, 200, owner)
        );
        assert_eq!(
            (location.offset, location.bytes, location.place),
            (0, None, place)
        );
        assert_eq!(locals.variable_type(owner), Some(ty));
    }
}

#[test]
fn independent_missing_and_unsupported_pieces_keep_object_byte_offsets() {
    let mut fixture = Fixture::new();
    let ty = fixture.ty("aggregate");
    fixture.function(100, 200);
    fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[
            reference_attr(gimli::DW_AT_type, ty),
            // reg10, missing 4 bytes, unsupported reg11 arithmetic, stack piece.
            expr(&[
                0x5a, 0x93, 4, 0x93, 4, 0x5b, 0x23, 1, 0x93, 4, 0x72, 36, 0x93, 4,
            ]),
        ],
    );
    // A non-byte bit piece stops subsequent offsets, retaining prior pieces.
    fixture.die(
        gimli::DW_TAG_formal_parameter,
        false,
        &[
            reference_attr(gimli::DW_AT_type, ty),
            expr(&[0x5c, 0x93, 4, 0x5d, 0x9d, 7, 1, 0x5e, 0x93, 4]),
        ],
    );
    // Unsupported expression/register produces no known location.
    fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[
            reference_attr(gimli::DW_AT_type, ty),
            expr(&[0x03, 0, 0x20, 0, 0]),
        ],
    );
    fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[reference_attr(gimli::DW_AT_type, ty), expr(&[0x90, 32])],
    );
    fixture.end();
    let (_, types) = read_fixture(&fixture.finish(), DebugMetadata::TypedLocations);
    let locals = types.locations.unwrap();
    assert_eq!(locals.registers.len(), 3);
    assert_eq!(
        (
            locals.registers[&10][0].offset,
            locals.registers[&10][0].bytes
        ),
        (0, Some(4))
    );
    assert_eq!(
        (
            locals.registers[&2][0].offset,
            locals.registers[&2][0].bytes
        ),
        (12, Some(4))
    );
    assert_eq!(locals.registers[&12][0].offset, 0);
    assert!(!locals.registers.contains_key(&11));
    assert!(!locals.registers.contains_key(&13));
    assert!(!locals.registers.contains_key(&14));
}

#[test]
fn lexical_ranges_and_location_lists_preserve_gaps_and_explicit_start_scope() {
    let mut fixture = Fixture::new();
    let ty = fixture.ty("word");
    fixture.function(100, 200);
    for (start, end) in [(110_u32, 120_u32), (140, 150), (0, 0)] {
        fixture.ranges.extend_from_slice(&start.to_le_bytes());
        fixture.ranges.extend_from_slice(&end.to_le_bytes());
    }
    fixture.die(
        gimli::DW_TAG_lexical_block,
        true,
        &[number(gimli::DW_AT_ranges, gimli::DW_FORM_sec_offset, 0)],
    );
    fixture.die(
        gimli::DW_TAG_formal_parameter,
        false,
        &[reference_attr(gimli::DW_AT_type, ty), expr(&[0x5a])],
    );
    fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[
            reference_attr(gimli::DW_AT_type, ty),
            expr(&[0x5b]),
            number(gimli::DW_AT_start_scope, gimli::DW_FORM_data4, 1),
        ],
    );
    fixture.end();
    for (start, end) in [(112_u32, 115_u32), (145, 148)] {
        fixture.loc.extend_from_slice(&start.to_le_bytes());
        fixture.loc.extend_from_slice(&end.to_le_bytes());
        fixture.loc.extend_from_slice(&1_u16.to_le_bytes());
        fixture.loc.push(0x5c); // reg12
    }
    fixture.loc.extend_from_slice(&[0; 8]);
    fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[
            reference_attr(gimli::DW_AT_type, ty),
            number(gimli::DW_AT_location, gimli::DW_FORM_sec_offset, 0),
        ],
    );
    fixture.end();
    let (_, types) = read_fixture(&fixture.finish(), DebugMetadata::TypedLocations);
    let locals = types.locations.unwrap();
    let ranges = |register| {
        locals.registers[&register]
            .iter()
            .map(|location| (location.start, location.end))
            .collect::<Vec<_>>()
    };
    assert_eq!(ranges(10), [(110, 120), (140, 150)]);
    assert_eq!(ranges(12), [(112, 115), (145, 148)]);
    assert!(
        !locals.registers.contains_key(&11),
        "start_scope expressions are not extended to the containing scope"
    );
    for pc in [109, 120, 130, 150] {
        assert!(
            !locals.registers[&10]
                .iter()
                .any(|location| location.start <= pc && pc < location.end)
        );
    }
}

#[test]
fn local_types_follow_origins_and_specifications_without_inheriting_locations() {
    let mut fixture = Fixture::new();
    let origin_ty = fixture.ty("origin");
    let own_ty = fixture.ty("own");
    let abstract_ = fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[reference_attr(gimli::DW_AT_type, origin_ty), expr(&[0x5e])],
    );
    let specification = fixture.die(
        gimli::DW_TAG_formal_parameter,
        false,
        &[reference_attr(gimli::DW_AT_type, own_ty)],
    );
    fixture.function(100, 200);
    let origin = fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[
            reference_attr(gimli::DW_AT_abstract_origin, abstract_),
            expr(&[0x5a]),
        ],
    );
    let specified = fixture.die(
        gimli::DW_TAG_formal_parameter,
        false,
        &[
            reference_attr(gimli::DW_AT_specification, specification),
            expr(&[0x5b]),
        ],
    );
    let own = fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[
            reference_attr(gimli::DW_AT_abstract_origin, abstract_),
            reference_attr(gimli::DW_AT_type, own_ty),
            expr(&[0x5c]),
        ],
    );
    let preferred = fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[
            reference_attr(gimli::DW_AT_abstract_origin, abstract_),
            reference_attr(gimli::DW_AT_specification, specification),
            expr(&[0x5d]),
        ],
    );
    fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[reference_attr(gimli::DW_AT_abstract_origin, abstract_)],
    );
    let invalid_origin = fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[
            number(gimli::DW_AT_abstract_origin, gimli::DW_FORM_data4, 1),
            reference_attr(gimli::DW_AT_specification, specification),
            expr(&[0x5f]),
        ],
    );
    let cycle = fixture.info.len();
    fixture.die(
        gimli::DW_TAG_variable,
        false,
        &[
            reference_attr(gimli::DW_AT_abstract_origin, cycle),
            expr(&[0x60]),
        ],
    );
    fixture.end();
    let (_, types) = read_fixture(&fixture.finish(), DebugMetadata::TypedLocations);
    let locals = types.locations.unwrap();
    assert_eq!(locals.variable_type(origin), Some(origin_ty));
    assert_eq!(locals.variable_type(specified), Some(own_ty));
    assert_eq!(locals.variable_type(own), Some(own_ty));
    assert_eq!(locals.variable_type(preferred), Some(origin_ty));
    assert_eq!(
        locals.variable_type(invalid_origin),
        None,
        "a present but invalid origin does not fall through to specification"
    );
    assert_eq!(
        locals.variable_type(cycle),
        None,
        "reference cycles stay unknown"
    );
    assert!(
        !locals.registers.contains_key(&14),
        "a concrete DIE does not inherit abstract locations"
    );
}

#[test]
fn noreturn_does_not_inherit_origins_or_specifications_and_conflicts_stay_false() {
    let mut fixture = Fixture::new();
    let abstract_ = fixture.die(
        gimli::DW_TAG_subprogram,
        false,
        &[flag(gimli::DW_AT_noreturn, true)],
    );
    for (pc, attribute) in [
        (100, gimli::DW_AT_abstract_origin),
        (120, gimli::DW_AT_specification),
    ] {
        let mut attrs = range(pc, pc + 4).to_vec();
        attrs.push(reference_attr(attribute, abstract_));
        fixture.die(gimli::DW_TAG_subprogram, false, &attrs);
    }
    for proof in [Some(true), None, Some(false)] {
        let mut attrs = range(140, 144).to_vec();
        if let Some(proof) = proof {
            attrs.push(flag(gimli::DW_AT_noreturn, proof));
        }
        fixture.die(gimli::DW_TAG_subprogram, false, &attrs);
    }
    let mut attrs = range(160, 164).to_vec();
    attrs.push(flag(gimli::DW_AT_noreturn, true));
    fixture.die(gimli::DW_TAG_subprogram, false, &attrs);
    let elf = fixture.finish();
    for mode in [DebugMetadata::Standard, DebugMetadata::TypedLocations] {
        let (dwarf, _) = read_fixture(&elf, mode);
        for pc in [100, 120, 140] {
            assert!(!dwarf.noreturn(Callee::Address(pc)));
        }
        assert!(dwarf.noreturn(Callee::Address(160)));
    }
}

fn local_image(start: u32, register: u8, count: usize) -> Rc<[u8]> {
    let mut fixture = Fixture::new();
    let ty = fixture.ty("word");
    fixture.function(start, start + 16);
    for _ in 0..count {
        fixture.die(
            gimli::DW_TAG_formal_parameter,
            false,
            &[
                reference_attr(gimli::DW_AT_type, ty),
                expr(&[0x50 + register]),
            ],
        );
    }
    fixture.end();
    fixture.finish()
}

#[test]
fn local_metadata_of_two_elfs_with_identical_die_offsets_is_independent() {
    let main = local_image(100, 10, 1);
    let companion = local_image(500, 11, 1);
    let (_, a) = read_fixture(&main, DebugMetadata::TypedLocations);
    let (_, b) = read_fixture(&companion, DebugMetadata::TypedLocations);
    let a = a.locations.unwrap();
    let b = b.locations.unwrap();
    assert_eq!(
        a.variables, b.variables,
        "DIE identities intentionally collide across files"
    );
    assert_eq!(
        (a.registers[&10][0].start, a.registers[&10][0].end),
        (100, 116)
    );
    assert_eq!(
        (b.registers[&11][0].start, b.registers[&11][0].end),
        (500, 516)
    );
    assert!(!a.registers.contains_key(&11));
    assert!(!b.registers.contains_key(&10));
}

struct Units(u64);
impl RunControl for Units {
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.0 = self
            .0
            .checked_sub(units)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "work exhausted"))?;
        Ok(())
    }
}

#[test]
fn requested_local_metadata_obeys_budgets_and_releases_all_reservations() {
    let elf = local_image(100, 10, 64);
    let mut measurements = Vec::new();
    for mode in [DebugMetadata::Standard, DebugMetadata::TypedLocations] {
        let memory = WorkingMemory::new(1 << 24).unwrap();
        let mut control = Units(u64::MAX);
        let mut keep = Ledger::new(&memory, &mut control).unwrap();
        let (_, types) = read(&elf, mode, &mut keep, &mut control).unwrap();
        assert_eq!(
            types.locations.is_some(),
            mode == DebugMetadata::TypedLocations
        );
        assert_eq!(
            memory.used(),
            keep.bytes(),
            "per-unit scopes and temporary pieces were released"
        );
        measurements.push((memory.peak(), u64::MAX - control.0));
        drop(types);
        drop(keep);
        assert_eq!(memory.used(), 0);
    }
    let ((standard_peak, standard_work), (typed_peak, typed_work)) =
        (measurements[0], measurements[1]);
    assert!(typed_peak > standard_peak);
    assert!(typed_work > standard_work);
    for (bytes, units) in [(standard_peak, u64::MAX), (1 << 24, standard_work)] {
        let memory = WorkingMemory::new(bytes).unwrap();
        let mut control = Units(units);
        let mut keep = Ledger::new(&memory, &mut control).unwrap();
        let error = read(&elf, DebugMetadata::TypedLocations, &mut keep, &mut control)
            .err()
            .expect("requested metadata cannot silently fall back");
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        assert_eq!(
            memory.used(),
            keep.bytes(),
            "scratch reservations released on failure"
        );
        drop(keep);
        assert_eq!(memory.used(), 0);
    }
}

#[test]
fn standard_metadata_does_not_decode_unrequested_parameter_locations() {
    let mut fixture = Fixture::new();
    let ty = fixture.ty("word");
    fixture.function(100, 200);
    // An incomplete breg instruction is malformed rather than a supported
    // location. The standard signature reader has no reason to decode it.
    fixture.die(
        gimli::DW_TAG_formal_parameter,
        false,
        &[reference_attr(gimli::DW_AT_type, ty), expr(&[0x72])],
    );
    fixture.end();
    let elf = fixture.finish();
    read_fixture(&elf, DebugMetadata::Standard);
    let error = read(
        &elf,
        DebugMetadata::TypedLocations,
        &mut crate::tests::ledger(),
        &mut || Ok(()),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, ErrorCode::Integrity);
}
