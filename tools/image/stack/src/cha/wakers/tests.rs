use super::*;
use crate::cha::{Type, Variant};
use crate::dwarf::locations::{LocalLocations, Location};
use crate::tests::{Symbol, analyze, debug, executable, ledger};
use crate::{Analysis, Dwarf, Fact, Resolutions, waker_resolutions};
use oer_analysis_riscv_model::{Error, ErrorCode};
use std::collections::{BTreeMap, BTreeSet};

const TEXT: u32 = 0x1000;
const RET: u32 = 0x0000_8067;
const CALL: u32 = 0x0007_80e7;
const TAIL: u32 = 0x0007_8067;
const ADD: u32 = 0x00d5_85b3; // add a1,a1,a3.
const LOAD: u32 = 0x0005_a783; // lw a5,0(a1).

fn record(
    tag: gimli::DwTag,
    size: u64,
    members: &[(&str, u64, usize)],
    target: Option<usize>,
) -> Type {
    Type {
        waker: None,
        has_variants: false,
        discriminant: None,
        variants: Vec::new(),
        tag,
        name: None,
        size: Some(size),
        members: members.iter().map(|(_, at, ty)| (*at, *ty)).collect(),
        named_members: members
            .iter()
            .map(|(name, at, _)| (name.to_string(), *at))
            .collect(),
        target,
    }
}

/// Explicit type/location facts keep machine counterexamples independent of
/// optimizer choices. The compiler regression separately exercises the DIE walk.
fn types(start: u32, end: u32) -> TypeFacts {
    let structure = gimli::DW_TAG_structure_type;
    let pointer = gimli::DW_TAG_pointer_type;
    let mut types = TypeFacts {
        types: BTreeMap::from([
            (1, record(structure, 8, &[("waker", 0, 2)], None)),
            // Deliberately use a different field order from the observed compiler
            // fixture: the vtable piece is derived from layout, never hardcoded.
            (
                2,
                record(structure, 8, &[("data", 0, 7), ("vtable", 4, 4)], None),
            ),
            (
                3,
                record(
                    structure,
                    16,
                    &[
                        ("clone", 0, 6),
                        ("wake", 4, 6),
                        ("wake_by_ref", 8, 6),
                        ("drop", 12, 6),
                    ],
                    None,
                ),
            ),
            (4, record(pointer, 4, &[], Some(3))),
            (5, record(gimli::DW_TAG_subroutine_type, 0, &[], None)),
            (6, record(pointer, 4, &[], Some(5))),
            (7, record(pointer, 4, &[], None)),
            (8, record(structure, 8, &[], None)),
            (9, record(structure, 8, &[("__0", 0, 1)], None)),
            (10, record(structure, 0, &[], None)),
            (11, record(gimli::DW_TAG_base_type, 4, &[], None)),
        ]),
        ..TypeFacts::default()
    };
    types.identify_waker(1, "core::task::wake::Waker");
    types.identify_waker(2, "core::task::wake::RawWaker");
    types.identify_waker(3, "core::task::wake::RawWakerVTable");
    types.identify_waker(8, "core::option::Option<core::task::wake::Waker>");
    let option = types.types.get_mut(&8).unwrap();
    option.has_variants = true;
    option.discriminant = Some((101, Some((4, 11))));
    option.variants = vec![
        Variant {
            supported: true,
            value: Some(0),
            members: vec![(0, 10)],
        },
        Variant {
            supported: true,
            value: None,
            members: vec![(0, 9)],
        },
    ];
    types.locations = Some(LocalLocations {
        variables: BTreeMap::from([(100, (Some(1), None))]),
        registers: BTreeMap::from([(
            13,
            vec![Location {
                start,
                end,
                owner: 100,
                offset: 4,
                bytes: Some(4),
                place: Place::Register(13),
            }],
        )]),
    });
    types
}

struct Fixture {
    analysis: Analysis,
    dwarf: Dwarf,
    types: TypeFacts,
    table: [u32; 4],
    site: u32,
}
impl Fixture {
    fn new(words: Vec<u32>, anchor: u32, site: u32) -> Self {
        let size = words.len() as u32 * 4;
        let elf = executable(&[
            Symbol {
                name: "root",
                words,
                frame: Some(0),
            },
            Symbol {
                name: "clone",
                words: vec![RET],
                frame: Some(0),
            },
            Symbol {
                name: "wake",
                words: vec![RET],
                frame: Some(0),
            },
            Symbol {
                name: "by_ref",
                words: vec![RET],
                frame: Some(0),
            },
            Symbol {
                name: "drop",
                words: vec![RET],
                frame: Some(0),
            },
        ]);
        let analysis = analyze(&elf, &[], &[]).unwrap();
        let (dwarf, _) = debug(&elf);
        let mut types = types(TEXT + anchor, TEXT + site + 4);
        types.source = Some(elf.into());
        Self {
            analysis,
            dwarf,
            types,
            table: [0, 4, 8, 12].map(|at| TEXT + size + at),
            site: TEXT + site,
        }
    }
    fn merged() -> Self {
        Self::new(
            vec![
                0x0005_0663,
                0x0040_0593,
                0x0080_006f,
                0x00c0_0593,
                ADD,
                LOAD,
                CALL,
                RET,
            ],
            16,
            24,
        )
    }
    fn resolve(&self) -> Resolutions {
        waker_resolutions(
            &self.analysis,
            &self.dwarf,
            &self.types,
            Some(&self.table),
            &mut ledger(),
            &mut || Ok(()),
        )
        .unwrap()
    }
    fn location(&mut self) -> &mut Location {
        &mut self
            .types
            .locations
            .as_mut()
            .unwrap()
            .registers
            .get_mut(&13)
            .unwrap()[0]
    }
    fn hole(&self) {
        assert!(self.resolve().get(self.site).is_none());
    }
}

#[test]
fn merged_wake_drop_keeps_both_slots_and_no_assumption() {
    let f = Fixture::merged();
    let resolved = f.resolve();
    let r = resolved.get(f.site).unwrap();
    assert_eq!(r.targets, BTreeSet::from([f.table[1], f.table[3]]));
    assert_eq!(r.facts, BTreeSet::from([Fact::WakerVtables]));
    let bound = f
        .analysis
        .bound_with(TEXT, &resolved, &mut ledger(), &mut || Ok(()))
        .unwrap();
    assert!(bound.bytes.is_some() && bound.unresolved.is_empty() && bound.assumptions.is_empty());
    let error = waker_resolutions(
        &f.analysis,
        &f.dwarf,
        &f.types,
        Some(&f.table),
        &mut ledger(),
        &mut || Err(Error::new(ErrorCode::ResourceLimited, "cancelled")),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
}

#[test]
fn every_byte_from_base_through_jalr_needs_the_same_live_piece() {
    for (start, end) in [(17, 28), (16, 24), (16, 27)] {
        let mut f = Fixture::merged();
        f.location().start = TEXT + start;
        f.location().end = TEXT + end;
        f.hole();
    }
    for gap in [false, true] {
        let mut f = Fixture::merged();
        let mut second = *f.location();
        f.location().end = TEXT + 22;
        second.start = TEXT + 22 + u32::from(gap);
        f.types
            .locations
            .as_mut()
            .unwrap()
            .registers
            .get_mut(&13)
            .unwrap()
            .push(second);
        assert_eq!(f.resolve().get(f.site).is_some(), !gap);
    }
    let mut f = Fixture::merged();
    let mut second = *f.location();
    f.location().end = TEXT + 22;
    second.start = TEXT + 22;
    second.owner = 200;
    let debug = f.types.locations.as_mut().unwrap();
    debug.variables.insert(200, (Some(1), None));
    debug.registers.get_mut(&13).unwrap().push(second);
    f.hole();
}

#[test]
fn another_variable_namespace_or_incomplete_piece_cannot_supply_the_base() {
    let mut f = Fixture::merged();
    f.location().place = Place::Register(14);
    f.hole();
    let mut f = Fixture::merged();
    f.types.identify_waker(1, "application::Waker");
    f.hole();
    for (offset, bytes) in [(0, Some(4)), (4, Some(2)), (4, None)] {
        let mut f = Fixture::merged();
        f.location().offset = offset;
        f.location().bytes = bytes;
        f.hole();
    }
    for place in [Place::Unknown, Place::Memory(13, 0), Place::Pointer(13, 0)] {
        let mut f = Fixture::merged();
        f.location().place = place;
        f.hole();
    }
}

#[test]
fn non_slots_and_nonconstant_or_unsupported_arithmetic_remain_unknown() {
    for constant in [2, 6, 16, 32] {
        Fixture::new(vec![(constant << 20) | 0x593, ADD, LOAD, CALL, RET], 4, 12).hole();
    }
    // add a1,a2,a3; sub a1,a1,a3; mul a1,a1,a3; an incoming path skips li.
    for words in [
        vec![0x00d6_05b3, LOAD, CALL, RET],
        vec![0x0040_0593, 0x40d5_85b3, LOAD, CALL, RET],
        vec![0x0040_0593, 0x02d5_85b3, LOAD, CALL, RET],
        vec![0x0005_0463, 0x0040_0593, ADD, LOAD, CALL, RET],
    ] {
        let site = (words.len() as u32 - 2) * 4;
        Fixture::new(words, 0, site).hole();
    }
}

#[test]
fn addi_copies_raw_wakers_and_every_exact_slot_are_supported() {
    for slot in 0..4 {
        // mv a2,a3; addi a1,a2,slot*4; lw a5,0(a1); jalr a5.
        let mut f = Fixture::new(
            vec![0x0006_8613, ((slot * 4) << 20) | 0x60593, LOAD, CALL, RET],
            0,
            12,
        );
        f.types
            .locations
            .as_mut()
            .unwrap()
            .variables
            .insert(100, (Some(2), None));
        assert_eq!(
            f.resolve().get(f.site).unwrap().targets,
            BTreeSet::from([f.table[slot as usize]])
        );
    }
}

#[test]
fn option_requires_a_described_null_niche_and_nonzero_on_every_path() {
    // beq a3,zero,ret; li a1,4; add a1,a1,a3; load; call; ret.
    for bypass in [false, true] {
        let words = if bypass {
            vec![0x0005_0463, 0x0006_8a63, 0x0040_0593, ADD, LOAD, CALL, RET]
        } else {
            vec![0x0006_8a63, 0x0040_0593, ADD, LOAD, CALL, RET]
        };
        let site = (words.len() as u32 - 2) * 4;
        let mut f = Fixture::new(words, 0, site);
        f.types
            .locations
            .as_mut()
            .unwrap()
            .variables
            .insert(100, (Some(8), None));
        assert_eq!(f.resolve().get(f.site).is_some(), !bypass);
        f.types.types.get_mut(&8).unwrap().discriminant = None;
        f.hole();
    }
    let mut f = Fixture::merged();
    f.types
        .locations
        .as_mut()
        .unwrap()
        .variables
        .insert(100, (Some(8), None));
    f.hole();
}

#[test]
fn two_open_tails_are_still_unresolved() {
    let f = Fixture::new(
        vec![
            0x0005_0a63,
            0x0040_0593,
            ADD,
            LOAD,
            TAIL,
            0x00c0_0593,
            ADD,
            LOAD,
            TAIL,
        ],
        0,
        16,
    );
    let resolved = f.resolve();
    assert!(resolved.get(TEXT + 16).is_none() && resolved.get(TEXT + 32).is_none());
    assert!(f.analysis.functions[&TEXT].target_predecessors.is_empty());
}

#[test]
fn a_foreign_code_extent_supplies_no_typed_location_proof() {
    let mut f = Fixture::merged();
    f.analysis
        .functions
        .get_mut(&TEXT)
        .unwrap()
        .function
        .section = usize::MAX;
    f.hole();
}

#[test]
fn typed_proofs_do_not_cross_memory_writes_calls_or_cycles() {
    for words in [
        // t0 keeps the Waker base, but intervening effects are barriers.
        vec![0x0006_8293, 0x0006_2023, 0x0042_8593, LOAD, CALL, RET],
        vec![0x0006_8293, 0x0040_00ef, 0x0042_8593, LOAD, CALL, RET],
        // One incoming path loops with an unknown offset.
        vec![0x0005_0463, 0xffdf_f06f, ADD, LOAD, CALL, RET],
    ] {
        let site = (words.len() as u32 - 2) * 4;
        Fixture::new(words, 0, site).hole();
    }
}

#[test]
fn cancellation_releases_typed_address_scratch() {
    use oer_analysis_riscv_model::{Ledger, WorkingMemory};
    let f = Fixture::merged();
    let source = f.types.source.as_ref().unwrap();
    let file = oer_analysis_elf::Elf::parse(source).unwrap();
    let facts = &f.analysis.functions[&TEXT];
    let load = &facts.target_loads[&f.site][0];
    let memory = WorkingMemory::new(1 << 20).unwrap();
    let mut checkpoints = 0;
    {
        let mut keep = Ledger::new(&memory, &mut || Ok(())).unwrap();
        crate::waker_locations::slots(&file, facts, &f.types, load, f.site, &mut keep, &mut || {
            checkpoints += 1;
            Ok(())
        })
        .unwrap();
    }
    assert_eq!(memory.used(), 0);
    for stop in 0..checkpoints {
        let mut left = stop;
        {
            let mut keep = Ledger::new(&memory, &mut || Ok(())).unwrap();
            let result = crate::waker_locations::slots(
                &file,
                facts,
                &f.types,
                load,
                f.site,
                &mut keep,
                &mut || {
                    if left == 0 {
                        return Err(Error::new(ErrorCode::ResourceLimited, "cancelled"));
                    }
                    left -= 1;
                    Ok(())
                },
            );
            assert_eq!(result.unwrap_err().code, ErrorCode::ResourceLimited);
        }
        assert_eq!(memory.used(), 0, "checkpoint {stop}");
    }
}

#[test]
fn a_missing_census_or_inconsistent_layout_cannot_resolve_a_typed_call() {
    let f = Fixture::merged();
    let resolutions = waker_resolutions(
        &f.analysis,
        &f.dwarf,
        &f.types,
        None,
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    assert!(resolutions.get(f.site).is_none_or(|r| !r.resolves()));
    let mut f = Fixture::merged();
    f.types
        .types
        .get_mut(&3)
        .unwrap()
        .named_members
        .insert("wake".into(), 8);
    f.hole();
    let mut f = Fixture::merged();
    f.types.types.get_mut(&2).unwrap().has_variants = true;
    f.hole();
}
