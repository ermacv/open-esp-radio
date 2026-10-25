//! Finite current-archive calibration leaves against shipping HAL/PHY code.
//!
//! Expected transactions come from independent instruction reading of the
//! authenticated archive and its ROM AGC child, not from the production result.
//! Production runs each leaf on the probe image's session shared PHY, which
//! a setup before the case starts.
use crate::evidence::stop;
use crate::harness::direct;
use crate::harness::{Result, case};
use crate::i2c::{I2c, all_complete, returned_low, word_writes};
use crate::layout::*;
use crate::session::request;
use oer_analysis_blobray_domain::{
    ComparisonVerdict, DeviceBehavior, DeviceDeclaration, ExecutionCase, ExecutionGap,
    ExecutionStop, Invocation, MemoryAccess, RegionLifetime, RegisterCell, SessionReset,
};
use std::collections::BTreeMap;

fn invoke(target: u32, arguments: &[u32], cells: &BTreeMap<u32, u32>) -> Result<Invocation> {
    let models = if cells.is_empty() {
        vec![]
    } else {
        vec![DeviceDeclaration {
            id: "leaf-registers".into(),
            applicability: "explicit retained calibration register inputs".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: cells
                    .iter()
                    .map(|(a, v)| RegisterCell {
                        address: *a,
                        width: 4,
                        value: *v,
                    })
                    .collect(),
            },
        }]
    };
    Ok(direct(target, arguments, vec![], models, vec![]))
}

/// Expected writes and optional returned low word of one compared case.
type Expectation = (Vec<(u32, u32)>, Option<u32>);

pub fn exercise(ctx: &mut I2c) -> Result<()> {
    let production = ctx.probe("open_phy_calibration_leaf");
    let (mut cases, mut expected): (Vec<ExecutionCase>, Vec<(u32, Expectation)>) = (vec![], vec![]);
    let mut add = |name: &str,
                   root: &str,
                   profile: u32,
                   arguments: &[u32],
                   cells: &[(u32, u32)],
                   writes: Vec<(u32, u32)>,
                   returned: Option<u32>|
     -> Result<()> {
        let cells: BTreeMap<u32, u32> = cells.iter().copied().collect();
        let mut production_arguments = vec![profile];
        production_arguments.extend(arguments);
        let mut row = case(
            name,
            invoke(ctx.root(root), arguments, &cells)?,
            Some(invoke(production, &production_arguments, &cells)?),
            SessionReset::Warm,
            false,
        );
        row.relation.as_mut().unwrap().returns.low = returned.is_some();
        cases.push(ctx.session_setup(name)?);
        expected.push((cases.len() as u32, (writes, returned)));
        cases.push(row);
        Ok(())
    };
    for (name, value) in [("restore-a", 0xa596_783cu32), ("restore-b", 0x5a69_87c3)] {
        let first = value & 0xffff_ff00;
        let second = (first & 0xffff_00ff) | 0xfb00;
        let third = (second & 0xff00_ffff) | 0x30000;
        let writes = [first, second, third, third & 0x00ff_ffff]
            .map(|v| (0x2010_0410, v))
            .to_vec();
        add(
            name,
            "phy_txgain_comp_pacfg_new",
            0,
            &[1],
            &[(0x2010_0410, value)],
            writes,
            None,
        )?;
    }
    for (name, enabled, a, b, initial) in [
        (
            "enable-calibration-gain",
            1u32,
            0xffff_ff88u32,
            0xffff_ff88u32,
            0xa5fe_1234u32,
        ),
        (
            "disable-calibration-gain",
            0,
            0xffff_ff88,
            0xffff_ff88,
            0x5aff_4321,
        ),
        (
            "independent-signed-gains",
            1,
            0xffff_ff80,
            0x7f,
            0x96a5_5a69,
        ),
    ] {
        let first = (initial & 0xfffe_ffff) | (enabled << 16);
        let second = (first & 0xffff_ff00) | (a & 255);
        let third = (second & 0xffff_00ff) | ((b & 255) << 8);
        let writes = [first, second, third].map(|v| (GAIN_BASE, v)).to_vec();
        add(
            name,
            "phy_force_dig_gain",
            1,
            &[enabled, a, b],
            &[(GAIN_BASE, initial)],
            writes,
            None,
        )?;
    }
    for (name, value) in [("agc-a", 0xa596_783cu32), ("agc-b", 0x5a69_87c3)] {
        let first = (value & 0xffff_ff80) | 0x17;
        let writes = vec![
            (0x2010_705c, value | 0x0400_0000),
            (0x2010_7064, 0x0818_212d),
            (0x2010_7114, 0x0818_212d),
            (0x2010_7104, (value & 0xffff_fe00) | 0x1c0),
            (0x2010_78c8, first),
            (0x2010_78c8, (first & 0xffff_c07f) | 0xb80),
        ];
        let cells = [
            (0x2010_705c, value),
            (0x2010_7104, value),
            (0x2010_78c8, value),
            (0x2010_7064, 0),
            (0x2010_7114, 0),
        ];
        add(name, "phy_reg_update_new", 3, &[], &cells, writes, None)?;
    }
    let records = ctx.compare(
        "calibration-leaves",
        cases.clone(),
        ComparisonVerdict::Match,
        4096,
    )?;
    for (i, (writes, returned)) in &expected {
        for side in [false, true] {
            let low = returned_low(&records, *i, side);
            if returned.is_some() {
                assert_eq!(low, *returned, "{i} {side}");
            }
            assert_eq!(&word_writes(&records, *i, side), writes, "{i} {side}");
            assert!(all_complete(&records, *i, side));
        }
    }
    // Independent signed gains with a changed production input: the
    // second gain argument's low byte is published.
    let mut different = cases[8..10].to_vec();
    different[1].name = "changed-gain".into();
    let argument = &mut different[1].replacement.as_mut().unwrap().arguments[3];
    *argument = argument.map(|word| (word & !0xff) | 0x7e);
    ctx.compare(
        "calibration-different",
        different,
        ComparisonVerdict::Diff,
        4096,
    )?;
    let mut unknown = cases[8..10].to_vec();
    // The consumed gain argument (`a3`) is unknown.
    unknown[1].name = "unknown-argument".into();
    unknown[1].replacement.as_mut().unwrap().arguments[3] = None;
    ctx.compare(
        "calibration-unknown",
        unknown,
        ComparisonVerdict::Incomplete,
        4096,
    )?;
    let mut missing = cases[..2].to_vec();
    missing[1].name = "missing-register-input".into();
    missing[1].vendor.models.clear();
    let records = ctx.compare(
        "calibration-missing",
        missing,
        ComparisonVerdict::Incomplete,
        4096,
    )?;
    assert!(matches!(
        stop(&records, 1, false),
        ExecutionStop::Incomplete {
            reason: ExecutionGap::Memory {
                address: 0x2010_0410,
                access: MemoryAccess::Read
            },
            ..
        }
    ));
    let limited = request(
        &ctx.vendor,
        Some(&ctx.replacement),
        None,
        cases[..2].to_vec(),
        1,
    );
    ctx.capacity_failure("calibration-capacity", &limited)
}
