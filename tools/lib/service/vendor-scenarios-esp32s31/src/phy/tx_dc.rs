//! Complete TX-DC/PWDET root over the captured archive and ROM against the
//! compiled production executor.
//!
//! The real search, PBus and SAR children execute for Wi-Fi and Bluetooth with
//! constant, alternating and precheck-cycling synthetic SAR samples, both
//! tone-clear and settle paths. Both sides compare ordered effects and the
//! measured DC rows; the vendor must keep the independent Wi-Fi gain
//! adjustment. Production-only PBus and SAR faults must not publish
//! calibration, and the SAR observation limit is a failure distinct from time
//! or work limits. Synthetic measurements establish software effects, not RF
//! accuracy.
use crate::contracts::{OutputField, omitted_read, output_projection, phy_contract};
use crate::evidence::{events, output, stop};
use crate::harness::{Buffer, Result, case, known, selection, with_stack_fill};
use crate::i2c::{all_complete, returned_low};
use crate::layout::*;
use crate::phy::delay_calls;
use crate::phy::{PhyImage, PhyOptions, Right, image_layout, phy_sdk_input, select, start_session};
use crate::session::request;
use blobray_domain::{
    ComparisonVerdict, DeviceBehavior, DeviceDeclaration, EffectRule, ExecutionCase,
    ExecutionEvent, ExecutionEvidence, ExecutionStop, Invocation, LinkRequest, RegionLifetime,
    SessionReset,
};
use oer_riscv_model::ArtifactId;
use std::path::Path;

pub type Options = PhyOptions;

/// Power-detector readiness word; bits 16:14 report a ready detector.
pub(crate) const DETECTOR_STATUS: u32 = 0x2010_080c;
pub(crate) const DETECTOR_READY: u32 = 7 << 14;
/// SAR result word; the tone average consumes its upper sample (bits 29:17).
const SAR_RESULT: u32 = 0x2010_081c;
const SAR_SAMPLE_SHIFT: u32 = 17;
/// Result words the ROM `phy_read_sar_dout` snapshots but its tone-average
/// caller never consumes; production does not read them.
pub(crate) const SAR_UNUSED: [u32; 3] = [0x2010_0820, 0x2010_0824, 0x2010_0828];
/// Production TX-DC entry compared with `phy_txdc_cal_pwdet_init`.
const PRODUCTION_ENTRY: &str = "open_phy_calibration_trace_tx_dc_pwdet";
/// Bluetooth PBus path word.
const BLUETOOTH_PBUS_PATH: u32 = 0x2010_0894;
/// Low-power SAR control word outside the radio block.
const LP_SAR_CONTROL: u32 = 0x2070_1068;
/// Idle PBus status with writable retained clock bits.
pub(crate) const PBUS_IDLE: u32 = 0x1234;
const WORK_MODE_SETTLE: u32 = 2;
/// Stuck PBus status and stuck detector readiness of the fault cases.
const PBUS_STUCK: u32 = 0x8000_0000;
const DETECTOR_STUCK: u32 = 0;
/// Production outcomes of a failed PBus transaction and of the SAR readiness
/// observation limit.
const FAILED_CALIBRATION: u32 = 4;
const SAR_OBSERVATION_LIMIT: u32 = 5;
/// Root arguments and the TX path value both sides select.
const TX_PATH: u8 = 7;
/// `phy_param` offsets: TX path, tone-clear flag, Wi-Fi gain adjustment and
/// the Wi-Fi and Bluetooth DC rows.
const PARAMETER_TX_PATH: usize = 20;
const PARAMETER_CLEAR: usize = 426;
const PARAMETER_ADJUSTMENT: usize = 434;
const WIFI_ROWS: usize = 168;
const BLUETOOTH_ROWS: usize = 260;
/// Three rows of four DC halfwords.
const ROW_WORDS: usize = 12;
const ROW_BYTES: u32 = (ROW_WORDS * 2) as u32;
/// Production output: Wi-Fi rows, then Bluetooth rows. The probe writes the
/// selected band's rows; the other band's rows start zero, like the vendor's
/// unselected `phy_param` rows.
const OUTPUT_BYTES: u32 = 2 * ROW_BYTES;
/// Committed `phy_param` DC rows of both bands in production output order.
const COMMITTED: [OutputField; 2] = [
    OutputField {
        name: "wifi-dc-rows",
        parameter: WIFI_ROWS as u32,
        output: 0,
        width: 2,
        count: ROW_WORDS as u32,
    },
    OutputField {
        name: "bluetooth-dc-rows",
        parameter: BLUETOOTH_ROWS as u32,
        output: ROW_BYTES,
        width: 2,
        count: ROW_WORDS as u32,
    },
];
/// Alternating SAR sample period.
const ALTERNATING: [u32; 8] = [100, 100, 130, 130, 90, 90, 80, 80];
/// SAR samples whose three-sample period is odd against the precheck
/// measurement pair: the positive and negative prechecks keep differing, so a
/// component ends with repeated prechecks and the next component restarts
/// its precheck count.
const PRECHECK_CYCLE: &[u32] = &[100, 100, 140];
/// Event capacity of one TX-DC root side: a complete root records about
/// 41,000 events, and the bounded fault polls stay within the same capacity.
const TXDC_EVENTS: u32 = 1 << 17;
/// Cases of one profile and the root's index within them.
const PROFILE_CASES: u32 = 3;
const ROOT: u32 = 2;

/// Synthetic SAR measurement stream.
#[derive(Clone, Copy, Debug)]
pub enum Samples {
    Constant(u32),
    Alternating,
    /// Samples returned in turn with this period.
    Cycle(&'static [u32]),
}

/// One TX-DC/PWDET root profile.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    pub bluetooth: bool,
    pub samples: Samples,
    /// Clear the tone after the detector is ready.
    pub clear: bool,
    pub fill: u8,
    pub settle: bool,
}

impl Profile {
    fn label(&self) -> String {
        let samples = match self.samples {
            Samples::Cycle(values) => format!(
                "cycle{}",
                values
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("_")
            ),
            samples => format!("{samples:?}"),
        };
        format!(
            "txdc-bt{}-{samples}-clear{}-fill{:x}-settle{}",
            u8::from(self.bluetooth),
            u8::from(self.clear),
            self.fill,
            u8::from(self.settle)
        )
        .to_lowercase()
        .replace(['(', ')'], "")
    }
    fn rows(&self) -> usize {
        if self.bluetooth {
            BLUETOOTH_ROWS
        } else {
            WIFI_ROWS
        }
    }
    /// Production output address of the selected band's rows.
    fn output(&self) -> u32 {
        OUTPUT + u32::from(self.bluetooth) * ROW_BYTES
    }
    /// Production output address of the other band's rows.
    fn unselected_output(&self) -> u32 {
        OUTPUT + u32::from(!self.bluetooth) * ROW_BYTES
    }
}

/// Wi-Fi and Bluetooth, five sample streams, both clear paths, both fills and
/// both settle branches.
pub fn profiles() -> Vec<Profile> {
    let mut result = vec![];
    for bluetooth in [false, true] {
        for samples in [
            Samples::Constant(0),
            Samples::Constant(123),
            Samples::Constant(8191),
            Samples::Alternating,
            Samples::Cycle(PRECHECK_CYCLE),
        ] {
            for clear in [false, true] {
                for fill in FILLS {
                    for settle in [false, true] {
                        result.push(Profile {
                            bluetooth,
                            samples,
                            clear,
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

fn cyclic_sar(values: &[u32]) -> DeviceDeclaration {
    DeviceDeclaration {
        id: "sar-samples".into(),
        applicability: "periodic synthetic SAR measurement stream".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::CyclicRead {
            address: SAR_RESULT,
            width: 4,
            values: values.iter().map(|v| v << SAR_SAMPLE_SHIFT).collect(),
        },
    }
}

/// Seeded DC rows: flat for the first fill, ramped for the second.
pub fn seeded_rows(fill: u8) -> [u16; ROW_WORDS] {
    core::array::from_fn(|i| {
        if fill == FILLS[0] {
            256
        } else {
            240 + i as u16
        }
    })
}

fn row_bytes(rows: &[u16; ROW_WORDS]) -> Vec<u8> {
    rows.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Captured `phy_param` image of one profile.
pub fn parameter_image(profile: &Profile) -> Vec<u8> {
    let mut image = vec![0u8; PHY_PARAM_BYTES as usize];
    image[PARAMETER_TX_PATH] = TX_PATH;
    image[PARAMETER_CLEAR] = u8::from(profile.clear);
    image[PARAMETER_ADJUSTMENT] = profile.fill;
    let rows = profile.rows();
    image[rows..rows + ROW_BYTES as usize].copy_from_slice(&row_bytes(&seeded_rows(profile.fill)));
    image
}

/// Explicit inputs over the retained radio aperture; `status` and `detector`
/// replace the idle PBus status and the ready detector in the fault cases.
pub fn tx_models(profile: &Profile, status: u32, detector: u32) -> Vec<DeviceDeclaration> {
    let fill = fill_word(profile.fill);
    let sar = match profile.samples {
        Samples::Constant(value) => {
            constant_read("sar-samples", SAR_RESULT, value << SAR_SAMPLE_SHIFT)
        }
        Samples::Alternating => cyclic_sar(&ALTERNATING),
        Samples::Cycle(values) => cyclic_sar(values),
    };
    let mut models = vec![
        register_bank(
            "txdc-inputs",
            "work-mode settle branch, PBus status and detector readiness",
            vec![
                (WORK_MODE, u32::from(profile.settle) * WORK_MODE_SETTLE),
                (PBUS_STATUS, status),
                (DETECTOR_STATUS, detector),
            ],
        ),
        register_bank(
            "lp-sar",
            "retained low-power SAR control word",
            vec![(LP_SAR_CONTROL, fill)],
        ),
        sar,
        constant_read("bluetooth-pbus-path", BLUETOOTH_PBUS_PATH, fill),
    ];
    for (i, address) in SAR_UNUSED.iter().enumerate() {
        models.push(constant_read(&format!("sar-unused-{i}"), *address, fill));
    }
    models.push(radio_aperture(profile.fill));
    models
}

/// Reviewed rules of the TX-DC root, which issues no analog I2C command:
/// the three SAR result words the vendor snapshots but never consumes.
pub fn tx_rules() -> Vec<EffectRule> {
    let mut rules = vec![];
    for address in SAR_UNUSED {
        rules.push(omitted_read(
            format!("sar-unused-{address:08x}"),
            address,
            TXDC_EVENTS,
            "phy_read_sar_dout snapshots this result word; its tone-average caller never consumes it",
        ));
    }
    rules
}

pub struct TxDc {
    pub image: PhyImage,
    rom_delay: u32,
    production_delay: u32,
    effects: ArtifactId,
    committed: ArtifactId,
}

impl std::ops::Deref for TxDc {
    type Target = PhyImage;
    fn deref(&self) -> &PhyImage {
        &self.image
    }
}
impl std::ops::DerefMut for TxDc {
    fn deref_mut(&mut self) -> &mut PhyImage {
        &mut self.image
    }
}

impl TxDc {
    pub fn new(options: &Options, phy_sdk: &Path) -> Result<Self> {
        let session = start_session(
            options,
            &[phy_sdk_input(phy_sdk)?],
            "captured TX-DC/PWDET calibration and fault containment; no RF qualification",
        )?;
        let link = LinkRequest {
            companions: vec![],
            inputs: vec![session.input_id(0)?],
            entry: select(&session, 0, "phy_txdc_cal_pwdet_init")?,
            roots: vec![select(&session, 0, "phy_get_romfunc_addr")?],
            layout: image_layout(),
            absent: vec![],
        };
        let mut image = PhyImage::link(
            session,
            &link,
            &options.linker,
            "phy_txdc_cal_pwdet_init",
            // ROM first; the co-located RFPLL diagnostics reference `phy_printf`,
            // bound to the PHY SDK firmware and never executed.
            &[ROM_INPUT, PHY_SDK_INPUT],
        )?;
        let applicability = "TX-DC/PWDET calibration under synthetic SAR samples and explicit PBus and detector inputs";
        let (vendor, production) = (
            image.vendor_endpoint("phy_txdc_cal_pwdet_init")?,
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
        let contract = phy_contract(vendor, production, tx_rules(), applicability);
        let effects = image.review_effects(
            "txdc-effects",
            "esp32s31.phy.tx-dc-pwdet.effects",
            contract,
            "every TX-DC register effect compares exactly except unused SAR snapshots",
        )?;
        let committed = image.review_projection(
            "txdc-committed",
            "esp32s31.phy.tx-dc-pwdet.committed",
            projection,
            "production publishes the selected band's DC rows and leaves the other band's rows",
        )?;
        Ok(Self {
            rom_delay: image.sym(1, "ets_delay_us"),
            production_delay: image.sym(2, "ets_delay_us"),
            image,
            effects,
            committed,
        })
    }

    fn vendor_phase(&self, profile: &Profile) -> Invocation {
        let mut phase = self.enter(
            self.root("phy_txdc_cal_pwdet_init"),
            &[0, 0, u32::from(profile.bluetooth)],
            vec![],
            vec![selection(self.parameter, PHY_PARAM_BYTES)],
            tx_models(profile, PBUS_IDLE, DETECTOR_READY),
        );
        phase.calls = delay_calls("txdc-delay", self.rom_delay);
        phase
    }

    pub fn production_phase(
        &self,
        profile: &Profile,
        models: Vec<DeviceDeclaration>,
    ) -> Result<Invocation> {
        let probe = self.probes.invoke(
            PRODUCTION_ENTRY,
            vec![
                (
                    "input",
                    Buffer::new(INPUT, row_bytes(&seeded_rows(profile.fill))).into(),
                ),
                ("bluetooth", i64::from(profile.bluetooth).into()),
                ("tx_path_value", i64::from(TX_PATH).into()),
                ("clear_tone_after_ready", i64::from(profile.clear).into()),
                (
                    "output",
                    Buffer::new(profile.output(), vec![profile.fill; ROW_BYTES as usize]).into(),
                ),
            ],
            models,
            vec![selection(OUTPUT, OUTPUT_BYTES)],
        )?;
        let mut phase = self.enter_probe(probe);
        phase.memory.push(known(
            profile.unselected_output(),
            ROW_BYTES,
            &[0; ROW_BYTES as usize],
        )?);
        phase.calls = delay_calls("txdc-delay", self.production_delay);
        Ok(phase)
    }

    fn rows(&self, profile: &Profile) -> Result<Vec<ExecutionCase>> {
        let image = parameter_image(profile);
        let mut root = case(
            profile.label(),
            self.vendor_phase(profile),
            Some(self.production_phase(profile, tx_models(profile, PBUS_IDLE, DETECTOR_READY))?),
            SessionReset::Warm,
            false,
        );
        // Blobray compares every effect and the committed rows under the
        // reviewed contract and projection.
        let relation = root.relation.as_mut().unwrap();
        relation.effects = Some(self.effects.clone());
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
                case(
                    "install-captured-callbacks",
                    self.install_callbacks(INSTALLED_CALLBACK_SLOT)?,
                    Some(self.noop()),
                    SessionReset::Warm,
                    false,
                ),
                root,
            ],
            profile.fill,
        ))
    }
}

fn check(label: &str, profile: &Profile, records: &[ExecutionEvidence], root: u32) {
    assert_eq!(returned_low(records, root, true), Some(0), "{label}");
    for side in [false, true] {
        assert!(all_complete(records, root, side), "{label} {side}");
    }
    let parameters = output(records, root, false);
    assert_eq!(
        parameters[PARAMETER_ADJUSTMENT], profile.fill,
        "{label}: Wi-Fi gain adjustment overwritten"
    );
}

/// All profiles are one request; each starts cold with its own stack fill.
pub fn exercise(ctx: &mut TxDc) -> Result<()> {
    let all = profiles();
    let mut rows = vec![];
    for profile in &all {
        rows.extend(ctx.rows(profile)?);
    }
    let executed = ctx.image.execute_without_events(
        "txdc",
        rows,
        FILLS[0],
        Right::Production,
        ComparisonVerdict::Match,
        TXDC_EVENTS,
    )?;
    for (i, profile) in all.iter().enumerate() {
        let root = i as u32 * PROFILE_CASES + ROOT;
        check(&profile.label(), profile, &executed.records, root);
    }
    faults(ctx)?;
    negative(ctx)
}

/// A stuck PBus transaction and a detector that never becomes ready fail with
/// distinct typed causes and publish no DC rows; the SAR result is never read.
fn faults(ctx: &mut TxDc) -> Result<()> {
    let profile = Profile {
        bluetooth: false,
        samples: Samples::Constant(123),
        clear: false,
        fill: FILLS[0],
        settle: false,
    };
    let faults = [
        ("pbus-stuck", PBUS_STUCK, DETECTOR_READY, FAILED_CALIBRATION),
        (
            "detector-never-ready",
            PBUS_IDLE,
            DETECTOR_STUCK,
            SAR_OBSERVATION_LIMIT,
        ),
    ];
    let mut rows = vec![];
    for (name, status, detector, _) in faults {
        let mut row = case(
            name,
            ctx.production_phase(&profile, tx_models(&profile, status, detector))?,
            None,
            SessionReset::Cold,
            false,
        );
        row.relation = None;
        rows.push(row);
    }
    let production = ctx.production.clone();
    let request = request(&production, None, Some(profile.fill), rows, TXDC_EVENTS);
    let records = ctx.submit("txdc-faults", &request, None)?.records.clone();
    for (case, (name, _, _, outcome)) in faults.iter().enumerate() {
        let case = case as u32;
        assert_eq!(
            returned_low(&records, case, false),
            Some(*outcome),
            "{name}"
        );
        assert!(all_complete(&records, case, false), "{name}");
        let rows = output(&records, case, false);
        let selected = (profile.output() - OUTPUT) as usize;
        assert_eq!(
            rows[selected..][..ROW_BYTES as usize],
            [profile.fill; ROW_BYTES as usize],
            "{name}: DC rows published"
        );
        assert!(
            !events(&records, case, false).iter().any(|e| matches!(
                e,
                ExecutionEvent::Read {
                    address: SAR_RESULT,
                    ..
                }
            )),
            "{name}: SAR result consumed"
        );
    }
    Ok(())
}

/// A changed production tone-clear path is a DIFF, and so are changed
/// unselected production rows with unchanged effects. Omitted callback
/// installation leaves the vendor INCOMPLETE, and an undersized event
/// capacity publishes nothing.
fn negative(ctx: &mut TxDc) -> Result<()> {
    let profile = profiles()[0];
    let mut unselected = ctx.rows(&profile)?;
    let production = unselected[ROOT as usize].replacement.as_mut().unwrap();
    let rows = production
        .memory
        .iter_mut()
        .find(|r| r.seed.address == profile.unselected_output())
        .expect("unselected production rows");
    rows.seed.bytes = vec![profile.fill; ROW_BYTES as usize];
    ctx.image.execute(
        "txdc-negative-unselected-rows",
        unselected,
        profile.fill,
        Right::Production,
        ComparisonVerdict::Diff,
        TXDC_EVENTS,
    )?;
    let mut changed = ctx.rows(&profile)?;
    let other = Profile {
        clear: !profile.clear,
        ..profile
    };
    changed[ROOT as usize].replacement =
        Some(ctx.production_phase(&other, tx_models(&other, PBUS_IDLE, DETECTOR_READY))?);
    ctx.image.execute(
        "txdc-negative-changed-clear",
        changed,
        profile.fill,
        Right::Production,
        ComparisonVerdict::Diff,
        TXDC_EVENTS,
    )?;
    let mut uninstalled = ctx.rows(&profile)?;
    uninstalled[1].vendor = ctx.noop();
    let records = ctx
        .image
        .execute(
            "txdc-negative-uninstalled-callbacks",
            uninstalled,
            profile.fill,
            Right::Production,
            ComparisonVerdict::Incomplete,
            TXDC_EVENTS,
        )?
        .records;
    assert!(matches!(
        stop(&records, ROOT, false),
        ExecutionStop::Incomplete { .. }
    ));
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
    ctx.capacity_failure("txdc-negative-capacity", &limited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blobray_domain::EffectDisposition;

    fn read(address: u32, value: u32) -> ExecutionEvent {
        ExecutionEvent::Read {
            address,
            width: 4,
            value,
        }
    }

    #[test]
    fn matrix_covers_both_protocols_samples_clear_fills_and_settle() {
        let all = profiles();
        assert_eq!(all.len(), 2 * 5 * 2 * FILLS.len() * 2);
        assert!(
            all.iter()
                .any(|p| matches!(p.samples, Samples::Alternating))
        );
        assert!(
            all.iter()
                .any(|p| matches!(p.samples, Samples::Cycle(PRECHECK_CYCLE)))
        );
        let labels: std::collections::BTreeSet<_> = all.iter().map(Profile::label).collect();
        assert_eq!(labels.len(), all.len());
    }

    #[test]
    fn parameter_image_seeds_rows_path_clear_and_adjustment() {
        for bluetooth in [false, true] {
            let profile = Profile {
                bluetooth,
                samples: Samples::Constant(0),
                clear: true,
                fill: FILLS[1],
                settle: false,
            };
            let image = parameter_image(&profile);
            let rows = profile.rows();
            assert_eq!(image[rows..rows + 2], 240u16.to_le_bytes());
            assert_eq!(image[rows + 22..rows + 24], 251u16.to_le_bytes());
            assert_eq!(
                (
                    image[PARAMETER_TX_PATH],
                    image[PARAMETER_CLEAR],
                    image[PARAMETER_ADJUSTMENT]
                ),
                (TX_PATH, 1, FILLS[1])
            );
        }
        assert_eq!(seeded_rows(FILLS[0]), [256; ROW_WORDS]);
    }

    #[test]
    fn contract_omits_only_unused_sar_words() {
        crate::install();
        let rules = tx_rules();
        let omitted = |event: &ExecutionEvent| {
            rules
                .iter()
                .filter(|r| r.vendor.unwrap().selects(event, None))
                .map(|r| r.disposition)
                .collect::<Vec<_>>()
        };
        for address in SAR_UNUSED {
            assert_eq!(omitted(&read(address, 1)), [EffectDisposition::Omitted]);
        }
        assert!(omitted(&read(SAR_RESULT, 123 << SAR_SAMPLE_SHIFT)).is_empty());
        assert!(omitted(&read(DETECTOR_STATUS, DETECTOR_READY)).is_empty());
    }
}
