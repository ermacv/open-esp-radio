//! Expression identity, admitted ownership and measured interning complexity.
use super::*;
#[derive(Default)]
struct Meter(u64);
impl RunControl for Meter {
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.0 += units;
        Ok(())
    }
}
fn expression(value: u32) -> Expression {
    Expression::Integer {
        op: IntegerOp::Add,
        left: AbstractValue::Expression { id: 0 },
        right: AbstractValue::Constant { value },
    }
}
#[test]
fn full_keys_survive_collisions_growth_and_keep_site_provenance() {
    let memory = WorkingMemory::new(4 * 1024 * 1024).unwrap();
    let mut c = Meter::default();
    let mut symbols = Symbols::new(2048, &memory, &mut c).unwrap();
    let first = expression(0);
    let bucket = Symbols::hash(12, &first, &mut c).unwrap() & 63;
    let second = (1..10000)
        .map(expression)
        .find(|e| Symbols::hash(12, e, &mut c).unwrap() & 63 == bucket)
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(
        symbols.expression(12, first.clone(), &mut c).unwrap(),
        Value::Expr(0)
    );
    assert_eq!(
        symbols.expression(12, second.clone(), &mut c).unwrap(),
        Value::Expr(1)
    );
    assert_eq!(
        symbols.expression(16, first.clone(), &mut c).unwrap(),
        Value::Expr(2)
    );
    for n in 0..2048 {
        assert_eq!(
            symbols
                .expression(20 + n as u64 * 4, expression(n), &mut c)
                .unwrap(),
            Value::Expr(n + 3)
        );
    }
    for (id, site, expr) in [(0, 12, first.clone()), (1, 12, second), (2, 16, first)] {
        assert_eq!(
            symbols.expression(site, expr.clone(), &mut c).unwrap(),
            Value::Expr(id)
        );
        assert_eq!(symbols.records[id as usize], (site, expr));
    }
    drop(symbols);
    assert_eq!(memory.used(), 0);
}
#[test]
fn expression_work_grows_below_threefold_per_doubling() {
    let mut previous = None;
    for count in [512, 1024, 2048] {
        let memory = WorkingMemory::new(4 * 1024 * 1024).unwrap();
        let mut c = Meter::default();
        let mut symbols = Symbols::new(count, &memory, &mut c).unwrap();
        for n in 0..count {
            symbols
                .expression(n as u64 * 4, expression(n as u32), &mut c)
                .unwrap();
        }
        if let Some(previous) = previous {
            assert!(c.0 < previous * 3, "{count}: {} / {previous}", c.0);
        }
        eprintln!("expressions={count}, work={}", c.0);
        previous = Some(c.0);
        drop(symbols);
        assert_eq!(memory.used(), 0);
    }
}
#[test]
fn index_admission_cancellation_and_record_failure_preserve_usable_owner() {
    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let mut c = Meter::default();
    let mut symbols = Symbols::new(64, &memory, &mut c).unwrap();
    for n in 0..32 {
        symbols.expression(n, expression(n as u32), &mut c).unwrap();
    }
    let before = memory.used();
    // Fail partway through replacement-index construction.
    let mut calls = 0;
    let mut cancel = || {
        calls += 1;
        if calls == 20 {
            Err(Error::new(ErrorCode::Cancelled, "test cancellation"))
        } else {
            Ok(())
        }
    };
    assert_eq!(
        symbols
            .expression(128, expression(32), &mut cancel)
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(memory.used(), before);
    assert_eq!(symbols.records.len(), 32);
    let hold = memory.reserve(1024 * 1024 - before, c.position()).unwrap();
    assert_eq!(
        symbols
            .expression(128, expression(32), &mut c)
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimited
    );
    assert_eq!(
        symbols.expression(0, expression(0), &mut c).unwrap(),
        Value::Expr(0)
    );
    drop(hold);
    assert_eq!(memory.used(), before);
    assert_eq!(
        symbols.expression(128, expression(32), &mut c).unwrap(),
        Value::Expr(32)
    );
    drop(symbols);
    assert_eq!(memory.used(), 0);

    let mut symbols = Symbols::new(64, &memory, &mut c).unwrap();
    for n in 0..8 {
        symbols.expression(n, expression(n as u32), &mut c).unwrap();
    }
    // Pre-admit payload-vector capacity; then deny only late record growth.
    symbols
        .payloads
        .push(memory.reserve(0, c.position()).unwrap(), c.position())
        .unwrap();
    symbols.payloads.pop();
    let payload_expression = Expression::Load {
        address: AbstractValue::Alternatives {
            values: ValueAlternatives::new(vec![
                ValueAlternative::Constant { value: 1 },
                ValueAlternative::Constant { value: 2 },
            ])
            .unwrap(),
        },
        width: 4,
        signed: false,
    };
    let before = memory.used();
    let hold = memory
        .reserve(
            1024 * 1024 - before - payload_expression.allocated_bytes(),
            c.position(),
        )
        .unwrap();
    assert_eq!(
        symbols
            .expression(8, payload_expression, &mut c)
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimited
    );
    assert_eq!(symbols.records.len(), 8);
    assert_eq!(symbols.payloads.len(), 8);
    drop(hold);
    assert_eq!(memory.used(), before);
    assert_eq!(
        symbols.expression(8, expression(8), &mut c).unwrap(),
        Value::Expr(8)
    );
    symbols.limit = symbols.records.len();
    assert_eq!(
        symbols.expression(8, expression(8), &mut c).unwrap(),
        Value::Expr(8)
    );
    assert_eq!(
        symbols
            .expression(9, expression(9), &mut c)
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimited
    );
    drop(symbols);
    assert_eq!(memory.used(), 0);
}
