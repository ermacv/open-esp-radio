//! Transport cases over captured ROM callbacks and compiled PHY transactions.
//!
//! Scenario expectations are independent of either execution result. The
//! peripheral assumption supplies responses only; ROM/PAC/PHY code performs
//! every transaction.
use crate::contracts::port_polling;
use crate::evidence::stop;
use crate::harness::direct;
use crate::harness::{Result, case, known, words};
use crate::i2c::{I2c, models, returned_low, word_writes};
use crate::layout::*;
use blobray_domain::{
    CommandCell, ComparisonVerdict, DeviceBehavior, DeviceDeclaration, DeviceIssue, ExecutionCase,
    ExecutionEvent, ExecutionEvidence, ExecutionRegion, ExecutionStop, Invocation, ModelStatus,
    RegionLifetime, RegisterCell, SessionReset,
};
use oer_riscv_model::ArtifactId;

/// Independent `.iram1` +0x44..+0x60 instruction reading.
const CONFIGURATION: u32 = 0x1237_fa08;

pub fn controls() -> DeviceDeclaration {
    DeviceDeclaration {
        id: "controls".into(),
        applicability: "captured complemented read mask and host-map RMW".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: vec![
                RegisterCell {
                    address: I2C_READ_MASK,
                    width: 4,
                    value: 0,
                },
                RegisterCell {
                    address: I2C_HOST_MAP,
                    width: 4,
                    value: 0x1234_5678,
                },
            ],
        },
    }
}

/// Shared analog bytes; samples at issue, writes at ready observation.
pub fn bank(
    selector: Option<u32>,
    sample: u32,
    scripted: bool,
    busy: u32,
    initial: [u32; 2],
) -> DeviceDeclaration {
    analog_bank(
        "analog",
        "shared analog bytes; samples at issue, writes at ready observation",
        busy,
        initial,
        [0x026b, 0x0466]
            .into_iter()
            .map(|s| CommandCell {
                selector: s,
                initial: if Some(s) == selector { sample } else { 0 },
                reads: (scripted && Some(s) == selector).then(|| vec![sample]),
            })
            .collect(),
    )
}

struct Transport {
    callbacks: Vec<u32>,
}

impl Transport {
    /// The ROM interface pointer has no PT_LOAD mapping. Its explicit RAM
    /// value selects captured code, not callback response models.
    fn memory(&self) -> Result<Vec<ExecutionRegion>> {
        Ok(vec![
            known(ROM_INTERFACE_POINTER, 4, &words(&[CALLBACK_TABLE]))?,
            known(CALLBACK_TABLE, 16, &words(&self.callbacks))?,
        ])
    }

    fn invoke(
        &self,
        target: u32,
        arguments: &[u32],
        models: Vec<DeviceDeclaration>,
    ) -> Result<Invocation> {
        Ok(direct(target, arguments, self.memory()?, models, vec![]))
    }

    /// Production reads busy before issuing a read; ROM's org leaf does not.
    /// The reviewed contract ignores port polling only; every other effect
    /// and the selected return compare.
    #[allow(clippy::too_many_arguments)]
    fn paired(
        &self,
        effects: &ArtifactId,
        name: &str,
        left: u32,
        left_words: &[u32],
        right: u32,
        right_words: &[u32],
        models: Vec<DeviceDeclaration>,
        returns: bool,
    ) -> Result<ExecutionCase> {
        let mut row = case(
            name,
            self.invoke(left, left_words, models.clone())?,
            Some(self.invoke(right, right_words, models)?),
            SessionReset::Cold,
            false,
        );
        let relation = row.relation.as_mut().unwrap();
        relation.effects = Some(effects.clone());
        relation.returns.low = returns;
        Ok(row)
    }
}

/// Expected writes, returned low word and issued command count of one case.
type Expectation = (Vec<(u32, u32)>, Option<u32>, u64);

pub fn exercise(ctx: &mut I2c) -> Result<()> {
    let transport = Transport {
        callbacks: [
            "phy_i2c_enter_critical",
            "phy_i2c_exit_critical",
            "phy_get_i2c_read_mask_new",
            "phy_get_i2c_hostid_new",
        ]
        .map(|n| ctx.root(n))
        .to_vec(),
    };
    let transfer = ctx.probe("open_phy_trace_i2c_transfer");
    let applicability = "analog I2C transactions under explicit port responses and busy states";
    let review = |ctx: &mut I2c, name: &str, vendor, production: &str| {
        let production = ctx.input_endpoint(2, production)?;
        ctx.review_pair(
            name,
            vendor,
            production,
            port_polling(MAX_EVENTS),
            applicability,
        )
    };
    let host_effects = review(
        ctx,
        "transport-host",
        ctx.root_endpoint("phy_get_i2c_hostid_new")?,
        "open_phy_trace_i2c_host",
    )?;
    let (mut cases, mut expected): (Vec<ExecutionCase>, Vec<Expectation>) = (vec![], vec![]);
    let hosts = [1, 1, 1, 0, 0, 0, 1, 0, 0, 1, 1, 0, 0];
    for (block, host) in (0x61..0x6e).zip(hosts) {
        cases.push(transport.paired(
            &host_effects,
            &format!("host-{block:x}"),
            ctx.root("phy_get_i2c_hostid_new"),
            &[block],
            ctx.probe("open_phy_trace_i2c_host"),
            &[block],
            vec![controls()],
            true,
        )?);
        expected.push((vec![(I2C_HOST_MAP, CONFIGURATION)], Some(host), 0));
    }
    let names = [
        "phy_i2c_readReg",
        "phy_i2c_writeReg",
        "phy_i2c_readReg_Mask",
        "phy_i2c_writeReg_Mask",
    ];
    let mut transfer_effects = vec![];
    for name in names {
        let vendor = ctx.input_endpoint(1, name)?;
        transfer_effects.push(review(
            ctx,
            &format!("transport-{name}"),
            vendor,
            "open_phy_trace_i2c_transfer",
        )?);
    }
    for profile in 0..8u32 {
        let host = profile / 4;
        let (block, register, sample, high, low, maximum, mask, cleared, set_value) = if host == 0 {
            (0x66, 4, 0xa6, 3, 2, 3, 0xffff_ff7f, 0xa2, 0xae)
        } else {
            (0x6b, 2, 0xa5, 7, 4, 15, 0xffff_fff7, 5, 0xf5)
        };
        let selector = (register << 8) | block;
        let mode = profile % 4;
        let target = ctx.captured(1, names[mode as usize]);
        for busy in [0, 2] {
            let values = match mode {
                1 => vec![0, 255],
                3 => vec![0, maximum],
                _ => vec![0],
            };
            for value in values {
                let mut arguments = vec![block, host, register];
                if mode == 1 {
                    arguments.push(value);
                }
                if mode >= 2 {
                    arguments.extend([high, low]);
                }
                if mode == 3 {
                    arguments.push(value);
                }
                let models = vec![
                    controls(),
                    bank(Some(selector), sample, mode != 1, busy, [0, 0]),
                ];
                cases.push(transport.paired(
                    &transfer_effects[mode as usize],
                    &format!("transfer-{profile}-{busy}-{value}"),
                    target,
                    &arguments,
                    transfer,
                    &[profile, value, 32],
                    models,
                    matches!(mode, 0 | 2),
                )?);
                let mut writes = vec![(I2C_HOST_MAP, CONFIGURATION)];
                let port = I2C_PORT_0 + 4 * host;
                if mode != 1 {
                    writes.extend([(I2C_READ_MASK, mask), (port, COMMAND_READ | selector)]);
                }
                if mode == 3 {
                    writes.push((I2C_HOST_MAP, CONFIGURATION));
                }
                if matches!(mode, 1 | 3) {
                    let data = if mode == 1 {
                        value
                    } else if value == 0 {
                        cleared
                    } else {
                        set_value
                    };
                    writes.push((port, COMMAND_WRITE | data << 16 | selector));
                }
                let returned = match mode {
                    0 => Some(sample),
                    2 => Some(if host == 0 { 1 } else { 10 }),
                    _ => None,
                };
                expected.push((writes, returned, if mode == 3 { 2 } else { 1 }));
            }
        }
    }
    let reset = ctx.captured(1, "phy_i2c_master_reset");
    let reset_replacement = ctx.probe("open_phy_trace_i2c_reset");
    let reset_effects = review(
        ctx,
        "transport-reset",
        ctx.input_endpoint(1, "phy_i2c_master_reset")?,
        "open_phy_trace_i2c_reset",
    )?;
    for (initial, busy) in [([0, 0], 0), ([1, 1], 0), ([1, 1], 2)] {
        cases.push(transport.paired(
            &reset_effects,
            &format!("reset-{}-{busy}", initial[0]),
            reset,
            &[],
            reset_replacement,
            &[],
            vec![bank(None, 0, false, busy, initial)],
            false,
        )?);
        let writes = (0..2)
            .filter(|i| initial[*i as usize] != 0)
            .map(|i| (I2C_PORT_0 + 4 * i, COMMAND_READ))
            .collect();
        expected.push((
            writes,
            None,
            initial.iter().filter(|v| **v != 0).count() as u64,
        ));
    }
    // Every independent cold case is one request: requests are retained by
    // identity, so the control-message bound does not split the matrix.
    for (batch, (rows, expectations)) in cases
        .chunks(cases.len())
        .zip(expected.chunks(expected.len()))
        .enumerate()
    {
        let records = ctx.compare(
            &format!("transport-{batch}"),
            rows.to_vec(),
            ComparisonVerdict::Match,
            MAX_EVENTS,
        )?;
        for (i, (writes, returned, commands)) in expectations.iter().enumerate() {
            let i = i as u32;
            for side in [false, true] {
                let low = returned_low(&records, i, side);
                if let Some(value) = returned {
                    assert_eq!(low, Some(*value), "{batch} {i} {side}");
                }
                assert_eq!(
                    &word_writes(&records, i, side),
                    writes,
                    "{batch} {i} {side}"
                );
                for model in models(&records, i, side) {
                    assert!(
                        model.status == ModelStatus::Complete
                            && model.closed
                            && model.issue.is_none()
                    );
                    if let Some(observed) = model.commands {
                        assert_eq!(observed.pending, 0);
                        assert_eq!(observed.issued, *commands);
                    }
                }
            }
        }
    }
    // No sample fallback: identical exhausted environments are still incomplete.
    let mut exhausted = cases[13].clone();
    exhausted.name = "exhausted-samples".into();
    for side in [Some(&mut exhausted.vendor), exhausted.replacement.as_mut()]
        .into_iter()
        .flatten()
    {
        if let DeviceBehavior::CommandBank(bank) = &mut side.models[1].behavior {
            for cell in &mut bank.cells {
                if cell.reads.is_some() {
                    cell.reads = Some(vec![]);
                }
            }
        }
    }
    let records = ctx.compare(
        "transport-exhausted",
        vec![exhausted],
        ComparisonVerdict::Incomplete,
        MAX_EVENTS,
    )?;
    assert!(records.iter().all(|r| match r {
        ExecutionEvidence::Model { observation, .. } if observation.id == "analog" => {
            observation.issue == Some(DeviceIssue::ExhaustedReads)
        }
        _ => true,
    }));
    // The captured masked-write ABI permits out-of-field bits to spill; the
    // production field update clips them. Preserve that known difference.
    let wide = transport.paired(
        &transfer_effects[3],
        "out-of-field-value",
        ctx.captured(1, "phy_i2c_writeReg_Mask"),
        &[0x66, 0, 4, 3, 2, 7],
        transfer,
        &[3, 7, 32],
        vec![controls(), bank(Some(0x0466), 0xa6, true, 0, [0, 0])],
        false,
    )?;
    let records = ctx.compare(
        "transport-field-domain",
        vec![wide],
        ComparisonVerdict::Diff,
        MAX_EVENTS,
    )?;
    let port: Vec<u32> = records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                event:
                    ExecutionEvent::Write {
                        address: I2C_PORT_0,
                        value,
                        ..
                    },
                ..
            } => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(port, [0x0400_0466, 0x05be_0466, 0x0400_0466, 0x05ae_0466]);
    // Finite supplied completion edges expire before a delayed read becomes ready.
    let timeout = transport.paired(
        &transfer_effects[0],
        "edge-schedule-exhausted",
        ctx.captured(1, "phy_i2c_readReg"),
        &[0x66, 0, 4],
        transfer,
        &[0, 0, 2],
        vec![controls(), bank(Some(0x0466), 0xa6, true, 4, [0, 0])],
        false,
    )?;
    let records = ctx.compare(
        "transport-edge-timeout",
        vec![timeout],
        ComparisonVerdict::Incomplete,
        MAX_EVENTS,
    )?;
    assert!(matches!(
        stop(&records, 0, true),
        ExecutionStop::Returned {
            low: Some(0x10001),
            ..
        }
    ));
    // The actual shipping reset helper has a finite 10,000-observation bound;
    // ROM keeps polling. Exclude void returns but keep the pending model obligation.
    let timeout = transport.paired(
        &reset_effects,
        "production-reset-timeout",
        reset,
        &[],
        reset_replacement,
        &[],
        vec![bank(None, 0, false, 10001, [1, 0])],
        false,
    )?;
    let records = ctx.compare(
        "transport-reset-timeout",
        vec![timeout],
        ComparisonVerdict::Incomplete,
        MAX_EVENTS,
    )?;
    assert!(matches!(
        stop(&records, 0, true),
        ExecutionStop::Returned {
            low: Some(0x10001),
            ..
        }
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_samples_apply_only_to_the_selected_cell() {
        crate::install();
        let DeviceBehavior::CommandBank(bank) = bank(Some(0x0466), 0xa6, true, 2, [1, 0]).behavior
        else {
            panic!("command bank")
        };
        assert_eq!(bank.cells[0].reads, None);
        assert_eq!(bank.cells[1].reads, Some(vec![0xa6]));
        assert_eq!(bank.cells[1].initial, 0xa6);
        assert_eq!(
            (bank.ports[0].initial_busy_reads, bank.ports[1].busy_reads),
            (1, 2)
        );
    }
}
