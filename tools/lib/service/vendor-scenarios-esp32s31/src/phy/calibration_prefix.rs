//! Captured PBus/DCODE children and compiled-production failure containment.
use crate::contracts::port_polling;
use crate::evidence::{events, output, stop};
use crate::harness::direct;
use crate::harness::{Result, case, known, region, selection, with_stack_fill, words};
use crate::i2c::{I2c, all_complete, models, returned_low};
use crate::layout::*;
use crate::phy::delay_calls;
use blobray_domain::{
    CommandCell, CommandObservation, ComparisonDifference, ComparisonVerdict, DeviceBehavior,
    DeviceDeclaration, ExecutionCase, ExecutionEvent, ExecutionEvidence, ExecutionGap,
    ExecutionRegion, ExecutionStop, Invocation, MemorySelection, ModelStatus, ReadRun,
    RegionLifetime, RegisterCell, SessionReset,
};

const DESTINATION: u32 = PARAMETER_DESTINATION;

fn pbus_models(initial: u32, settle: bool, busy: u32) -> Vec<DeviceDeclaration> {
    vec![
        DeviceDeclaration {
            id: "pbus-registers".into(),
            applicability: "explicit retained PBus and work-mode registers".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: [
                    (0x2010_0884, initial),
                    (0x2010_088c, initial),
                    (WORK_MODE, if settle { 2 } else { 0 }),
                    (0x2010_702c, initial),
                ]
                .map(|(address, value)| RegisterCell {
                    address,
                    width: 4,
                    value,
                })
                .to_vec(),
            },
        },
        DeviceDeclaration {
            id: "pbus-status".into(),
            applicability: "twelve finite command-completion scripts".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::SequenceRead {
                address: PBUS_STATUS,
                width: 4,
                runs: (0..12)
                    .flat_map(|_| {
                        let mut runs = vec![];
                        if busy != 0 {
                            runs.push(ReadRun {
                                value: 0x8000_0000,
                                count: busy,
                            });
                        }
                        runs.push(ReadRun::once(0));
                        runs
                    })
                    .collect(),
            },
        },
    ]
}

/// Independent instruction reading of captured force-mode/force-test and the
/// twelve call arguments in `phy_pbus_clear_reg`.
pub fn expected_writes(initial: u32, settle: bool) -> Vec<(u32, u32)> {
    let second = initial & 0xfbff_ffff;
    let mut first = initial | 1;
    let mut writes = vec![(0x2010_088c, second), (0x2010_0884, first)];
    for (channel, mode, data) in [
        (4u32, 1u32, 0u32),
        (4, 2, 0),
        (5, 1, 0),
        (5, 2, 0),
        (0, 1, 0),
        (0, 2, 0),
        (1, 1, 0),
        (1, 2, 0),
        (2, 1, 256),
        (3, 1, 256),
        (2, 2, 256),
        (3, 2, 256),
    ] {
        let issued =
            (first & 0xfffe_0001) | (((channel * 4) | (mode << 15) | (data << 6)) & 0x1fffc) | 2;
        first = issued & 0xffff_fffd;
        writes.extend([(0x2010_0884, issued), (0x2010_0884, first)]);
    }
    writes.extend([
        (0x2010_0884, first & 0xffff_fffe),
        (0x2010_088c, second | COMMAND_READ),
    ]);
    if settle {
        let pulse = (initial & 0x00ff_ffff) | 0x3200_0000;
        writes.extend([
            (0x2010_702c, pulse),
            (0x2010_702c, pulse | 0x0080_0000),
            (0x2010_702c, pulse & 0xff7f_ffff),
        ]);
    }
    writes
}

/// ROM's channel table is [1, 5, 10, 14]: 2412, 2432, 2457, 2484 MHz.
pub fn dcode_frequency_writes() -> (Vec<u32>, Vec<u32>) {
    let (mut frequency, mut writes, mut nrx) = (0x4128_0055u32, vec![], vec![]);
    for mhz in [2412u32, 2432, 2457, 2484] {
        frequency = (frequency & 0xffff_ff00) | (mhz - 2400);
        writes.extend([frequency, frequency | 0x80000, frequency & 0xfff7_ffff]);
        frequency &= 0xfff7_ffff;
        nrx.push(0x1600_0000 | ((80 << 22) / mhz));
    }
    (writes, nrx)
}

/// Read/write commands of four DCODE measurements over retained CKGEN bytes.
pub fn dcode_commands(fill: u8) -> Vec<u32> {
    let mut retained = [
        (4u32, u32::from(fill)),
        (19, u32::from(fill)),
        (20, u32::from(fill)),
    ];
    let mut commands = vec![];
    for _ in 0..4 {
        for (register, mask, value) in [
            (19u32, 0x40u32, 0u32),
            (20, 0x40, 0),
            (4, 0x80, 0),
            (4, 0x80, 0x80),
        ] {
            let selector = (register << 8) | 0x62;
            let cell = retained.iter_mut().find(|(r, _)| *r == register).unwrap();
            cell.1 = (cell.1 & !mask) | value;
            commands.extend([
                COMMAND_READ | selector,
                COMMAND_WRITE | cell.1 << 16 | selector,
            ]);
        }
        commands.extend([0x0400_1162, 0x0400_1262]);
    }
    commands
}

struct Prefix {
    parameter: u32,
    memcpy: u32,
    rom_delay: u32,
    production_delay: u32,
    /// Register-preserving short-delay event of the production probes.
    short_delay: u32,
    callbacks: Vec<u32>,
}

impl Prefix {
    /// Requested-delay models of one side: the ROM delay, and on production
    /// also the register-preserving short-delay event.
    fn delays(&self, id: &str, side: bool) -> Vec<blobray_domain::CallDeclaration> {
        if !side {
            return delay_calls(id, self.rom_delay);
        }
        let mut calls = delay_calls(id, self.production_delay);
        calls.extend(delay_calls(&format!("{id}-short"), self.short_delay));
        calls
    }

    fn invoke(
        &self,
        target: u32,
        models: Vec<DeviceDeclaration>,
        settle: bool,
        side: bool,
    ) -> Result<Invocation> {
        let mut result = direct(target, &[], vec![], models, vec![]);
        if settle {
            result.calls = self.delays("settle-delay", side);
        }
        Ok(result)
    }

    fn enter(
        &self,
        target: u32,
        arguments: &[u32],
        memory: Vec<ExecutionRegion>,
        models: Vec<DeviceDeclaration>,
        observe: Vec<MemorySelection>,
    ) -> Result<Invocation> {
        Ok(direct(target, arguments, memory, models, observe))
    }

    /// Setup executes the captured ROM memcpy; image bytes are not overwritten
    /// by a side channel. Warm phases retain that initialized parameter buffer.
    fn setup(&self, crystal: u8, side: bool) -> Result<Invocation> {
        let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
        data[79] = crystal;
        data[417..425].fill(0xa5);
        let mut memory = vec![known(PARAMETER_SOURCE, PHY_PARAM_BYTES, &data)?];
        if side {
            memory.push(region(
                DESTINATION,
                PHY_PARAM_BYTES,
                &[],
                None,
                RegionLifetime::Session,
            )?);
        }
        self.enter(
            self.memcpy,
            &[
                if side { DESTINATION } else { self.parameter },
                PARAMETER_SOURCE,
                PHY_PARAM_BYTES,
            ],
            memory,
            vec![],
            vec![],
        )
    }

    fn dcode_models(&self, fill: u8, busy: u32) -> Vec<DeviceDeclaration> {
        let mut result = pbus_models(0, false, 0);
        result.push(DeviceDeclaration {
            id: "dcode-registers".into(),
            applicability: "explicit frequency, NRX and transport control values".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: [
                    (FREQUENCY_CONTROL, 0x4128_0055),
                    (0x2010_7848, 0x1655_a55a),
                    (I2C_READ_MASK, 0),
                    (I2C_HOST_MAP, 0),
                ]
                .map(|(address, value)| RegisterCell {
                    address,
                    width: 4,
                    value,
                })
                .to_vec(),
            },
        });
        result.push(DeviceDeclaration {
            id: "channel-ready".into(),
            applicability: "four bounded frequency readiness scripts".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::SequenceRead {
                address: CHANNEL_STATUS,
                width: 4,
                runs: (0..4)
                    .flat_map(|_| {
                        let mut runs = vec![];
                        if busy != 0 {
                            runs.push(ReadRun {
                                value: 0x2582_4e58,
                                count: busy,
                            });
                        }
                        runs.push(ReadRun::once(0x2582_4f58));
                        runs
                    })
                    .collect(),
            },
        });
        let mut cells: Vec<CommandCell> = [4u32, 19, 20]
            .map(|r| CommandCell {
                selector: (r << 8) | 0x62,
                initial: u32::from(fill),
                reads: None,
            })
            .to_vec();
        cells.push(CommandCell {
            selector: 0x1162,
            initial: 0,
            reads: Some(vec![0xc0, 0xdf, 0xe0, 0xff]),
        });
        cells.push(CommandCell {
            selector: 0x1262,
            initial: 0,
            reads: Some(vec![0xff, 0xe0, 0xdf, 0xc0]),
        });
        result.push(analog_bank(
            "ckgen",
            "shared retained CKGEN bytes and eight declared six-bit samples",
            busy,
            [0, 0],
            cells,
        ));
        result
    }

    fn measured(
        &self,
        ctx: &I2c,
        crystal: u32,
        fill: u8,
        busy: u32,
        side: bool,
    ) -> Result<Invocation> {
        let mut memory = vec![
            known(ROM_INTERFACE_POINTER, 4, &words(&[CALLBACK_TABLE]))?,
            known(CALLBACK_TABLE, 16, &words(&self.callbacks))?,
        ];
        if !side {
            memory.push(known(ROM_PARAMETER_POINTER, 4, &words(&[self.parameter]))?);
        }
        let output = if side { DESTINATION } else { self.parameter } + 417;
        let (target, arguments) = if side {
            (
                ctx.probe("open_phy_calibration_trace_dcode"),
                vec![crystal, output],
            )
        } else {
            (
                ctx.probe("open_phy_trace_two_void_entries"),
                vec![
                    ctx.captured(1, "phy_pbus_clear_reg"),
                    ctx.captured(1, "phy_dcode_cal_init"),
                ],
            )
        };
        let mut result = self.enter(
            target,
            &arguments,
            memory,
            self.dcode_models(fill, busy),
            vec![selection(output, 8)],
        )?;
        result.calls = self.delays("frequency-delay", side);
        Ok(result)
    }
}

pub fn exercise(ctx: &mut I2c) -> Result<()> {
    let prefix = Prefix {
        parameter: ctx.parameter,
        memcpy: ctx.captured(1, "memcpy"),
        rom_delay: ctx.captured(1, "ets_delay_us"),
        // The ROM delay boundary is a captured symbol, not a declared probe.
        production_delay: ctx.captured(2, "ets_delay_us"),
        short_delay: ctx.probe("open_phy_trace_delay_event"),
        callbacks: [
            "phy_i2c_enter_critical",
            "phy_i2c_exit_critical",
            "phy_get_i2c_read_mask_new",
            "phy_get_i2c_hostid_new",
        ]
        .map(|n| ctx.root(n))
        .to_vec(),
    };
    let pbus = ctx.captured(1, "phy_pbus_clear_reg");
    let production = ctx.probe("open_phy_calibration_trace_pbus_clear");
    let (vendor, replacement) = (ctx.vendor.clone(), ctx.replacement.clone());
    // Both fills are one request; each case sets its own stack fill.
    {
        let (mut cases, mut expected) = (vec![], vec![]);
        for fill in FILLS {
            for initial in [0u32, 0xa5a5_5a58, 0x5a5a_a5a4] {
                for settle in [false, true] {
                    for busy in [0u32, 2] {
                        let m = pbus_models(initial, settle, busy);
                        let mut row = case(
                            format!(
                                "pbus-{initial:x}-{}-{busy}-{fill}",
                                if settle { "True" } else { "False" }
                            ),
                            prefix.invoke(pbus, m.clone(), settle, false)?,
                            Some(prefix.invoke(production, m, settle, true)?),
                            SessionReset::Cold,
                            false,
                        );
                        row.stack_fill = Some(fill);
                        cases.push(row);
                        expected.push((expected_writes(initial, settle), settle, busy));
                    }
                }
            }
        }
        for (batch, (rows, expectations)) in cases
            .chunks(cases.len())
            .zip(expected.chunks(expected.len()))
            .enumerate()
        {
            let records = ctx.submit_with(
                &format!("pbus-{batch}"),
                &vendor,
                Some(&replacement),
                None,
                rows.to_vec(),
                MAX_EVENTS,
                Some(ComparisonVerdict::Match),
            )?;
            for (i, (writes, settle, busy)) in expectations.iter().enumerate() {
                let i = i as u32;
                for side in [false, true] {
                    let low = returned_low(&records, i, side);
                    if side {
                        assert_eq!(low, Some(0));
                    }
                    let observed = events(&records, i, side);
                    assert_eq!(&writes_of(&observed), writes, "{batch} {i} {side}");
                    assert_eq!(
                        delays_of(&observed),
                        if *settle { vec![1, 2] } else { vec![] }
                    );
                    let polls = observed
                        .iter()
                        .filter(|e| {
                            matches!(
                                e,
                                ExecutionEvent::Read {
                                    address: PBUS_STATUS,
                                    ..
                                }
                            )
                        })
                        .count();
                    assert_eq!(polls as u32, 12 * (busy + 1));
                    assert!(all_complete(&records, i, side));
                }
            }
        }
    }
    // Real production timeout after zero, five or eleven completed commands.
    // Twenty thousand explicit busy responses exceed its own polling bound.
    for completed in [0u32, 5, 11] {
        let mut m = pbus_models(0, true, 0);
        if let DeviceBehavior::SequenceRead { runs, .. } = &mut m[1].behavior {
            *runs = vec![];
            if completed != 0 {
                runs.push(ReadRun {
                    value: 0,
                    count: completed,
                });
            }
            runs.push(ReadRun {
                value: 0x8000_0000,
                count: 20_000,
            });
        }
        let phase = prefix.invoke(production, m, false, true)?;
        let label = format!("pbus-timeout-{completed}");
        let mut row = case(label.clone(), phase, None, SessionReset::Cold, false);
        row.relation = None;
        let records = ctx.submit_with(
            &label,
            &replacement,
            None,
            None,
            vec![row],
            MAX_EVENTS,
            None,
        )?;
        assert_eq!(returned_low(&records, 0, false), Some(2));
        let observed = events(&records, 0, false);
        let stuck = observed
            .iter()
            .position(|e| {
                matches!(
                    e,
                    ExecutionEvent::Read {
                        address: PBUS_STATUS,
                        value: 0x8000_0000,
                        ..
                    }
                )
            })
            .expect("stuck status poll");
        assert!(observed[stuck..].iter().all(|e| matches!(
            e,
            ExecutionEvent::Read {
                address: PBUS_STATUS,
                ..
            }
        )));
        let status = models(&records, 0, false)
            .into_iter()
            .find(|m| m.id == "pbus-status")
            .expect("status model");
        assert!(
            status.issue.is_none()
                && status.remaining_reads > 0
                && status.status == ModelStatus::Incomplete
        );
        assert!(!observed.iter().any(|e| matches!(
            e,
            ExecutionEvent::Read {
                address: WORK_MODE | 0x2010_702c,
                ..
            } | ExecutionEvent::Write {
                address: WORK_MODE | 0x2010_702c,
                ..
            }
        )));
    }
    let (frequency_writes, nrx_writes) = dcode_frequency_writes();
    // Production performs an additional busy precheck before each read
    // command; the reviewed contract ignores port reads, and every other
    // read, write, fence, delay and the measured bytes compare.
    let dcode = ctx.review_pair(
        "prefix-dcode",
        ctx.input_endpoint(2, "open_phy_trace_two_void_entries")?,
        ctx.input_endpoint(2, "open_phy_calibration_trace_dcode")?,
        port_polling(MAX_EVENTS),
        "PBus clear then DCODE calibration under explicit crystal, CKGEN and busy inputs",
    )?;
    let dcode_rows =
        |ctx: &I2c, crystal: u32, fill: u8, busy: u32, label: &str| -> Result<Vec<ExecutionCase>> {
            Ok(with_stack_fill(
                vec![
                    case(
                        "initialize-parameters",
                        prefix.setup(crystal as u8, false)?,
                        Some(prefix.setup(crystal as u8, true)?),
                        SessionReset::Cold,
                        false,
                    ),
                    case(
                        label,
                        prefix.measured(ctx, crystal, fill, busy, false)?,
                        Some(prefix.measured(ctx, crystal, fill, busy, true)?),
                        SessionReset::Warm,
                        true,
                    ),
                ],
                fill,
            ))
        };
    // The raw event difference stays visible without the contract.
    let raw = dcode_rows(ctx, 0, 0x5a, 0, "dcode-0-90-0")?;
    ctx.submit_with(
        "dcode-raw-polling-difference",
        &vendor,
        Some(&replacement),
        None,
        raw,
        MAX_EVENTS,
        Some(ComparisonVerdict::Diff),
    )?;
    let mut labels = vec![];
    let mut rows = vec![];
    for crystal in 0..4u32 {
        for fill in FILLS {
            for busy in [0u32, 2] {
                let label = format!("dcode-{crystal}-{fill}-{busy}");
                let mut profile = dcode_rows(ctx, crystal, fill, busy, &label)?;
                profile[1].relation.as_mut().unwrap().effects = Some(dcode.clone());
                rows.extend(profile);
                labels.push((label, fill));
            }
        }
    }
    let records = ctx.submit_with(
        "dcode",
        &vendor,
        Some(&replacement),
        None,
        rows,
        MAX_EVENTS,
        Some(ComparisonVerdict::Match),
    )?;
    for (i, (label, fill)) in labels.iter().enumerate() {
        let (label, fill, measured) = (label.as_str(), *fill, 2 * i as u32 + 1);
        for side in [false, true] {
            let low = returned_low(&records, measured, side);
            if side {
                assert_eq!(low, Some(0), "{label}");
            }
            assert_eq!(
                output(&records, measured, side),
                [0, 63, 31, 32, 32, 31, 63, 0],
                "{label} {side}"
            );
            let observed = events(&records, measured, side);
            assert_eq!(delays_of(&observed), [1, 10, 1, 10, 1, 10, 1, 10]);
            let writes = writes_of(&observed);
            assert_eq!(writes[..28], expected_writes(0, false)[..], "{label}");
            let at = |address: u32| {
                writes
                    .iter()
                    .filter(|(a, _)| *a == address)
                    .map(|(_, v)| *v)
                    .collect::<Vec<_>>()
            };
            assert_eq!(at(FREQUENCY_CONTROL), frequency_writes, "{label}");
            assert_eq!(at(0x2010_7848), nrx_writes, "{label}");
            let ports: Vec<_> = writes
                .iter()
                .filter(|(a, _)| matches!(*a, I2C_PORT_0 | I2C_PORT_1))
                .copied()
                .collect();
            assert_eq!(
                ports,
                dcode_commands(fill)
                    .into_iter()
                    .map(|v| (I2C_PORT_1, v))
                    .collect::<Vec<_>>(),
                "{label}"
            );
            let ckgen = models(&records, measured, side)
                .into_iter()
                .find(|m| m.id == "ckgen")
                .expect("ckgen model");
            assert_eq!(
                ckgen.commands,
                Some(CommandObservation {
                    issued: 40,
                    completed: 40,
                    resets: 0,
                    aborted: 0,
                    pending: 0,
                    scripted_reads: 8
                }),
                "{label}"
            );
            assert!(
                models(&records, measured, side)
                    .iter()
                    .all(|m| m.status == ModelStatus::Complete)
            );
        }
    }
    for (ready, busy, expected_commands) in
        [(false, 0u32, 0usize), (true, 65535, 1), (true, 6000, 2)]
    {
        let label = format!(
            "dcode-failure-{}-{busy}",
            if ready { "True" } else { "False" }
        );
        let initial = prefix.setup(0, true)?;
        let mut failed = prefix.measured(ctx, 0, 0x5a, busy, true)?;
        failed.calls[0].responses.truncate(2);
        for model in &mut failed.models {
            if model.id == "channel-ready" {
                model.behavior = DeviceBehavior::ConstantRead {
                    address: CHANNEL_STATUS,
                    width: 4,
                    value: if ready { 0x100 } else { 0 },
                };
            }
            if model.id == "ckgen"
                && let DeviceBehavior::CommandBank(bank) = &mut model.behavior
            {
                bank.cells.retain(|c| c.reads.is_none());
            }
        }
        let mut rows = vec![
            case(
                "initialize-parameters",
                initial,
                None,
                SessionReset::Cold,
                false,
            ),
            case(label.clone(), failed, None, SessionReset::Warm, false),
        ];
        for row in &mut rows {
            row.relation = None;
        }
        let records = ctx.submit_with(
            &label,
            &replacement,
            None,
            Some(0xa5),
            rows,
            MAX_EVENTS,
            None,
        )?;
        assert_eq!(
            returned_low(&records, 1, false),
            Some(if ready { 5 } else { 7 }),
            "{label}"
        );
        assert_eq!(output(&records, 1, false), [0xa5; 8], "{label}");
        let observed = events(&records, 1, false);
        let issued = observed
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ExecutionEvent::Write {
                        address: I2C_PORT_0 | I2C_PORT_1,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(issued, expected_commands);
        let ckgen = models(&records, 1, false)
            .into_iter()
            .find(|m| m.id == "ckgen")
            .expect("ckgen model");
        let commands = ckgen.commands.expect("command accounting");
        assert!(
            ckgen.issue.is_none()
                && commands.issued == expected_commands as u64
                && commands.pending == u32::from(ready)
        );
        if !ready {
            assert!(!observed.iter().any(|e| matches!(
                e,
                ExecutionEvent::Write {
                    address: 0x2010_7848,
                    ..
                }
            )));
        }
    }
    let mut original = vec![
        case(
            "initialize-parameters",
            prefix.setup(0, false)?,
            Some(prefix.setup(0, true)?),
            SessionReset::Cold,
            false,
        ),
        case(
            "measured",
            prefix.measured(ctx, 0, 0x5a, 0, false)?,
            Some(prefix.measured(ctx, 0, 0x5a, 0, true)?),
            SessionReset::Warm,
            true,
        ),
    ];
    original[1].relation.as_mut().unwrap().effects = Some(dcode.clone());
    let mut changed = original.clone();
    if let DeviceBehavior::CommandBank(bank) = &mut changed[1]
        .replacement
        .as_mut()
        .unwrap()
        .models
        .last_mut()
        .unwrap()
        .behavior
    {
        bank.cells[1].reads.as_mut().unwrap()[0] = 0xc1;
    }
    let records = ctx.submit_with(
        "dcode-changed-sample",
        &vendor,
        Some(&replacement),
        Some(0x5a),
        changed,
        MAX_EVENTS,
        Some(ComparisonVerdict::Diff),
    )?;
    assert!(records.iter().any(|r| matches!(r,
        ExecutionEvidence::Comparison { case: 1, result } if matches!(result.difference, Some(ComparisonDifference::Memory { .. })))));
    let mut unknown = original.clone();
    unknown[1]
        .vendor
        .memory
        .last_mut()
        .unwrap()
        .seed
        .bytes
        .clear();
    let records = ctx.submit_with(
        "dcode-unknown-parameters",
        &vendor,
        Some(&replacement),
        Some(0x5a),
        unknown,
        MAX_EVENTS,
        Some(ComparisonVerdict::Incomplete),
    )?;
    // The unknown parameter pointer loads an unknown address, which is never
    // dereferenced.
    assert!(matches!(
        stop(&records, 1, false),
        ExecutionStop::Incomplete {
            reason: ExecutionGap::UnknownRegister { .. },
            ..
        }
    ));
    // The contract's occurrence bounds exceed a one-event capacity; the
    // exhaustion under test precedes any effect comparison.
    original[1].relation.as_mut().unwrap().effects = None;
    let limited = crate::session::request(&vendor, Some(&replacement), Some(0x5a), original, 1);
    ctx.capacity_failure("dcode-capacity", &limited)
}

pub fn writes_of(events: &[ExecutionEvent]) -> Vec<(u32, u32)> {
    events
        .iter()
        .filter_map(|e| match e {
            ExecutionEvent::Write { address, value, .. } => Some((*address, *value)),
            _ => None,
        })
        .collect()
}

pub fn delays_of(events: &[ExecutionEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|e| match e {
            ExecutionEvent::DelayMicros { value } => Some(*value),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbus_oracle_issues_twelve_commands_and_optional_settle_pulse() {
        assert_eq!(expected_writes(0, false).len(), 2 + 24 + 2);
        let settle = expected_writes(0xa5a5_5a58, true);
        assert_eq!(settle.len(), 31);
        assert_eq!(
            settle[28..],
            [
                (0x2010_702c, 0x32a5_5a58),
                (0x2010_702c, 0x32a5_5a58 | 0x0080_0000),
                (0x2010_702c, 0x3225_5a58)
            ]
        );
    }

    #[test]
    fn dcode_oracles_cover_four_channels_and_retained_ckgen_bytes() {
        let (frequency, nrx) = dcode_frequency_writes();
        assert_eq!(frequency.len(), 12);
        assert_eq!(frequency[0], 0x4128_000c);
        assert_eq!(nrx[0], 0x1600_0000 | ((80 << 22) / 2412));
        let commands = dcode_commands(0xff);
        assert_eq!(commands.len(), 40);
        assert_eq!(commands[..2], [0x0400_1362, 0x0500_1362 | 0xbf << 16]);
    }
}
