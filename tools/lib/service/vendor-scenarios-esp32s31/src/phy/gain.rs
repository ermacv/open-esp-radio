//! Captured Wi-Fi/BT gain arithmetic and complete publication comparison.
//!
//! Explicit finite software inputs only; no RF, protocol or whole-TXCAL
//! qualification. Private inputs and retained requests/evidence belong in the
//! selected ignored output.
use crate::evidence::{
    Access, Effect, PhyEffect, calls, effects, events, has_events, output, phy_effects, stop,
};
use crate::harness::{
    Budget, Buffer, Input, Result, case, data_request, filled, invalid, known, named_object,
    named_section, region, selection, symbol, words,
};
use crate::layout::*;
use crate::phy::{PhyImage, Right, image_layout};
use crate::session::Session;
use blobray_domain::{
    CallCapture, ComparisonVerdict, DataSelector, DeviceBehavior, DeviceDeclaration,
    ExecutionEvent, ExecutionRequest, ExecutionStop, Invocation, LinkRequest, ObservedCallTarget,
    RegionLifetime, RegisterCell, SessionReset,
};
use std::path::PathBuf;

pub const OBJECT_SHA: &str = "86f09f42ad559b84e36906060b7e2b82f739d68873f8c780df96acda1d195e5a";
/// Independently extracted with llvm-ar/llvm-objcopy from the pinned archive;
/// the native export must match before these data may support a comparison.
pub const COEFFICIENT_SHA: &str =
    "748936005c8ba31c1b826d0d0d2bb56f8982548c7d9826fed90930100d79f7a1";
pub const CURVES: [[u8; 6]; 4] = [
    [0; 6],
    [0, 5, 10, 15, 20, 25],
    [127, 128, 255, 1, 2, 3],
    [255, 0, 127, 128, 7, 9],
];

pub fn signed8(value: i32) -> i32 {
    (value & 127) - (value & 128)
}

fn halfwords(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect()
}

/// Independent interval oracle: choose each interval without cursor state.
pub fn arithmetic(
    coefficients: &[u8],
    curve: &[u8],
    base: i32,
    correction: i32,
    channel: Option<u32>,
) -> Vec<u8> {
    let low = halfwords(&coefficients[0..36]);
    let mid = halfwords(&coefficients[36..72]);
    let high: Vec<i32> = halfwords(&coefficients[72..108])
        .into_iter()
        .map(|v| i32::from(v as i16))
        .collect();
    let (targets, interpolation): (Vec<i32>, i32) = match channel {
        None => (
            (0..16)
                .map(|i| base - correction + (-96 + 12 * i).clamp(-72, 80))
                .collect(),
            signed8(i32::from(curve[1])),
        ),
        Some(channel) => {
            let channel = channel as i32;
            let interpolation = if channel <= 6 {
                let (a, b) = (signed8(i32::from(curve[0])), signed8(i32::from(curve[1])));
                signed8((b - a) * (channel - 1) / 5 + a)
            } else if channel <= 11 {
                let (a, b) = (signed8(i32::from(curve[1])), signed8(i32::from(curve[2])));
                signed8((b - a) * (channel - 6) / 5 + a)
            } else {
                signed8(signed8(i32::from(curve[2])) + 2)
            };
            (
                (0..32).map(|i| base - correction + 84 - 4 * i).collect(),
                interpolation,
            )
        }
    };
    let indexes: Vec<usize> = targets
        .iter()
        .map(|target| high.iter().position(|h| target >= h).unwrap_or(17))
        .collect();
    let mut residuals: Vec<i32> = targets
        .iter()
        .zip(&indexes)
        .map(|(t, i)| t - high[*i] - interpolation)
        .collect();
    if channel.is_none() {
        for v in &mut residuals {
            *v = (*v).clamp(-60, 24);
        }
    }
    let mut result: Vec<u8> = residuals.iter().map(|v| (v & 255) as u8).collect();
    result.extend(indexes.iter().flat_map(|i| mid[*i].to_le_bytes()));
    result.extend(indexes.iter().flat_map(|i| low[*i].to_le_bytes()));
    result
}

/// Force and release sequences of an outermost `phy_force_txrx_off_new`
/// pair over a cleared mode word: bits 11:8 become 8, then 10, then 2, then
/// 0, each write followed by a one-microsecond settle.
pub fn force_txrx_pair() -> Vec<PhyEffect> {
    let mut effects = vec![];
    let mut value = 0;
    for mode in [0x800, 0xa00, 0x200, 0] {
        effects.extend([
            PhyEffect::Read(PBUS_STATUS, value),
            PhyEffect::Write(PBUS_STATUS, mode),
            PhyEffect::Delay(1),
        ]);
        value = mode;
    }
    effects
}

pub fn phy_effect((access, address, value): Effect) -> PhyEffect {
    match access {
        Access::Read => PhyEffect::Read(address, value),
        Access::Write => PhyEffect::Write(address, value),
    }
}

/// Retained force-TX/RX mode word of `phy_force_txrx_off_new`, cleared.
pub fn force_txrx_model() -> DeviceDeclaration {
    DeviceDeclaration {
        id: "force-txrx-mode".into(),
        applicability: "explicit retained force-TX/RX mode word".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: vec![RegisterCell {
                address: PBUS_STATUS,
                width: 4,
                value: 0,
            }],
        },
    }
}

pub fn gain_models(base: u32, fill: u8) -> Vec<DeviceDeclaration> {
    vec![
        DeviceDeclaration {
            id: "gain-base".into(),
            applicability: "explicit gain bank base".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::ConstantRead {
                address: GAIN_BASE,
                width: 4,
                value: (base << 24) | 0x005a_a55a,
            },
        },
        DeviceDeclaration {
            id: "gain-ports".into(),
            applicability: "retained index and three passive data ports; no gain algorithm".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: std::iter::once(GAIN_INDEX)
                    .chain(GAIN_DATA)
                    .map(|address| RegisterCell {
                        address,
                        width: 4,
                        value: if address == GAIN_INDEX {
                            fill_word(fill)
                        } else {
                            0
                        },
                    })
                    .collect(),
            },
        },
    ]
}

/// Instruction-derived packed-word oracle, independent of the device model.
pub fn publication(
    seed_words: &[u32],
    config: u32,
    calculated: &[u8],
    count: usize,
    base: u32,
    fill: u8,
    bluetooth: bool,
) -> Vec<Effect> {
    let bb = halfwords(&calculated[count..count * 3]);
    let rf = halfwords(&calculated[count * 3..count * 5]);
    // The publisher reads its four seed halfwords from the seed words and the
    // calculated bytes that follow them in memory, so baseband indices 3 and 4
    // select calculated bytes.
    let seed_data: Vec<u8> = words(seed_words)
        .into_iter()
        .chain(calculated.iter().copied())
        .collect();
    let mut result = vec![(Access::Read, GAIN_BASE, (base << 24) | 0x005a_a55a)];
    let mut control = fill_word(fill);
    for i in 0..count {
        let index = match bb[i] {
            128 => 1,
            256 => 2,
            32 => 3,
            160 => 4,
            _ => 0,
        };
        let field = |n: usize| {
            u64::from(u16::from_le_bytes([
                seed_data[index * 8 + n * 2],
                seed_data[index * 8 + n * 2 + 1],
            ]))
        };
        let (a, b, c, d) = (field(0), field(1), field(2), field(3));
        let (bb, rf) = (u64::from(bb[i]), u64::from(rf[i]));
        let word0 = ((c << 22) | (b << 31) | (d << 13) | u64::from(config & 8191)) as u32;
        let word1 = ((a << 8)
            | (b >> 1)
            | (((bb >> 6) & 255) << 17)
            | ((rf & 7) << 31)
            | ((bb & 63) << 20)
            | 0x1000_0000) as u32;
        let word2 = (((rf >> 1) & 31) | (u64::from(calculated[i]) << 15) | 0x7f80) as u32;
        let slot = (base + if bluetooth { 32 } else { 0 } + i as u32) & 255;
        // The captured ROM publisher writes data first, then updates its index.
        result.extend([
            (Access::Write, GAIN_DATA[0], word0),
            (Access::Write, GAIN_DATA[1], word1),
            (Access::Write, GAIN_DATA[2], word2),
            (Access::Read, GAIN_INDEX, control),
        ]);
        control = (control & 0xfff0_0000) | 0x80000 | (slot << 11);
        result.push((Access::Write, GAIN_INDEX, control));
    }
    result
}

pub struct Options {
    pub library: PathBuf,
    pub rom: PathBuf,
    pub production: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub budget: Budget,
    pub rftest: Option<PathBuf>,
    /// Point mutants applied to the loaded production image of every comparison.
    pub patches: Vec<blobray_application::in_process::ImagePatch>,
}

/// Linked captured gain image, compiled production and retained evidence.
pub struct Gain {
    pub image: PhyImage,
    pub coefficients: Vec<u8>,
    wifi_request: Option<ExecutionRequest>,
    bluetooth_request: Option<ExecutionRequest>,
    publication_request: Option<ExecutionRequest>,
    coefficient_address: Option<u32>,
}

impl std::ops::Deref for Gain {
    type Target = PhyImage;
    fn deref(&self) -> &PhyImage {
        &self.image
    }
}
impl std::ops::DerefMut for Gain {
    fn deref_mut(&mut self) -> &mut PhyImage {
        &mut self.image
    }
}

impl Gain {
    pub fn new(options: &Options) -> Result<Self> {
        let mut inputs = vec![
            Input {
                role: "phy",
                path: &options.library,
                sha256: Some(crate::artifacts::sha256("libphy")?),
            },
            Input {
                role: "rom",
                path: &options.rom,
                sha256: Some(crate::artifacts::sha256("rom")?),
            },
            Input {
                role: "production",
                path: &options.production,
                sha256: None,
            },
        ];
        if let Some(rftest) = &options.rftest {
            inputs.push(Input {
                role: "rftest",
                path: rftest,
                sha256: Some(crate::artifacts::sha256("librftest")?),
            });
        }
        let session = Session::start(
            &options.output,
            options.budget,
            &inputs,
            "captured Wi-Fi/BT gain, calibration storage and RF-test policy; no RF qualification",
            &options.patches,
        )?;
        let inventory = &session.inventory;
        let object = named_object(inventory, 0, "phy_tx_gain.o")?;
        let section = named_section(object, ".rodata")?;
        let request = data_request(
            object,
            DataSelector::Section {
                section: section.index,
                offset: 0,
                length: 216,
            },
        );
        let exported = session.data("coefficients", &request)?;
        let coefficients = exported.bytes;
        if exported.payload.as_str() != OBJECT_SHA
            || oer_durable::sha256_bytes(&coefficients) != COEFFICIENT_SHA
        {
            return Err(invalid("gain object or coefficient identity mismatch"));
        }
        let mut roots: Vec<&str> = vec![
            "phy_wifi_get_tx_tab_new",
            "phy_bt_get_tx_tab_new",
            "phy_set_tx_gain_mem_new",
            "phy_bt_set_tx_gain_new",
            "phy_get_romfunc_addr",
        ];
        // Calibration storage roots need no RF-test input and always participate.
        roots.extend([
            "phy_rf_cal_data_backup_new",
            "phy_rf_cal_data_recovery_new",
            "register_chipv7_phy_init_param",
            "phy_wifi_set_tx_gain_new",
        ]);
        let root = |input: usize, name: &str| -> Result<oer_riscv_model::SymbolId> {
            Ok(symbol(inventory, input, name)?.id.clone())
        };
        let mut link = LinkRequest {
            companions: vec![],
            inputs: vec![session.input_id(0)?],
            entry: root(0, roots[0])?,
            roots: roots[1..]
                .iter()
                .map(|n| root(0, n))
                .collect::<Result<_>>()?,
            layout: image_layout(),
            absent: vec![],
        };
        if options.rftest.is_some() {
            link.inputs.push(session.input_id(3)?);
            for name in ["set_rate_power_index", "mac_power_set"] {
                link.roots.push(root(3, name)?);
            }
        }
        // Every ROM definition the closure references, including those of
        // co-located archive sections, is proposed from the captured ROM.
        let image = PhyImage::link(session, &link, &options.linker, roots[0], &[ROM_INPUT])?;
        Ok(Self {
            image,
            coefficients,
            wifi_request: None,
            bluetooth_request: None,
            publication_request: None,
            coefficient_address: None,
        })
    }

    pub fn wifi_phase(&self, channel: u32, fill: u8, capture: bool) -> Invocation {
        let mut phase = self.enter(
            self.root("phy_wifi_get_tx_tab_new"),
            &[channel, OUTPUT, OUTPUT + 32, OUTPUT + 96, 0],
            vec![filled(OUTPUT, 160, fill).unwrap()],
            vec![selection(OUTPUT, 160)],
            vec![],
        );
        if capture {
            phase.observe_calls = Some(CallCapture {
                include_tail: true,
                argument_words: 0,
                overrides: vec![
                    blobray_domain::CallWordCount {
                        target: self.sym(1, "memcpy"),
                        words: 3,
                    },
                    blobray_domain::CallWordCount {
                        target: self.sym(1, "phy_wifi_get_tx_gain"),
                        words: 11,
                    },
                ],
            });
        }
        phase
    }

    pub fn coefficient_boundaries(&mut self) -> Result<()> {
        // The legacy input matrix leaves some table intervals unselected in the
        // production path. Select every authenticated threshold exactly, without
        // reading private production constants or supplying vendor results.
        for bluetooth in [false, true] {
            let coefficients = if bluetooth {
                self.coefficients[..108].to_vec()
            } else {
                self.coefficients[108..].to_vec()
            };
            let high: Vec<i32> = halfwords(&coefficients[72..])
                .into_iter()
                .map(|v| i32::from(v as i16))
                .collect();
            let length = if bluetooth { 80 } else { 160 };
            // Every threshold of one profile is one request over both fills.
            let mut parts = vec![];
            for fill in FILLS {
                {
                    let (mut rows, mut expected) = (vec![], vec![]);
                    for (index, threshold) in high.iter().enumerate() {
                        let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
                        let (left, right, correction);
                        if bluetooth {
                            let base = 64;
                            correction = -8 - threshold;
                            data[292] = base as u8;
                            data[254] = (correction & 255) as u8;
                            left = self.enter(
                                self.root("phy_bt_get_tx_tab_new"),
                                &[OUTPUT + 48, OUTPUT + 16, OUTPUT, 0],
                                vec![filled(OUTPUT, 80, fill)?],
                                vec![selection(OUTPUT, 80)],
                                vec![],
                            );
                            let mut packed = vec![0u32; 7];
                            packed.extend([((correction & 255) as u32) << 24, base as u32]);
                            right = self.probes.invoke(
                                "open_phy_bluetooth_trace_calculate_gain",
                                vec![
                                    ("input", Buffer::new(INPUT, words(&packed)).into()),
                                    ("output", Buffer::filled(OUTPUT, fill).into()),
                                ],
                                vec![],
                                vec![selection(OUTPUT, 80)],
                            )?;
                            expected.push(arithmetic(
                                &coefficients,
                                &[0; 3],
                                base,
                                correction,
                                None,
                            ));
                        } else {
                            let base = -64;
                            correction = 20 - threshold;
                            data[291] = (base & 255) as u8;
                            data[247] = (correction & 255) as u8;
                            left = self.wifi_phase(1, fill, false);
                            right = self.probes.invoke(
                                "open_phy_channel_trace_calculate_tx_gain",
                                vec![
                                    ("channel", 1.into()),
                                    ("curve", Buffer::new(INPUT, [0u8; 6]).into()),
                                    ("correction", i64::from(correction).into()),
                                    ("base_and_delta", i64::from(base).into()),
                                    ("output", Buffer::filled(OUTPUT, fill).into()),
                                ],
                                vec![],
                                vec![selection(OUTPUT, 160)],
                            )?;
                            expected.push(arithmetic(
                                &coefficients,
                                &[0; 6],
                                base,
                                correction,
                                Some(1),
                            ));
                        }
                        assert!((-128..=127).contains(&correction));
                        let right = self.enter_probe(right);
                        rows.push(case(
                            "initialize",
                            self.setup(&data, false),
                            Some(self.setup(&data, true)),
                            SessionReset::Cold,
                            false,
                        ));
                        rows.push(case(
                            format!("coefficient-interval-{index}"),
                            left,
                            Some(right),
                            SessionReset::Warm,
                            true,
                        ));
                    }
                    parts.push((fill, rows, expected));
                }
            }
            let label = format!("coefficient-boundary-{}", python_bool(bluetooth));
            for (expected, records) in self.compare_fills(&label, parts, MAX_EVENTS)?.parts {
                assert!(!has_events(&records));
                for (i, value) in expected.iter().enumerate() {
                    let case = 2 * i as u32 + 1;
                    assert_eq!(value.len(), length);
                    assert_eq!(&output(&records, case, false), value);
                    assert_eq!(&output(&records, case, true), value);
                }
            }
        }
        Ok(())
    }

    pub fn characterize(&mut self) -> Result<()> {
        let memcpy = self.sym(1, "memcpy");
        let kernel_address = self.sym(1, "phy_wifi_get_tx_gain");
        let mut specs = vec![];
        for curve in [CURVES[0], CURVES[2], CURVES[3]] {
            for channel in [1, 6, 11, 12, 13] {
                for (base, adjustment, correction) in [
                    (0, 0, 0),
                    (127, 1, 127),
                    (128, 255, -128),
                    (255, 1, -17),
                    (0, 255, 17),
                ] {
                    specs.push((curve, channel, base, adjustment, correction));
                }
            }
        }
        let data_for =
            |(curve, _, base, adjustment, correction): &([u8; 6], u32, i32, i32, i32)| {
                let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
                data[241..247].copy_from_slice(curve);
                data[247] = (correction & 255) as u8;
                data[291] = *base as u8;
                data[434] = *adjustment as u8;
                data
            };
        // Both fills are one request per phase; each case sets its stack fill.
        let mut parts = vec![];
        for fill in FILLS {
            let mut rows = vec![];
            for spec in &specs {
                rows.push(case(
                    "initialize",
                    self.setup(&data_for(spec), false),
                    None,
                    SessionReset::Cold,
                    false,
                ));
                rows.push(case(
                    "current-kernel-inputs",
                    self.wifi_phase(spec.1, fill, true),
                    None,
                    SessionReset::Warm,
                    false,
                ));
            }
            parts.push((fill, rows, fill));
        }
        let mut direct_parts = vec![];
        for (fill, records) in self.characterize_fills("kernel-inputs", parts)?.parts {
            let mut direct_rows = vec![];
            for (i, spec) in specs.iter().enumerate() {
                let (_, channel, base, adjustment, correction) = *spec;
                let observed = events(&records, 2 * i as u32 + 1, false);
                assert!(observed.iter().all(|e| matches!(
                    e,
                    ExecutionEvent::CallTransfer { .. } | ExecutionEvent::TransferArgument { .. }
                )));
                let kernel = calls(&observed, kernel_address);
                let effective = signed8((base + adjustment) & 255);
                assert_eq!(kernel.len(), 1);
                assert_eq!(
                    kernel[0][..4],
                    [
                        channel,
                        self.parameter + 241,
                        correction as u32,
                        effective as u32
                    ]
                );
                let copies = calls(&observed, memcpy);
                assert!(copies.len() == 3 && copies.iter().all(|c| c[2] == 36));
                let sources: Vec<u32> = copies.iter().map(|c| c[1]).collect();
                assert_eq!(
                    sources,
                    (0..3).map(|i| sources[0] + i * 36).collect::<Vec<_>>()
                );
                if self.coefficient_address.is_none() {
                    let address = sources[0] - 108;
                    self.coefficient_address = Some(address);
                    assert_eq!(
                        self.image_data("linked-coefficients", address, 216)?,
                        self.coefficients
                    );
                }
                assert_eq!(sources[0], self.coefficient_address.unwrap() + 108);
                assert_eq!(
                    kernel[0][4..7],
                    copies.iter().map(|c| c[0]).collect::<Vec<_>>()[..]
                );
                assert_eq!(kernel[0][7..], [OUTPUT, OUTPUT + 32, OUTPUT + 96, 0]);
                let mut arguments = vec![
                    channel,
                    self.parameter + 241,
                    correction as u32,
                    effective as u32,
                ];
                arguments.extend(&sources);
                arguments.extend([OUTPUT, OUTPUT + 32, OUTPUT + 96, 0]);
                let direct = self.enter(
                    kernel_address,
                    &arguments,
                    vec![filled(OUTPUT, 160, fill)?],
                    vec![selection(OUTPUT, 160)],
                    vec![],
                );
                direct_rows.push(case(
                    "initialize",
                    self.setup(&data_for(spec), false),
                    None,
                    SessionReset::Cold,
                    false,
                ));
                direct_rows.push(case(
                    "direct-ROM-kernel",
                    direct,
                    None,
                    SessionReset::Warm,
                    false,
                ));
            }
            direct_parts.push((fill, direct_rows, records));
        }
        for (records, direct) in self
            .characterize_fills("kernel-direct", direct_parts)?
            .parts
        {
            assert!(!has_events(&direct));
            for (i, (curve, channel, base, adjustment, correction)) in specs.iter().enumerate() {
                let expected = arithmetic(
                    &self.coefficients[108..],
                    curve,
                    signed8((base + adjustment) & 255),
                    *correction,
                    Some(*channel),
                );
                let case = 2 * i as u32 + 1;
                assert_eq!(output(&records, case, false), expected);
                assert_eq!(output(&direct, case, false), expected);
            }
        }
        Ok(())
    }

    pub fn wifi(&mut self) -> Result<()> {
        let mut specs = vec![];
        for curve in CURVES {
            for channel in [1, 2, 5, 6, 7, 10, 11, 12, 13] {
                for (base, correction) in [(0, 0), (-128, 127), (127, -128), (31, -17), (-31, 17)] {
                    specs.push((curve, channel, base, correction));
                }
            }
        }
        // Both fills are one request; each case sets its own stack fill.
        let mut parts = vec![];
        for fill in FILLS {
            {
                let mut rows = vec![];
                for &(curve, channel, base, correction) in &specs {
                    let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
                    data[241..247].copy_from_slice(&curve);
                    data[247] = (correction & 255) as u8;
                    data[291] = (base & 255) as u8;
                    rows.push(case(
                        "initialize",
                        self.setup(&data, false),
                        Some(self.setup(&data, true)),
                        SessionReset::Cold,
                        false,
                    ));
                    let right = self.probes.invoke(
                        "open_phy_channel_trace_calculate_tx_gain",
                        vec![
                            ("channel", i64::from(channel).into()),
                            ("curve", Buffer::new(INPUT, curve).into()),
                            ("correction", i64::from(correction).into()),
                            ("base_and_delta", i64::from(base).into()),
                            ("output", Buffer::filled(OUTPUT, fill).into()),
                        ],
                        vec![],
                        vec![selection(OUTPUT, 160)],
                    )?;
                    let right = self.enter_probe(right);
                    rows.push(case(
                        format!("wifi-{curve:?}-{channel}-{base}-{correction}"),
                        self.wifi_phase(channel, fill, false),
                        Some(right),
                        SessionReset::Warm,
                        true,
                    ));
                }
                parts.push((fill, rows, fill));
            }
        }
        let executed = self.compare_fills("wifi", parts, MAX_EVENTS)?;
        self.wifi_request = Some(executed.request);
        for (fill, records) in executed.parts {
            {
                assert!(!has_events(&records));
                for (i, &(curve, channel, base, correction)) in specs.iter().enumerate() {
                    let expected = arithmetic(
                        &self.coefficients[108..],
                        &curve,
                        base,
                        correction,
                        Some(channel),
                    );
                    let case = 2 * i as u32 + 1;
                    assert_eq!(output(&records, case, false), expected, "{fill} {case}");
                    assert_eq!(output(&records, case, true), expected, "{fill} {case}");
                }
            }
        }
        Ok(())
    }

    pub fn publish_wifi(&mut self) -> Result<()> {
        // Both fills are one request; each case sets its own stack fill.
        let mut parts = vec![];
        for fill in FILLS {
            let (mut rows, mut expectations) = (vec![], vec![]);
            for seed_value in [0u32, 0x1357_2468, 0xffff_ffff] {
                for base in [0u32, 32, 224, 255] {
                    let mut image: Vec<u32> = (0..47)
                        .map(|i| seed_value.wrapping_add(i * 0x0102_0305))
                        .collect();
                    for i in 0..32usize {
                        let shift = (i % 2) * 16;
                        // Every baseband gain `phy_index_to_txbbgain` encodes.
                        let value = [0u32, 128, 256, 32, 160][i % 5];
                        image[14 + i / 2] =
                            (image[14 + i / 2] & !(0xffff << shift)) | (value << shift);
                    }
                    let left = self.enter(
                        self.root("phy_set_tx_gain_mem_new"),
                        &[
                            0,
                            32,
                            INPUT + 120,
                            INPUT + 56,
                            INPUT + 24,
                            INPUT,
                            INPUT + 184,
                        ],
                        vec![known(INPUT, 188, &words(&image))?],
                        vec![],
                        gain_models(base, fill),
                    );
                    let right = self.probes.invoke(
                        "open_phy_channel_trace_publish_tx_gain",
                        vec![("input", Buffer::new(INPUT, words(&image)).into())],
                        gain_models(base, fill),
                        vec![],
                    )?;
                    let right = self.enter_probe(right);
                    rows.push(case(
                        format!("publish-{seed_value}-{base}"),
                        left,
                        Some(right),
                        SessionReset::Cold,
                        false,
                    ));
                    expectations.push(publication(
                        &image[..6],
                        image[46],
                        &words(&image[6..46]),
                        32,
                        base,
                        fill,
                        false,
                    ));
                }
            }
            parts.push((fill, rows, expectations));
        }
        let executed = self.compare_fills("publish-wifi", parts, MAX_EVENTS)?;
        self.publication_request = Some(executed.request);
        for (expectations, records) in executed.parts {
            for (i, expected) in expectations.iter().enumerate() {
                assert!(matches!(
                    stop(&records, i as u32, true),
                    ExecutionStop::Returned { low: Some(0), .. }
                ));
                for side in [false, true] {
                    let observed = events(&records, i as u32, side);
                    assert_eq!(observed.len(), 161);
                    assert_eq!(&effects(&observed, false), expected, "{i} {side}");
                }
            }
        }
        Ok(())
    }

    pub fn bluetooth(&mut self) -> Result<()> {
        let tab = self.root("phy_bt_get_tx_tab_new");
        let mut specs = vec![];
        for curve in [[0u8, 0, 0], [127, 128, 255], [255, 31, 128]] {
            for (base, attenuation, correction) in [
                (0, 0, 0),
                (127, 255, 127),
                (128, 1, -128),
                (255, 31, -17),
                (0, 127, 17),
            ] {
                for bank in [0u32, 32, 224, 255] {
                    specs.push((curve, base, attenuation, correction, bank));
                }
            }
        }
        // Both fills are one request; each case sets its own stack fill.
        let mut parts = vec![];
        for fill in FILLS {
            {
                let (mut rows, mut expected) = (vec![], vec![]);
                for &(curve, base, attenuation, correction, bank) in &specs {
                    let mut packed: Vec<u32> = (0..6)
                        .map(|i| fill_word(fill).wrapping_add(i * 0x0102_0305))
                        .collect();
                    let mut curve_word = curve.to_vec();
                    curve_word.push((correction & 255) as u8);
                    packed.extend([
                        fill_word(fill) & 0xffff,
                        u32::from_le_bytes(curve_word.try_into().unwrap()),
                        base as u32 | ((attenuation as u32) << 8),
                    ]);
                    let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
                    data[260..284].copy_from_slice(&words(&packed[..6]));
                    data[208..210].copy_from_slice(&packed[6].to_le_bytes()[..2]);
                    data[251..255].copy_from_slice(&words(&[packed[7]]));
                    data[292] = base as u8;
                    data[8] = attenuation as u8;
                    data[291] = fill;
                    data[434] = fill;
                    rows.push(case(
                        "initialize",
                        self.setup(&data, false),
                        Some(self.setup(&data, true)),
                        SessionReset::Cold,
                        false,
                    ));
                    let install = self.install_callbacks(INSTALLED_CALLBACK_SLOT)?;
                    let noop = self.noop();
                    rows.push(case(
                        "install-captured-callbacks",
                        install,
                        Some(noop),
                        SessionReset::Warm,
                        false,
                    ));
                    let left = self.enter(
                        tab,
                        &[OUTPUT + 48, OUTPUT + 16, OUTPUT, 0],
                        vec![region(
                            OUTPUT,
                            80,
                            &[],
                            Some(fill),
                            RegionLifetime::Session,
                        )?],
                        vec![selection(OUTPUT, 80)],
                        vec![],
                    );
                    let right = self.probes.invoke(
                        "open_phy_bluetooth_trace_calculate_gain",
                        vec![
                            ("input", Buffer::new(INPUT, words(&packed)).into()),
                            ("output", Buffer::filled(OUTPUT, fill).session().into()),
                        ],
                        vec![],
                        vec![selection(OUTPUT, 80)],
                    )?;
                    rows.push(case(
                        "bt-calculation",
                        left,
                        Some(self.enter_probe(right)),
                        SessionReset::Warm,
                        true,
                    ));
                    // The standalone gain child brackets its publication with
                    // an outermost force-TX/RX pair.
                    let mut child_models = gain_models(bank, fill);
                    child_models.push(force_txrx_model());
                    let delay = self.sym(1, "ets_delay_us");
                    let mut left = self.enter(
                        self.root("phy_bt_set_tx_gain_new"),
                        &[0],
                        vec![],
                        vec![selection(OUTPUT, 80)],
                        child_models.clone(),
                    );
                    left.calls = crate::phy::delay_calls("force-txrx-delay", delay);
                    left.observe_calls = Some(CallCapture {
                        include_tail: true,
                        argument_words: 0,
                        overrides: vec![],
                    });
                    let right = self.probes.invoke(
                        "open_phy_bluetooth_trace_tx_gain",
                        vec![
                            ("input", Buffer::new(INPUT, words(&packed)).into()),
                            ("output", i64::from(OUTPUT).into()),
                        ],
                        child_models,
                        vec![selection(OUTPUT, 80)],
                    )?;
                    let mut right = self.enter_probe(right);
                    right.calls = crate::phy::delay_calls("force-txrx-delay", delay);
                    rows.push(case(
                        "bt-complete-publication",
                        left,
                        Some(right),
                        SessionReset::Warm,
                        true,
                    ));
                    let calculated = arithmetic(
                        &self.coefficients[..108],
                        &curve,
                        signed8((base - attenuation) & 255),
                        correction,
                        None,
                    );
                    let writes =
                        publication(&packed[..6], packed[6], &calculated, 16, bank, fill, true);
                    expected.push((calculated, writes));
                }
                parts.push((fill, rows, expected));
            }
        }
        let executed = self.compare_fills("bluetooth", parts, MAX_EVENTS)?;
        self.bluetooth_request = Some(executed.request);
        for (expected, records) in executed.parts {
            let records = &records;
            {
                for (i, (calculated, writes)) in expected.iter().enumerate() {
                    let i = i as u32;
                    assert_eq!(output(records, 4 * i + 1, false), tab.to_le_bytes());
                    assert!(events(records, 4 * i + 3, false).iter().any(|e| matches!(e,
                        ExecutionEvent::CallTransfer { target, indirect: true, target_kind: ObservedCallTarget::CapturedCode, .. } if *target == tab)));
                    for side in [false, true] {
                        assert!(events(records, 4 * i + 2, side).is_empty());
                        assert_eq!(&output(records, 4 * i + 2, side), calculated);
                        assert_eq!(&output(records, 4 * i + 3, side), calculated);
                        let forced = force_txrx_pair();
                        let (enter, leave) = forced.split_at(forced.len() / 2);
                        let expected: Vec<PhyEffect> = enter
                            .iter()
                            .copied()
                            .chain(writes.iter().copied().map(phy_effect))
                            .chain(leave.iter().copied())
                            .collect();
                        let observed = phy_effects(&events(records, 4 * i + 3, side));
                        assert_eq!(observed, expected, "{i} {side}");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn additive(&mut self) -> Result<()> {
        let mut profiles = vec![(0u8, 0u8, 0u8)];
        profiles.extend([1, 31, 127, 255].map(|a| (0, a, 0)));
        for (base, adjustment) in [(0u8, 1u8), (0, 31), (127, 1), (255, 1), (10, 255)] {
            profiles.push((base, 0, adjustment));
            profiles.push((base.wrapping_add(adjustment), 0, 0));
        }
        let data_for = |base: u8, attenuation: u8, adjustment: u8| {
            let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
            data[291] = base;
            data[8] = attenuation;
            data[434] = adjustment;
            data
        };
        let mut rows = vec![];
        for &(base, attenuation, adjustment) in &profiles {
            rows.push(case(
                "initialize",
                self.setup(&data_for(base, attenuation, adjustment), false),
                None,
                SessionReset::Cold,
                false,
            ));
            rows.push(case(
                "additive-current",
                self.wifi_phase(13, 0xa5, false),
                None,
                SessionReset::Warm,
                false,
            ));
        }
        let records = self
            .characterize_vendor("additive-current", rows, 0x5a)?
            .records;
        assert!(!has_events(&records));
        let baseline = output(&records, 1, false);
        for i in 1..5 {
            assert_eq!(output(&records, i * 2 + 1, false), baseline);
        }
        for i in (5..15).step_by(2) {
            assert_eq!(
                output(&records, i * 2 + 1, false),
                output(&records, (i + 1) * 2 + 1, false)
            );
        }
        assert_ne!(output(&records, 15, false), baseline); // (base=0, adjustment=31)
        let mut old = vec![];
        for (base, attenuation, adjustment) in [(0, 0, 0), (31, 31, 0), (0, 0, 31)] {
            let phase = self.enter(
                self.sym(1, "phy_wifi_get_tx_tab_"),
                &[13, OUTPUT, OUTPUT + 32, OUTPUT + 96, 0],
                vec![
                    known(self.sym(1, "phy_param_rom"), 4, &words(&[PARAMETER_COPY]))?,
                    filled(OUTPUT, 160, 0xa5)?,
                ],
                vec![selection(OUTPUT, 160)],
                vec![],
            );
            old.push(case(
                "initialize",
                self.setup(&data_for(base, attenuation, adjustment), true),
                None,
                SessionReset::Cold,
                false,
            ));
            old.push(case(
                "subtractive-ROM",
                phase,
                None,
                SessionReset::Warm,
                false,
            ));
        }
        let observed = self
            .characterize_vendor("subtractive-ROM", old, 0x5a)?
            .records;
        assert!(!has_events(&observed));
        let first = output(&observed, 1, false);
        assert_eq!(output(&observed, 3, false), first);
        assert_eq!(output(&observed, 5, false), first);
        assert_ne!(first, baseline);
        Ok(())
    }

    pub fn negative(&mut self) -> Result<()> {
        let mut unknown = self.wifi_request.clone().expect("Wi-Fi matrix ran").cases;
        unknown.truncate(2);
        let curve = &mut unknown[1]
            .replacement
            .as_mut()
            .unwrap()
            .memory
            .iter_mut()
            .find(|r| r.seed.address == INPUT)
            .expect("curve input region")
            .seed;
        curve.bytes.clear();
        curve.fill = None;
        let records = self
            .execute(
                "gain-unknown-curve",
                unknown,
                0x5a,
                Right::Production,
                ComparisonVerdict::Incomplete,
                MAX_EVENTS,
            )?
            .records;
        // The unknown curve propagates into the published gain words, which
        // the comparison cannot prove.
        assert!(matches!(
            stop(&records, 1, true),
            ExecutionStop::Returned { .. }
        ));
        assert!(records.iter().any(|r| matches!(
            r,
            blobray_domain::ExecutionEvidence::FinalMemory { case: 1, replacement: true, chunk }
                if chunk.mask().is_some_and(|mask| chunk.known & mask != mask)
        )));
        let mut uninstalled = self
            .bluetooth_request
            .clone()
            .expect("Bluetooth matrix ran")
            .cases;
        uninstalled.truncate(4);
        // Keep parameter setup and the output calculation, but do not execute
        // the installer. The archive's callback pointer remains unknown.
        uninstalled.remove(1);
        uninstalled.last_mut().unwrap().name = "missing-callback-installation".into();
        let records = self
            .execute(
                "gain-missing-callback",
                uninstalled,
                0x5a,
                Right::Production,
                ComparisonVerdict::Incomplete,
                MAX_EVENTS,
            )?
            .records;
        assert!(matches!(
            stop(&records, 2, false),
            ExecutionStop::Incomplete { .. }
        ));
        assert!(!events(&records, 2, false).iter().any(|e| matches!(
            e,
            ExecutionEvent::Read { .. } | ExecutionEvent::Write { .. }
        )));
        let mut limited = self.publication_request.clone().expect("publication ran");
        limited.cases.truncate(1);
        limited.max_events = 1;
        self.capacity_failure("gain-capacity", &limited)?;
        // The same ROM kernel receives caller-owned coefficient bytes. This is
        // explicitly a vendor-boundary characterization, not a production match.
        let data = vec![0u8; PHY_PARAM_BYTES as usize];
        let direct = self.enter(
            self.sym(1, "phy_wifi_get_tx_gain"),
            &[
                13,
                PARAMETER_COPY + 241,
                0,
                0,
                INPUT + 256,
                INPUT + 292,
                INPUT + 328,
                OUTPUT,
                OUTPUT + 32,
                OUTPUT + 96,
                0,
            ],
            vec![
                known(INPUT + 256, 108, &self.coefficients[108..])?,
                filled(OUTPUT, 160, 0x5a)?,
            ],
            vec![selection(OUTPUT, 160)],
            vec![],
        );
        let rows = vec![
            case(
                "initialize",
                self.setup(&data, false),
                Some(self.setup(&data, true)),
                SessionReset::Cold,
                false,
            ),
            case(
                "current-coefficients",
                self.wifi_phase(13, 0x5a, false),
                Some(direct),
                SessionReset::Warm,
                true,
            ),
        ];
        let original = self
            .execute(
                "gain-coefficients-original",
                rows.clone(),
                0x5a,
                Right::Vendor,
                ComparisonVerdict::Match,
                MAX_EVENTS,
            )?
            .identity;
        let mut changed = rows;
        changed[1]
            .replacement
            .as_mut()
            .unwrap()
            .memory
            .iter_mut()
            .find(|r| r.seed.address == INPUT + 256)
            .expect("coefficient region")
            .seed
            .bytes[72] ^= 1;
        let different = self.execute(
            "gain-coefficients-changed",
            changed,
            0x5a,
            Right::Vendor,
            ComparisonVerdict::Diff,
            MAX_EVENTS,
        )?;
        assert_ne!(different.identity, original);
        assert_ne!(
            output(&different.records, 1, false),
            output(&different.records, 1, true)
        );
        Ok(())
    }
}

fn python_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publisher_oracle_retains_index_control_and_wraps_bank() {
        let wifi = publication(&[0; 6], 0, &[0; 160], 32, 255, 0xa5, false);
        assert_eq!(wifi.len(), 161);
        assert_eq!(
            wifi[1..4],
            [
                (Access::Write, GAIN_DATA[0], 0),
                (Access::Write, GAIN_DATA[1], 0x1000_0000),
                (Access::Write, GAIN_DATA[2], 0x7f80),
            ]
        );
        assert_eq!(wifi[4], (Access::Read, GAIN_INDEX, 0xa5a5_a5a5));
        assert_eq!(wifi[5], (Access::Write, GAIN_INDEX, 0xa5af_f800));
        assert_eq!(wifi[10], (Access::Write, GAIN_INDEX, 0xa5a8_0000));
        // Default is the Wi-Fi bank; explicit BT selects +32 and wraps to zero.
        assert_eq!(
            publication(&[0; 6], 0, &[0; 80], 16, 224, 0, false)[5].2,
            0xf0000
        );
        let bluetooth = publication(&[0; 6], 0, &[0; 80], 16, 224, 0, true);
        assert_eq!(bluetooth.len(), 81);
        assert_eq!(bluetooth[5].2, 0x80000);
    }

    #[test]
    fn arithmetic_selects_intervals_and_narrows_signed_residuals() {
        let mut coefficients = vec![0u8; 108];
        // Descending thresholds 170, 160, ..., 0; mid/low tables echo the index.
        for i in 0..18u16 {
            coefficients[2 * i as usize..][..2].copy_from_slice(&i.to_le_bytes());
            coefficients[36 + 2 * i as usize..][..2].copy_from_slice(&(100 + i).to_le_bytes());
            coefficients[72 + 2 * i as usize..][..2].copy_from_slice(&(170 - 10 * i).to_le_bytes());
        }
        let wifi = arithmetic(&coefficients, &[0; 6], 0, 0, Some(13));
        assert_eq!(wifi.len(), 160);
        // Target 84 selects threshold 80 (index 9): residual 4 - interpolation 2.
        assert_eq!(wifi[0], 2);
        assert_eq!(u16::from_le_bytes([wifi[32], wifi[33]]), 109);
        let bluetooth = arithmetic(&coefficients, &[0; 3], 0, 0, None);
        assert_eq!(bluetooth.len(), 80);
        // Target -72 is below every threshold: index 17 and a clamped residual.
        assert_eq!(bluetooth[0], (-60i32 & 255) as u8);
        assert_eq!(u16::from_le_bytes([bluetooth[16], bluetooth[17]]), 117);
    }
}
