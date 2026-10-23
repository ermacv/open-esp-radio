//! Alternating board captures. Every image write uses the run's leased port.
use crate::{
    Result,
    calibration_capture::{
        boots::{Images, Lifecycle, ProductionBoot, Readings, Transmitted, VendorBoot},
        space::{Register, Space},
        vendor,
    },
    comparison::Comparison,
};
use oer_hil_protocol::phy::*;
use oer_hil_scenario::workload::context::Context;
use oer_image_build::esp_idf::catalog;
use std::{
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};

const VENDOR_BOOT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_millis(100);
const RESTART_TIMEOUT: Duration = Duration::from_secs(30);
const REGISTER_IMAGE_TIMEOUT: Duration = Duration::from_secs(3);
const PREPARE_TIMEOUT: Duration = Duration::from_secs(120);
const CONSOLE_TAIL_CHARS: usize = 2000;

pub(crate) fn run<C: Comparison>(
    lifecycle: Lifecycle,
    boots: u16,
    vendor_only: bool,
    vendor_windows: &[crate::scenario::Window],
    vendor_transmit: bool,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let root = oer_os_process::built_root();
    let entries = catalog::entries(&root)?;
    let entry = catalog::entry(
        &entries,
        if lifecycle.ieee802154() {
            "ieee802154-vendor-reference"
        } else {
            "vendor-calibration"
        },
    )?;
    if entry.chip != C::CHIP || entry.pins != C::CHIP || entry.hold.is_some() {
        return Err("the calibration reference is held or targets another chip".into());
    }
    let build = catalog::build(&root, &entry.image)?;
    let bundle = catalog::bundle(&root, entry, &build, &output.join("vendor-image"))?;
    let images = context.images()?;
    let identity = Images {
        started_unix_seconds: oer_os_durable::unix_seconds(),
        vendor_project: lifecycle.vendor_project().into(),
        vendor_application_sha256: build.application_sha256.clone(),
        vendor_idf_revision: build.idf_revision.clone(),
        production_image: "diagnostic-phy".into(),
        production_application_sha256: oer_os_durable::sha256_file(
            &images.scenario_image().application(),
        )?,
    };
    context.results.observe("images", &identity);
    let published = C::images(&root)?;
    // Discovery may reset the reference; none of its values enter the gate.
    // Every subsequent measured capture must stay in its one primary boot.
    images.flash(&bundle, &entry.image)?;
    let discovered = reference_boot::<C>(
        context,
        lifecycle,
        &published,
        vendor_windows,
        vendor_transmit,
        &output.join("vendor-readability"),
        None,
    );
    let restored = images.restore();
    let (discovered, _) = discovered?;
    restored?;
    std::fs::write(
        output.join("vendor-readability.json"),
        serde_json::to_vec_pretty(&discovered)?,
    )?;
    context
        .results
        .observe("vendor-readability-discovery", &discovered);
    let mut vendors = Vec::new();
    let mut productions = Vec::new();
    for boot in 1..=boots {
        oer_os_process::check_cancelled()?;
        images.flash(&bundle, &entry.image)?;
        let stem = output.join(format!("vendor-{boot:03}"));
        let captured = reference_boot::<C>(
            context,
            lifecycle,
            &published,
            vendor_windows,
            vendor_transmit,
            &stem,
            Some(&discovered),
        );
        // Restore the diagnostic image even if a reference boot/read failed.
        let restored = images.restore();
        let (vendor_boot, channel) = captured?;
        restored?;
        std::fs::write(
            stem.with_extension("json"),
            serde_json::to_vec_pretty(&vendor_boot)?,
        )?;
        context
            .results
            .observe(format!("vendor-boot-{boot:03}"), &vendor_boot);
        context
            .results
            .observe(format!("vendor-channel-{boot:03}"), &channel);
        let readable: Vec<usize> = published
            .registers
            .iter()
            .enumerate()
            .filter(|(_, reg)| vendor_boot.registers.values.contains_key(&reg.address))
            .map(|(index, _)| index)
            .collect();
        vendors.push(vendor_boot);
        if !vendor_only {
            let directory = output.join(format!("production-{boot:03}"));
            let production = production_boot(
                context,
                lifecycle,
                channel,
                &directory,
                &published.registers,
                &readable,
                &published.analog,
            )?;
            std::fs::write(
                directory.join("capture.json"),
                serde_json::to_vec_pretty(&production)?,
            )?;
            context
                .results
                .observe(format!("production-boot-{boot:03}"), &production);
            productions.push(production);
        }
    }
    if vendor_only {
        context
            .results
            .claim("vendor boot spread captured", &["production-equivalence"]);
        return Ok(());
    }
    let compared = C::compare(lifecycle, &identity, &vendors, &productions, &published)?;
    context.results.observe("comparison", &compared.summary);
    std::fs::write(
        output.join("comparison.json"),
        serde_json::to_vec_pretty(&compared.summary)?,
    )?;
    match compared.mismatch {
        None => Ok(()),
        Some(reason) => Err(format!("calibration cross-check {reason}").into()),
    }
}

/// Capture only the scenario's production image. No vendor catalog build,
/// discovery, image flash or restore operation occurs on this path.
pub(crate) fn recorded<C: Comparison>(
    lifecycle: Lifecycle,
    boots: u16,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let root = oer_os_process::built_root();
    let board = &context.lab.dut.id;
    let reference = match C::recorded(&root, board, lifecycle) {
        Ok(reference) => reference,
        Err(error) => {
            let summary =
                serde_json::json!({"verdict":"incomplete", "reasons":[error.to_string()]});
            context.results.observe("comparison", &summary);
            std::fs::write(
                output.join("comparison.json"),
                serde_json::to_vec_pretty(&summary)?,
            )?;
            return Err(format!("recorded calibration INCOMPLETE: {error}").into());
        }
    };
    let identity = Images {
        started_unix_seconds: oer_os_durable::unix_seconds(),
        vendor_project: lifecycle.vendor_project().into(),
        vendor_application_sha256: reference.vendor_application_sha256.clone(),
        vendor_idf_revision: reference.vendor_idf_revision.clone(),
        production_image: "diagnostic-phy".into(),
        production_application_sha256: oer_os_durable::sha256_file(
            &context.images()?.scenario_image().application(),
        )?,
    };
    context.results.observe("images", &identity);
    context
        .results
        .observe("recorded-reference", &reference.document);
    let published = C::images(&root)?;
    let mut captures = vec![];
    for boot in 1..=boots {
        oer_os_process::check_cancelled()?;
        let directory = output.join(format!("production-{boot:03}"));
        let capture = production_boot(
            context,
            lifecycle,
            reference.channel,
            &directory,
            &published.registers,
            &reference.readable,
            &published.analog,
        )?;
        std::fs::write(
            directory.join("capture.json"),
            serde_json::to_vec_pretty(&capture)?,
        )?;
        context
            .results
            .observe(format!("production-boot-{boot:03}"), &capture);
        captures.push(capture);
    }
    let compared = C::compare_recorded(&root, board, &reference, &identity, &captures, &published)?;
    context.results.observe("comparison", &compared.summary);
    std::fs::write(
        output.join("comparison.json"),
        serde_json::to_vec_pretty(&compared.summary)?,
    )?;
    match compared.mismatch {
        None => Ok(()),
        Some(reason) => Err(format!("recorded calibration {reason}").into()),
    }
}

fn reference_boot<C: Comparison>(
    context: &Context<'_>,
    lifecycle: Lifecycle,
    published: &crate::comparison::RegisterImages,
    windows: &[crate::scenario::Window],
    transmit: bool,
    stem: &Path,
    known: Option<&VendorBoot>,
) -> Result<(VendorBoot, u8)> {
    if lifecycle.ieee802154() {
        ieee802154_boot(
            context,
            lifecycle,
            &published.registers,
            &published.analog,
            windows,
            transmit,
            stem,
            known,
        )
    } else {
        calibration_boot(
            context,
            lifecycle,
            C::VENDOR_OBJECT,
            &published.registers,
            &published.analog,
            stem,
            known,
        )
    }
}

fn validate_ready(ready: &CaptureReady, registers: usize, analog: usize) -> Result<()> {
    if ready.epoch == 0
        || !ready.full_calibration_performed
        || ready.path != CalibrationPath::FullForCache
        || ready.projection_schema != 1
        || ready.calibration_words == 0
        || usize::from(ready.registers) != registers
        || usize::from(ready.analog_bytes) != analog
    {
        return Err(
            "diagnostic capture did not establish full calibration and the published images".into(),
        );
    }
    Ok(())
}

fn check_page<T>(
    ready: &CaptureReady,
    window: CaptureWindow,
    epoch: u32,
    first: u16,
    total: u16,
    values: &[T],
    expected: u16,
) -> Result<()> {
    if epoch != ready.epoch
        || first != window.first
        || total != expected
        || values.len() != usize::from(window.count)
    {
        return Err(
            "capture page is missing, stale, truncated or changes its declared image".into(),
        );
    }
    Ok(())
}

fn production_boot(
    context: &Context<'_>,
    lifecycle: Lifecycle,
    channel: u8,
    directory: &Path,
    registers: &[Register],
    readable: &[usize],
    analog: &[Register],
) -> Result<ProductionBoot> {
    std::fs::create_dir_all(directory)?;
    let mut captured = ProductionBoot::default();
    let outcome = context.with_capture(directory, |capture| {
        let keys = capture.request_image_keys(REGISTER_IMAGE_TIMEOUT)?;
        if !keys.has::<PrepareCapture>() || !keys.has::<ReadAnalogImage>() {
            return Err("diagnostic image lacks capture endpoints".into());
        }
        let radio = if lifecycle.ieee802154() {
            CaptureRadio::Ieee802154 {
                power_dbm: crate::reference::POWER_DBM,
            }
        } else {
            CaptureRadio::Wifi
        };
        let mut ready = capture.request(0, PrepareCapture { channel, radio }, PREPARE_TIMEOUT)?;
        let result: Result<()> = (|| {
            validate_ready(&ready, registers.len(), analog.len())?;
            if ready.lifecycle
                != if lifecycle.ieee802154() {
                    CaptureLifecycle::Ieee802154
                } else {
                    CaptureLifecycle::Cold
                }
                || ready.channel != channel
                || ready.radio != radio
            {
                return Err("wrong cold capture point".into());
            }
            context.results.observe(
                format!(
                    "{}-registration",
                    directory
                        .file_name()
                        .ok_or("capture directory has no name")?
                        .to_string_lossy()
                ),
                &ready,
            );
            if matches!(lifecycle, Lifecycle::Restart | Lifecycle::Ieee802154Restart) {
                let restarted = capture.request(
                    0,
                    RestartCaptureRadio { epoch: ready.epoch },
                    RESTART_TIMEOUT,
                )?;
                let previous_epoch = ready.epoch;
                ready = restarted.ready;
                if !restarted.rf_closed
                    || !restarted.rf_woken
                    || ready.epoch <= previous_epoch
                    || ready.lifecycle
                        != if lifecycle.ieee802154() {
                            CaptureLifecycle::Ieee802154Restart
                        } else {
                            CaptureLifecycle::Restart
                        }
                    || ready.channel != channel
                    || ready.radio != radio
                {
                    return Err(
                        "radio restart did not establish a new closed-and-woken epoch".into(),
                    );
                }
                ready = restarted.ready;
                validate_ready(&ready, registers.len(), analog.len())?;
                context.results.observe(
                    format!(
                        "{}-restart",
                        directory
                            .file_name()
                            .ok_or("capture directory has no name")?
                            .to_string_lossy()
                    ),
                    &restarted,
                );
            }
            if !lifecycle.ieee802154() {
                let mut projection = Vec::new();
                for window in windows(
                    ready.epoch,
                    &(0..usize::from(ready.calibration_words)).collect::<Vec<_>>(),
                )? {
                    let page = capture.request(
                        0,
                        ReadCalibrationProjection(window),
                        REGISTER_IMAGE_TIMEOUT,
                    )?;
                    check_page(
                        &ready,
                        window,
                        page.epoch,
                        page.first,
                        page.total,
                        &page.values,
                        ready.calibration_words,
                    )?;
                    projection.extend(page.values.iter().flat_map(|value| value.to_le_bytes()));
                }
                captured.calibration = Some(projection);
            }
            for window in windows(ready.epoch, readable)? {
                let page = capture.request(0, ReadRegisterImage(window), REGISTER_IMAGE_TIMEOUT)?;
                check_page(
                    &ready,
                    window,
                    page.epoch,
                    page.first,
                    page.total,
                    &page.values,
                    ready.registers,
                )?;
                for (offset, value) in page.values.iter().enumerate() {
                    captured.registers.values.insert(
                        registers
                            .get(usize::from(window.first) + offset)
                            .ok_or("register page exceeds the published model")?
                            .address,
                        *value,
                    );
                }
            }
            for window in windows(ready.epoch, &(0..analog.len()).collect::<Vec<_>>())? {
                let page = capture.request(0, ReadAnalogImage(window), REGISTER_IMAGE_TIMEOUT)?;
                check_page(
                    &ready,
                    window,
                    page.epoch,
                    page.first,
                    page.total,
                    &page.values,
                    ready.analog_bytes,
                )?;
                for (offset, value) in page.values.iter().enumerate() {
                    captured.analog.values.insert(
                        analog
                            .get(usize::from(window.first) + offset)
                            .ok_or("analog page exceeds the published model")?
                            .address,
                        u32::from(*value),
                    );
                }
            }
            Ok(())
        })();
        let stopped = capture.request(0, StopCapture { epoch: ready.epoch }, RESTART_TIMEOUT);
        result?;
        let stopped = stopped?;
        if stopped.epoch != ready.epoch || !stopped.rf_closed {
            return Err("capture cleanup did not close RF".into());
        }
        Ok(())
    });
    if let Err(error) = outcome {
        let incomplete = serde_json::json!({
            "verdict": "INCOMPLETE", "failure": error.to_string(), "partial": &captured,
        });
        std::fs::write(
            directory.join("capture-incomplete.json"),
            serde_json::to_vec_pretty(&incomplete)?,
        )?;
        context.results.observe(
            format!(
                "capture-incomplete-{}",
                directory
                    .file_name()
                    .ok_or("capture directory has no name")?
                    .to_string_lossy()
            ),
            &incomplete,
        );
        return Err(format!("capture INCOMPLETE: {error}").into());
    }
    Ok(captured)
}

fn windows(epoch: u32, indices: &[usize]) -> Result<Vec<CaptureWindow>> {
    let mut windows: Vec<CaptureWindow> = Vec::new();
    for &index in indices {
        let first = u16::try_from(index)?;
        match windows.last_mut() {
            Some(last)
                if usize::from(last.first) + usize::from(last.count) == index
                    && usize::from(last.count) < CAPTURE_PAGE =>
            {
                last.count += 1
            }
            Some(last) if index < usize::from(last.first) + usize::from(last.count) => {
                return Err("capture indices must be unique and increasing".into());
            }
            _ => windows.push(CaptureWindow {
                epoch,
                first,
                count: 1,
            }),
        }
    }
    Ok(windows)
}
fn reset_console(port: &Path) -> Result<oer_lab_board::port::Port> {
    use oer_lab_board::reset::{open_without_reset, reset_usb_serial_jtag};
    let mut serial = open_without_reset(port)?;
    serial.set_timeout(READ_TIMEOUT)?;
    reset_usb_serial_jtag(&mut serial)?;
    Ok(serial)
}

/// One boot of the vendor calibration firmware: its report, then its
/// register and analog images, after one Wi-Fi radio restart at the
/// [`Lifecycle::Restart`] point. The console goes beside the observation.
fn calibration_boot(
    context: &Context<'_>,
    lifecycle: Lifecycle,
    vendor_object: &str,
    registers: &[Register],
    analog: &[Register],
    stem: &Path,
    known: Option<&VendorBoot>,
) -> Result<(VendorBoot, u8)> {
    let (console, mut serial) = vendor_boot(&context.lab.dut.serial)?;
    std::fs::write(stem.with_extension("log"), &console)?;
    let mut objects = vendor::parse(&console)?;
    let calibration = objects
        .remove(vendor_object)
        .ok_or_else(|| format!("the vendor boot reported no {vendor_object}"))?;
    let mut replies = String::new();
    let mut analog_replies = String::new();
    let result: Result<()> = (|| {
        if lifecycle == Lifecycle::Restart {
            vendor_restart(&mut serial, &mut replies)?;
        }
        vendor_registers(
            &mut serial,
            Space::Mmio,
            registers,
            lifecycle,
            &mut replies,
            known.map(|boot| &boot.registers),
        )?;
        vendor_registers(
            &mut serial,
            Space::Analog,
            analog,
            lifecycle,
            &mut analog_replies,
            known.map(|boot| &boot.analog),
        )?;
        Ok(())
    })();
    std::fs::write(stem.with_extension("registers.log"), &replies)?;
    std::fs::write(stem.with_extension("analog.log"), &analog_replies)?;
    if let Err(error) = result {
        std::fs::write(
            stem.with_extension("incomplete.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({ "verdict": "INCOMPLETE", "failure": error.to_string() }),
            )?,
        )?;
        return Err(error);
    }
    let channel = vendor::home_channel(&console)?;
    Ok((
        VendorBoot {
            calibration: Some(calibration),
            registers: Readings::parse(Space::Mmio, &replies)?,
            analog: Readings::parse(Space::Analog, &analog_replies)?,
            windows: None,
            transmitted: None,
        },
        channel,
    ))
}

/// The live reference peer preserves its reset/hang observations and restores
/// the requested RX lifecycle after each unreadable register.
fn ieee802154_boot(
    context: &Context<'_>,
    lifecycle: Lifecycle,
    registers: &[Register],
    analog: &[Register],
    windows: &[crate::scenario::Window],
    transmit: bool,
    stem: &Path,
    known: Option<&VendorBoot>,
) -> Result<(VendorBoot, u8)> {
    let mut peer = crate::reference::Peer::boot(
        reset_console(&context.lab.dut.serial)?,
        lifecycle == Lifecycle::Ieee802154Restart,
    )?;
    let mut partial = VendorBoot::default();
    let result: Result<()> = (|| {
        let replies = peer.registers(Space::Mmio, registers, known.map(|b| &b.registers))?;
        std::fs::write(stem.with_extension("registers.log"), &replies)?;
        partial.registers = Readings::parse(Space::Mmio, &replies)?;
        let replies = peer.registers(Space::Analog, analog, known.map(|b| &b.analog))?;
        std::fs::write(stem.with_extension("analog.log"), &replies)?;
        partial.analog = Readings::parse(Space::Analog, &replies)?;
        let extra = window_registers(windows)?;
        if !extra.is_empty() {
            let replies =
                peer.registers(Space::Mmio, &extra, known.and_then(|b| b.windows.as_ref()))?;
            std::fs::write(stem.with_extension("windows.log"), &replies)?;
            partial.windows = Some(Readings::parse(Space::Mmio, &replies)?);
        }
        if transmit {
            peer.transmit()?;
            let replies = peer.registers(
                Space::Analog,
                analog,
                known
                    .and_then(|b| b.transmitted.as_ref())
                    .map(|t| &t.analog),
            )?;
            std::fs::write(stem.with_extension("transmitted-analog.log"), &replies)?;
            let mut transmitted = Transmitted {
                analog: Readings::parse(Space::Analog, &replies)?,
                windows: None,
            };
            if !extra.is_empty() {
                let replies = peer.registers(
                    Space::Mmio,
                    &extra,
                    known
                        .and_then(|b| b.transmitted.as_ref())
                        .and_then(|t| t.windows.as_ref()),
                )?;
                std::fs::write(stem.with_extension("transmitted-windows.log"), &replies)?;
                transmitted.windows = Some(Readings::parse(Space::Mmio, &replies)?);
            }
            partial.transmitted = Some(transmitted);
        }
        Ok(())
    })();
    std::fs::write(stem.with_extension("log"), &peer.console)?;
    if let Err(error) = result {
        std::fs::write(
            stem.with_extension("incomplete.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({ "verdict": "INCOMPLETE", "failure": error.to_string(), "partial": partial }),
            )?,
        )?;
        return Err(error);
    }
    Ok((partial, crate::reference::CHANNEL))
}

fn window_registers(windows: &[crate::scenario::Window]) -> Result<Vec<Register>> {
    let mut registers = std::collections::BTreeMap::new();
    for window in windows {
        if window.words == 0 || window.words > 1024 || window.address % 4 != 0 {
            return Err("invalid vendor window".into());
        }
        for index in 0..window.words {
            let address = window
                .address
                .checked_add(index * 4)
                .ok_or("vendor window overflows")?;
            registers.entry(address).or_insert_with(|| Register {
                mask: u32::MAX,
                address,
                name: format!("window-{address:08x}"),
            });
        }
    }
    Ok(registers.into_values().collect())
}

fn vendor_boot(port: &Path) -> Result<(String, oer_lab_board::port::Port)> {
    let mut serial = reset_console(port)?;
    let started = Instant::now();
    let mut console = Vec::new();
    let mut buffer = [0; 1024];
    while started.elapsed() < VENDOR_BOOT_TIMEOUT {
        match serial.read(&mut buffer) {
            Ok(read) => console.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
        let text = String::from_utf8_lossy(&console);
        if vendor::complete(&text) {
            return Ok((text.into_owned(), serial));
        }
    }
    Err(format!(
        "no closed vendor report within {VENDOR_BOOT_TIMEOUT:?}; console:\n{}",
        String::from_utf8_lossy(&console)
    )
    .into())
}

/// Restart the vendor Wi-Fi radio and require that stopping the client
/// released every PHY modem, which closes RF. `replies` accumulates the
/// console.
fn vendor_restart(serial: &mut (impl Read + Write), replies: &mut String) -> Result<()> {
    let done = vendor::restarts(replies)?.len();
    serial.write_all(vendor::RESTART_REQUEST.as_bytes())?;
    let started = Instant::now();
    let mut buffer = [0; 256];
    loop {
        if let Some(&flags) = vendor::restarts(replies)?.get(done) {
            if flags != 0 {
                return Err(format!(
                    "the stopped vendor Wi-Fi client left PHY modem flags {flags:#x}, so RF stayed open"
                )
                .into());
            }
            return Ok(());
        }
        if started.elapsed() > RESTART_TIMEOUT {
            return Err(format!("no vendor restart reply within {RESTART_TIMEOUT:?}").into());
        }
        match serial.read(&mut buffer) {
            Ok(read) => replies.push_str(&String::from_utf8_lossy(&buffer[..read])),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// The vendor firmware's replies to reads of `registers`. A read that
/// resets the chip, such as a register whose clock domain the calibration
/// leaves off, shows as a new boot report instead of a reply: the register
/// is recorded as unreadable in this state and the reads continue once the
/// new boot reports, after restarting the radio again at the
/// [`Lifecycle::Restart`] point. `replies` holds the console so far, and
/// its restarts.
fn vendor_registers(
    serial: &mut (impl Read + Write),
    space: Space,
    registers: &[Register],
    lifecycle: Lifecycle,
    replies: &mut String,
    known: Option<&Readings>,
) -> Result<()> {
    let mut buffer = [0; 256];
    for register in registers {
        if let Some(known) = known {
            if !known.permits_read(register.address)? {
                replies.push_str(&vendor::unreadable_line(register.address));
                continue;
            }
        }
        serial.write_all(space.request(register.address).as_bytes())?;
        let boots = vendor::reports(replies);
        let started = Instant::now();
        loop {
            if space.replies(replies)?.contains_key(&register.address) {
                break;
            }
            if vendor::reports(replies) > boots {
                if known.is_some() {
                    return Err(format!(
                        "measured reference reset while reading {}; capture would span boots",
                        register.name
                    )
                    .into());
                }
                replies.push_str(&vendor::unreadable_line(register.address));
                if lifecycle == Lifecycle::Restart {
                    vendor_restart(serial, replies)?;
                }
                break;
            }
            if started.elapsed() > VENDOR_BOOT_TIMEOUT {
                let tail = replies
                    .char_indices()
                    .rev()
                    .nth(CONSOLE_TAIL_CHARS)
                    .map_or(replies.as_str(), |(start, _)| &replies[start..]);
                return Err(format!(
                    "no reply for {} ({:#010x}); console tail:\n{tail}",
                    register.name, register.address
                )
                .into());
            }
            match serial.read(&mut buffer) {
                Ok(read) => replies.push_str(&String::from_utf8_lossy(&buffer[..read])),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready() -> CaptureReady {
        CaptureReady {
            epoch: 1,
            lifecycle: CaptureLifecycle::Cold,
            full_calibration_performed: true,
            path: CalibrationPath::FullForCache,
            projection_schema: 1,
            calibration_words: 20,
            registers: 3,
            analog_bytes: 2,
            channel: 6,
            radio: CaptureRadio::Wifi,
        }
    }

    #[test]
    fn full_capture_contract_rejects_every_other_native_path() {
        for path in [
            CalibrationPath::FullUncached,
            CalibrationPath::FullAfterRejectedCache,
            CalibrationPath::PartialFromCache,
        ] {
            assert!(validate_ready(&CaptureReady { path, ..ready() }, 3, 2).is_err());
        }
    }

    #[test]
    fn incomplete_registration_and_changed_image_lengths_are_rejected() {
        validate_ready(&ready(), 3, 2).unwrap();
        assert!(
            validate_ready(
                &CaptureReady {
                    full_calibration_performed: false,
                    ..ready()
                },
                3,
                2
            )
            .is_err()
        );
        assert!(
            validate_ready(
                &CaptureReady {
                    epoch: 0,
                    ..ready()
                },
                3,
                2
            )
            .is_err()
        );
        assert!(validate_ready(&ready(), 4, 2).is_err());
        let window = CaptureWindow {
            epoch: 1,
            first: 2,
            count: 1,
        };
        assert!(check_page(&ready(), window, 2, 2, 3, &[9], 3).is_err());
        assert!(check_page::<u8>(&ready(), window, 1, 2, 3, &[], 3).is_err());
        assert!(check_page(&ready(), window, 1, 2, 4, &[9], 3).is_err());
    }
}

#[cfg(test)]
mod reference_boot_tests {
    use super::*;
    use std::collections::VecDeque;

    struct Serial {
        responses: VecDeque<Vec<u8>>,
        pending: VecDeque<u8>,
        requests: Vec<String>,
    }
    impl Read for Serial {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.pending.is_empty() {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
            }
            let count = buffer.len().min(self.pending.len());
            for byte in &mut buffer[..count] {
                *byte = self.pending.pop_front().unwrap();
            }
            Ok(count)
        }
    }
    impl Write for Serial {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.requests
                .push(String::from_utf8(data.to_vec()).unwrap());
            self.pending.extend(
                self.responses
                    .pop_front()
                    .expect("one scripted reply per command"),
            );
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn register(address: u32) -> Register {
        Register {
            mask: u32::MAX,
            address,
            name: format!("r{address}"),
        }
    }
    fn serial(responses: &[&str]) -> Serial {
        Serial {
            responses: responses.iter().map(|s| s.as_bytes().to_vec()).collect(),
            pending: VecDeque::new(),
            requests: Vec::new(),
        }
    }
    #[test]
    fn discovered_unreadable_reads_are_not_reissued_in_the_measured_boot() {
        let mut io = serial(&["oer-vendor-calibration-register 00000004 00000009\n"]);
        let known = Readings {
            values: [(4, 1)].into(),
            unreadable: [8].into(),
        };
        let mut log = String::new();
        vendor_registers(
            &mut io,
            Space::Mmio,
            &[register(4), register(8)],
            Lifecycle::Cold,
            &mut log,
            Some(&known),
        )
        .unwrap();
        assert_eq!(io.requests, ["r 00000004\n"]);
        let readings = Readings::parse(Space::Mmio, &log).unwrap();
        assert_eq!(readings.values, [(4, 9)].into());
        assert_eq!(readings.unreadable, [8].into());
    }
    #[test]
    fn a_new_boot_aborts_measurement_but_discovery_may_recover() {
        let mut io = serial(&["oer-vendor-calibration-end\n"]);
        let known = Readings {
            values: [(4, 1)].into(),
            unreadable: Default::default(),
        };
        let mut log = String::new();
        let result = vendor_registers(
            &mut io,
            Space::Mmio,
            &[register(4)],
            Lifecycle::Cold,
            &mut log,
            Some(&known),
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("capture would span boots")
        );
        assert!(log.contains("oer-vendor-calibration-end"));
        let mut io = serial(&["oer-vendor-calibration-end\n"]);
        let mut log = String::new();
        vendor_registers(
            &mut io,
            Space::Mmio,
            &[register(4)],
            Lifecycle::Cold,
            &mut log,
            None,
        )
        .unwrap();
        assert_eq!(
            Readings::parse(Space::Mmio, &log).unwrap().unreadable,
            [4].into()
        );
    }
    #[test]
    fn discovery_failure_or_an_unknown_address_never_becomes_a_skipped_read() {
        let mut io = serial(&[]);
        let mut log = String::new();
        assert!(
            vendor_registers(
                &mut io,
                Space::Mmio,
                &[register(4)],
                Lifecycle::Cold,
                &mut log,
                Some(&Readings::default())
            )
            .is_err()
        );
        assert!(io.requests.is_empty());
        let known = Readings {
            values: [(4, 1)].into(),
            unreadable: [4].into(),
        };
        assert!(
            vendor_registers(
                &mut io,
                Space::Mmio,
                &[register(4)],
                Lifecycle::Cold,
                &mut log,
                Some(&known)
            )
            .is_err()
        );
        assert!(io.requests.is_empty());
    }
}
