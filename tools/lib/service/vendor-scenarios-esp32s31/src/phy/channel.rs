//! Channel restoration over the captured archive's installed ROM callbacks
//! against compiled production.
//!
//! Every transition compares its complete effect stream under the reviewed
//! channel contract. Full-root cases cover every channel class and bandwidth
//! and establish TX gain publication and the committed channel, bandwidth and
//! temperature. Temperature-prefix cases cover every sensor window at boundary
//! codes and check a single sensor sample before the gain bank is first read.
//! Stuck readiness is production containment, not vendor timing equivalence. Software comparison under explicit peripheral inputs, never
//! hardware qualification.
use crate::contracts::{OutputField, output_projection, phy_contract, plumbing};
use crate::evidence::{PhyEffect, events, output, phy_effects, stop};
use crate::harness::{Buffer, Result, case, selection, with_stack_fill};
use crate::i2c::{all_complete, returned_low};
use crate::layout::*;
use crate::phy::delay_calls;
use crate::phy::{PhyImage, PhyOptions, Right, image_layout, select, start_session};
use crate::session::request;
use blobray_domain::{
    CommandCell, ComparisonVerdict, DeviceDeclaration, ExecutionCase, ExecutionEvent,
    ExecutionEvidence, ExecutionStop, Invocation, LinkRequest, SessionReset,
};
use oer_riscv_model::ArtifactId;

/// Offsets of the committed channel, temperature, bandwidth, 802.11p
/// enable/configuration and temperature-sensor index bytes in `phy_param`.
const PARAMETER_CHANNEL: usize = 284;
const PARAMETER_TEMPERATURE: usize = 0;
const PARAMETER_BANDWIDTH: usize = 287;
const PARAMETER_DOT11P: usize = 0x28;
const PARAMETER_SENSOR_INDEX: usize = 0x16;
/// Production output: channel, temperature, bandwidth, 802.11p and sensor
/// index halfwords.
const SEMANTIC_BYTES: u32 = 10;
/// Committed `phy_param` fields in production output order.
const COMMITTED: [OutputField; 5] = [
    OutputField {
        name: "channel",
        parameter: PARAMETER_CHANNEL as u32,
        output: 0,
        width: 2,
        count: 1,
    },
    OutputField {
        name: "temperature",
        parameter: PARAMETER_TEMPERATURE as u32,
        output: 2,
        width: 2,
        count: 1,
    },
    OutputField {
        name: "bandwidth",
        parameter: PARAMETER_BANDWIDTH as u32,
        output: 4,
        width: 1,
        count: 1,
    },
    OutputField {
        name: "dot11p",
        parameter: PARAMETER_DOT11P as u32,
        output: 6,
        width: 1,
        count: 2,
    },
    OutputField {
        name: "sensor-index",
        parameter: PARAMETER_SENSOR_INDEX as u32,
        output: 8,
        width: 1,
        count: 1,
    },
];
/// The sensor index the temperature transition commits, in production output order.
const SENSOR_COMMITTED: [OutputField; 1] = [OutputField {
    name: "sensor-index",
    parameter: PARAMETER_SENSOR_INDEX as u32,
    output: 0,
    width: 1,
    count: 1,
}];
/// Production temperature output: the sensor index halfword.
const SENSOR_BYTES: u32 = 2;
/// Untouched production output bytes.
const OUTPUT_FILL: u8 = 0xa5;
/// Cases of one transition: parameter setup, callback installation and the
/// transition itself, at this index.
const TRANSITION_CASES: u32 = 3;
const TRANSITION: u32 = 2;
/// Retained temperature-sensor DAC analog selector (block 0x69, register 6).
const TEMPERATURE_DAC: u32 = 0x0669;
/// Retained PLL analog selector (block 0x6b, register 2).
const PLL_REGISTER: u32 = 0x026b;

pub type Options = PhyOptions;

/// One channel transition: requested channel, bandwidth, sensor DAC, sensor
/// code, stack/register fill and the 802.11p enable and configuration bytes
/// both sides start with.
#[derive(Clone, Copy, Debug)]
pub struct Transition {
    pub channel: u32,
    pub cbw: u32,
    pub dac: u32,
    pub code: u32,
    pub fill: u8,
    pub dot11p: [u8; 2],
}

impl Transition {
    fn label(&self) -> String {
        let dot11p = match self.dot11p {
            [0, _] => String::new(),
            [enabled, configuration] => format!("-dot11p{enabled}-{configuration:x}"),
        };
        format!(
            "channel-{}-cbw{}-dac{:x}-code{}-fill{:x}{dot11p}",
            self.channel, self.cbw, self.dac, self.code, self.fill
        )
    }
}

/// Lowest request value both sides read as a 2.4-GHz frequency in MHz rather
/// than a channel number.
const FIRST_FREQUENCY_MHZ: u32 = 2412;
/// Frequency of 2.4-GHz channel zero and the channel spacing, in MHz.
const CHANNEL_ZERO_MHZ: u32 = 2407;
const CHANNEL_SPACING_MHZ: u32 = 5;

impl Transition {
    /// Channel number the transition commits: a MHz request selects the
    /// 2.4-GHz channel at or below it.
    fn committed_channel(&self) -> u32 {
        if self.channel >= FIRST_FREQUENCY_MHZ {
            (self.channel - CHANNEL_ZERO_MHZ) / CHANNEL_SPACING_MHZ
        } else {
            self.channel
        }
    }
}

/// Bandwidth request bytes beyond 0 and 1: a low nibble of 2 or more and a
/// nonzero high nibble reach every `phy_bb_cbw_chan_cfg` path.
const OTHER_CBW: [u32; 3] = [2, 3, 0x11];

/// Enabled 802.11p with a configuration byte distinct from every fill.
const DOT11P: [u8; 2] = [1, 0x3c];

/// Full-root matrix: every tracked channel class at both bandwidths, MHz
/// requests including one between channel centers, other bandwidth request
/// bytes, and one channel at both bandwidths with 802.11p enabled.
pub fn full_transitions() -> Vec<Transition> {
    let mut result = transitions(
        [1, 6, 11, 13]
            .into_iter()
            .flat_map(|channel| [0, 1].map(|cbw| (channel, cbw, 5, 100)))
            .chain([2412, 2437, 2472, 2474].map(|mhz| (mhz, 0, 5, 100)))
            .chain(OTHER_CBW.map(|cbw| (6, cbw, 5, 100))),
    );
    result.extend(
        transitions([0, 1].into_iter().map(|cbw| (6, cbw, 5, 100)))
            .into_iter()
            .map(|t| Transition {
                dot11p: DOT11P,
                ..t
            }),
    );
    result
}

/// Every sensor code in every DAC window, with the first fill. Each boundary
/// of the temperature conversion and of the next-DAC selection is reached
/// without deriving it from vendor thresholds.
pub fn sensor_transitions() -> Vec<Transition> {
    SENSOR_WINDOWS
        .iter()
        .flat_map(|window| {
            (0..=u8::MAX).map(|code| {
                let fill = FILLS[0];
                Transition {
                    channel: 13,
                    cbw: 1,
                    dac: window.0 | u32::from(fill & 0xf0),
                    code: u32::from(code),
                    fill,
                    dot11p: [0, 0],
                }
            })
        })
        .collect()
}

/// Cases of one sensor transition: parameter initialization, callback
/// installation, then the sample.
const SENSOR_CASES: u32 = 3;

/// Temperature-prefix matrix: every sensor DAC range with boundary codes.
pub fn prefix_transitions() -> Vec<Transition> {
    transitions(
        [5, 7, 15, 11, 10]
            .into_iter()
            .flat_map(|dac| [0, 64, 100, 255].map(move |code| (13, 1, dac, code))),
    )
}

fn transitions(specs: impl Iterator<Item = (u32, u32, u32, u32)>) -> Vec<Transition> {
    specs
        .flat_map(|(channel, cbw, dac, code)| {
            FILLS.map(|fill| Transition {
                channel,
                cbw,
                // The DAC field is the low nibble; retained high bits follow the fill.
                dac: dac | u32::from(fill & 0xf0),
                code,
                fill,
                dot11p: [0, 0],
            })
        })
        .collect()
}

/// Explicit peripheral inputs of one transition; no channel algorithm.
/// Explicit peripheral inputs of one transition; every other radio register
/// is retained storage starting with the fill pattern. No channel algorithm.
pub fn channel_models(transition: &Transition, ready: bool) -> Vec<DeviceDeclaration> {
    vec![
        analog_bank(
            "channel-analog",
            "explicit sensor DAC and PLL analog registers",
            0,
            [0, 0],
            [(TEMPERATURE_DAC, transition.dac), (PLL_REGISTER, 0)]
                .map(|(selector, initial)| CommandCell {
                    selector,
                    initial,
                    reads: None,
                })
                .to_vec(),
        ),
        register_bank(
            "channel-inputs",
            "cleared work mode and transport read-mask and host-map words",
            vec![(WORK_MODE, 0), (I2C_READ_MASK, 0), (I2C_HOST_MAP, 0)],
        ),
        constant_read("temperature-code", TEMPERATURE_CODE, transition.code),
        constant_read(
            "channel-ready",
            CHANNEL_STATUS,
            if ready { CHANNEL_READY } else { 0 },
        ),
        radio_aperture(transition.fill),
    ]
}

/// Production channel entry compared with `phy_chip_set_chan`.
const PRODUCTION_ENTRY: &str = "open_phy_channel_trace_state";
/// ROM temperature-sensor transition, the callback target of `phy_tsens_temp_read`.
pub const SENSOR_ROOT: &str = "phy_tsens_temp_read_local";
/// Production temperature transition compared with `SENSOR_ROOT`.
pub const SENSOR_ENTRY: &str = "open_phy_trace_temperature_sample";

/// Channel image, its production probe and the reviewed transition claims.
pub struct Channel {
    pub image: PhyImage,
    rom_delay: u32,
    production_delay: u32,
    /// Register-preserving short-delay event of the production probes: the
    /// short settles of the transition, such as the force-TX/RX and RFPLL
    /// waits, reach it instead of `ets_delay_us`.
    short_delay: u32,
    effects: ArtifactId,
    committed: ArtifactId,
    sensor_effects: ArtifactId,
    sensor_committed: ArtifactId,
}

impl std::ops::Deref for Channel {
    type Target = PhyImage;
    fn deref(&self) -> &PhyImage {
        &self.image
    }
}
impl std::ops::DerefMut for Channel {
    fn deref_mut(&mut self) -> &mut PhyImage {
        &mut self.image
    }
}

/// Effects before the gain bank is first read; exactly one sensor read.
pub fn temperature_prefix(effects: &[PhyEffect]) -> &[PhyEffect] {
    let end = effects
        .iter()
        .position(|e| matches!(e, PhyEffect::Read(GAIN_BASE, _)))
        .expect("channel must reach gain publication after temperature sampling");
    let prefix = &effects[..end];
    assert_eq!(
        prefix
            .iter()
            .filter(|e| matches!(e, PhyEffect::Read(TEMPERATURE_CODE, _)))
            .count(),
        1
    );
    prefix
}

impl Channel {
    pub fn new(options: &Options) -> Result<Self> {
        let session = start_session(
            options,
            &[],
            "captured channel restoration, temperature prefix and stuck readiness; no RF qualification",
        )?;
        let link = LinkRequest {
            companions: vec![],
            inputs: vec![session.input_id(0)?],
            entry: select(&session, 0, "phy_chip_set_chan")?,
            roots: vec![select(&session, 0, "phy_get_romfunc_addr")?],
            layout: image_layout(),
            absent: vec![],
        };
        let mut image = PhyImage::link(
            session,
            &link,
            &options.linker,
            "phy_chip_set_chan",
            &[ROM_INPUT],
        )?;
        let applicability = "channel transitions under explicit sensor, PLL and readiness inputs";
        let (vendor, production) = (
            image.vendor_endpoint("phy_chip_set_chan")?,
            image.production_endpoint(PRODUCTION_ENTRY)?,
        );
        let projection = output_projection(
            vendor.clone(),
            image.parameter,
            production.clone(),
            SEMANTIC_BYTES,
            &COMMITTED,
            applicability,
        );
        let contract = phy_contract(vendor, production, plumbing(&[], MAX_EVENTS), applicability);
        let effects = image.review_effects(
            "channel-effects",
            "esp32s31.phy.channel-transition.effects",
            contract,
            "every channel register effect compares exactly except analog I2C polling",
        )?;
        let committed = image.review_projection(
            "channel-committed",
            "esp32s31.phy.channel-transition.committed",
            projection,
            "production publishes the committed channel, temperature and bandwidth",
        )?;
        let sensor_applicability =
            "one temperature-sensor transition under an explicit DAC and code";
        let (sensor_vendor, sensor_production) = (
            image.session.input_endpoint(ROM_INPUT, SENSOR_ROOT)?,
            image.production_endpoint(SENSOR_ENTRY)?,
        );
        let sensor_projection = output_projection(
            sensor_vendor.clone(),
            image.parameter,
            sensor_production.clone(),
            SENSOR_BYTES,
            &SENSOR_COMMITTED,
            sensor_applicability,
        );
        let sensor_contract = phy_contract(
            sensor_vendor,
            sensor_production,
            plumbing(&[], MAX_EVENTS),
            sensor_applicability,
        );
        let sensor_effects = image.review_effects(
            "temperature-effects",
            "esp32s31.phy.temperature-transition.effects",
            sensor_contract,
            "every sensor register effect compares exactly except analog I2C polling",
        )?;
        let sensor_committed = image.review_projection(
            "temperature-committed",
            "esp32s31.phy.temperature-transition.committed",
            sensor_projection,
            "production publishes the sensor index the transition commits",
        )?;
        Ok(Self {
            sensor_effects,
            sensor_committed,
            rom_delay: image.sym(1, "ets_delay_us"),
            production_delay: image.sym(2, "ets_delay_us"),
            short_delay: image.probes.entry("open_phy_trace_delay_event")?,
            image,
            effects,
            committed,
        })
    }

    fn vendor_phase(&self, t: &Transition, ready: bool) -> Invocation {
        let mut phase = self.enter(
            self.root("phy_chip_set_chan"),
            &[t.channel, t.cbw],
            vec![],
            vec![selection(self.parameter, PHY_PARAM_BYTES)],
            channel_models(t, ready),
        );
        phase.calls = delay_calls("channel-delay", self.rom_delay);
        phase
    }

    pub fn production_phase(&self, t: &Transition, ready: bool) -> Result<Invocation> {
        let probe = self.probes.invoke(
            PRODUCTION_ENTRY,
            vec![
                ("channel_or_frequency", i64::from(t.channel).into()),
                ("cbw", i64::from(t.cbw).into()),
                ("dot11p", Buffer::new(INPUT, t.dot11p).into()),
                (
                    "output",
                    Buffer::new(OUTPUT, [OUTPUT_FILL; SEMANTIC_BYTES as usize]).into(),
                ),
            ],
            channel_models(t, ready),
            vec![selection(OUTPUT, SEMANTIC_BYTES)],
        )?;
        let mut phase = self.enter_probe(probe);
        phase.calls = delay_calls("channel-delay", self.production_delay);
        phase
            .calls
            .extend(delay_calls("channel-short-delay", self.short_delay));
        Ok(phase)
    }

    /// Zeroed parameters, captured callback installation for the ROM I2C
    /// helpers, then one temperature-sensor transition of each side that must
    /// return the same temperature with the same sensor effects.
    fn sensor_rows(&self, t: &Transition) -> Result<Vec<ExecutionCase>> {
        let mut vendor = self.enter(
            self.sym(ROM_INPUT as usize, SENSOR_ROOT),
            &[],
            vec![],
            vec![selection(self.parameter, PHY_PARAM_BYTES)],
            channel_models(t, true),
        );
        vendor.calls = delay_calls("sensor-delay", self.rom_delay);
        let probe = self.probes.invoke(
            SENSOR_ENTRY,
            vec![(
                "output",
                Buffer::new(OUTPUT, [OUTPUT_FILL; SENSOR_BYTES as usize]).into(),
            )],
            channel_models(t, true),
            vec![selection(OUTPUT, SENSOR_BYTES)],
        )?;
        let mut production = self.enter_probe(probe);
        production.calls = delay_calls("sensor-delay", self.production_delay);
        let mut sample = case(
            format!("sensor-{}", t.label()),
            vendor,
            Some(production),
            SessionReset::Warm,
            false,
        );
        let relation = sample.relation.as_mut().unwrap();
        relation.effects = Some(self.sensor_effects.clone());
        relation.projection = Some(self.sensor_committed.clone());
        relation.returns.low = true;
        let zero = vec![0u8; PHY_PARAM_BYTES as usize];
        Ok(with_stack_fill(
            vec![
                case(
                    "initialize",
                    self.setup(&zero, false),
                    Some(self.setup(&zero, true)),
                    SessionReset::Cold,
                    false,
                ),
                case(
                    "install-captured-callbacks",
                    self.install_callbacks(INSTALLED_CALLBACK_SLOT)?,
                    Some(self.noop()),
                    SessionReset::Warm,
                    false,
                ),
                sample,
            ],
            t.fill,
        ))
    }

    /// Zeroed parameters, captured callback installation, then the transition.
    fn rows(&self, t: &Transition, install: bool) -> Result<Vec<ExecutionCase>> {
        let zero = vec![0u8; PHY_PARAM_BYTES as usize];
        let mut parameters = zero.clone();
        parameters[PARAMETER_DOT11P..PARAMETER_DOT11P + 2].copy_from_slice(&t.dot11p);
        let noop = self.noop();
        let mut transition = case(
            t.label(),
            self.vendor_phase(t, true),
            Some(self.production_phase(t, true)?),
            SessionReset::Warm,
            false,
        );
        // Blobray compares every effect and the committed state under the
        // reviewed contract and projection.
        let relation = transition.relation.as_mut().unwrap();
        relation.effects = Some(self.effects.clone());
        relation.projection = Some(self.committed.clone());
        let rows = vec![
            case(
                "initialize",
                self.setup(&parameters, false),
                Some(self.setup(&zero, true)),
                SessionReset::Cold,
                false,
            ),
            case(
                "install-captured-callbacks",
                if install {
                    self.install_callbacks(INSTALLED_CALLBACK_SLOT)?
                } else {
                    noop.clone()
                },
                Some(noop),
                SessionReset::Warm,
                false,
            ),
            transition,
        ];
        Ok(with_stack_fill(rows, t.fill))
    }
}

/// Committed vendor channel, temperature, bandwidth, 802.11p and sensor index
/// as production output bytes.
pub fn vendor_semantic(parameters: &[u8]) -> [u8; SEMANTIC_BYTES as usize] {
    [
        parameters[PARAMETER_CHANNEL],
        parameters[PARAMETER_CHANNEL + 1],
        parameters[PARAMETER_TEMPERATURE],
        parameters[PARAMETER_TEMPERATURE + 1],
        parameters[PARAMETER_BANDWIDTH],
        0,
        parameters[PARAMETER_DOT11P],
        parameters[PARAMETER_DOT11P + 1],
        parameters[PARAMETER_SENSOR_INDEX],
        0,
    ]
}

/// Independent semantic expectations of one completed transition. Returns the
/// vendor's raw events.
fn check_transition(
    label: &str,
    t: &Transition,
    records: &[ExecutionEvidence],
    root: u32,
) -> Vec<ExecutionEvent> {
    assert_eq!(returned_low(records, root, true), Some(0), "{label}");
    for side in [false, true] {
        assert!(all_complete(records, root, side), "{label} {side}");
    }
    let committed = vendor_semantic(&output(records, root, false));
    let (temperature, _) = sensor_expectation(t.dac)(t.code);
    let mut expected = [0u8; SEMANTIC_BYTES as usize];
    expected[..2].copy_from_slice(&(t.committed_channel() as u16).to_le_bytes());
    expected[2..4].copy_from_slice(&(temperature as i16).to_le_bytes());
    expected[4] = t.cbw as u8;
    expected[6..8].copy_from_slice(&t.dot11p);
    expected[8] = sensor_index(t.dac);
    assert_eq!(committed, expected, "{label}: vendor commit");
    events(records, root, false)
}

fn gain_writes(effects: &[PhyEffect]) -> usize {
    effects
        .iter()
        .filter(|e| matches!(e, PhyEffect::Write(a, _) if GAIN_DATA.contains(a)))
        .count()
}

pub fn exercise(ctx: &mut Channel) -> Result<()> {
    // Full-root evidence: every channel class and bandwidth, including TX
    // gain publication. Temperature-prefix evidence: every sensor DAC window
    // at boundary codes, compared completely, with one sensor sample before
    // gain publication. Each matrix is one request; every transition starts
    // cold with its own stack fill.
    for (name, matrix) in [
        ("full", full_transitions()),
        ("prefix", prefix_transitions()),
    ] {
        let mut rows = vec![];
        for t in &matrix {
            rows.extend(ctx.rows(t, true)?);
        }
        let executed = ctx.compare(&format!("channel-{name}"), rows, FILLS[0])?;
        let cases = crate::evidence::case_slices(&executed.records);
        for (i, t) in matrix.iter().enumerate() {
            let label = format!("{name}-{}", t.label());
            let root = i as u32 * TRANSITION_CASES + TRANSITION;
            let vendor = phy_effects(&check_transition(&label, t, cases[&root], root));
            if name == "full" {
                assert!(gain_writes(&vendor) > 0, "{label}: no gain publication");
            } else {
                temperature_prefix(&vendor);
            }
        }
    }
    sensor(ctx)?;
    stuck_readiness(ctx)?;
    negative(ctx)
}

/// The temperature-sensor transition alone, over every code of every window:
/// both sides return the oracle temperature with the same sensor effects.
fn sensor(ctx: &mut Channel) -> Result<()> {
    let matrix = sensor_transitions();
    let mut rows = vec![];
    for t in &matrix {
        rows.extend(ctx.sensor_rows(t)?);
    }
    let executed = ctx.compare("channel-sensor", rows, FILLS[0])?;
    let cases = crate::evidence::case_slices(&executed.records);
    for (i, t) in matrix.iter().enumerate() {
        let sample = i as u32 * SENSOR_CASES + SENSOR_CASES - 1;
        let records = cases[&sample];
        let (temperature, _) = sensor_expectation(t.dac)(t.code);
        for side in [false, true] {
            assert!(all_complete(records, sample, side), "{} {side}", t.label());
        }
        assert_eq!(
            returned_low(records, sample, false),
            Some(temperature as u32),
            "{}: vendor temperature",
            t.label()
        );
        assert_eq!(
            output(records, sample, false)[PARAMETER_SENSOR_INDEX],
            sensor_index(t.dac),
            "{}: vendor sensor index",
            t.label()
        );
    }
    Ok(())
}

/// Production containment: a channel that never reports readiness fails
/// without publishing gain or semantic output. Not vendor timing equivalence.
fn stuck_readiness(ctx: &mut Channel) -> Result<()> {
    let mut rows = vec![];
    for fill in FILLS {
        let t = Transition {
            channel: 13,
            cbw: 1,
            dac: 5,
            code: 100,
            fill,
            dot11p: [0, 0],
        };
        let mut row = case(
            format!("stuck-readiness-fill{fill:x}"),
            ctx.production_phase(&t, false)?,
            None,
            SessionReset::Cold,
            false,
        );
        row.relation = None;
        row.stack_fill = Some(fill);
        rows.push(row);
    }
    let production = ctx.production.clone();
    let request = request(&production, None, None, rows, MAX_EVENTS);
    let records = ctx
        .submit("stuck-readiness", &request, None)?
        .records
        .clone();
    for (case, fill) in FILLS.iter().enumerate() {
        let (case, label) = (case as u32, format!("stuck-readiness-fill{fill:x}"));
        assert_eq!(returned_low(&records, case, false), Some(1), "{label}");
        assert!(all_complete(&records, case, false), "{label}");
        assert_eq!(
            output(&records, case, false),
            [OUTPUT_FILL; SEMANTIC_BYTES as usize],
            "{label}: semantic output published"
        );
        let observed = events(&records, case, false);
        let effects = phy_effects(&observed);
        assert_eq!(gain_writes(&effects), 0, "{label}: gain published");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, PhyEffect::Read(CHANNEL_STATUS, 0))),
            "{label}: readiness never sampled"
        );
    }
    Ok(())
}

/// A changed production channel is a DIFF, uninstalled callbacks leave the
/// vendor INCOMPLETE, and an undersized event capacity publishes nothing.
fn negative(ctx: &mut Channel) -> Result<()> {
    let t = full_transitions()[0];
    let mut changed = ctx.rows(&t, true)?;
    let other = Transition { channel: 6, ..t };
    changed[TRANSITION as usize].replacement = Some(ctx.production_phase(&other, true)?);
    ctx.image.execute(
        "negative-changed-channel",
        changed,
        t.fill,
        Right::Production,
        ComparisonVerdict::Diff,
        MAX_EVENTS,
    )?;
    let uninstalled = ctx.rows(&t, false)?;
    let records = ctx
        .image
        .execute(
            "negative-uninstalled-callbacks",
            uninstalled,
            t.fill,
            Right::Production,
            ComparisonVerdict::Incomplete,
            MAX_EVENTS,
        )?
        .records;
    assert!(
        matches!(
            stop(&records, TRANSITION, false),
            ExecutionStop::Incomplete { .. }
        ),
        "uninstalled callbacks must not complete"
    );
    // The contract's occurrence bounds exceed a one-event capacity; the
    // exhaustion under test precedes any effect comparison.
    let mut rows = ctx.rows(&t, true)?;
    let relation = rows[TRANSITION as usize].relation.as_mut().unwrap();
    relation.effects = None;
    relation.projection = None;
    let limited = request(&ctx.vendor, Some(&ctx.production), Some(t.fill), rows, 1);
    ctx.capacity_failure("negative-capacity", &limited)
}

/// Sensor windows of the ROM `phy_tsens_attribute` object at 0x2f84_d9ec:
/// DAC, code calibration, and the inclusive accepted temperature range.
/// An independent research reading, used only as an expectation.
const SENSOR_WINDOWS: [(u32, i32, i32, i32); 5] = [
    (5, -2, 50, 125),
    (7, -1, 20, 100),
    (15, 0, -10, 80),
    (11, 1, -30, 50),
    (10, 2, -40, 20),
];

/// ROM `phy_code_to_temp`: saturating fixed-point code conversion.
pub fn temperature(code: u32, calibration: i32) -> i32 {
    let scaled = code as i32 * 44 - calibration * 2788;
    if scaled > 27_151 {
        250
    } else {
        ((scaled - 2052) / 100).max(-200)
    }
}

/// Expected temperature and whether the sample leaves the current DAC window.
/// ROM `phy_tsens_dac_to_index`: the DAC's position in the sensor windows.
pub fn sensor_index(dac: u32) -> u8 {
    SENSOR_WINDOWS
        .iter()
        .position(|w| w.0 == dac & 0xf)
        .expect("tracked sensor DAC") as u8
}

pub fn sensor_expectation(dac: u32) -> impl Fn(u32) -> (i32, bool) {
    let (_, calibration, low, high) = *SENSOR_WINDOWS
        .iter()
        .find(|w| w.0 == dac & 0xf)
        .expect("tracked sensor DAC");
    move |code| {
        let value = temperature(code, calibration);
        (value, !(low..=high).contains(&value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_oracle_follows_rom_windows() {
        // DAC 5: 100 * 44 + 2 * 2788 - 2052 = 7924 -> 79 C, inside 50..=125.
        assert_eq!(sensor_expectation(5)(100), (79, false));
        // DAC 10: 64 * 44 - 2 * 2788 - 2052 = -4812 -> -48 C, below -40.
        assert_eq!(sensor_expectation(0xa0 | 10)(64), (-48, true));
        assert_eq!(sensor_expectation(11)(64), (-20, false));
        assert!(sensor_expectation(5)(255).1 && sensor_expectation(15)(0).1);
    }

    #[test]
    fn matrices_cover_channels_windows_and_fills() {
        let full = full_transitions();
        assert_eq!(full.len(), (4 * 2 + 4 + OTHER_CBW.len() + 2) * FILLS.len());
        let committed: Vec<_> = full.iter().map(Transition::committed_channel).collect();
        assert!(committed.iter().all(|c| (1..=13).contains(c)));
        assert!(full.iter().any(|t| t.dot11p == DOT11P));
        let sensor = sensor_transitions();
        assert_eq!(sensor.len(), SENSOR_WINDOWS.len() * 256);
        assert!(sensor.len() as u32 * SENSOR_CASES <= blobray_domain::MAX_EXECUTION_CASES as u32);
        let prefix = prefix_transitions();
        assert_eq!(prefix.len(), SENSOR_WINDOWS.len() * 4 * FILLS.len());
        for t in &prefix {
            assert_eq!(t.dac & 0xf0, u32::from(t.fill & 0xf0));
            assert!(SENSOR_WINDOWS.iter().any(|w| w.0 == t.dac & 0xf));
        }
    }

    #[test]
    fn prefix_ends_at_gain_base_after_one_sample() {
        let effects = [
            PhyEffect::Read(TEMPERATURE_CODE, 1),
            PhyEffect::Write(WORK_MODE, 4),
            PhyEffect::Read(GAIN_BASE, 0),
            PhyEffect::Read(TEMPERATURE_CODE, 2),
        ];
        assert_eq!(temperature_prefix(&effects), &effects[..2]);
        let twice = [effects[0], effects[0], effects[2]];
        assert!(std::panic::catch_unwind(|| temperature_prefix(&twice).len()).is_err());
        assert!(std::panic::catch_unwind(|| temperature_prefix(&effects[..2]).len()).is_err());
    }

    #[test]
    fn semantic_bytes_select_channel_temperature_bandwidth_dot11p_and_sensor_index() {
        let mut parameters = vec![0u8; PHY_PARAM_BYTES as usize];
        parameters[PARAMETER_CHANNEL..PARAMETER_CHANNEL + 2].copy_from_slice(&[13, 0]);
        parameters[..2].copy_from_slice(&(-48i16).to_le_bytes());
        parameters[PARAMETER_BANDWIDTH] = 1;
        parameters[PARAMETER_DOT11P..PARAMETER_DOT11P + 2].copy_from_slice(&DOT11P);
        parameters[PARAMETER_SENSOR_INDEX] = 4;
        assert_eq!(
            vendor_semantic(&parameters),
            [13, 0, 0xd0, 0xff, 1, 0, DOT11P[0], DOT11P[1], 4, 0]
        );
    }

    #[test]
    fn sensor_index_follows_the_rom_dac_order() {
        assert_eq!([5, 7, 15, 11, 0xa0 | 10].map(sensor_index), [0, 1, 2, 3, 4]);
    }
}
