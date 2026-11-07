//! Frozen semantic leaves/outcomes from 0a1bec30, before replacing the RX
//! transport. The temporary prototype compared every leaf and every product
//! directly, then recorded these checksums. No old execution path is linked.
use super::*;
fn hash(bytes: impl Iterator<Item = u8>) -> u64 {
    bytes.fold(0xcbf2_9ce4_8422_2325_u64, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}
#[test]
fn every_leaf_and_terminal_product_match_the_frozen_semantic_oracle() {
    let cases = [
        (
            0,
            false,
            false,
            687,
            43,
            0xd3d9dac6102046ef,
            0x0e1a20db2c4dd0a7,
        ),
        (
            0,
            true,
            false,
            190,
            0,
            0x9db87fb777513c5c,
            0xb69a96d353a82ce9,
        ),
        (0, false, true, 2, 0, 0x2777b615e92f411e, 0x81fa98ce9bd70626),
        (0, true, true, 2, 0, 0x2777b615e92f411e, 0x81fa98ce9bd70626),
        (
            1,
            false,
            false,
            2499,
            496,
            0x38d9a6d50a276914,
            0x7b1a9eafc5190f99,
        ),
        (
            1,
            true,
            false,
            190,
            0,
            0x9db87fb777513c5c,
            0xb69a96d353a82ce9,
        ),
        (1, false, true, 2, 0, 0x2777b615e92f411e, 0x81fa98ce9bd70626),
        (1, true, true, 2, 0, 0x2777b615e92f411e, 0x81fa98ce9bd70626),
        (
            2,
            false,
            false,
            2747,
            744,
            0x833d48656fa85d81,
            0x74a720c29fe085ef,
        ),
        (
            2,
            true,
            false,
            190,
            0,
            0x9db87fb777513c5c,
            0xb69a96d353a82ce9,
        ),
        (2, false, true, 2, 0, 0x2777b615e92f411e, 0x81fa98ce9bd70626),
        (2, true, true, 2, 0, 0x2777b615e92f411e, 0x81fa98ce9bd70626),
        (
            3,
            false,
            false,
            5971,
            3968,
            0x6fe5e011fe5d5e7d,
            0x16d09bb92ddeb391,
        ),
        (
            3,
            true,
            false,
            190,
            0,
            0x9db87fb777513c5c,
            0xb69a96d353a82ce9,
        ),
        (3, false, true, 2, 0, 0x2777b615e92f411e, 0x81fa98ce9bd70626),
        (3, true, true, 2, 0, 0x2777b615e92f411e, 0x81fa98ce9bd70626),
    ];
    for (scenario, calibrated, initialized, leaves, measurements, trace_hash, outcome_hash) in cases
    {
        let mut p = parameters();
        p.dc_calibrated = calibrated;
        p.tables_initialized = initialized;
        p.dc.rx_saturation_detected = scenario == 2;
        let mut program = RxProgram::new(p);
        let mut oracle = Oracle {
            scenario,
            ..Oracle::default()
        };
        oracle.run(&mut program);
        assert_eq!(oracle.trace.len(), leaves);
        assert_eq!(oracle.measurements, measurements);
        assert_eq!(
            hash(
                oracle
                    .trace
                    .iter()
                    .flat_map(|line| line.bytes().chain(*b"\n"))
            ),
            trace_hash,
            "scenario={scenario}, calibrated={calibrated}, initialized={initialized}"
        );
        let outcome = program
            .terminal()
            .expect("terminal")
            .expect("successful program");
        assert_eq!(hash(format!("{outcome:?}").bytes()), outcome_hash);
    }
}
