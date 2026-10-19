use super::*;

#[test]
fn ordinary_unaligned_loads_and_stores_preserve_bytes_and_width() {
    for (width, load, store, reload) in [
        (2, 0x00055283, 0x00559023, 0x0005d503),
        (4, 0x00052283, 0x0055a023, 0x0005a503),
    ] {
        // Load t0 from a0, store it through a1, return the reloaded value.
        let f = Fixture::new(&[load, store, reload, 0x00008067]);
        let mut request = f.request();
        let template = request.cases[0].clone();
        request.cases.clear();
        for offset in 1..=3u32 {
            let mut case = template.clone();
            case.name = format!("width-{width}-offset-{offset}");
            case.vendor.arguments = vec![Some(0x3000 + offset), Some(0x3010 + offset)];
            case.vendor.memory = vec![ram(MemorySeed {
                address: 0x3000,
                length: 32,
                fill: Some(0xa5),
                bytes: (0x80..0x90).collect(),
            })];
            case.vendor.observe_timeline.writes = true;
            case.replacement = Some(case.vendor.clone());
            request.cases.push(case);
        }
        let run = f.run(request, budget()).unwrap();
        let result = run.facts();
        assert_eq!(result["verdict"], "MATCH");
        let mut writes = 0;
        for r in result["records"].as_array().unwrap() {
            let v = &r;
            if v["kind"] == "event" {
                let offset = v["case"].as_u64().unwrap() as u32 + 1;
                let transaction = &v["event"]["transaction"];
                assert_eq!(transaction["kind"], "write");
                assert_eq!(transaction["address"], 0x3010 + offset);
                assert_eq!(transaction["width"], width);
                writes += 1;
            }
            if v["kind"] == "outcome" {
                let offset = v["case"].as_u64().unwrap() as u32 + 1;
                let expected = (0..width).fold(0, |n, i| n | ((0x80 + offset + i) << (i * 8)));
                assert_eq!(v["stop"]["kind"], "returned");
                assert_eq!(v["stop"]["low"], expected);
            }
        }
        assert_eq!(writes, 6);
    }
}

#[test]
fn ordinary_unaligned_access_keeps_unknown_boundaries_and_devices_unavailable() {
    for instruction in [0x00052503, 0x00b52023] {
        // lw a0,0(a0); sw a1,0(a0)
        let f = Fixture::new(&[instruction, 0x00008067]);
        let mut addresses = vec![0x3006, 0x6001, 0xffff_fffe];
        if instruction == 0x00b52023 {
            addresses.push(0x1001); // Captured read-only code is never writable.
        }
        for address in addresses {
            let mut request = f.request();
            request.cases[0].vendor.arguments = vec![Some(address), Some(0x12345678)];
            request.cases[0].vendor.memory = vec![
                ram(MemorySeed {
                    address: 0x3000,
                    length: 8,
                    fill: Some(0xa5),
                    bytes: vec![],
                }),
                ram(MemorySeed {
                    address: 0x3008,
                    length: 8,
                    fill: Some(0x5a),
                    bytes: vec![],
                }),
            ];
            request.cases[0].vendor.observe_memory = vec![MemorySelection {
                name: "unchanged".into(),
                address: 0x3000,
                length: 8,
            }];
            request.cases[0].vendor.models = vec![register_bank(vec![RegisterCell {
                address: 0x6000,
                width: 4,
                value: 9,
            }])];
            request.cases[0].replacement = Some(request.cases[0].vendor.clone());
            let run = f.run(request, budget()).unwrap();
            let result = run.facts();
            assert_eq!(result["verdict"], "INCOMPLETE");
            for r in result["records"].as_array().unwrap() {
                let v = &r;
                if v["kind"] == "outcome" {
                    assert_eq!(v["stop"]["reason"]["address"], address);
                }
                if v["kind"] == "final-memory" {
                    assert_eq!(v["chunk"]["known"], 255);
                    let bytes = v["chunk"]["bytes"].as_array().unwrap();
                    assert!(bytes[..8].iter().all(|b| b == 0xa5));
                }
                if v["kind"] == "model" {
                    assert_eq!(v["observation"]["reads"], 0);
                    assert_eq!(v["observation"]["writes"], 0);
                }
            }
        }
    }
    let f = Fixture::new(&[0x00052503, 0x00008067]);
    let mut request = f.request();
    request.cases[0].vendor.arguments[0] = Some(0x3001);
    request.cases[0].vendor.memory = vec![ram(MemorySeed {
        address: 0x3000,
        length: 8,
        fill: None,
        bytes: vec![1, 2, 3, 4],
    })];
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let run = f.run(request, budget()).unwrap();
    assert_eq!(run.facts()["verdict"], "INCOMPLETE");
}

#[test]
fn unaligned_ordinary_store_invalidates_an_overlapping_atomic_reservation() {
    // LR at a0, SW at a0+1, SC at a0: it must fail despite the unaligned store.
    let f = Fixture::new(&[
        atomics::op(2, 0, 5, 10, 0),
        0x00b520a3,
        atomics::op(3, 0, 5, 10, 11),
        0x00028513,
        0x00008067,
    ]);
    let mut request = atomics::scenario(&f, Some(9));
    request.cases[0].vendor.memory[0].seed.fill = Some(0);
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    let run = f.run(request, budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["verdict"], "MATCH");
    assert_eq!(result["records"][0]["stop"]["low"], 1);
}
