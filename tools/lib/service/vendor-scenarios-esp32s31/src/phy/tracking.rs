//! Combined calibration and tracking parents over the captured archive and
//! ROM against the compiled production executors.
//!
//! The vendor roots execute their real children: RX DC calibration, channel
//! restoration, TX-DC/PWDET, gain publication and, for the parent, power,
//! I2C band and RFPLL tracking. Each case compares every effect under a
//! reviewed contract and the committed `phy_param` state under a reviewed
//! output projection. Peripheral inputs are synthetic; grant overrides and
//! RF exclusion are outside this software comparison.
use crate::contracts::{
    omitted_read, omitted_read_before, output_projection, phy_contract, plumbing,
};
use crate::harness::{Buffer, Result, case, selection, with_stack_fill};
use crate::i2c::returned_low;
use crate::layout::*;
use crate::phy::committed::{
    CALIBRATION, CALIBRATION_BYTES, COMMITTED_BYTES, COMMON_REFERENCE, CURRENT_TEMPERATURE,
    GAIN_ADJUSTMENT, I2C_BAND, OutputField, PARAMETER_BANDWIDTH, PARAMETER_CHANNEL, PARENT,
    SENSOR_INDEX, SHARED_LAST_INDEX, TRANSMIT_REFERENCE, WIFI_LAST_INDEX, committed,
};
use crate::phy::{
    PhyImage, PhyOptions, Right, delay_calls, image_layout, phy_sdk_input, select, start_session,
};
use crate::tx_dc::{self, DETECTOR_READY, PBUS_IDLE, SAR_UNUSED, Samples};
use blobray_domain::{
    CommandCell, ComparisonVerdict, DeviceDeclaration, EffectRule, ExecutionCase,
    ExecutionEvidence, Invocation, LinkRequest, ReadRun, SessionReset,
};
use oer_riscv_model::ArtifactId;
use std::path::Path;

pub type Options = PhyOptions;

/// Event capacity of one tracking root side.
const TRACKING_EVENTS: u32 = 1 << 20;
/// Production probe input and output words of the combined root and parent.
const COMBINED_INPUT_WORDS: usize = 8;
const PARENT_INPUT_WORDS: usize = 15;
const COMBINED_OUTPUT_BYTES: u32 = 170;
const PARENT_OUTPUT_BYTES: u32 = 184;
/// Untouched production output bytes.
const OUTPUT_FILL: u8 = 0xa5;
/// Channel, bandwidth and crystal selector of every case.
const CHANNEL: u16 = 13;
const BANDWIDTH: u16 = 1;
const CRYSTAL: u16 = 0;
/// Threshold input that selects the default threshold.
const DEFAULT_THRESHOLD: u16 = 256;
/// `phy_param` offsets of the tracking inputs.
const PARAMETER_RX_PATH: usize = 2;
const PARAMETER_TX_PATH: usize = 20;
const THRESHOLD_OVERRIDE: usize = 432;
const RFPLL_ENABLED: usize = 10;
const POWER_ENABLED: usize = 11;
const TONE_CLEAR: usize = 427;
/// Sensor index of a fresh production state: DAC 15, the third sensor window.
const INITIAL_SENSOR_INDEX: u8 = 2;
/// Initial retained values the children consume.
const RX_PATH: u8 = 0xbf;
const TX_PATH: u8 = 1;
const INITIAL_SHARED_LAST: u8 = 75;
const INITIAL_WIFI_LAST: u8 = 71;
const THRESHOLD_FLAG: u8 = 2;
/// Channel readiness word, and the frequency-control word a nonzero RFPLL
/// correction starts from.
const CHANNEL_READY_WORD: u32 = 0x100;
const RFPLL_CHANNEL_WORD: u32 = 0x2582_4f58;
/// Out-of-block modem word the RX channel restoration samples.
const MODEM_CHANNEL: u32 = 0x2081_8000;
const MODEM_CHANNEL_VALUE: u32 = 128;
/// RX DC estimator readiness and sample words.
const ESTIMATOR_READY: u32 = 0x2010_047c;
const ESTIMATOR_DONE: u32 = 0x10000;
const ESTIMATOR_SAMPLE: u32 = 0x2010_0464;
const ESTIMATOR_NEGATED: u32 = 0x2010_0468;
const ESTIMATOR_MAGNITUDE: u32 = 0x2010_046c;
/// Signed estimator sample per stack fill.
fn rx_sample(fill: u8) -> i32 {
    if fill == FILLS[0] { 64 } else { -64 }
}

/// Additional committed parent fields: power temperature, shared cache,
/// `phy_param` guard bytes of the parent: a nonzero inhibit byte skips every
/// child (the vendor ORs it with byte 0x17), and a nonzero calibration byte
/// skips calibration tracking.
const TRACKING_INHIBIT: usize = 0x195;
const CALIBRATION_TRACKING_DISABLED: usize = 0x192;

/// Which root and production entry a case family compares.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Root {
    Combined,
    Parent,
}

impl Root {
    fn vendor(self) -> &'static str {
        match self {
            Root::Combined => "phy_cal_param_track",
            Root::Parent => "phy_param_track_tot",
        }
    }
    fn production(self) -> &'static str {
        match self {
            Root::Combined => "open_phy_calibration_trace_combined",
            Root::Parent => "open_phy_tracking_trace_parent",
        }
    }
    fn input_words(self) -> usize {
        match self {
            Root::Combined => COMBINED_INPUT_WORDS,
            Root::Parent => PARENT_INPUT_WORDS,
        }
    }
    fn output_bytes(self) -> u32 {
        match self {
            Root::Combined => COMBINED_OUTPUT_BYTES,
            Root::Parent => PARENT_OUTPUT_BYTES,
        }
    }
    fn fields(self) -> Vec<OutputField> {
        let mut fields = Vec::from(CALIBRATION);
        if self == Root::Parent {
            fields.extend(PARENT);
        }
        fields.extend(committed(self.output_bytes() - COMMITTED_BYTES));
        fields
    }
}

/// One tracking case: current, common-reference and transmit-reference
/// temperatures, whether RX calibration runs (selects estimator inputs only;
/// the fixture does not reproduce the gate algorithm), threshold input and
/// the RFPLL correction when RFPLL tracking is enabled.
#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub name: &'static str,
    pub temperatures: [i16; 3],
    pub rx: bool,
    pub threshold: u16,
    pub correction: Option<i8>,
    pub clients: (bool, bool),
    pub fill: u8,
    /// Vendor code of the I2C tracking band both parent sides start from.
    pub previous_band: u8,
    /// Whether parent Bluetooth/802.15.4 TX-power tracking is enabled.
    pub power_tracking: bool,
    /// Whether a vendor guard inhibits the whole parent.
    pub inhibited: bool,
    /// Whether the parent runs calibration tracking.
    pub calibration_tracking: bool,
}

impl Case {
    fn label(&self, root: Root) -> String {
        format!(
            "{root:?}-{}-band{}-power{}-inhibit{}-cal{}-wifi{}-bt{}-fill{:x}-rfpll{:?}",
            self.name,
            self.previous_band,
            u8::from(self.power_tracking),
            u8::from(self.inhibited),
            u8::from(self.calibration_tracking),
            u8::from(self.clients.0),
            u8::from(self.clients.1),
            self.fill,
            self.correction
        )
        .to_lowercase()
    }
    fn inputs(&self, root: Root) -> Vec<u16> {
        let [t0, t1, t2] = self.temperatures.map(|t| t as u16);
        let mut words = Vec::with_capacity(root.input_words());
        words.extend([t0, t1, t2, CHANNEL, BANDWIDTH, CRYSTAL, self.threshold]);
        if root == Root::Parent {
            words.extend([
                u16::from(self.fill),
                u16::from(self.fill == FILLS[0]),
                u16::from(self.correction.is_some()),
            ]);
        }
        // The retained RX-gain table last indices both sides start from.
        words.push(u16::from_le_bytes([INITIAL_SHARED_LAST, INITIAL_WIFI_LAST]));
        if root == Root::Parent {
            words.extend([
                u16::from(self.previous_band),
                u16::from(self.power_tracking),
                u16::from(self.inhibited),
                u16::from(self.calibration_tracking),
            ]);
        }
        words
    }
    fn parameters(&self, root: Root) -> Vec<u8> {
        let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
        let words = self.inputs(root);
        for (offset, value) in [
            (CURRENT_TEMPERATURE, words[0]),
            (COMMON_REFERENCE, words[1]),
            (TRANSMIT_REFERENCE, words[2]),
            (PARAMETER_CHANNEL, words[3]),
        ] {
            data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [
            (PARAMETER_RX_PATH, RX_PATH),
            (PARAMETER_TX_PATH, TX_PATH),
            (PARAMETER_BANDWIDTH, BANDWIDTH as u8),
            (SHARED_LAST_INDEX, INITIAL_SHARED_LAST),
            (WIFI_LAST_INDEX, INITIAL_WIFI_LAST),
            (SENSOR_INDEX, INITIAL_SENSOR_INDEX),
        ] {
            data[offset] = value;
        }
        if root == Root::Parent {
            data[TONE_CLEAR] = u8::from(self.fill == FILLS[0]);
            data[GAIN_ADJUSTMENT] = self.fill;
            data[POWER_ENABLED] = u8::from(self.power_tracking);
            data[RFPLL_ENABLED] = u8::from(self.correction.is_some());
            data[I2C_BAND] = self.previous_band;
            data[TRACKING_INHIBIT] = u8::from(self.inhibited);
            data[CALIBRATION_TRACKING_DISABLED] = u8::from(!self.calibration_tracking);
        }
        if self.threshold < DEFAULT_THRESHOLD {
            data[THRESHOLD_OVERRIDE..THRESHOLD_OVERRIDE + 2]
                .copy_from_slice(&[THRESHOLD_FLAG, self.threshold as u8]);
        }
        data
    }
}

/// Explicit peripheral inputs of one case; every other radio register is
/// retained storage starting with the fill.
pub fn tracking_models(case: &Case, search: bool, detector: u32) -> Vec<DeviceDeclaration> {
    let fill = case.fill;
    let tx = tx_dc::Profile {
        bluetooth: false,
        samples: Samples::Alternating,
        clear: false,
        fill,
        settle: fill == FILLS[0],
    };
    let mut models = tx_dc::tx_models(&tx, PBUS_IDLE, detector);
    let correction = case.correction.filter(|_| search);
    let mut cells: Vec<CommandCell> = [
        (0x0162u32, 100u32),
        (0x0262, 0x95),
        (0x0562, 100),
        (0x0762, 0xc2),
        (0x0b62, 0x15),
        (0x0462, u32::from(fill)),
        (0x1362, u32::from(fill)),
        (0x1462, u32::from(fill)),
        (0x0367, u32::from(fill)),
        (0x0669, 0x55),
        (0x026b, 0),
        (0x036b, u32::from(fill)),
        (0x076b, u32::from(fill)),
    ]
    .map(|(selector, initial)| CommandCell {
        selector,
        initial,
        reads: None,
    })
    .to_vec();
    if case.rx {
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
    }
    if let Some(delta) = correction {
        let statuses: Vec<u32> = match delta {
            0 => vec![0xaf; 2 * crate::rfpll::SEARCH_SAMPLES as usize],
            5 => [vec![0xa7; 2], vec![0xa3; 10], vec![0xab; 2]].concat(),
            -5 => [vec![0xa3; 10], vec![0xa7; 2], vec![0xab; 2]].concat(),
            _ => panic!("unsupported fixture correction"),
        };
        cells.push(CommandCell {
            selector: 0x0c62,
            initial: 0,
            reads: Some(statuses),
        });
    }
    models.push(analog_bank(
        "tracking-analog",
        "explicit retained analog cells, RX DC samples and RFPLL lock statuses",
        0,
        [0, 0],
        cells,
    ));
    let nonzero = correction.is_some_and(|delta| delta != 0);
    let mut registers = vec![(
        CHANNEL_STATUS,
        if nonzero {
            RFPLL_CHANNEL_WORD
        } else {
            CHANNEL_READY_WORD
        },
    )];
    if nonzero {
        registers.extend([(0x2010_0020, 0xa5a4_5678), (0x2010_002c, 0)]);
        models.push(sequence_read(
            "rfpll-frequency-memory",
            0x2010_0040,
            (0..85u32)
                .map(|i| ReadRun::once(0x0055_0000 | (i << 8) | (100 + i)))
                .collect(),
        ));
    }
    models.push(register_bank(
        "tracking-inputs",
        "channel readiness and RFPLL frequency-control words",
        registers,
    ));
    let sample = rx_sample(fill);
    for (id, address, value) in [
        ("estimator-ready", ESTIMATOR_READY, ESTIMATOR_DONE),
        ("estimator-sample", ESTIMATOR_SAMPLE, sample as u32),
        (
            "estimator-negated",
            ESTIMATOR_NEGATED,
            sample.wrapping_neg() as u32,
        ),
        (
            "estimator-magnitude",
            ESTIMATOR_MAGNITUDE,
            sample.unsigned_abs(),
        ),
        ("modem-channel", MODEM_CHANNEL, MODEM_CHANNEL_VALUE),
    ] {
        models.push(constant_read(id, address, value));
    }
    models
}

/// Reviewed rules of every tracking root: transport plumbing, the PBus
/// polling interval, the vendor's unused SAR snapshots and its channel-status
/// resample before a frequency-control read.
fn tracking_rules() -> Vec<EffectRule> {
    let mut rules = plumbing(&[PBUS_STATUS], TRACKING_EVENTS);
    for address in SAR_UNUSED {
        rules.push(omitted_read(
            format!("sar-unused-{address:08x}"),
            address,
            TRACKING_EVENTS,
            "phy_read_sar_dout snapshots this result word; its tone-average caller never consumes it",
        ));
    }
    rules.push(omitted_read_before(
        "channel-status-resample".into(),
        CHANNEL_STATUS,
        FREQUENCY_CONTROL,
        1,
        "the vendor samples channel status again immediately before reading frequency control",
    ));
    rules
}

pub struct Tracking {
    pub image: PhyImage,
    rom_delay: u32,
    production_delay: u32,
    /// Register-preserving short-delay event of the production probes.
    short_delay: u32,
    reviews: Vec<(Root, ArtifactId, ArtifactId)>,
}

impl std::ops::Deref for Tracking {
    type Target = PhyImage;
    fn deref(&self) -> &PhyImage {
        &self.image
    }
}
impl std::ops::DerefMut for Tracking {
    fn deref_mut(&mut self) -> &mut PhyImage {
        &mut self.image
    }
}

impl Tracking {
    pub fn new(options: &Options, phy_sdk: &Path) -> Result<Self> {
        let session = start_session(
            options,
            &[phy_sdk_input(phy_sdk)?],
            "captured combined calibration and parameter tracking parents; no RF qualification",
        )?;
        let link = LinkRequest {
            companions: vec![],
            inputs: vec![session.input_id(0)?],
            entry: select(&session, 0, "phy_param_track_tot")?,
            roots: vec![
                select(&session, 0, "phy_get_romfunc_addr")?,
                select(&session, 0, "phy_cal_param_track")?,
            ],
            layout: image_layout(),
            absent: vec![],
        };
        let mut image = PhyImage::link(
            session,
            &link,
            &options.linker,
            "phy_param_track_tot",
            &[ROM_INPUT, PHY_SDK_INPUT],
        )?;
        let mut reviews = vec![];
        for root in [Root::Combined, Root::Parent] {
            let applicability =
                "tracking roots under synthetic analog, estimator, SAR, PBus and RFPLL inputs";
            let (vendor, production) = (
                image.vendor_endpoint(root.vendor())?,
                image.production_endpoint(root.production())?,
            );
            let projection = output_projection(
                vendor.clone(),
                image.parameter,
                production.clone(),
                root.output_bytes(),
                &root.fields(),
                applicability,
            );
            let contract = phy_contract(vendor, production, tracking_rules(), applicability);
            let name = format!("{root:?}").to_lowercase();
            let effects = image.review_effects(
                &format!("{name}-effects"),
                &format!("esp32s31.phy.{name}-tracking.effects"),
                contract,
                "every tracking register effect compares exactly except transport polling and reviewed vendor snapshots",
            )?;
            let committed = image.review_projection(
                &format!("{name}-committed"),
                &format!("esp32s31.phy.{name}-tracking.committed"),
                projection,
                "production publishes the committed tracking references, DC rows and gain state",
            )?;
            reviews.push((root, effects, committed));
        }
        Ok(Self {
            rom_delay: image.sym(1, "ets_delay_us"),
            production_delay: image.sym(2, "ets_delay_us"),
            short_delay: image.probes.entry("open_phy_trace_delay_event")?,
            image,
            reviews,
        })
    }

    fn vendor_phase(&self, root: Root, case: &Case, search: bool) -> Invocation {
        let arguments: Vec<u32> = match root {
            Root::Combined => vec![0, case.clients.0.into(), case.clients.1.into()],
            Root::Parent => vec![case.clients.0.into(), case.clients.1.into()],
        };
        let mut phase = self.enter(
            self.root(root.vendor()),
            &arguments,
            vec![],
            vec![selection(self.parameter, PHY_PARAM_BYTES)],
            tracking_models(case, search, DETECTOR_READY),
        );
        phase.calls = delay_calls("tracking-delay", self.rom_delay);
        phase
    }

    fn production_phase(&self, root: Root, case: &Case, search: bool) -> Result<Invocation> {
        let input: Vec<u8> = case
            .inputs(root)
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let length = root.output_bytes();
        let probe = self.probes.invoke(
            root.production(),
            vec![
                ("input", Buffer::new(INPUT, input).into()),
                ("wifi", i64::from(case.clients.0).into()),
                ("bluetooth", i64::from(case.clients.1).into()),
                (
                    "output",
                    Buffer::new(OUTPUT, vec![OUTPUT_FILL; length as usize]).into(),
                ),
            ],
            tracking_models(case, search, DETECTOR_READY),
            vec![selection(OUTPUT, length)],
        )?;
        let mut phase = self.enter_probe(probe);
        phase.calls = delay_calls("tracking-delay", self.production_delay);
        phase
            .calls
            .extend(delay_calls("tracking-short-delay", self.short_delay));
        Ok(phase)
    }

    /// Parameter setup, callback installation and the root of one case.
    pub fn rows(&self, root: Root, profile: &Case) -> Result<Vec<ExecutionCase>> {
        let search = profile.correction.is_some()
            && !matches!(profile.name, "rfpll-below" | "calibration-without-rfpll");
        let (_, effects, committed) = self
            .reviews
            .iter()
            .find(|(r, _, _)| *r == root)
            .expect("reviewed root");
        let parameters = profile.parameters(root);
        let noop = self.noop();
        let mut tracked = case(
            profile.label(root),
            self.vendor_phase(root, profile, search),
            Some(self.production_phase(root, profile, search)?),
            SessionReset::Warm,
            false,
        );
        let relation = tracked.relation.as_mut().unwrap();
        relation.effects = Some(effects.clone());
        relation.projection = Some(committed.clone());
        Ok(vec![
            case(
                "initialize",
                self.setup(&parameters, false),
                Some(noop.clone()),
                SessionReset::Cold,
                false,
            ),
            case(
                "install-captured-callbacks",
                self.install_callbacks(INSTALLED_CALLBACK_SLOT)?,
                Some(noop),
                SessionReset::Warm,
                false,
            ),
            tracked,
        ])
    }
}

/// Cases of one root family; the RFPLL family replaces the temperature cases.
pub fn cases(root: Root, rfpll: Option<i8>) -> Vec<Case> {
    let mut specs: Vec<(&'static str, [i16; 3], bool, u16)> = vec![
        ("no-op", [40, 40, 40], false, DEFAULT_THRESHOLD),
        ("tx-only", [40, 40, 0], false, DEFAULT_THRESHOLD),
        ("rx-and-tx", [40, 0, 0], true, DEFAULT_THRESHOLD),
        ("rx-removes-tx-demand", [40, 0, 90], true, DEFAULT_THRESHOLD),
        ("cooling-tx", [0, 0, 40], false, DEFAULT_THRESHOLD),
        ("rx-creates-tx-demand", [40, 0, 40], true, DEFAULT_THRESHOLD),
        ("exact-default", [30, 0, 30], true, DEFAULT_THRESHOLD),
        ("below-default", [29, 0, 29], false, DEFAULT_THRESHOLD),
        ("debug-threshold", [20, 0, 20], true, 20),
        ("zero-threshold", [40, 40, 40], true, 0),
    ];
    if root == Root::Parent {
        for (name, temperature) in [
            ("cold-clamp", -61),
            ("cold-limit", -60),
            ("cold-band", -20),
            ("nominal-low", -19),
            ("nominal-high", 54),
            ("elevated-low", 55),
            ("wifi-clamp", 81),
            ("elevated-high", 94),
            ("hot-low", 95),
            ("bt-clamp", 106),
            ("small-delta", 2),
            ("threshold-boundary", 10),
        ] {
            specs.push((name, [temperature; 3], false, DEFAULT_THRESHOLD));
        }
    }
    if rfpll.is_some() {
        specs = vec![
            ("rfpll-and-power", [40, 40, 40], false, DEFAULT_THRESHOLD),
            ("rfpll-and-calibration", [40, 0, 0], true, DEFAULT_THRESHOLD),
            ("rfpll-below", [14, 14, 14], false, DEFAULT_THRESHOLD),
            ("rfpll-exact", [15, 15, 15], false, DEFAULT_THRESHOLD),
            ("rfpll-cooling", [-40, -40, -40], false, DEFAULT_THRESHOLD),
            (
                "calibration-without-rfpll",
                [10, 10, 40],
                false,
                DEFAULT_THRESHOLD,
            ),
        ];
    }
    // Parent cases that start from a retained band: each non-nominal band
    // held, and a nominal temperature leaving an elevated band;
    // and one with Bluetooth/802.15.4 TX-power tracking disabled.
    // the parent inhibited, and calibration tracking disabled where it is due
    // (no RX calibration runs, so no DCODE reads are scripted).
    type Held = (&'static str, [i16; 3], bool, u8, bool, bool, bool);
    let held: &[Held] = if root == Root::Parent && rfpll.is_none() {
        &[
            ("cold-held", [-20; 3], false, 1, true, false, true),
            ("elevated-held", [55; 3], false, 2, true, false, true),
            ("hot-held", [95; 3], false, 3, true, false, true),
            (
                "nominal-from-elevated",
                [20; 3],
                false,
                2,
                true,
                false,
                true,
            ),
            (
                "power-tracking-disabled",
                [106; 3],
                false,
                0,
                false,
                false,
                true,
            ),
            (
                "power-tracking-disabled-cold",
                [-61; 3],
                false,
                0,
                false,
                false,
                true,
            ),
            ("inhibited", [40, 0, 0], false, 0, true, true, true),
            (
                "calibration-disabled",
                [40, 0, 0],
                false,
                0,
                true,
                false,
                false,
            ),
        ]
    } else {
        &[]
    };
    let specs: Vec<_> = specs
        .into_iter()
        .map(|(name, temperatures, rx, threshold)| {
            (name, temperatures, rx, threshold, 0, true, false, true)
        })
        .chain(held.iter().map(
            |&(name, temperatures, rx, band, power, inhibited, calibration)| {
                (
                    name,
                    temperatures,
                    rx,
                    DEFAULT_THRESHOLD,
                    band,
                    power,
                    inhibited,
                    calibration,
                )
            },
        ))
        .collect();
    let mut result = vec![];
    for clients in [(false, false), (true, false), (false, true), (true, true)] {
        for &(
            name,
            temperatures,
            rx,
            threshold,
            previous_band,
            power_tracking,
            inhibited,
            calibration_tracking,
        ) in &specs
        {
            for fill in FILLS {
                result.push(Case {
                    name,
                    temperatures,
                    rx,
                    threshold,
                    correction: rfpll,
                    clients,
                    fill,
                    previous_band,
                    power_tracking,
                    inhibited,
                    calibration_tracking,
                });
            }
        }
    }
    result
}

/// Each family is one request; each case starts cold with its own stack fill.
pub fn exercise(ctx: &mut Tracking) -> Result<()> {
    let mut families = vec![(Root::Combined, None), (Root::Parent, None)];
    families.extend([0, 5, -5].map(|delta| (Root::Parent, Some(delta))));
    for (root, rfpll) in families {
        let cases = cases(root, rfpll);
        let mut parts = vec![];
        for case in &cases {
            parts.push((case.fill, ctx.rows(root, case)?, *case));
        }
        let label = format!("{root:?}-rfpll{rfpll:?}").to_lowercase();
        let executed = ctx
            .image
            .compare_fills_without_events(&label, parts, TRACKING_EVENTS)?;
        for (case, records) in executed.parts {
            check(root, &case, &records);
        }
    }
    failed_tx(ctx)?;
    crate::tracking_graph::exercise(ctx)?;
    negative(ctx)
}

/// Production calibrating only the Wi-Fi client is a DIFF, omitted callback
/// installation leaves the vendor INCOMPLETE, and an undersized event
/// capacity publishes nothing.
fn negative(ctx: &mut Tracking) -> Result<()> {
    let base = cases(Root::Combined, None)
        .into_iter()
        .find(|c| c.name == "rx-and-tx" && c.clients == (true, true))
        .expect("combined rx-and-tx case");
    let mut changed = with_stack_fill(ctx.rows(Root::Combined, &base)?, base.fill);
    let other = Case {
        clients: (true, false),
        ..base
    };
    changed[2].replacement = Some(ctx.production_phase(Root::Combined, &other, false)?);
    ctx.image.execute(
        "tracking-negative-changed-clients",
        changed,
        base.fill,
        Right::Production,
        ComparisonVerdict::Diff,
        TRACKING_EVENTS,
    )?;
    let mut uninstalled = with_stack_fill(ctx.rows(Root::Combined, &base)?, base.fill);
    uninstalled[1].vendor = ctx.noop();
    ctx.image.execute(
        "tracking-negative-uninstalled-callbacks",
        uninstalled,
        base.fill,
        Right::Production,
        ComparisonVerdict::Incomplete,
        TRACKING_EVENTS,
    )?;
    // The contract's occurrence bounds exceed a one-event capacity; the
    // exhaustion under test precedes any effect comparison.
    let mut rows = with_stack_fill(ctx.rows(Root::Combined, &base)?, base.fill);
    let relation = rows[2].relation.as_mut().unwrap();
    relation.effects = None;
    relation.projection = None;
    let limited = crate::session::request(&ctx.vendor, Some(&ctx.production), None, rows, 1);
    ctx.capacity_failure("tracking-negative-capacity", &limited)
}

/// Detector state that never reports readiness.
const DETECTOR_STUCK: u32 = 0;
/// Production outcome of a failed combined transaction.
const FAILED_TRANSACTION: u32 = 1;
/// Parent power children's committed words at 40 degrees: power temperature,
/// shared cache and both gain bases (BT computes +8; Wi-Fi reuses the cache).
const COMPLETED_POWER: [u16; 4] = [40, 8, 8, 8];

/// Production-only: a TX detector that never becomes ready fails the whole
/// combined transaction after RX channel restoration ran. The output keeps
/// the pre-calibration coefficients; the parent keeps the power and RFPLL
/// children that completed before the failure.
fn failed_tx(ctx: &mut Tracking) -> Result<()> {
    let mut families = vec![(Root::Combined, None), (Root::Parent, None)];
    families.extend([0, 5, -5].map(|delta| (Root::Parent, Some(delta))));
    let mut rows = vec![];
    let mut expectations = vec![];
    for (root, correction) in families {
        for fill in FILLS {
            let profile = Case {
                name: "failed-tx",
                temperatures: [40, 0, 0],
                rx: true,
                threshold: DEFAULT_THRESHOLD,
                correction,
                clients: (true, true),
                fill,
                previous_band: 0,
                power_tracking: true,
                inhibited: false,
                calibration_tracking: true,
            };
            let input: Vec<u8> = profile
                .inputs(root)
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect();
            let length = root.output_bytes();
            let probe = ctx.probes.invoke(
                root.production(),
                vec![
                    ("input", Buffer::new(INPUT, input).into()),
                    ("wifi", 1.into()),
                    ("bluetooth", 1.into()),
                    (
                        "output",
                        Buffer::new(OUTPUT, vec![OUTPUT_FILL; length as usize]).into(),
                    ),
                ],
                tracking_models(&profile, correction.is_some(), DETECTOR_STUCK),
                vec![selection(OUTPUT, length)],
            )?;
            let mut phase = ctx.enter_probe(probe);
            phase.calls = delay_calls("tracking-delay", ctx.production_delay);
            phase
                .calls
                .extend(delay_calls("tracking-short-delay", ctx.short_delay));
            let mut row = case(profile.label(root), phase, None, SessionReset::Cold, false);
            row.relation = None;
            row.stack_fill = Some(fill);
            rows.push(row);
            let mut expected: Vec<u8> = profile.inputs(root)[..5]
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect();
            expected.resize(CALIBRATION_BYTES as usize, 0);
            if root == Root::Parent {
                let mut words = COMPLETED_POWER.to_vec();
                words.extend([
                    fill as i8 as i16 as u16,
                    0,
                    if correction.is_some() { 40 } else { 0 },
                ]);
                expected.extend(words.iter().flat_map(|w| w.to_le_bytes()));
            }
            // Cleared DCODE codes and status bytes, the initial last indices,
            // no progress and the initial sensor index.
            expected.extend([0; COMMITTED_BYTES as usize - 6]);
            expected.extend([INITIAL_SHARED_LAST, INITIAL_WIFI_LAST, 0, 0]);
            expected.extend([INITIAL_SENSOR_INDEX, 0]);
            expectations.push((profile.label(root), expected));
        }
    }
    let production = ctx.production.clone();
    let request = crate::session::request(&production, None, None, rows, TRACKING_EVENTS);
    let records = ctx
        .submit("tracking-failed-tx", &request, None)?
        .records
        .clone();
    for (case, (label, expected)) in expectations.iter().enumerate() {
        let case = case as u32;
        assert_eq!(
            returned_low(&records, case, false),
            Some(FAILED_TRANSACTION),
            "{label}: failed TX committed the transaction"
        );
        assert!(crate::i2c::all_complete(&records, case, false), "{label}");
        let observed = crate::evidence::events(&records, case, false);
        let read = |target: u32| {
            observed.iter().any(|e| {
                matches!(e, blobray_domain::ExecutionEvent::Read { address, .. } if *address == target)
            })
        };
        assert!(
            read(MODEM_CHANNEL),
            "{label}: RX channel restoration never ran"
        );
        assert!(
            read(tx_dc::DETECTOR_STATUS),
            "{label}: TX fault never reached"
        );
        assert_eq!(
            &crate::evidence::output(&records, case, false),
            expected,
            "{label}: state at the failed parent boundary"
        );
    }
    Ok(())
}

/// Production success and the parent's signed-word encoding; the committed
/// values themselves compare under the reviewed projection.
fn check(root: Root, case: &Case, records: &[ExecutionEvidence]) {
    let label = case.label(root);
    assert_eq!(
        returned_low(records, 2, true),
        Some(0),
        "{label}: production failed"
    );
    if root == Root::Parent {
        let parameters = crate::evidence::output(records, 2, false);
        assert_eq!(
            parameters[GAIN_ADJUSTMENT], case.fill,
            "{label}: runtime parent changed the gain adjustment"
        );
        let output = crate::evidence::output(records, 2, true);
        let word = |name: &str| {
            PARENT
                .iter()
                .find(|field| field.name == name)
                .map(|field| field.output as usize)
                .unwrap()
        };
        for name in [
            "shared-cache",
            "wifi-gain-base",
            "bluetooth-gain-base",
            "gain-adjustment",
        ] {
            let low = word(name);
            let sign = if output[low] & 0x80 != 0 { 0xff } else { 0 };
            assert_eq!(output[low + 1], sign, "{label}: signed word {name}");
        }
        assert_eq!(output[word("i2c-band") + 1], 0, "{label}: I2C band word");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_match_the_probe_arrays_and_fields_tile_the_outputs() {
        let case = cases(Root::Parent, Some(5))[0];
        for root in [Root::Combined, Root::Parent] {
            assert_eq!(case.inputs(root).len(), root.input_words());
        }
        for (root, padding) in [
            (Root::Combined, vec![9, 169]),
            (Root::Parent, vec![9, 157, 159, 161, 163, 165, 183]),
        ] {
            let mut covered = vec![false; root.output_bytes() as usize];
            for field in root.fields() {
                let length = (u32::from(field.width) * field.count) as usize;
                for byte in &mut covered[field.output as usize..][..length] {
                    assert!(!*byte, "{}", field.name);
                    *byte = true;
                }
            }
            let unclaimed: Vec<_> = (0..covered.len()).filter(|i| !covered[*i]).collect();
            assert_eq!(unclaimed, padding, "{root:?}");
        }
    }

    #[test]
    fn families_cover_clients_fills_and_parent_bands() {
        assert_eq!(cases(Root::Combined, None).len(), 4 * 10 * FILLS.len());
        let parent = cases(Root::Parent, None);
        assert_eq!(parent.len(), 4 * (22 + 8) * FILLS.len());
        assert!(parent.iter().any(|c| c.inhibited));
        assert!(parent.iter().any(|c| !c.calibration_tracking));
        assert!(parent.iter().any(|c| !c.power_tracking));
        assert!((1..=3).all(|band| parent.iter().any(|c| c.previous_band == band)));
        assert_eq!(cases(Root::Parent, Some(-5)).len(), 4 * 6 * FILLS.len());
    }
}
