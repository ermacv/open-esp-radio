//! Captured Wi-Fi/BT gain arithmetic and complete publication comparison.
//!
//! Explicit finite software inputs only; no RF, protocol or whole-TXCAL
//! qualification. Private inputs and retained requests/evidence belong in the
//! selected ignored output.
use crate::evidence::{Access, Effect, PhyEffect, calls, events, has_events, output};
use crate::harness::{
    Budget, Input, Result, case, data_request, filled, invalid, named_object, named_section,
    selection, symbol, words,
};
use crate::layout::*;
use crate::phy::{PhyImage, image_layout};
use crate::session::Session;
use oer_analysis_blobray_domain::{
    CallCapture, DataSelector, DeviceBehavior, DeviceDeclaration, ExecutionEvent, Invocation,
    LinkRequest, RegionLifetime, RegisterCell, SessionReset,
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
}

/// Linked captured gain image, compiled production and retained evidence.
pub struct Gain {
    pub image: PhyImage,
    pub coefficients: Vec<u8>,
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
            || oer_os_durable::sha256_bytes(&coefficients) != COEFFICIENT_SHA
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
        let root = |input: usize, name: &str| -> Result<oer_analysis_riscv_model::SymbolId> {
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
                    oer_analysis_blobray_domain::CallWordCount {
                        target: self.sym(1, "memcpy"),
                        words: 3,
                    },
                    oer_analysis_blobray_domain::CallWordCount {
                        target: self.sym(1, "phy_wifi_get_tx_gain"),
                        words: 11,
                    },
                ],
            });
        }
        phase
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
