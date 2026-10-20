use super::alignment::{andi, branch};
use super::*;

struct Fixture {
    words: Vec<u32>,
    guard: usize,
    bulk: Vec<usize>,
    back: usize,
}
fn add(dest: u32, left: u32, right: u32) -> u32 {
    (right << 20) | (left << 15) | (dest << 7) | 0x33
}
fn sub(dest: u32, left: u32, right: u32) -> u32 {
    add(dest, left, right) | (0x20 << 25)
}
fn fixture(test: u32, extra_entry: bool) -> Fixture {
    let mut w = vec![add(13, 10, 12), andi(14, 10, 3), 0, addi(15, 10, 0), 0];
    let merge = w.len();
    w.extend([andi(14, 13, !3), sub(16, 14, 15), addi(12, 0, 32)]);
    let guard = w.len();
    w.push(0);
    let extra = w.len();
    if extra_entry {
        w.push(0);
    }
    w.extend([store(11, 15, 0, 0), store(11, 15, 4, 2), RET]);
    let init = w.len();
    w.extend([addi(15, 10, 0), 0]);
    let body = w.len();
    w.extend([
        (lw(14, 11, 0) & !(7 << 12)) | (4 << 12),
        addi(15, 15, 1),
        addi(11, 11, 1),
        store(14, 15, -1, 0),
    ]);
    let align_guard = w.len();
    w.extend([
        andi(14, 15, 3),
        branch(14, 0, ((body as i32) - (align_guard as i32 + 1)) * 4, 1),
        jal(0, ((merge as i32) - (align_guard as i32 + 2)) * 4),
    ]);
    let bulk_entry = w.len();
    let mut bulk = Vec::new();
    for delta in 0..9 {
        bulk.push(w.len());
        w.push(store(11, 15, delta * 4, 2));
    }
    w.push(addi(15, 15, 36));
    let back = w.len();
    w.extend([
        branch(15, 14, ((bulk_entry as i32) - (back as i32)) * 4, 1),
        RET,
    ]);
    w[2] = branch(14, 0, ((init as i32) - 2) * 4, 1);
    w[4] = jal(0, ((merge as i32) - 4) * 4);
    w[init + 1] = jal(0, ((align_guard as i32) - (init as i32 + 1)) * 4);
    w[guard] = branch(12, 16, ((bulk_entry as i32) - (guard as i32)) * 4, test);
    if extra_entry {
        w[extra] = branch(17, 0, ((bulk_entry as i32) - (extra as i32)) * 4, 1);
    }
    Fixture {
        words: w,
        guard,
        bulk,
        back,
    }
}
fn caller(
    length: Option<u32>,
    destination: Option<u32>,
    target: u32,
    memory: &WorkingMemory,
) -> RecordBuffer<'_> {
    let mut words = Vec::new();
    for (register, value) in [(10, destination), (12, length)] {
        if let Some(value) = value {
            words.push((value.wrapping_add(0x800) & 0xfffff000) | (register << 7) | 0x37);
            words.push(addi(register, register, ((value << 20) as i32) >> 20));
        }
    }
    words.push(jal(1, target as i32 - (0x1000 + words.len() as i32 * 4)));
    words.push(RET);
    facts(&words, 0x1000, memory)
}
fn call_site(records: &[FunctionRecord]) -> u64 {
    records
        .iter()
        .find_map(|r| match r {
            FunctionRecord::Transfer {
                offset, call: true, ..
            } => Some(*offset),
            _ => None,
        })
        .unwrap()
}
fn specialized<'m>(
    f: &Fixture,
    entry: u64,
    context: &CallContext,
    memory: &'m WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<FunctionWrites<'m>> {
    let code: Vec<_> = f.words.iter().flat_map(|w| w.to_le_bytes()).collect();
    contextual_function_writes(
        FunctionInput {
            image: Some(&NoMemory),
            section: 1,
            extent: CodeRange {
                start: entry,
                length: code.len() as u64,
            },
            bytes: &code,
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        },
        &RiscvDecoder,
        context,
        &[],
        memory,
        control,
    )
}
#[test]
fn short_call_excludes_all_bulk_stores_only_under_proved_entry_length() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let f = fixture(4, false);
    let generic_records = facts(&f.words, 0x3000, &memory);
    let generic = summary(&generic_records, 0x3000, &memory);
    assert_eq!(generic.writes.len(), 14);
    let trampoline_records = facts(&[jal(0, 0x1000)], 0x2000, &memory);
    let trampoline = summary_with_callees(&trampoline_records, 0x2000, &[&generic], &memory);
    let caller_records = caller(Some(20), None, 0x2000, &memory);
    let site = call_site(&caller_records);
    let caller_index = index(&caller_records, &memory);
    let context = call_context(
        &caller_index,
        0x1000,
        site,
        ABI,
        &trampoline,
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(context.entry(), 0x3000);
    assert_eq!(context.arguments()[2], Some(20));
    let specialized = specialized(&f, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    assert_eq!(specialized.writes.as_ref(), generic.writes.as_ref());
    let proof = specialized.specialization().unwrap();
    let branch = proof
        .branches()
        .iter()
        .find(|b| b.site == 0x3000 + f.guard as u64 * 4)
        .unwrap();
    assert_eq!(&branch.left[..branch.left_count as usize], &[32]);
    assert_eq!(&branch.right[..branch.right_count as usize], &[16, 20]);
    assert!(!branch.taken);
    for bulk in &f.bulk {
        assert!(proof.unreachable().contains(&(0x3000 + *bulk as u64 * 4)));
    }
    assert!(proof.unreachable().contains(&(0x3000 + f.back as u64 * 4)));
    let trampoline = summary_with_callees(&trampoline_records, 0x2000, &[&specialized], &memory);
    let applied = call_writes(
        &caller_index,
        0x1000,
        site,
        ABI,
        &[&trampoline],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(applied.writes.len(), 5);
    assert_eq!(
        applied
            .writes
            .iter()
            .filter(|w| matches!(w.write.proof, WriteProof::AlignmentLoop(_)))
            .count(),
        3
    );
    assert!(
        !applied
            .preserves_stack_word(0x1000, -600, &mut || Ok(()))
            .unwrap()
    );
    for length in [None, Some(21), Some(128)] {
        let changed = caller(length, None, 0x2000, &memory);
        let applied = call_writes(
            &index(&changed, &memory),
            0x1000,
            call_site(&changed),
            ABI,
            &[&trampoline],
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(applied.writes.len(), 14);
    }
}
