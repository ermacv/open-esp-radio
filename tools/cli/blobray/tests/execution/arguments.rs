use super::*;

#[test]
fn register_and_stack_words_execute_and_compare() {
    // lw t0,0(sp); lw t1,4(sp); add a0,a7,t0; add a0,a0,t1;
    // andi a1,sp,15; ret. Expected result derives from explicit input words.
    let f = Fixture::new(&[
        0x00012283, 0x00412303, 0x00588533, 0x00650533, 0x00f17593, 0x00008067,
    ]);
    let mut request = f.request();
    request.cases[0].vendor.arguments = vec![None; 7];
    request.cases[0]
        .vendor
        .arguments
        .extend([Some(9), Some(10), Some(11)]);
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let run = f.run(request.clone(), budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["verdict"], "MATCH");
    for record in result["records"].as_array().unwrap().iter().take(2) {
        assert_eq!(record["stop"]["low"], 30);
        assert_eq!(record["stop"]["high"], 0);
    }
    request.cases[0].replacement.as_mut().unwrap().arguments[9] = Some(12);
    let run = f.run(request, budget()).unwrap();
    assert_eq!(run.facts()["verdict"], "DIFF");
}

#[test]
fn unknown_words_override_seeded_stack_and_omitted_registers_stay_unknown() {
    for (code, words) in [
        // An unknown stack word loads an unknown value; an omitted register
        // (`addi a0, a1, 1`) makes the result unknown. Neither is provable.
        vec![0x00012503, 0x00008067],
        vec![0x00158513, 0x00008067],
    ]
    .into_iter()
    .zip([vec![None; 9], vec![Some(5)]])
    {
        let f = Fixture::new(&code);
        let mut request = f.request();
        request.vendor.stack.fill = Some(0xff);
        request.vendor.stack.bytes = vec![0x42; 4096];
        request.replacement = Some(request.vendor.clone());
        request.cases[0].vendor.arguments = words;
        request.cases[0].replacement = Some(request.cases[0].vendor.clone());
        let run = f.run(request, budget()).unwrap();
        let result = run.facts();
        assert_eq!(result["verdict"], "INCOMPLETE");
        let stop = &result["records"][0]["stop"];
        assert_eq!(stop["kind"], "returned");
        assert!(stop["low"].is_null(), "{stop}");
    }
    // A supplied unknown word is harmless when code overwrites it before use.
    let f = Fixture::new(&[0x00500513, 0x00008067]);
    let mut request = f.request();
    request.cases[0].vendor.arguments.clear();
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let run = f.run(request, budget()).unwrap();
    assert_eq!(run.facts()["verdict"], "MATCH");
}

#[test]
fn stack_alignment_capacity_and_unknown_padding_are_explicit() {
    let f = Fixture::new(&[0x00010513, 0x00008067]); // mv a0,sp; ret
    for (count, expected) in [
        (0, 0x9000),
        (8, 0x9000),
        (9, 0x8ff0),
        (12, 0x8ff0),
        (13, 0x8fe0),
        (256, 0x8c20),
    ] {
        let mut request = f.request();
        request.cases[0].vendor.arguments = vec![None; count];
        request.cases[0].replacement = Some(request.cases[0].vendor.clone());
        let run = f.run(request, budget()).unwrap();
        let result = run.facts();
        assert_eq!(result["records"][0]["stop"]["low"], expected);
        assert_eq!(result["verdict"], "MATCH");
    }
    let f = Fixture::new(&[0x00412503, 0x00008067]); // Load unused padding after argument 9.
    let mut request = f.request();
    request.cases[0].vendor.arguments.push(Some(10));
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let run = f.run(request, budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["verdict"], "INCOMPLETE");
    assert!(result["records"][0]["stop"]["low"].is_null());
}

#[test]
fn invalid_argument_geometry_is_rejected_before_execution() {
    let f = Fixture::new(&[0x00008067]);
    for variant in 0..5 {
        let mut request = f.request();
        match variant {
            0 => request.cases[0].vendor.arguments.resize(257, None),
            1 => {
                request.cases[0]
                    .replacement
                    .as_mut()
                    .unwrap()
                    .arguments
                    .resize(13, None);
                let stack = &mut request.replacement.as_mut().unwrap().stack;
                stack.address = 0x8ff0;
                stack.length = 16;
            }
            2 => request.vendor.stack.address += 1,
            3 => request.vendor.stack.length = u32::MAX,
            _ => request.schema = 1,
        }
        let error = f.run(request, budget()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
}

#[test]
fn compressed_andi_matches_independent_signed_masks_in_concrete_execution() {
    // One entry for each immediate: C.ANDI a0,imm; C.JR ra. Ordinary ANDI
    // entries follow in the same captured image. Expected values come from
    // signed integer masks, not from either decoder or the comparison verdict.
    let mut code = Vec::new();
    for signed in -32i32..32 {
        let bits = signed as u32 & 63;
        code.push(0x8082_0000 | 0x8901 | ((bits & 31) << 2) | ((bits & 32) << 7));
    }
    for signed in -32i32..32 {
        code.extend([((signed as u32 & 0xfff) << 20) | 0x57513, 0x00008067]);
    }
    let f = Fixture::new(&code);
    for input in [0x8013, 0xa5a5_5a5a, 0xffff_ffff] {
        let mut request = f.request();
        let template = request.cases[0].clone();
        request.cases.clear();
        for (index, signed) in (-32i32..32).enumerate() {
            let mut case = template.clone();
            case.name = format!("andi-{signed}");
            case.vendor.entry = 0x1000 + index as u32 * 4;
            case.vendor.arguments[0] = Some(input);
            let mut replacement = case.vendor.clone();
            replacement.entry = 0x1100 + index as u32 * 8;
            case.replacement = Some(replacement);
            request.cases.push(case);
        }
        let run = f.run(request, budget()).unwrap();
        let result = run.facts();
        assert_eq!(result["verdict"], "MATCH");
        let outcomes: Vec<_> = result["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["kind"] == "outcome")
            .collect();
        assert_eq!(outcomes.len(), 128);
        for row in outcomes {
            let signed = row["case"].as_i64().unwrap() as i32 - 32;
            assert_eq!(row["stop"]["kind"], "returned");
            assert_eq!(row["stop"]["low"], input & signed as u32);
        }
    }
}

#[test]
fn case_stack_fill_replaces_the_target_fill_for_both_sides() {
    let f = Fixture::new(&[0xffc12503, 0x00008067]); // lw a0,-4(sp); ret
    let mut request = f.request();
    request.vendor.stack.fill = None;
    request.vendor.stack.bytes.clear();
    request.replacement = Some(request.vendor.clone());
    request.cases[0].vendor.arguments.clear();
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let unfilled = request.cases[0].clone();
    request.cases = [0x5a, 0xa5]
        .map(|fill| ExecutionCase {
            stack_fill: Some(fill),
            ..unfilled.clone()
        })
        .to_vec();
    request.cases.push(unfilled);
    let run = f.run(request, budget()).unwrap();
    let result = run.facts();
    let returned: Vec<_> = result["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "outcome")
        .map(|r| (r["case"].clone(), r["stop"].clone()))
        .collect();
    for side in 0..2 {
        assert_eq!(returned[side].1["low"], 0x5a5a_5a5a_u32);
        assert_eq!(returned[2 + side].1["low"], 0xa5a5_a5a5_u32);
        // Without a case fill, the target's unknown stack stays unknown.
        assert!(returned[4 + side].1["low"].is_null());
    }
    assert_eq!(result["verdict"], "INCOMPLETE");
}
