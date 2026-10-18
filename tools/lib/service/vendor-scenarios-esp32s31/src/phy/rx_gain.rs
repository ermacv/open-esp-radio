//! Complete RX-gain root over the captured archive and ROM against compiled
//! production.
//!
//! Publication cases take the table/DC guards; calibration cases execute the
//! complete DC estimator path with signed samples, delayed analog I2C and both
//! work-mode settle branches. Both compare ordered effects, the projected
//! per-gain and fine DC coefficients and the two bank limits. Production-only
//! cases require a failed channel, a failed minimum search and an exhausted
//! shared budget to leave coefficients and gain memory unpublished. Software
//! comparison under explicit peripheral inputs, never hardware qualification.
use crate::contracts::{
    OutputField, omitted_read_before, output_projection, phy_contract, transport,
};
use crate::evidence::{PhyEffect, events, output, phy_effects, steps};
use crate::harness::{Buffer, Result, case, selection, setup, with_stack_fill};
use crate::i2c::{all_complete, returned_low};
use crate::layout::*;
use crate::phy::delay_calls;
use crate::phy::{PhyImage, PhyOptions, Right, image_layout, phy_sdk_input, select, start_session};
use crate::session::request;
use blobray_domain::{
    CommandCell, ComparisonVerdict, DeviceDeclaration, EffectRule, ExecutionCase,
    ExecutionEvidence, Invocation, LinkRequest, ReadRun, SessionReset,
};
use oer_riscv_model::ArtifactId;
use std::path::Path;

pub type Options = PhyOptions;

/// Vendor root arguments: the 2437 MHz channel frequency and bandwidth 0.
const ROOT_FREQUENCY: u32 = 2437;
const ROOT_BANDWIDTH: u32 = 0;
/// PBus RX path value selected by both sides.
const RX_PATH: u8 = 0xbf;
/// Coefficient words: Wi-Fi per-gain DC, the five Wi-Fi fine DC pairs of
/// `phy_rxdc_fine_cal`, then shared per-gain DC.
const WIFI_INDEX_WORDS: usize = 16;
const WIFI_FINE_WORDS: usize = 10;
const SHARED_INDEX_WORDS: usize = 22;
const COEFFICIENT_WORDS: usize = WIFI_INDEX_WORDS + WIFI_FINE_WORDS + SHARED_INDEX_WORDS;
/// Semantic parameter words: the coefficients and one Wi-Fi auxiliary word.
const PARAMETER_WORDS: usize = COEFFICIENT_WORDS + 1;
/// Production output: the coefficient words, then the Wi-Fi and shared last
/// gain indices.
const OUTPUT_WORDS: usize = COEFFICIENT_WORDS + 2;
const OUTPUT_BYTES: u32 = (OUTPUT_WORDS * 2) as u32;
/// Untouched production output bytes beyond the seeded coefficients.
const OUTPUT_FILL: u8 = 0xa5;
/// `phy_param` offsets of the semantic RX state.
const WIFI_INDEX_DC: usize = 334;
const WIFI_FINE_DC: usize = 366;
const SHARED_INDEX_DC: usize = 436;
const WIFI_AUXILIARY: usize = 212;
const GUARD_FLAGS: usize = 164;
const TABLE_MODE: usize = 16;
const PARAMETER_RX_PATH: usize = 2;
const SHARED_LAST_INDEX: usize = 288;
const WIFI_LAST_INDEX: usize = 289;
/// Initial last-index bytes and two cleared selector bytes.
const INITIAL_SHARED_LAST: u8 = 75;
const INITIAL_WIFI_LAST: u8 = 71;
const CLEARED: [usize; 2] = [79, 430];
/// Guard flags: DC already calibrated (bit 0) and tables initialized (bit 1).
const DC_CALIBRATED: u8 = 1;
const TABLES_INITIALIZED: u8 = 2;
/// Outer DC control snapshot the vendor reads even when DC is skipped.
const DC_CONTROL: u32 = 0x2010_0434;
/// Registers the vendor reads immediately after that skipped-DC snapshot,
/// before table initialization and with initialized tables. Calibration never
/// reads DC control immediately before either.
const SKIPPED_DC_SUCCESSORS: [u32; 2] = [0x2010_088c, 0x2010_702c];
/// Skipped-DC snapshots per root.
const SKIPPED_DC_SNAPSHOTS: u32 = 1;
/// Production RX entry compared with `phy_set_rx_gain_table`.
const PRODUCTION_ENTRY: &str = "open_phy_calibration_trace_rx_gain";
const ARBITER_INIT_ENTRY: &str = "open_phy_validation_arbiter_init";
/// DC estimator readiness (bit 16), signed samples and activity.
const ESTIMATOR_READY: u32 = 0x2010_047c;
const ESTIMATOR_DONE: u32 = 0x10000;
const ESTIMATOR_SAMPLE: u32 = 0x2010_0464;
const ESTIMATOR_NEGATED: u32 = 0x2010_0468;
const ESTIMATOR_MAGNITUDE: u32 = 0x2010_046c;
const ESTIMATOR_ACTIVITY: u32 = 0x2010_08d0;
/// Nonzero activity field (bits 20-21) of `ESTIMATOR_ACTIVITY`.
const ESTIMATOR_ACTIVE: u32 = 1 << 20;
/// Probe flag and `phy_param` byte of a detected RX saturation, which admits
/// estimates whose readiness wait saw activity.
const RX_SATURATION_FLAG: u8 = 4;
const RX_SATURATION_DETECTED: usize = 0x1ae;
/// PBus readiness word of the calibration path.
const PBUS_READY: u32 = 0x2010_0894;
const PBUS_READY_VALUE: u32 = 0x0100_0100;
/// Idle PBus status with writable retained clock bits.
const PBUS_IDLE: u32 = 0x1234;
const WORK_MODE_SETTLE: u32 = 2;
/// Retained RX analog selector (block 0x67, register 3).
const RX_ANALOG: u32 = 0x0367;
/// Production may execute at most this multiple of the vendor's steps.
const MAX_PRODUCTION_STEP_MULTIPLIER: u64 = 2;
/// Production outcomes of the typed RX calibration failures.
const FAILED_CALIBRATION: u32 = 4;
const OPERATION_LIMIT: u32 = 5;
/// Estimator readiness samples of the failed and slow minimum profiles.
const NEVER_READY_SAMPLES: u32 = 10_000;
const SLOW_MINIMA: u32 = 20;
const SLOW_MINIMUM_POLLS: u32 = 9_000;
/// Event capacity of the shared-budget case: every slow poll is a recorded read.
const BUDGET_EVENTS: u32 = SLOW_MINIMA * (SLOW_MINIMUM_POLLS + 1) + MAX_EVENTS;
/// Event capacity of a long root: its minimum searches may make every
/// attempt, and activity roots poll readiness twice per estimate.
const LONG_EVENTS: u32 = 1 << 18;
/// Cases of one profile: parameter setup, callback installation and the root.
const PROFILE_CASES: u32 = 3;
/// Case index of the root within its profile.
const ROOT: u32 = 2;

/// Signed DC estimator accumulator stream of the calibration path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Estimator {
    /// Every estimate reads this sample, with a matching power accumulator.
    Constant(i32),
    /// Successive estimates read these samples in turn with a zero power
    /// accumulator, so estimates differ as corrections take effect.
    Cycle(&'static [i32]),
    /// Independent periodic I, Q and power accumulator streams.
    Streams {
        i: &'static [i32],
        q: &'static [i32],
        power: &'static [u32],
    },
}

/// One RX root profile.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    /// Guard flags; zero executes calibration.
    pub flags: u8,
    pub estimator: Estimator,
    /// Every estimator readiness wait first sees a not-ready poll with
    /// activity, then one without, then readiness.
    pub activity: bool,
    pub rx_saturation: bool,
    pub seed: u16,
    pub fill: u8,
    /// Work-mode settle branch and delayed analog I2C completion.
    pub settle: bool,
}

impl Profile {
    fn calibrates(&self) -> bool {
        self.flags == 0
    }
    /// Whether the root needs the long event capacity.
    fn long(&self) -> bool {
        self.activity || matches!(self.estimator, Estimator::Streams { .. })
    }
    fn label(&self) -> String {
        let estimator = match self.estimator {
            Estimator::Constant(sample) => format!("sample{sample}"),
            Estimator::Cycle(samples) => format!(
                "cycle{}",
                samples
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join("_")
            ),
            Estimator::Streams { i, q, power } => {
                let join = |values: Vec<String>| values.join("_");
                format!(
                    "streams-i{}-q{}-p{}",
                    join(i.iter().map(i32::to_string).collect()),
                    join(q.iter().map(i32::to_string).collect()),
                    join(power.iter().map(u32::to_string).collect())
                )
            }
        };
        format!(
            "rx-flags{}-{estimator}{}{}-seed{}-fill{:x}-settle{}",
            self.flags,
            if self.activity { "-activity" } else { "" },
            if self.rx_saturation { "-saturated" } else { "" },
            self.seed,
            self.fill,
            u8::from(self.settle)
        )
    }
}

fn profiles(flags: &[u8], estimators: &[Estimator]) -> Vec<Profile> {
    let mut result = vec![];
    for &estimator in estimators {
        for &flags in flags {
            for seed in [1, 17] {
                for fill in FILLS {
                    for settle in [false, true] {
                        result.push(Profile {
                            flags,
                            estimator,
                            activity: false,
                            rx_saturation: false,
                            seed,
                            fill,
                            settle,
                        });
                    }
                }
            }
        }
    }
    result
}

/// Publication through both table/DC guards.
pub fn publication_profiles() -> Vec<Profile> {
    profiles(
        &[DC_CALIBRATED, DC_CALIBRATED | TABLES_INITIALIZED],
        &[Estimator::Constant(0)],
    )
}

/// Complete calibration with zero, small and saturating signed samples, and
/// with a periodic stream whose corrections reach the published codes.
pub fn calibration_profiles() -> Vec<Profile> {
    profiles(
        &[0],
        &[0, 64, -64, 1 << 24, -(1 << 24)]
            .map(Estimator::Constant)
            .into_iter()
            .chain([Estimator::Cycle(CALIBRATION_CYCLE)])
            .collect::<Vec<_>>(),
    )
}

/// Complete calibration whose every readiness wait sees activity, admitted
/// because an RX saturation was detected.
pub fn activity_profiles() -> Vec<Profile> {
    profiles(&[0], &[Estimator::Constant(0)])
        .into_iter()
        .map(|p| Profile {
            activity: true,
            rx_saturation: true,
            ..p
        })
        .collect()
}

/// Complete calibration under independent I, Q and power streams in whole
/// estimate units.
pub fn stream_profiles() -> Vec<Profile> {
    profiles(&[0], ESTIMATOR_STREAMS)
}

/// Independent estimator streams: one channel below and the other above the
/// correction threshold in both orders, including small positive residuals
/// that take the unit correction; alternating residuals; a seven-unit
/// residual over a small low estimate that takes the unit correction; power
/// that keeps every search from converging, always or on alternate reads;
/// and power that completes a low search only after three attempts while the
/// following high search completes at once, so the low power is the larger.
const ESTIMATOR_STREAMS: &[Estimator] = &[
    Estimator::Streams {
        i: &[0],
        q: &[30 * ESTIMATE_UNIT],
        power: &[0],
    },
    Estimator::Streams {
        i: &[ESTIMATE_UNIT],
        q: &[30 * ESTIMATE_UNIT],
        power: &[0],
    },
    Estimator::Streams {
        i: &[30 * ESTIMATE_UNIT],
        q: &[ESTIMATE_UNIT],
        power: &[0],
    },
    Estimator::Streams {
        i: &[3 * ESTIMATE_UNIT, 60 * ESTIMATE_UNIT],
        q: &[60 * ESTIMATE_UNIT, 3 * ESTIMATE_UNIT, 0],
        power: &[0],
    },
    Estimator::Streams {
        i: &[55 * ESTIMATE_UNIT, 0, -55 * ESTIMATE_UNIT],
        q: &[-55 * ESTIMATE_UNIT, 0, 55 * ESTIMATE_UNIT, 0],
        power: &[HIGH_POWER, 0, 0],
    },
    Estimator::Streams {
        i: &[0],
        q: &[0],
        power: &[HIGH_POWER],
    },
    Estimator::Streams {
        i: &[0],
        q: &[0],
        power: &[0, HIGH_POWER],
    },
    Estimator::Streams {
        i: &[0],
        q: &[0],
        power: &[HIGH_POWER, 0],
    },
    Estimator::Streams {
        i: &[10 * ESTIMATE_UNIT, 10 * ESTIMATE_UNIT, 17 * ESTIMATE_UNIT],
        q: &[10 * ESTIMATE_UNIT, 10 * ESTIMATE_UNIT, 17 * ESTIMATE_UNIT],
        power: &[0],
    },
    Estimator::Streams {
        i: &[0],
        q: &[0],
        power: &[MID_POWER, MID_POWER, MID_POWER, 0],
    },
];

/// Power accumulator whose estimate exceeds every RX-DC power threshold.
const HIGH_POWER: u32 = 1 << 28;
/// Power accumulator whose estimate completes a minimum search only after
/// three attempts (between the 36 and 48 thresholds).
const MID_POWER: u32 = 3 << 20;

/// Accumulator value of one RX-DC estimate: the calibration requests
/// estimator control `0x800`, and the estimator shifts the accumulator right
/// by six and divides it by the control plus one.
const ESTIMATE_UNIT: i32 = (1 << 6) * (0x800 + 1);

/// Estimator stream whose three-read period is odd against the low/high read
/// pair of a baseband iteration, so successive iterations see different
/// deltas: corrections are applied until a later iteration converges and
/// publishes the corrected code, and a seven-unit delta against a fifty-unit
/// low estimate reaches the low-estimate correction of the shared bank.
const CALIBRATION_CYCLE: &[i32] = &[50 * ESTIMATE_UNIT, 57 * ESTIMATE_UNIT, 50 * ESTIMATE_UNIT];

/// Sample, negated-sample and power accumulator reads of `estimator`.
fn estimator_models(estimator: Estimator) -> Vec<DeviceDeclaration> {
    let streams = |sample: i32, power: u32| {
        [
            ("estimator-sample", ESTIMATOR_SAMPLE, sample as u32),
            (
                "estimator-negated",
                ESTIMATOR_NEGATED,
                sample.wrapping_neg() as u32,
            ),
            ("estimator-magnitude", ESTIMATOR_MAGNITUDE, power),
        ]
    };
    match estimator {
        Estimator::Constant(sample) => streams(sample, sample.unsigned_abs())
            .into_iter()
            .map(|(id, address, value)| constant_read(id, address, value))
            .collect(),
        Estimator::Cycle(samples) => (0..3)
            .map(|stream| {
                let reads = samples.iter().map(|&sample| streams(sample, 0)[stream]);
                let (id, address, _) = streams(0, 0)[stream];
                DeviceDeclaration {
                    id: id.into(),
                    applicability: "periodic synthetic DC estimator stream with zero power".into(),
                    lifetime: blobray_domain::RegionLifetime::Phase,
                    behavior: blobray_domain::DeviceBehavior::CyclicRead {
                        address,
                        width: 4,
                        values: reads.map(|(_, _, value)| value).collect(),
                    },
                }
            })
            .collect(),
        Estimator::Streams { i, q, power } => [
            (
                "estimator-sample",
                ESTIMATOR_SAMPLE,
                i.iter().map(|&v| v as u32).collect::<Vec<_>>(),
            ),
            (
                "estimator-negated",
                ESTIMATOR_NEGATED,
                q.iter().map(|&v| v as u32).collect(),
            ),
            ("estimator-magnitude", ESTIMATOR_MAGNITUDE, power.to_vec()),
        ]
        .into_iter()
        .map(|(id, address, values)| DeviceDeclaration {
            id: id.into(),
            applicability: "independent periodic synthetic I, Q and power streams".into(),
            lifetime: blobray_domain::RegionLifetime::Phase,
            behavior: blobray_domain::DeviceBehavior::CyclicRead {
                address,
                width: 4,
                values,
            },
        })
        .collect(),
    }
}

/// Deterministic 9-bit DC words and the auxiliary word, derived from `seed`.
pub fn parameters(seed: u16) -> [u16; PARAMETER_WORDS] {
    core::array::from_fn(|i| match i {
        0..COEFFICIENT_WORDS => seed.wrapping_add(i as u16 * 7) & 0x1ff,
        _ => 0x125,
    })
}

fn put_words(image: &mut [u8], offset: usize, values: &[u16]) {
    for (i, value) in values.iter().enumerate() {
        image[offset + 2 * i..offset + 2 * i + 2].copy_from_slice(&value.to_le_bytes());
    }
}

/// Captured `phy_param` image of one profile.
pub fn parameter_image(profile: &Profile) -> Vec<u8> {
    let values = parameters(profile.seed);
    let mut image = vec![0u8; PHY_PARAM_BYTES as usize];
    let guards = u32::from(profile.flags & DC_CALIBRATED) * 128
        + u32::from((profile.flags & TABLES_INITIALIZED) >> 1) * 512;
    image[GUARD_FLAGS..GUARD_FLAGS + 4].copy_from_slice(&guards.to_le_bytes());
    // The qualified normal gain table; the alternate vendor table mode is excluded.
    image[TABLE_MODE..TABLE_MODE + 2].copy_from_slice(&0u16.to_le_bytes());
    let fine = WIFI_INDEX_WORDS + WIFI_FINE_WORDS;
    put_words(&mut image, WIFI_INDEX_DC, &values[..WIFI_INDEX_WORDS]);
    put_words(&mut image, WIFI_FINE_DC, &values[WIFI_INDEX_WORDS..fine]);
    put_words(
        &mut image,
        SHARED_INDEX_DC,
        &values[fine..COEFFICIENT_WORDS],
    );
    put_words(&mut image, WIFI_AUXILIARY, &values[COEFFICIENT_WORDS..]);
    image[PARAMETER_RX_PATH] = RX_PATH;
    image[SHARED_LAST_INDEX] = INITIAL_SHARED_LAST;
    image[WIFI_LAST_INDEX] = INITIAL_WIFI_LAST;
    for offset in CLEARED {
        image[offset] = 0;
    }
    image[RX_SATURATION_DETECTED] = u8::from(profile.rx_saturation);
    image
}

/// Committed RX `phy_param` fields in production output order: the
/// coefficient words, then the Wi-Fi and shared last gain indices, each in
/// the low byte of an output word.
pub const COMMITTED: [OutputField; 5] = [
    OutputField {
        name: "wifi-index-dc",
        parameter: WIFI_INDEX_DC as u32,
        output: 0,
        width: 2,
        count: WIFI_INDEX_WORDS as u32,
    },
    OutputField {
        name: "wifi-fine-dc",
        parameter: WIFI_FINE_DC as u32,
        output: 2 * WIFI_INDEX_WORDS as u32,
        width: 2,
        count: WIFI_FINE_WORDS as u32,
    },
    OutputField {
        name: "shared-index-dc",
        parameter: SHARED_INDEX_DC as u32,
        output: 2 * (WIFI_INDEX_WORDS + WIFI_FINE_WORDS) as u32,
        width: 2,
        count: SHARED_INDEX_WORDS as u32,
    },
    OutputField {
        name: "wifi-last-index",
        parameter: WIFI_LAST_INDEX as u32,
        output: 2 * COEFFICIENT_WORDS as u32,
        width: 1,
        count: 1,
    },
    OutputField {
        name: "shared-last-index",
        parameter: SHARED_LAST_INDEX as u32,
        output: 2 * COEFFICIENT_WORDS as u32 + 2,
        width: 1,
        count: 1,
    },
];

/// Production output seeded with the input coefficients and untouched indices.
fn seeded_output(profile: &Profile) -> Vec<u8> {
    let mut bytes: Vec<u8> = parameters(profile.seed)[..COEFFICIENT_WORDS]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    bytes.extend([OUTPUT_FILL; 4]);
    bytes
}

/// Explicit peripheral inputs; `ready` selects channel readiness.
pub fn rx_models(profile: &Profile, ready: bool) -> Vec<DeviceDeclaration> {
    // Work mode selects the settle branch and PBus starts idle; every other
    // radio register is retained storage starting with the fill pattern.
    let mut inputs = vec![
        (WORK_MODE, u32::from(profile.settle) * WORK_MODE_SETTLE),
        (PBUS_STATUS, PBUS_IDLE),
    ];
    let mut models = vec![];
    if profile.calibrates() {
        inputs.extend([(I2C_READ_MASK, 0), (I2C_HOST_MAP, 0)]);
        models.push(analog_bank(
            "rx-analog",
            "explicit retained RX analog register",
            u32::from(profile.settle) * 2,
            [0, 0],
            vec![CommandCell {
                selector: RX_ANALOG,
                initial: u32::from(profile.fill),
                reads: None,
            }],
        ));
        for (id, address, value) in [
            ("pbus-ready", PBUS_READY, PBUS_READY_VALUE),
            (
                "channel-ready",
                CHANNEL_STATUS,
                if ready { CHANNEL_READY } else { 0 },
            ),
        ] {
            models.push(constant_read(id, address, value));
        }
        if profile.activity {
            models.push(DeviceDeclaration {
                id: "estimator-ready".into(),
                applicability: "estimator readiness after two not-ready polls".into(),
                lifetime: blobray_domain::RegionLifetime::Phase,
                behavior: blobray_domain::DeviceBehavior::CyclicRead {
                    address: ESTIMATOR_READY,
                    width: 4,
                    values: vec![0, 0, ESTIMATOR_DONE],
                },
            });
            models.push(DeviceDeclaration {
                id: "estimator-activity".into(),
                applicability: "activity on the first not-ready poll of every wait only".into(),
                lifetime: blobray_domain::RegionLifetime::Phase,
                behavior: blobray_domain::DeviceBehavior::CyclicRead {
                    address: ESTIMATOR_ACTIVITY,
                    width: 4,
                    values: vec![ESTIMATOR_ACTIVE, 0],
                },
            });
        } else {
            models.push(constant_read(
                "estimator-ready",
                ESTIMATOR_READY,
                ESTIMATOR_DONE,
            ));
        }
        models.extend(estimator_models(profile.estimator));
    }
    models.insert(
        0,
        register_bank(
            "rx-inputs",
            "work-mode settle branch, idle PBus and transport controls",
            inputs,
        ),
    );
    models.push(radio_aperture(profile.fill));
    models
}

/// Reviewed rules of the RX root: transport plumbing, whose commands never
/// wait, and the vendor's unused skipped-DC snapshot, each selecting at most
/// `maximum` effects.
pub fn rx_rules(maximum: u32) -> Vec<EffectRule> {
    let mut rules = transport(maximum);
    for successor in SKIPPED_DC_SUCCESSORS {
        rules.push(omitted_read_before(
            format!("skipped-dc-snapshot-{successor:08x}"),
            DC_CONTROL,
            successor,
            SKIPPED_DC_SNAPSHOTS,
            "the vendor snapshots DC control on the skipped-DC path and never uses the value",
        ));
    }
    rules
}

pub struct RxGain {
    pub image: PhyImage,
    rom_delay: u32,
    production_delay: u32,
    effects: ArtifactId,
    /// The same rules for activity roots, within their event capacity.
    long_effects: ArtifactId,
    committed: ArtifactId,
}

impl std::ops::Deref for RxGain {
    type Target = PhyImage;
    fn deref(&self) -> &PhyImage {
        &self.image
    }
}
impl std::ops::DerefMut for RxGain {
    fn deref_mut(&mut self) -> &mut PhyImage {
        &mut self.image
    }
}

impl RxGain {
    pub fn new(options: &Options, phy_sdk: &Path) -> Result<Self> {
        let session = start_session(
            options,
            &[phy_sdk_input(phy_sdk)?],
            "captured RX gain publication and DC calibration; no RF qualification",
        )?;
        let link = LinkRequest {
            companions: vec![],
            inputs: vec![session.input_id(0)?],
            entry: select(&session, 0, "phy_set_rx_gain_table")?,
            roots: vec![select(&session, 0, "phy_get_romfunc_addr")?],
            layout: image_layout(),
            absent: vec![],
        };
        // ROM first; the co-located RFPLL diagnostics reference `phy_printf`,
        // bound to the authenticated PHY SDK firmware and never executed.
        let mut image = PhyImage::link(
            session,
            &link,
            &options.linker,
            "phy_set_rx_gain_table",
            &[ROM_INPUT, PHY_SDK_INPUT],
        )?;
        let applicability = "RX gain publication and DC calibration under explicit estimator, PBus and readiness inputs";
        let (vendor, production) = (
            image.vendor_endpoint("phy_set_rx_gain_table")?,
            image.production_endpoint(PRODUCTION_ENTRY)?,
        );
        let projection = output_projection(
            vendor.clone(),
            image.parameter,
            production.clone(),
            OUTPUT_BYTES,
            &COMMITTED,
            applicability,
        );
        let review = "every RX register effect compares exactly except transport polling and the unused skipped-DC snapshot";
        let contract = phy_contract(
            vendor.clone(),
            production.clone(),
            rx_rules(MAX_EVENTS),
            applicability,
        );
        let effects = image.review_effects(
            "rx-effects",
            "esp32s31.phy.rx-gain.effects",
            contract,
            review,
        )?;
        let contract = phy_contract(vendor, production, rx_rules(LONG_EVENTS), applicability);
        let long_effects = image.review_effects(
            "rx-long-effects",
            "esp32s31.phy.rx-gain.long-effects",
            contract,
            review,
        )?;
        let committed = image.review_projection(
            "rx-committed",
            "esp32s31.phy.rx-gain.committed",
            projection,
            "production publishes the committed DC coefficients and bank limits",
        )?;
        Ok(Self {
            rom_delay: image.sym(1, "ets_delay_us"),
            production_delay: image.sym(2, "ets_delay_us"),
            image,
            effects,
            long_effects,
            committed,
        })
    }

    fn vendor_phase(&self, profile: &Profile) -> Result<Invocation> {
        let mut phase = self.enter(
            self.root("phy_set_rx_gain_table"),
            &[ROOT_FREQUENCY, ROOT_BANDWIDTH],
            vec![],
            vec![selection(self.parameter, PHY_PARAM_BYTES)],
            rx_models(profile, true),
        );
        phase.calls = delay_calls("rx-delay", self.rom_delay);
        Ok(phase)
    }

    pub fn production_phase(
        &self,
        profile: &Profile,
        models: Vec<DeviceDeclaration>,
    ) -> Result<Invocation> {
        let input: Vec<u8> = parameters(profile.seed)
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let probe = self.probes.invoke(
            PRODUCTION_ENTRY,
            vec![
                ("input", Buffer::new(INPUT, input).into()),
                (
                    "flags",
                    i64::from(
                        profile.flags | (u8::from(profile.rx_saturation) * RX_SATURATION_FLAG),
                    )
                    .into(),
                ),
                ("crystal_selector", 0.into()),
                ("pbus_rx_path", i64::from(RX_PATH).into()),
                ("output", Buffer::new(OUTPUT, seeded_output(profile)).into()),
            ],
            models,
            vec![selection(OUTPUT, OUTPUT_BYTES)],
        )?;
        let mut phase = self.enter_probe(probe);
        phase.calls = delay_calls("rx-delay", self.production_delay);
        Ok(phase)
    }

    /// Build the probe's session arbiter before the compared root.
    fn arbiter_init(&self) -> Result<Invocation> {
        let probe = self
            .probes
            .invoke(ARBITER_INIT_ENTRY, vec![], vec![], vec![])?;
        Ok(self.enter_probe(probe))
    }

    fn rows(&self, profile: &Profile) -> Result<Vec<ExecutionCase>> {
        let image = parameter_image(profile);
        let mut root = case(
            profile.label(),
            self.vendor_phase(profile)?,
            Some(self.production_phase(profile, rx_models(profile, true))?),
            SessionReset::Warm,
            false,
        );
        // Blobray compares every effect and the committed state under the
        // reviewed contract and projection.
        let relation = root.relation.as_mut().unwrap();
        relation.effects = Some(if profile.long() {
            self.long_effects.clone()
        } else {
            self.effects.clone()
        });
        relation.projection = Some(self.committed.clone());
        Ok(with_stack_fill(
            vec![
                case(
                    "initialize",
                    self.setup(&image, false),
                    Some(self.setup(&image, true)),
                    SessionReset::Cold,
                    false,
                ),
                // Production builds its arbiter at boot, not in the compared
                // root, whose step budget therefore excludes the construction.
                // The two sides prepare different state, so nothing of it is
                // compared or claimed.
                setup(
                    "install-captured-callbacks",
                    self.install_callbacks(INSTALLED_CALLBACK_SLOT)?,
                    self.arbiter_init()?,
                    SessionReset::Warm,
                ),
                root,
            ],
            profile.fill,
        ))
    }
}

fn check(label: &str, records: &[ExecutionEvidence], root: u32) {
    assert_eq!(returned_low(records, root, true), Some(0), "{label}");
    for side in [false, true] {
        assert!(all_complete(records, root, side), "{label} {side}");
    }
    let (vendor_steps, production_steps) =
        (steps(records, root, false), steps(records, root, true));
    assert!(
        production_steps <= vendor_steps.saturating_mul(MAX_PRODUCTION_STEP_MULTIPLIER),
        "{label}: production {production_steps} steps, vendor {vendor_steps}"
    );
}

/// Each matrix is one request; every profile starts cold with its own stack fill.
pub fn exercise(ctx: &mut RxGain) -> Result<()> {
    for (name, matrix, capacity) in [
        ("publication", publication_profiles(), MAX_EVENTS),
        ("calibration", calibration_profiles(), MAX_EVENTS),
        ("activity", activity_profiles(), LONG_EVENTS),
        ("streams", stream_profiles(), LONG_EVENTS),
    ] {
        let mut rows = vec![];
        for profile in &matrix {
            rows.extend(ctx.rows(profile)?);
        }
        let executed = ctx.image.execute_without_events(
            &format!("rx-{name}"),
            rows,
            FILLS[0],
            Right::Production,
            ComparisonVerdict::Match,
            capacity,
        )?;
        for (i, profile) in matrix.iter().enumerate() {
            let root = i as u32 * PROFILE_CASES + ROOT;
            check(&profile.label(), &executed.records, root);
        }
    }
    containment(ctx)?;
    unadmitted_activity(ctx)?;
    negative(ctx)
}

/// One production-only failing root.
struct Failure {
    label: String,
    models: Vec<DeviceDeclaration>,
    /// Expected typed failure.
    outcome: u32,
    /// Successful estimator minima before the failure: exact, or a range.
    minima: std::ops::Range<u32>,
}

/// Unpublished coefficients and gain memory after a failed root.
fn assert_unpublished(label: &str, profile: &Profile, records: &[ExecutionEvidence], case: u32) {
    assert_eq!(
        output(records, case, false),
        seeded_output(profile),
        "{label}: output published"
    );
    assert!(
        !phy_effects(&events(records, case, false))
            .iter()
            .any(|e| matches!(e, PhyEffect::Write(GAIN_INDEX, _))),
        "{label}: gain memory published"
    );
}

/// A channel that never becomes ready, a minimum search whose estimator never
/// completes and slow successful minima that exhaust the shared operation
/// budget all fail without publishing coefficients or gain memory. One
/// production-only request per fill; each root starts cold.
fn containment(ctx: &mut RxGain) -> Result<()> {
    // Both fills are one request; each root sets its own stack fill.
    let (mut rows, mut parts) = (vec![], vec![]);
    for fill in FILLS {
        let profile = Profile {
            flags: 0,
            estimator: Estimator::Constant(0),
            activity: false,
            rx_saturation: false,
            seed: 17,
            fill,
            settle: false,
        };
        let mut failures = vec![Failure {
            label: "channel-never-ready".into(),
            models: rx_models(&profile, false),
            outcome: FAILED_CALIBRATION,
            minima: 0..1,
        }];
        for slow in [false, true] {
            let mut models = rx_models(&profile, true);
            models.retain(|m| m.id != "estimator-ready");
            let runs = if slow {
                (0..SLOW_MINIMA)
                    .flat_map(|_| {
                        [
                            ReadRun {
                                value: 0,
                                count: SLOW_MINIMUM_POLLS,
                            },
                            ReadRun::once(ESTIMATOR_DONE),
                        ]
                    })
                    .collect()
            } else {
                vec![ReadRun {
                    value: 0,
                    count: NEVER_READY_SAMPLES,
                }]
            };
            models.push(sequence_read("estimator-ready", ESTIMATOR_READY, runs));
            // The not-ready branch also samples estimator activity; idle here.
            models.push(constant_read("estimator-activity", ESTIMATOR_ACTIVITY, 0));
            failures.push(if slow {
                Failure {
                    label: "minimum-shared-budget".into(),
                    models,
                    outcome: OPERATION_LIMIT,
                    // Several minima complete, but not the whole input sequence.
                    minima: 2..SLOW_MINIMA,
                }
            } else {
                Failure {
                    label: "minimum-timeout".into(),
                    models,
                    outcome: FAILED_CALIBRATION,
                    minima: 0..1,
                }
            });
        }
        for failure in &failures {
            let mut row = case(
                format!("{}-fill{fill:x}", failure.label),
                ctx.production_phase(&profile, failure.models.clone())?,
                None,
                SessionReset::Cold,
                false,
            );
            row.relation = None;
            row.stack_fill = Some(fill);
            rows.push(row);
        }
        parts.push((profile, failures));
    }
    let production = ctx.production.clone();
    let request = request(&production, None, None, rows, BUDGET_EVENTS);
    let records = ctx
        .submit("rx-containment", &request, None)?
        .records
        .clone();
    let mut case = 0u32;
    for (profile, failures) in &parts {
        for failure in failures {
            let label = format!("rx-containment-fill{:x}-{}", profile.fill, failure.label);
            assert_eq!(
                returned_low(&records, case, false),
                Some(failure.outcome),
                "{label}"
            );
            assert_unpublished(&label, profile, &records, case);
            let minima = phy_effects(&events(&records, case, false))
                .iter()
                .filter(|e| matches!(e, PhyEffect::Read(ESTIMATOR_READY, ESTIMATOR_DONE)))
                .count() as u32;
            assert!(failure.minima.contains(&minima), "{label}: {minima} minima");
            case += 1;
        }
    }
    Ok(())
}

/// Characterized difference, not a claim: with readiness activity and no
/// detected RX saturation no estimate is admitted, and the ROM's first radio
/// search then returns its never-initialized output slot, stack contents
/// that production replaces with a cleared slot (user decision 2026-09-26).
fn unadmitted_activity(ctx: &mut RxGain) -> Result<()> {
    let profile = Profile {
        rx_saturation: false,
        ..activity_profiles()[0]
    };
    ctx.image.execute_without_events(
        "rx-unadmitted-activity",
        ctx.rows(&profile)?,
        profile.fill,
        Right::Production,
        ComparisonVerdict::Diff,
        LONG_EVENTS,
    )?;
    Ok(())
}

/// A saturating production sample is a DIFF, omitted callback installation leaves
/// the vendor INCOMPLETE, and an undersized event capacity publishes nothing.
fn negative(ctx: &mut RxGain) -> Result<()> {
    let profile = calibration_profiles()[0];
    let mut changed = ctx.rows(&profile)?;
    // A saturating sample drives a different minimum search.
    let other = Profile {
        estimator: Estimator::Constant(1 << 24),
        ..profile
    };
    changed[ROOT as usize].replacement =
        Some(ctx.production_phase(&other, rx_models(&other, true))?);
    ctx.image.execute(
        "rx-negative-changed-sample",
        changed,
        profile.fill,
        Right::Production,
        ComparisonVerdict::Diff,
        MAX_EVENTS,
    )?;
    let mut uninstalled = ctx.rows(&profile)?;
    uninstalled[1].vendor = ctx.noop();
    ctx.image.execute(
        "rx-negative-uninstalled-callbacks",
        uninstalled,
        profile.fill,
        Right::Production,
        ComparisonVerdict::Incomplete,
        MAX_EVENTS,
    )?;
    // The contract's occurrence bounds exceed a one-event capacity; the
    // exhaustion under test precedes any effect comparison.
    let mut rows = ctx.rows(&profile)?;
    let relation = rows[ROOT as usize].relation.as_mut().unwrap();
    relation.effects = None;
    relation.projection = None;
    let limited = request(
        &ctx.vendor,
        Some(&ctx.production),
        Some(profile.fill),
        rows,
        1,
    );
    ctx.capacity_failure("rx-negative-capacity", &limited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blobray_domain::ExecutionEvent;

    fn read(address: u32, value: u32) -> ExecutionEvent {
        ExecutionEvent::Read {
            address,
            width: 4,
            value,
        }
    }

    #[test]
    fn matrices_cover_guards_samples_seeds_fills_and_settle() {
        let publication = publication_profiles();
        assert_eq!(publication.len(), 2 * 2 * FILLS.len() * 2);
        assert!(publication.iter().all(|p| !p.calibrates()));
        let calibration = calibration_profiles();
        assert_eq!(calibration.len(), 6 * 2 * FILLS.len() * 2);
        let streams = stream_profiles();
        assert_eq!(streams.len(), ESTIMATOR_STREAMS.len() * 2 * FILLS.len() * 2);
        assert!(streams.iter().all(Profile::long));
        let activity = activity_profiles();
        assert_eq!(activity.len(), 2 * FILLS.len() * 2);
        assert!(
            activity
                .iter()
                .all(|p| p.activity && p.rx_saturation && p.calibrates())
        );
        let samples: std::collections::BTreeSet<_> =
            calibration.iter().map(|p| p.estimator).collect();
        assert!(
            samples.contains(&Estimator::Constant(1 << 24))
                && samples.contains(&Estimator::Constant(-(1 << 24)))
                && samples.contains(&Estimator::Cycle(CALIBRATION_CYCLE))
        );
    }

    #[test]
    fn parameter_image_places_every_semantic_field() {
        let profile = Profile {
            flags: DC_CALIBRATED | TABLES_INITIALIZED,
            estimator: Estimator::Constant(0),
            activity: false,
            rx_saturation: false,
            seed: 17,
            fill: 0x5a,
            settle: false,
        };
        let image = parameter_image(&profile);
        let values = parameters(17);
        let word = |offset: usize| u16::from_le_bytes([image[offset], image[offset + 1]]);
        assert_eq!(word(WIFI_INDEX_DC), values[0]);
        assert_eq!(word(WIFI_FINE_DC + 18), values[25]);
        assert_eq!(word(SHARED_INDEX_DC + 42), values[47]);
        assert_eq!(word(WIFI_AUXILIARY), 0x125);
        assert_eq!(
            u32::from_le_bytes(image[GUARD_FLAGS..GUARD_FLAGS + 4].try_into().unwrap()),
            128 + 512
        );
        assert_eq!(image[PARAMETER_RX_PATH], RX_PATH);
        assert_eq!(
            (image[SHARED_LAST_INDEX], image[WIFI_LAST_INDEX]),
            (INITIAL_SHARED_LAST, INITIAL_WIFI_LAST)
        );
    }

    #[test]
    fn committed_fields_tile_the_output_except_index_padding() {
        let mut covered = vec![false; OUTPUT_BYTES as usize];
        for field in &COMMITTED {
            let length = (u32::from(field.width) * field.count) as usize;
            assert!(field.parameter as usize + length <= PHY_PARAM_BYTES as usize);
            for byte in &mut covered[field.output as usize..][..length] {
                assert!(!*byte, "{}", field.name);
                *byte = true;
            }
        }
        let padding: Vec<_> = (0..covered.len()).filter(|i| !covered[*i]).collect();
        let index_high = 2 * COEFFICIENT_WORDS + 1;
        assert_eq!(padding, [index_high, index_high + 2]);
    }

    #[test]
    fn contract_selects_only_the_skipped_dc_snapshot_and_transport() {
        crate::install();
        let rules = rx_rules(MAX_EVENTS);
        let selected = |event: &ExecutionEvent, next: &ExecutionEvent| {
            rules
                .iter()
                .filter(|r| r.vendor.unwrap().selects(event, Some(next)))
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
        };
        let delay = ExecutionEvent::DelayMicros { value: 1 };
        // The RX root never polls with a wait: every wait compares.
        assert!(selected(&delay, &read(ESTIMATOR_READY, 0)).is_empty());
        assert!(selected(&delay, &read(PBUS_STATUS, 0)).is_empty());
        // Only the snapshot before a skipped-path successor may be omitted.
        for successor in SKIPPED_DC_SUCCESSORS {
            assert_eq!(selected(&read(DC_CONTROL, 7), &read(successor, 0)).len(), 1);
        }
        assert!(selected(&read(DC_CONTROL, 7), &read(DC_CONTROL, 7)).is_empty());
    }
}
