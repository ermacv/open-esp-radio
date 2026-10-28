use super::*;

fn merged_program(source: &str) -> String {
    source.replace(
        "pub extern \"C\" fn _start(waker: &Waker) {\n    waker.wake_by_ref();\n}",
        r#"pub extern "C" fn _start(slot: &mut Slot, current: &Waker) -> u32 {
    let previous = core::mem::replace(&mut slot.waker, Some(current.clone()));
    slot.count = slot.count.wrapping_add(1);
    wake_replaced(previous, current);
    core::hint::black_box(slot.count)
}

pub struct Slot { count: u32, waker: Option<Waker> }

#[inline(always)]
fn wake_replaced(previous: Option<Waker>, current: &Waker) {
    if let Some(previous) = previous && !previous.will_wake(current) {
        previous.wake();
    }
}"#,
    )
}

fn typed_debug(elf: &[u8]) -> (Dwarf, TypeFacts) {
    read_debug(
        &std::rc::Rc::from(elf),
        DebugMetadata::TypedLocations,
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap()
}

#[test]
fn pre_823_compiler_shape_proves_both_merged_slots_without_method_scopes() {
    let elf = compiled(&merged_program(WAKER_PROGRAM));
    let (dwarf, types) = typed_debug(&elf);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let table = census(&elf, &dwarf, &types, &analysis).unwrap().unwrap();
    let standard = waker_resolutions(
        &analysis,
        &dwarf,
        &TypeFacts::default(),
        Some(&table),
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    let resolved = waker_resolutions(
        &analysis,
        &dwarf,
        &types,
        Some(&table),
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    let merged = analysis
        .functions
        .values()
        .flat_map(|f| &f.transfers)
        .filter(|t| {
            t.target.is_none()
                && standard.get(t.site).is_none()
                && resolved
                    .get(t.site)
                    .is_some_and(|r| r.targets == BTreeSet::from([table[1], table[3]]))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        merged.len(),
        1,
        "standard={standard:x?} typed={resolved:x?}"
    );
    let site = merged[0].site;
    assert!(dwarf.inline_chain(site).unwrap()[0].contains("wake_replaced"));
    let root = analysis
        .functions
        .values()
        .find(|f| f.function.names.iter().any(|n| n == "_start"))
        .unwrap();
    let bound = analysis
        .bound_with(root.function.address, &resolved, &mut ledger(), &mut || {
            Ok(())
        })
        .unwrap();
    assert!(bound.unresolved.is_empty(), "{bound:?}");
    assert!(bound.assumptions.is_empty());

    // This failure is about coverage, not the continued presence of a type.
    let (_, mut shortened) = typed_debug(&elf);
    for locations in shortened.locations.as_mut().unwrap().registers.values_mut() {
        for location in locations {
            location.end = location.end.min(site);
        }
    }
    let unresolved = waker_resolutions(
        &analysis,
        &dwarf,
        &shortened,
        Some(&table),
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    assert!(unresolved.get(site).is_none());

    // Option alone needs its described niche and a machine proof of Some.
    let (_, mut option_only) = typed_debug(&elf);
    let debug = option_only.locations.as_mut().unwrap();
    // Keep only the helper's formal parameter (the Option<Waker>); the
    // concrete Waker local has the same piece but a different type identity.
    let mut option_owners = BTreeSet::new();
    for (&register, locations) in &debug.registers {
        for location in locations {
            if location.start <= site
                && site < location.end
                && option_only_owner(
                    &types,
                    register,
                    location.owner,
                    location.start,
                    location.end,
                )
            {
                option_owners.insert(location.owner);
            }
        }
    }
    for locations in debug.registers.values_mut() {
        locations.retain(|l| option_owners.contains(&l.owner));
    }
    let option_resolved = waker_resolutions(
        &analysis,
        &dwarf,
        &option_only,
        Some(&table),
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    assert!(
        option_resolved.get(site).is_some(),
        "Option locations must independently prove the niche: {option_owners:?}"
    );
}

fn option_only_owner(types: &TypeFacts, register: u8, owner: usize, start: u32, end: u32) -> bool {
    // Isolate one owner in a fresh read rather than inferring its type from
    // a variable name. The typed query reports whether a Some proof is needed.
    let (_, mut single) = typed_debug(types.source.as_ref().unwrap());
    for locations in single.locations.as_mut().unwrap().registers.values_mut() {
        locations.retain(|l| l.owner == owner);
    }
    single
        .waker_register(register, start, end, &mut || Ok(()))
        .unwrap()
        == Some(true)
}

#[test]
fn merged_compiler_fixture_with_two_vtables_fails_the_census() {
    let source = merged_program(&second_vtable_program());
    let elf = compiled_linking(&source, &["--undefined=keep_other"], "s");
    let (dwarf, types) = typed_debug(&elf);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    assert_eq!(
        census(&elf, &dwarf, &types, &analysis).unwrap_err().code,
        ErrorCode::Integrity
    );
}

#[test]
fn extracted_wake_replaced_with_two_compiler_tails_remains_unresolved() {
    let elf = compiled_linking(
        &merged_program(WAKER_PROGRAM).replace("#[inline(always)]", "#[inline(never)]"),
        &[],
        "3",
    );
    let (dwarf, types) = typed_debug(&elf);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let table = census(&elf, &dwarf, &types, &analysis).unwrap().unwrap();
    let helper = analysis
        .functions
        .values()
        .find(|f| f.function.names.iter().any(|n| n.contains("wake_replaced")))
        .unwrap();
    let tails: Vec<_> = helper
        .transfers
        .iter()
        .filter(|t| t.target.is_none() && t.kind == TransferKind::Tail)
        .collect();
    assert_eq!(tails.len(), 2, "{helper:x?}");
    let resolved = waker_resolutions(
        &analysis,
        &dwarf,
        &types,
        Some(&table),
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    assert!(tails.iter().all(|t| resolved.get(t.site).is_none()));
}
