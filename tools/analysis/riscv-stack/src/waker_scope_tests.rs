use super::*;

/// Add tiny DWARF4 scopes to an executable fixture. Its Waker inline range
/// deliberately ends just before the call, as optimized firmware can do.
fn scopes(mut elf: Vec<u8>, load: u32, call: u32) -> Vec<u8> {
    fn word(bytes: &[u8], at: usize) -> usize {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
    }
    fn put(bytes: &mut [u8], at: usize, value: usize) {
        bytes[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
    }
    let old_table = word(&elf, 32);
    let count = usize::from(u16::from_le_bytes(elf[48..50].try_into().unwrap()));
    let strings = usize::from(u16::from_le_bytes(elf[50..52].try_into().unwrap()));
    let mut headers = elf[old_table..old_table + count * 40].to_vec();
    let string_header = strings * 40;
    let old_strings = word(&headers, string_header + 16);
    let old_size = word(&headers, string_header + 20);
    let mut names = elf[old_strings..old_strings + old_size].to_vec();
    let mut info = vec![0; 4];
    info.extend_from_slice(&4_u16.to_le_bytes());
    info.extend_from_slice(&0_u32.to_le_bytes());
    info.push(4); // RV32 address size.
    info.push(1); // CU: name, low_pc, high_pc (length).
    info.extend_from_slice(b"producer-scopes\0");
    info.extend_from_slice(&TEXT.to_le_bytes());
    info.extend_from_slice(&0x100_u32.to_le_bytes());
    let origin = info.len() as u32;
    info.push(2); // Abstract Waker method, with a linkage name.
    info.extend_from_slice(b"<core::task::wake::Waker>::wake\0");
    info.push(3); // Concrete root, children follow.
    info.extend_from_slice(b"root\0");
    info.extend_from_slice(&TEXT.to_le_bytes());
    info.extend_from_slice(&(call + 8 - TEXT).to_le_bytes());
    info.push(4); // Inline method: origin, low_pc, high_pc (length).
    info.extend_from_slice(&origin.to_le_bytes());
    info.extend_from_slice(&load.to_le_bytes());
    info.extend_from_slice(&(call - load).to_le_bytes());
    info.extend_from_slice(&[0, 0]); // Root/CU children end.
    let length = (info.len() - 4) as u32;
    info[..4].copy_from_slice(&length.to_le_bytes());
    // DW_AT_name / linkage_name / low_pc / high_pc / abstract_origin;
    // DW_FORM_string / addr / data4 / ref4.
    let abbrev = [
        1, 0x11, 1, 0x03, 0x08, 0x11, 0x01, 0x12, 0x06, 0, 0, 2, 0x2e, 0, 0x6e, 0x08, 0, 0, 3,
        0x2e, 1, 0x03, 0x08, 0x11, 0x01, 0x12, 0x06, 0, 0, 4, 0x1d, 0, 0x31, 0x13, 0x11, 0x01,
        0x12, 0x06, 0, 0, 0,
    ];
    for (name, data) in [
        (".debug_abbrev", abbrev.as_slice()),
        (".debug_info", info.as_slice()),
    ] {
        let name_at = names.len();
        names.extend_from_slice(name.as_bytes());
        names.push(0);
        let mut header = vec![0; 40];
        put(&mut header, 0, name_at);
        put(&mut header, 4, 1); // SHT_PROGBITS.
        put(&mut header, 16, elf.len());
        put(&mut header, 20, data.len());
        put(&mut header, 32, 1);
        elf.extend_from_slice(data);
        headers.extend(header);
    }
    put(&mut headers, string_header + 16, elf.len());
    put(&mut headers, string_header + 20, names.len());
    elf.extend(names);
    let table = elf.len();
    elf.extend(headers);
    put(&mut elf, 32, table);
    elf[48..50].copy_from_slice(&((count + 2) as u16).to_le_bytes());
    elf
}

fn fixture(words: Vec<u32>, load: u32, call: u32) -> (Analysis, Dwarf, [u32; 4]) {
    let root_size = words.len() as u32 * 4;
    let elf = scopes(
        executable(&[
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
                words: vec![SP_DOWN_32, SP_UP_32, RET],
                frame: Some(32),
            },
            Symbol {
                name: "by_ref",
                words: vec![SP_DOWN_16, SP_UP_16, RET],
                frame: Some(16),
            },
            Symbol {
                name: "drop",
                words: vec![RET],
                frame: Some(0),
            },
        ]),
        load,
        call,
    );
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let (dwarf, _) = debug(&elf);
    let targets = [0, 4, 16, 28].map(|offset| TEXT + root_size + offset);
    (analysis, dwarf, targets)
}

const LOAD_WAKE: u32 = 0x0045_2783; // lw a5, 4(a0).

#[test]
fn a_waker_scope_ending_before_jalr_is_proved_by_its_exclusive_load() {
    let (analysis, dwarf, targets) = fixture(vec![LOAD_WAKE, CALL_A5, RET], TEXT, TEXT + 4);
    assert_eq!(
        dwarf
            .inline_chain(TEXT)
            .unwrap()
            .first()
            .map(String::as_str),
        Some("<core::task::wake::Waker>::wake")
    );
    assert_eq!(dwarf.inline_chain(TEXT + 4).unwrap(), ["root"]);
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(
        resolutions.get(TEXT + 4).unwrap().targets,
        BTreeSet::from([targets[1]])
    );
    assert_eq!(
        analysis.bound_with(TEXT, &resolutions).unwrap().bytes,
        Some(32)
    );
}

#[test]
fn a_branch_bypassing_the_load_cannot_borrow_its_waker_scope() {
    // beq a0, zero, +8 bypasses the Waker load and enters the call.
    let (analysis, dwarf, targets) = fixture(
        vec![0x0005_0463, LOAD_WAKE, CALL_A5, RET],
        TEXT + 4,
        TEXT + 8,
    );
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.get(TEXT + 8).is_none());
    assert!(
        analysis
            .bound_with(TEXT, &resolutions)
            .unwrap()
            .bytes
            .is_none()
    );
}

#[test]
fn a_different_vtable_slot_is_not_the_waker_method_the_scope_names() {
    let (analysis, dwarf, targets) = fixture(vec![0x0085_2783, CALL_A5, RET], TEXT, TEXT + 4);
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.is_empty());
}

#[test]
fn an_adjusted_function_pointer_does_not_inherit_the_producers_scope() {
    let (analysis, dwarf, targets) = fixture(vec![LOAD_WAKE, 0x0027_80e7, RET], TEXT, TEXT + 4);
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.is_empty());
}
