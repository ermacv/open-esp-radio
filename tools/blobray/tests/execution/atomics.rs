use super::*;

pub(super) fn op(function: u32, ordering: u32, dest: u32, base: u32, src: u32) -> u32 {
    function << 27 | ordering << 25 | src << 20 | base << 15 | 2 << 12 | dest << 7 | 0x2f
}
pub(super) fn scenario(f: &Fixture, value: Option<u32>) -> ExecutionRequest {
    let mut request = f.request();
    request.cases[0].vendor.arguments = vec![Some(0x3000), Some(5)];
    request.cases[0].vendor.memory.push(ram(MemorySeed {
        address: 0x3000,
        length: 8,
        fill: None,
        bytes: value.map(|n| n.to_le_bytes().to_vec()).unwrap_or_default(),
    }));
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    request
}

#[test]
fn every_word_amo_and_ordering_returns_old_value_and_updates_memory() {
    for (function, old, operand, expected) in [
        (1, 9, 5, 5u32),     // swap
        (0, u32::MAX, 5, 4), // add wraps
        (4, 0xaa, 0x0f, 0xa5),
        (12, 0xaa, 0x0f, 0x0a),
        (8, 0xaa, 0x0f, 0xaf),
        (16, 0x80000000, 5, 0x80000000),
        (20, 0x80000000, 5, 5),
        (24, 0x80000000, 5, 5),
        (28, 0x80000000, 5, 0x80000000),
    ] {
        for ordering in 0..4 {
            // amo t0,a1,(a0); lw a1,0(a0); mv a0,t0; ret
            let f = Fixture::new(&[
                op(function, ordering, 5, 10, 11),
                0x00052583,
                0x00028513,
                0x00008067,
            ]);
            let mut request = scenario(&f, Some(old));
            request.cases[0].vendor.arguments[1] = Some(operand);
            request.cases[0].replacement = Some(request.cases[0].vendor.clone());
            let run = f.run(request, budget()).unwrap();
            let result = run.facts();
            assert_eq!(result["verdict"], "MATCH");
            assert_eq!(result["records"][0]["stop"]["low"], old);
            assert_eq!(result["records"][0]["stop"]["high"], expected);
            // Outcome, outcome and comparison, then one coverage record per side.
            assert_eq!(
                result["records"].as_array().unwrap().len(),
                5,
                "RAM atomics must not emit MMIO events"
            );
        }
    }
}

#[test]
fn lr_sc_status_follows_the_reservation_across_phases() {
    // a1 nonzero: reserve/return. a1 zero: SC returns its status, then read memory.
    let f = Fixture::new(&[
        0x00058663,
        op(2, 2, 5, 10, 0),
        0x00008067,
        op(3, 1, 5, 10, 11),
        0x00052583,
        0x00028513,
        0x00008067,
    ]);
    let mut request = scenario(&f, Some(9));
    let mut second = request.cases[0].clone();
    second.reset = SessionReset::Warm;
    second.name = "no-reservation-crosses-phase".into();
    second.vendor.arguments[1] = Some(0);
    second.vendor.memory.clear();
    second.replacement = Some(second.vendor.clone());
    request.cases.push(second);
    let run = f.run(request, budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["records"][3]["stop"]["low"], 1);
    assert_eq!(result["records"][3]["stop"]["high"], 9);

    // Successful SC writes once and clears its reservation; the next SC fails.
    for twice in [false, true] {
        let mut code = vec![op(2, 3, 5, 10, 0), op(3, 3, 5, 10, 11)];
        if twice {
            code.push(op(3, 0, 5, 10, 11));
        }
        code.extend([0x00052583, 0x00028513, 0x00008067]);
        let f = Fixture::new(&code);
        let run = f.run(scenario(&f, Some(9)), budget()).unwrap();
        let result = run.facts();
        assert_eq!(result["records"][0]["stop"]["low"], u32::from(twice));
        assert_eq!(result["records"][0]["stop"]["high"], 5);
    }
}

#[test]
fn atomic_unknowns_permissions_alignment_and_mmio_do_not_fallback() {
    for instruction in [op(2, 0, 5, 10, 0), op(3, 0, 5, 10, 11), op(0, 0, 5, 10, 11)] {
        let f = Fixture::new(&[instruction, 0x00008067]);
        for address in [0x3001, 0x5000, 0x6000] {
            let mut request = scenario(&f, Some(9));
            request.cases[0].vendor.arguments[0] = Some(address);
            request.cases[0]
                .vendor
                .models
                .push(register_bank(vec![RegisterCell {
                    address: 0x6000,
                    width: 4,
                    value: 9,
                }]));
            request.cases[0].replacement = Some(request.cases[0].vendor.clone());
            let run = f.run(request, budget()).unwrap();
            let result = run.facts();
            assert_eq!(result["verdict"], "INCOMPLETE");
            assert_eq!(result["records"][1]["stop"]["reason"]["access"], "atomic");
            assert_eq!(result["records"][1]["stop"]["reason"]["address"], address);
        }
    }
    for instruction in [op(2, 0, 5, 10, 0), op(0, 0, 5, 10, 11)] {
        let f = Fixture::new(&[instruction, 0x00008067]);
        let run = f.run(scenario(&f, None), budget()).unwrap();
        assert_eq!(run.facts()["verdict"], "INCOMPLETE");
    }
    for instruction in [op(3, 0, 5, 10, 11), op(0, 0, 5, 10, 11)] {
        let f = Fixture::new(&[instruction, 0x00008067]);
        let mut request = scenario(&f, Some(9));
        request.cases[0].vendor.arguments[0] = Some(0x1000); // read-only executable mapping
        request.cases[0].replacement = Some(request.cases[0].vendor.clone());
        let run = f.run(request, budget()).unwrap();
        assert_eq!(run.facts()["verdict"], "INCOMPLETE");
    }
}

#[test]
fn atomic_effect_comparison_reads_the_changed_word() {
    // AMOADD discards old value; read the changed word into the selected return.
    let f = Fixture::new(&[op(0, 3, 0, 10, 11), 0x00052503, 0x00008067]);
    let mut request = scenario(&f, Some(9));
    request.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(6);
    let run = f.run(request, budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["verdict"], "DIFF");
    assert_eq!(result["records"][0]["stop"]["low"], 14);
    assert_eq!(result["records"][1]["stop"]["low"], 15);
}
