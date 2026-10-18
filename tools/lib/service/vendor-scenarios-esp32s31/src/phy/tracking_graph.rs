//! Parent call graphs with explicit modeled children: the order, ABI words
//! and guards of each parent's direct calls, and the calibration's reference
//! and flag commits. Modeled child completions are characterization only;
//! the complete-parent claim is the tracking comparison with real children.
use crate::evidence::{events, output};
use crate::harness::{Result, case, known, selection, words};
use crate::i2c::all_complete;
use crate::layout::*;
use crate::session::image_symbol;
use crate::tracking::Tracking;
use blobray_domain::{
    CallBinding, CallBoundary, CallCapture, CallDeclaration, CallRepetition, CallResponse,
    ExecutionCase, ExecutionEvent, ExecutionEvidence, ObservedCallTarget, ObservedWord,
    RegionLifetime, SessionReset,
};

/// ABI words recorded per direct call; every expected child takes at most three.
const CALL_WORDS: u16 = 3;
/// Stack fill of the characterizations.
const GRAPH_FILL: u8 = FILLS[1];
/// Fixture callback table the calibration's restore slot is read from.
const CALLBACK_FIXTURE: u32 = CALLBACK_TABLE;
const RESTORE_SLOT: u32 = 0x30;
/// Retained calibration-control word the calibration reads.
const CALIBRATION_CONTROL: u32 = WORK_MODE;
const CALIBRATION_CONTROL_VALUE: u32 = 0xa5a5_ffff;
/// Parent `phy_param` guard offsets; either set blocks every child.
const PARENT_GUARDS: [usize; 2] = [23, 405];
/// Parent inputs: power, RFPLL and power-tracking enables, calibration
/// disable and the temperatures every child reads.
const PARENT_ENABLES: [(usize, u8); 3] = [(9, 1), (11, 1), (23, 0)];
const PARENT_RFPLL: usize = 10;
const PARENT_CALIBRATION_DISABLED: usize = 402;
const PARENT_TEMPERATURES: [usize; 5] = [0, 4, 72, 304, 400];
const PARENT_TEMPERATURE: u16 = 20;
/// Calibration inputs and committed words.
const CURRENT: usize = 0;
const TRANSMIT_REFERENCE: usize = 72;
const COMMON_REFERENCE: usize = 400;
const GUARD_FLAGS: usize = 164;
const CHANNEL: usize = 284;
const BANDWIDTH: usize = 287;
const OVERRIDE: usize = 432;
const FLAGS: usize = 0x1e6;
const CALIBRATION_TEMPERATURE: i16 = 50;
const STALE_REFERENCE: i16 = 21;
const DUE_REFERENCE: i16 = 20;
const FLAGS_IDLE: u16 = 0xa000;
const RX_DONE: u16 = 8;
const TX_DONE: u16 = 16;
/// Forced digital gain of the TX bracket.
const FORCED_GAIN: u32 = (-120i32) as u32;

type Expected = Vec<(&'static str, Vec<u32>)>;

/// Direct calls of the parent: order and ABI words.
fn parent_expected(
    wifi: bool,
    shared: bool,
    rfpll: bool,
    calibration: bool,
    guarded: bool,
) -> Expected {
    let mut expected: Expected = vec![("phy_i2c_enter_critical", vec![])];
    if !guarded {
        if rfpll {
            expected.push(("phy_rfpll_cap_track_new", vec![1]));
        }
        if shared {
            expected.push(("phy_bt_track_tx_power_new", vec![1, 1]));
        }
        if wifi {
            expected.push(("phy_tx_i2c_track", vec![]));
            expected.push(("phy_wifi_track_tx_power_new", vec![1, 1]));
        }
        if calibration {
            expected.push(("phy_cal_param_track", vec![1, wifi.into(), shared.into()]));
        }
        expected.push(("phy_tsens_temp_read", vec![]));
    }
    expected.push(("phy_i2c_exit_critical", vec![]));
    expected
}

/// Direct calls of the combined calibration: RX and TX brackets, each with
/// its own grant and force-TX/RX pair, and the restore callback.
fn calibration_expected(wifi: bool, shared: bool, rx: bool, tx: bool) -> Expected {
    let restore = "phy_txgain_comp_pacfg_";
    let mut expected: Expected = vec![("phy_abs_temp", vec![])];
    if rx {
        expected.extend([
            ("phy_acquire_grant_protect", vec![]),
            ("phy_force_txrx_off_new", vec![1]),
            ("phy_pbus_clear_reg", vec![]),
            ("phy_dcode_cal_init", vec![]),
            ("phy_set_rx_gain_table", vec![2437, 0]),
            ("phy_chip_set_chan", vec![13, 1]),
            ("phy_mac_enable_bb", vec![]),
            ("phy_force_txrx_off_new", vec![0]),
            (restore, vec![1]),
            ("phy_release_grant_protect", vec![]),
        ]);
    }
    expected.push(("phy_abs_temp", vec![]));
    if tx {
        expected.extend([
            ("phy_acquire_grant_protect", vec![]),
            ("phy_dis_hw_set_freq_new", vec![]),
            ("phy_force_txrx_off_new", vec![1]),
            ("phy_force_dig_gain", vec![1, FORCED_GAIN, FORCED_GAIN]),
            ("phy_pbus_clear_reg", vec![]),
            ("phy_bb_cbw_chan_cfg", vec![0]),
        ]);
        if wifi {
            expected.extend([
                ("phy_txdc_cal_pwdet_init", vec![0, 0, 0]),
                ("phy_wifi_set_tx_gain_new", vec![13, 0]),
            ]);
        }
        if shared {
            expected.extend([
                ("phy_txdc_cal_pwdet_init", vec![0, 0, 1]),
                ("phy_bt_set_tx_gain_new", vec![0]),
            ]);
        }
        expected.extend([
            ("phy_bb_cbw_chan_cfg", vec![1]),
            ("phy_mac_enable_bb", vec![]),
            ("phy_force_dig_gain", vec![0, FORCED_GAIN, FORCED_GAIN]),
            ("phy_force_txrx_off_new", vec![0]),
            ("phy_en_hw_set_freq_new", vec![]),
            (restore, vec![1]),
            ("phy_release_grant_protect", vec![]),
        ]);
    }
    expected
}

/// Symbols that execute for real in the calibration characterization.
const REAL_CHILDREN: [&str; 3] = [
    "phy_abs_temp",
    "phy_acquire_grant_protect",
    "phy_release_grant_protect",
];

struct Graph<'a> {
    ctx: &'a Tracking,
}

impl Graph<'_> {
    /// Address and size of a linked image symbol, or of a ROM symbol.
    fn symbol(&self, name: &str) -> (u32, u64) {
        image_symbol(
            &self.ctx.run.join("image/image.elf"),
            &self.ctx.run.join("image-symbols.txt"),
            name,
        )
        .unwrap_or_else(|_| (self.ctx.sym(1, name), 0))
    }

    /// One zero-returning modeled response per expected call of each child.
    fn models(&self, expected: &Expected, real: &[&str]) -> Vec<CallDeclaration> {
        let mut declarations: Vec<CallDeclaration> = vec![];
        for (name, _) in expected.iter().filter(|(n, _)| !real.contains(n)) {
            let response = CallResponse {
                return_words: [Some(0), Some(0)],
                outputs: vec![],
                allocation: None,
                delay_micros: None,
            };
            if let Some(existing) = declarations.iter_mut().find(|d| d.id == *name) {
                existing.responses.push(response);
                continue;
            }
            declarations.push(CallDeclaration {
                id: (*name).into(),
                applicability: "explicit no-effect child isolating the vendor parent".into(),
                lifetime: RegionLifetime::Phase,
                binding: CallBinding {
                    address: self.symbol(name).0,
                    boundary: CallBoundary::CapturedCode,
                    allow_tail: true,
                },
                argument_words: CALL_WORDS,
                responses: vec![response],
                repetition: CallRepetition::Finite,
            });
        }
        declarations
    }

    /// Parameter setup, then `root` with modeled children and call capture.
    #[allow(clippy::too_many_arguments)]
    fn rows(
        &self,
        name: String,
        root: &str,
        arguments: &[u32],
        parameters: &[u8],
        expected: &Expected,
        real: &[&str],
        memory: Vec<blobray_domain::ExecutionRegion>,
        models: Vec<blobray_domain::DeviceDeclaration>,
        prelude: Option<blobray_domain::Invocation>,
    ) -> Vec<ExecutionCase> {
        let mut phase = self.ctx.enter(
            self.ctx.root(root),
            arguments,
            memory,
            vec![selection(self.ctx.parameter, PHY_PARAM_BYTES)],
            models,
        );
        phase.calls = self.models(expected, real);
        phase.observe_calls = Some(CallCapture {
            include_tail: true,
            argument_words: CALL_WORDS,
            overrides: vec![],
        });
        let mut rows = vec![case(
            "initialize",
            self.ctx.setup(parameters, false),
            None,
            SessionReset::Cold,
            false,
        )];
        if let Some(prelude) = prelude {
            rows.push(case("prelude", prelude, None, SessionReset::Warm, false));
        }
        rows.push(case(name, phase, None, SessionReset::Warm, false));
        for row in &mut rows {
            row.relation = None;
            row.stack_fill = Some(GRAPH_FILL);
        }
        rows
    }

    /// Direct calls of `root` in `case`: calls whose site lies in its body.
    fn direct_calls(
        &self,
        root: &str,
        records: &[ExecutionEvidence],
        case: u32,
    ) -> Vec<(String, Vec<Option<u32>>)> {
        let (start, size) = self.symbol(root);
        let body = u64::from(start)..u64::from(start) + size;
        let observed = events(records, case, false);
        let mut calls = vec![];
        for (index, event) in observed.iter().enumerate() {
            let ExecutionEvent::CallTransfer {
                site,
                target,
                words,
                target_kind,
                ..
            } = event
            else {
                continue;
            };
            if !body.contains(&u64::from(*site)) {
                continue;
            }
            assert!(
                matches!(
                    target_kind,
                    ObservedCallTarget::CapturedCode | ObservedCallTarget::CallModel
                ),
                "call to {target:#x} outside captured code"
            );
            let arguments = observed[index + 1..][..usize::from(*words)]
                .iter()
                .map(|e| match e {
                    ExecutionEvent::TransferArgument {
                        value: ObservedWord::Known { value },
                        ..
                    } => Some(*value),
                    // Words beyond a child's arity may be unknown.
                    ExecutionEvent::TransferArgument { .. } => None,
                    other => panic!("missing call argument record {other:?}"),
                })
                .collect();
            calls.push((self.name_of(*target), arguments));
        }
        calls
    }

    fn name_of(&self, address: u32) -> String {
        let listing = std::fs::read_to_string(self.ctx.run.join("image-symbols.txt")).unwrap();
        listing
            .lines()
            .filter_map(|l| {
                let mut parts = l.split(' ');
                let name = parts.next()?;
                let value = u32::from_str_radix(parts.next()?, 16).ok()?;
                (value == address).then(|| name.to_owned())
            })
            .next()
            .unwrap_or_else(|| {
                if self.ctx.sym(1, "phy_txgain_comp_pacfg_") == address {
                    "phy_txgain_comp_pacfg_".into()
                } else {
                    format!("{address:#x}")
                }
            })
    }
}

fn assert_calls(label: &str, actual: &[(String, Vec<Option<u32>>)], expected: &Expected) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{label}: call count {actual:?}"
    );
    for ((name, arguments), (expected_name, expected_arguments)) in actual.iter().zip(expected) {
        assert_eq!(name, expected_name, "{label}: call order");
        let expected_arguments: Vec<_> = expected_arguments.iter().map(|v| Some(*v)).collect();
        assert_eq!(
            &arguments[..expected_arguments.len()],
            &expected_arguments[..],
            "{label}: ABI {name}"
        );
    }
}

/// Parent and calibration call graphs, each one vendor characterization.
pub fn exercise(ctx: &mut Tracking) -> Result<()> {
    let (mut rows, mut parent_cases) = (vec![], vec![]);
    let graph = Graph { ctx };
    for (wifi, shared) in [(false, false), (true, false), (false, true), (true, true)] {
        for rfpll in [false, true] {
            for calibration in [false, true] {
                for guard in [None, Some(PARENT_GUARDS[0]), Some(PARENT_GUARDS[1])] {
                    let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
                    for (offset, value) in PARENT_ENABLES {
                        data[offset] = value;
                    }
                    data[PARENT_RFPLL] = rfpll.into();
                    data[PARENT_CALIBRATION_DISABLED] = (!calibration).into();
                    for offset in PARENT_TEMPERATURES {
                        data[offset..offset + 2].copy_from_slice(&PARENT_TEMPERATURE.to_le_bytes());
                    }
                    if let Some(guard) = guard {
                        data[guard] = 1;
                    }
                    let expected =
                        parent_expected(wifi, shared, rfpll, calibration, guard.is_some());
                    let label = format!(
                        "parent-graph-wifi{}-bt{}-rfpll{}-cal{}-guard{guard:?}",
                        u8::from(wifi),
                        u8::from(shared),
                        u8::from(rfpll),
                        u8::from(calibration)
                    )
                    .to_lowercase();
                    rows.extend(graph.rows(
                        label.clone(),
                        "phy_param_track_tot",
                        &[wifi.into(), shared.into()],
                        &data,
                        &expected,
                        &[],
                        vec![],
                        vec![],
                        None,
                    ));
                    parent_cases.push((label, expected));
                }
            }
        }
    }
    let (mut calibration_rows, mut calibration_cases) = (vec![], vec![]);
    let restore = ctx.sym(1, "phy_txgain_comp_pacfg_");
    let memcpy = ctx.sym(1, "memcpy");
    let callbacks = graph.symbol("g_phyFuns").0;
    for (wifi, shared) in [(false, false), (true, false), (false, true), (true, true)] {
        for (rx, tx) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
            let reference = |due: bool| if due { DUE_REFERENCE } else { STALE_REFERENCE };
            for (offset, value) in [
                (CURRENT, CALIBRATION_TEMPERATURE),
                (TRANSMIT_REFERENCE, reference(tx)),
                (COMMON_REFERENCE, reference(rx)),
            ] {
                data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
            }
            data[GUARD_FLAGS..GUARD_FLAGS + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            data[CHANNEL..CHANNEL + 2].copy_from_slice(&13u16.to_le_bytes());
            data[BANDWIDTH] = 1;
            data[OVERRIDE] = 0;
            data[FLAGS..FLAGS + 2].copy_from_slice(&FLAGS_IDLE.to_le_bytes());
            let expected = calibration_expected(wifi, shared, rx, tx);
            let label = format!(
                "calibration-graph-wifi{}-bt{}-rx{}-tx{}",
                u8::from(wifi),
                u8::from(shared),
                u8::from(rx),
                u8::from(tx)
            );
            let mut table = vec![0u8; (RESTORE_SLOT + 4) as usize];
            table[RESTORE_SLOT as usize..].copy_from_slice(&restore.to_le_bytes());
            let memory = vec![known(CALLBACK_FIXTURE, RESTORE_SLOT + 4, &table)?];
            // The callback pointer is image data: copy the fixture address in.
            let prelude = ctx.enter(
                memcpy,
                &[callbacks, INPUT, 4],
                vec![known(INPUT, 4, &words(&[CALLBACK_FIXTURE]))?],
                vec![],
                vec![],
            );
            let models = vec![register_bank(
                "calibration-control",
                "explicit retained calibration-control word",
                vec![(CALIBRATION_CONTROL, CALIBRATION_CONTROL_VALUE)],
            )];
            calibration_rows.extend(graph.rows(
                label.clone(),
                "phy_cal_param_track",
                &[0, wifi.into(), shared.into()],
                &data,
                &expected,
                &REAL_CHILDREN,
                memory,
                models,
                Some(prelude),
            ));
            calibration_cases.push((label, expected, rx, tx));
        }
    }
    let records = graph_request(ctx, "parent-graph", rows)?;
    for (i, (label, expected)) in parent_cases.iter().enumerate() {
        let case = 2 * i as u32 + 1;
        assert!(all_complete(&records, case, false), "{label}");
        let graph = Graph { ctx };
        assert_calls(
            label,
            &graph.direct_calls("phy_param_track_tot", &records, case),
            expected,
        );
        assert!(
            !events(&records, case, false).iter().any(|e| matches!(
                e,
                ExecutionEvent::Read { .. } | ExecutionEvent::Write { .. }
            )),
            "{label}: unexpected parent MMIO"
        );
    }
    let records = graph_request(ctx, "calibration-graph", calibration_rows)?;
    for (i, (label, expected, rx, tx)) in calibration_cases.iter().enumerate() {
        let case = 3 * i as u32 + 2;
        assert!(all_complete(&records, case, false), "{label}");
        let graph = Graph { ctx };
        assert_calls(
            label,
            &graph.direct_calls("phy_cal_param_track", &records, case),
            expected,
        );
        let state = output(&records, case, false);
        let word = |offset: usize| i16::from_le_bytes([state[offset], state[offset + 1]]);
        let committed = |done: bool| {
            if done {
                CALIBRATION_TEMPERATURE
            } else {
                STALE_REFERENCE
            }
        };
        assert_eq!(
            word(COMMON_REFERENCE),
            committed(*rx),
            "{label}: RX reference"
        );
        assert_eq!(
            word(TRANSMIT_REFERENCE),
            committed(*tx),
            "{label}: TX reference"
        );
        assert_eq!(
            word(FLAGS) as u16,
            FLAGS_IDLE | if *rx { RX_DONE } else { 0 } | if *tx { TX_DONE } else { 0 },
            "{label}: flags"
        );
    }
    Ok(())
}

fn graph_request(
    ctx: &mut Tracking,
    label: &str,
    rows: Vec<ExecutionCase>,
) -> Result<Vec<ExecutionEvidence>> {
    let vendor = ctx.vendor.clone();
    let request = crate::session::request(&vendor, None, None, rows, MAX_EVENTS);
    Ok(ctx.submit(label, &request, None)?.records.clone())
}
