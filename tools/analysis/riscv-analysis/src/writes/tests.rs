use super::*;

#[test]
fn cancellation_and_allocation_failure_release_summary_scratch() {
    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let records = [FunctionRecord::Expression {
        id: 0,
        offset: u64::MAX,
        expression: Expression::EntryRegister { register: 10 },
    }];
    let facts =
        FunctionIndex::new(&records, RecordPolicy::Canonical, &memory, &mut || Ok(())).unwrap();
    let before = memory.used();
    let mut calls = 0;
    let result = function_writes(&facts, 0, Some(CallAbi::RiscvInteger), &memory, &mut || {
        calls += 1;
        if calls == 2 {
            Err(Error::new(ErrorCode::Cancelled, "test cancellation"))
        } else {
            Ok(())
        }
    });
    assert_eq!(result.err().unwrap().code, ErrorCode::Cancelled);
    assert_eq!(memory.used(), before);
    let hold = memory
        .reserve(1024 * 1024 - before, RunPosition::default())
        .unwrap();
    assert_eq!(
        function_writes(&facts, 0, Some(CallAbi::RiscvInteger), &memory, &mut || Ok(
            ()
        ))
        .err()
        .unwrap()
        .code,
        ErrorCode::ResourceLimited
    );
    drop(hold);
    assert_eq!(memory.used(), before);
}
