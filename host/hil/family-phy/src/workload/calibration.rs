//! The vendor-versus-production calibration cross-check on the board under
//! test.
//!
//! Each round flashes and boots the vendor firmware, then writes the
//! scenario's production image back and boots it, so both sides see the
//! same board temperature drift. Every flash goes through the run's
//! [`BoardImages`], that is the flash operation under the run's lock, which
//! journals it. Each production boot receives a fresh startup artifact path,
//! so it calibrates fully and publishes its retained calibration. Every boot
//! is a typed observation of the repetition; the chip's comparison
//! ([`Comparison`]) is its result, and anything but MATCH fails the
//! repetition.
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use oer_hil_schema::image::ImageClass;
use oer_hil_link::{SerialCapture, Target};
use oer_hil_workload::context::Context;
use oer_vendor_phy_calibration_capture::boots::{
    Images, Lifecycle, ProductionBoot, Readings, Transmitted, VendorBoot,
};
use oer_vendor_phy_calibration_capture::space::{Register, Space};
use oer_vendor_phy_calibration_capture::vendor;

use super::reference::{self, Peer};
use crate::Result;
use crate::comparison::Comparison;
use crate::scenario::{Window, vendor_projects};

/// Longest wait for one vendor boot's closed report.
const VENDOR_BOOT_TIMEOUT: Duration = Duration::from_secs(20);
/// Serial read poll interval; the USB-Serial/JTAG console ignores the rate.
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// Longest wait for one Wi-Fi radio restart on either side.
const RESTART_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest wait for one production register image window.
const REGISTER_IMAGE_TIMEOUT: Duration = Duration::from_secs(2);
/// Longest wait for one IEEE 802.15.4 session command of production.
const SESSION_TIMEOUT: Duration = Duration::from_secs(10);
/// Console characters an unanswered register read reports.
const CONSOLE_TAIL_CHARS: usize = 2000;

/// What one repetition compares.
pub(crate) struct Calibration<'a> {
    pub lifecycle: Lifecycle,
    pub boots: u8,
    pub image: ImageClass,
    pub vendor_project: &'a str,
    pub windows: &'a [Window],
    pub transmit: bool,
    pub vendor_only: bool,
}

pub(crate) fn run<C: Comparison>(
    spec: &Calibration<'_>,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let root = oer_os_process::built_root();
    let chip = context.lab.chip();
    let projects = vendor_projects(chip)?;
    let images = context.images()?;
    // The pinned ESP-IDF build of the vendor firmware is incremental; the
    // pipeline encodes the HIL bootloader, partition table and OTA selection
    // around its application.
    let build = oer_image_esp_idf::build(
        &root,
        chip,
        &[oer_image_esp_idf::Project {
            name: spec.vendor_project.to_owned(),
            source: root.join(&projects).join(spec.vendor_project),
            chip: chip.to_owned(),
        }],
    )?
    .pop()
    .ok_or("the vendor firmware build produced no image")?;
    let vendor_bundle = oer_image_build::bundle::around(
        &root,
        chip,
        Path::new(&build.application),
        oer_image_build::bundle::BootFiles::Encode {
            elf: Path::new(&build.elf),
        },
        &output.join("vendor-image"),
    )?;
    let identity = Images {
        started_unix_seconds: oer_os_durable::unix_seconds(),
        vendor_project: spec.vendor_project.to_owned(),
        vendor_application_sha256: build.application_sha256.clone(),
        vendor_idf_revision: build.idf_revision.clone(),
        production_image: spec.image.id().to_owned(),
        production_application_sha256: oer_os_durable::sha256_file(
            &images.scenario_image().application(),
        )?,
    };
    context.results.observe("images", &identity);
    let images_read = C::images(&root)?;
    let (registers, analog) = (&images_read.registers, &images_read.analog);
    let windows = window_registers(spec.windows);
    let journal_name = format!("vendor-{}", spec.vendor_project);
    let (mut vendor, mut production) = (Vec::new(), Vec::new());
    for boot in 1..=spec.boots {
        images.flash(&vendor_bundle, &journal_name)?;
        let stem = output.join(format!("vendor-{boot:02}"));
        let vendor_boot = if spec.lifecycle.ieee802154() {
            reference_boot(context, spec, registers, analog, &windows, &stem)?
        } else {
            calibration_boot(
                context,
                spec.lifecycle,
                C::VENDOR_OBJECT,
                registers,
                analog,
                &stem,
            )?
        };
        context
            .results
            .observe(format!("vendor-boot-{boot:02}"), &vendor_boot);
        let readable: Vec<usize> = registers
            .iter()
            .enumerate()
            .filter(|(_, register)| vendor_boot.registers.values.contains_key(&register.address))
            .map(|(index, _)| index)
            .collect();
        vendor.push(vendor_boot);
        if spec.vendor_only {
            continue;
        }
        images.restore()?;
        let production_boot = production_boot(
            context,
            spec.lifecycle,
            &output.join(format!("production-{boot:02}")),
            registers,
            &readable,
            analog,
        )?;
        context
            .results
            .observe(format!("production-boot-{boot:02}"), &production_boot);
        production.push(production_boot);
    }
    if spec.vendor_only {
        // The board carries the vendor firmware; the runner flashes the
        // next scenario's image.
        context.results.claim(
            "vendor PHY state captured for an investigation",
            &["production-equivalence"],
        );
        return Ok(());
    }
    let compared = C::compare(
        spec.lifecycle,
        &identity,
        &vendor,
        &production,
        &images_read,
    )?;
    context.results.observe("comparison", &compared.summary);
    context.results.claim(
        "production PHY calibration and register state within the vendor's boot-to-boot spread",
        &["modem-sleep", "phy-param-bytes-outside-the-relation"],
    );
    match compared.mismatch {
        None => Ok(()),
        Some(mismatch) => Err(format!("calibration cross-check {mismatch}").into()),
    }
}

/// The console port of the board under test, just reset. The port is
/// opened through the stand's opener, which never resets the chip on its
/// own.
fn reset_console(port: &Path) -> Result<oer_lab_devices::port::Port> {
    use oer_lab_devices::reset::{open_without_reset, reset_usb_serial_jtag};
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
) -> Result<VendorBoot> {
    let (console, mut serial) = vendor_boot(&context.lab.dut.serial)?;
    std::fs::write(stem.with_extension("log"), &console)?;
    let mut objects = vendor::parse(&console)?;
    let calibration = objects
        .remove(vendor_object)
        .ok_or_else(|| format!("the vendor boot reported no {vendor_object}"))?;
    let mut replies = String::new();
    if lifecycle == Lifecycle::Restart {
        vendor_restart(&mut serial, &mut replies)?;
    }
    vendor_registers(&mut serial, Space::Mmio, registers, lifecycle, &mut replies)?;
    let mut analog_replies = String::new();
    vendor_registers(
        &mut serial,
        Space::Analog,
        analog,
        lifecycle,
        &mut analog_replies,
    )?;
    Ok(VendorBoot {
        calibration: Some(calibration),
        registers: Readings::parse(Space::Mmio, &replies)?,
        analog: Readings::parse(Space::Analog, &analog_replies)?,
        windows: None,
        transmitted: None,
    })
}

/// One boot of the IEEE 802.15.4 reference firmware, configured and
/// receiving, with the investigation reads the scenario asks for.
fn reference_boot(
    context: &Context<'_>,
    spec: &Calibration<'_>,
    registers: &[Register],
    analog: &[Register],
    windows: &[Register],
    stem: &Path,
) -> Result<VendorBoot> {
    let mut peer = Peer::boot(
        reset_console(&context.lab.dut.serial)?,
        spec.lifecycle == Lifecycle::Ieee802154Restart,
    )?;
    let replies = peer.registers(Space::Mmio, registers)?;
    let analog_replies = peer.registers(Space::Analog, analog)?;
    let window_readings = (!windows.is_empty())
        .then(|| Readings::parse(Space::Mmio, &peer.registers(Space::Mmio, windows)?))
        .transpose()?;
    let transmitted = if spec.transmit {
        peer.transmit()?;
        let analog = Readings::parse(Space::Analog, &peer.registers(Space::Analog, analog)?)?;
        let windows = (!windows.is_empty())
            .then(|| Readings::parse(Space::Mmio, &peer.registers(Space::Mmio, windows)?))
            .transpose()?;
        Some(Transmitted { analog, windows })
    } else {
        None
    };
    std::fs::write(stem.with_extension("log"), &peer.console)?;
    Ok(VendorBoot {
        calibration: None,
        registers: Readings::parse(Space::Mmio, &replies)?,
        analog: Readings::parse(Space::Analog, &analog_replies)?,
        windows: window_readings,
        transmitted,
    })
}

/// The console of one reset vendor boot, up to its closed report, and the
/// open port the firmware keeps answering on.
fn vendor_boot(port: &Path) -> Result<(String, oer_lab_devices::port::Port)> {
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
fn vendor_restart(
    serial: &mut oer_lab_devices::port::Port,
    replies: &mut String,
) -> Result<()> {
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
    serial: &mut oer_lab_devices::port::Port,
    space: Space,
    registers: &[Register],
    lifecycle: Lifecycle,
    replies: &mut String,
) -> Result<()> {
    let mut buffer = [0; 256];
    for register in registers {
        serial.write_all(space.request(register.address).as_bytes())?;
        let boots = vendor::reports(replies);
        let started = Instant::now();
        loop {
            if space.replies(replies)?.contains_key(&register.address) {
                break;
            }
            if vendor::reports(replies) > boots {
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

/// One cold production boot publishing its retained calibration to a fresh
/// startup artifact in `directory`, then reading the register image at the
/// `readable` indices the vendor boot answered and the whole analog image.
fn production_boot(
    context: &Context<'_>,
    lifecycle: Lifecycle,
    directory: &Path,
    registers: &[Register],
    readable: &[usize],
    analog: &[Register],
) -> Result<ProductionBoot> {
    std::fs::create_dir_all(directory)?;
    let artifact: PathBuf = directory.join("startup-artifact.bin");
    let mut lab = context.lab.clone();
    lab.dut.startup_artifact = Some(artifact.clone());
    let capture = context.capture_of(&lab, directory)?;
    let result = (|| {
        // The IEEE 802.15.4 session replaces the image's own initialization:
        // it is admitted only before it, and publishes no startup artifact.
        let status = if lifecycle.ieee802154() {
            capture.request_image_keys(SESSION_TIMEOUT)?;
            None
        } else {
            let (_, status) = capture.prepare_startup(Target {
                dut: &lab,
                station: &lab.station,
                settings: oer_hil_protocol::wifi::TargetSettings::default(),
            })?;
            status
        };
        if lifecycle == Lifecycle::Restart {
            let evidence = capture
                .wait_command::<oer_hil_protocol::wifi::RadioRestarted>(
                    capture.command(
                        oer_hil_protocol::wifi::RestartRadio,
                        oer_hil_link::PROTOCOL_READY_TIMEOUT,
                    )?,
                    RESTART_TIMEOUT,
                )
                .map(|completed| completed.0)?;
            if evidence.rf != oer_hil_protocol::wifi::WifiRadioRestartRf::ClosedAndWoken {
                return Err(format!("the production restart kept RF open: {evidence:?}").into());
            }
        }
        if lifecycle.ieee802154() {
            ieee802154_session(&capture)?;
            if lifecycle == Lifecycle::Ieee802154Restart {
                // The session stops and starts its radio client, then
                // configures and receives again without transmitting.
                let restarted = capture
                    .request(
                        0,
                        oer_hil_protocol::ieee802154::RestartSessionRadio,
                        SESSION_TIMEOUT,
                    )
                    .map(|response| response.0)?;
                if !restarted.rf_closed {
                    return Err(format!(
                        "the production session restart kept RF open: {restarted:?}"
                    )
                    .into());
                }
            }
        }
        let mut values = Readings::default();
        for window in windows(readable) {
            let words = capture
                .request(
                    0,
                    oer_hil_protocol::phy::ReadRegisterImage(window),
                    REGISTER_IMAGE_TIMEOUT,
                )?
                .0;
            for (offset, value) in words.values.iter().enumerate() {
                let register = &registers[usize::from(words.first) + offset];
                values.values.insert(register.address, *value);
            }
        }
        let mut analog_values = Readings::default();
        let every: Vec<usize> = (0..analog.len()).collect();
        for window in windows(&every) {
            let bytes = capture
                .request(
                    0,
                    oer_hil_protocol::phy::ReadAnalogImage(window),
                    REGISTER_IMAGE_TIMEOUT,
                )?
                .0;
            if usize::from(bytes.length) != analog.len() {
                return Err(format!(
                    "production reads {} analog registers, the published model {}",
                    bytes.length,
                    analog.len()
                )
                .into());
            }
            for (offset, value) in bytes.values.iter().enumerate() {
                let register = &analog[usize::from(bytes.first) + offset];
                analog_values
                    .values
                    .insert(register.address, u32::from(*value));
            }
        }
        // The firmware projects the calibration it published; the host
        // compares the words without replaying the calibration itself.
        let projection = if status.is_some() {
            Some(calibration_projection(&capture)?)
        } else {
            None
        };
        Ok((status, values, analog_values, projection))
    })();
    let (status, registers, analog, projection) = capture.finish_with(result)?;
    let calibration = if lifecycle.ieee802154() {
        None
    } else {
        let status = status.ok_or("the production image published no startup artifact")?;
        if status.disposition
            != oer_hil_protocol::phy::StartupArtifactDisposition::Created
        {
            return Err(format!(
                "the production boot did not calibrate cold: {:?}",
                status.disposition
            )
            .into());
        }
        Some(projection.ok_or("the production image projected no calibration")?)
    };
    Ok(ProductionBoot {
        calibration,
        registers,
        analog,
    })
}

/// Start production's IEEE 802.15.4 session on the reference firmware's
/// channel and receive; the session's default PIB transmits at the same
/// power.
fn ieee802154_session(capture: &SerialCapture) -> Result<()> {
    capture
        .request(
            0,
            oer_hil_protocol::ieee802154::StartSession(
                oer_hil_protocol::ieee802154::Ieee802154SessionConfig {
                    channel: reference::CHANNEL,
                    pan_id: reference::PAN_ID,
                    short_address: reference::SHORT_ADDRESS,
                    extended_address: reference::EXTENDED_ADDRESS,
                    promiscuous: false,
                    background_maintenance: false,
                    enhanced_ack: false,
                    wifi_coexistence: false,
                },
            ),
            SESSION_TIMEOUT,
        )
        .map(|response| response.0)?;
    capture
        .request(
            0,
            oer_hil_protocol::ieee802154::ReceiveSession,
            Duration::from_secs(5),
        )
        .map(drop)
}

/// Requests covering the ascending `indices` in runs of at most one reply.
/// The production output bytes of the parent root's calibration projection,
/// parent words and committed state words, as the image projected the
/// retained calibration it published (`phy/calibration-projection/read`):
/// each word little-endian, in the relation's order.
fn calibration_projection(capture: &SerialCapture) -> Result<Vec<u8>> {
    let mut words = Vec::new();
    let mut length = None;
    while length.is_none_or(|length| words.len() < length) {
        let window = capture
            .request(
                0,
                oer_hil_protocol::phy::ReadCalibrationProjection(
                    oer_hil_protocol::phy::PhyCalibrationProjectionRequest {
                        first: words.len() as u16,
                        count: length.map_or(
                            oer_hil_protocol::phy::PHY_CALIBRATION_PROJECTION_WORDS,
                            |length: usize| {
                                (length - words.len())
                                    .min(oer_hil_protocol::phy::PHY_CALIBRATION_PROJECTION_WORDS)
                            },
                        ) as u8,
                    },
                ),
                REGISTER_IMAGE_TIMEOUT,
            )?
            .0;
        if usize::from(window.first) != words.len() || window.values.is_empty() {
            return Err("the calibration projection window does not continue the words".into());
        }
        length = Some(usize::from(window.length));
        words.extend(window.values.iter().copied());
    }
    Ok(words.iter().flat_map(|word| word.to_le_bytes()).collect())
}

fn windows(indices: &[usize]) -> Vec<oer_hil_protocol::phy::PhyRegisterImageRequest> {
    let mut windows: Vec<oer_hil_protocol::phy::PhyRegisterImageRequest> = vec![];
    for &index in indices {
        match windows.last_mut() {
            Some(window)
                if usize::from(window.first) + usize::from(window.count) == index
                    && usize::from(window.count)
                        < oer_hil_protocol::phy::PHY_REGISTER_IMAGE_WORDS =>
            {
                window.count += 1
            }
            _ => windows.push(oer_hil_protocol::phy::PhyRegisterImageRequest {
                first: index as u16,
                count: 1,
            }),
        }
    }
    windows
}

/// The registers of vendor `windows`, one per word.
fn window_registers(windows: &[Window]) -> Vec<Register> {
    windows
        .iter()
        .flat_map(|window| (0..window.words).map(move |word| window.address + 4 * word))
        .map(|address| Register {
            name: format!("window.{address:08x}"),
            address,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_windows_become_word_registers() {
        let registers = window_registers(&[
            Window {
                address: 0x2010_fc00,
                words: 2,
            },
            Window {
                address: 0x2010_08b8,
                words: 1,
            },
        ]);
        let addresses: Vec<u32> = registers.iter().map(|r| r.address).collect();
        assert_eq!(addresses, [0x2010_fc00, 0x2010_fc04, 0x2010_08b8]);
    }

    #[test]
    fn readable_indices_group_into_reply_windows() {
        let indices: Vec<usize> = (0..20).chain([30, 31]).collect();
        let windows: Vec<_> = windows(&indices)
            .iter()
            .map(|w| (w.first, w.count))
            .collect();
        assert_eq!(windows, [(0, 16), (16, 4), (30, 2)]);
    }

    #[test]
    fn vendor_console_replies_become_typed_readings() {
        let console = format!(
            "{}{}{}",
            vendor::register_line(0x2010_0434, 0xabcd),
            vendor::unreadable_line(0x2010_2800),
            vendor::register_line(0x2010_0438, 1)
        );
        let readings = Readings::parse(Space::Mmio, &console).unwrap();
        assert_eq!(readings.values[&0x2010_0434], 0xabcd);
        assert!(readings.unreadable.contains(&0x2010_2800));
        let boot = VendorBoot {
            calibration: Some(vec![1, 2]),
            registers: readings,
            ..VendorBoot::default()
        };
        // The observation the run bundle carries reads back as the boot.
        let value = serde_json::to_value(&boot).unwrap();
        assert_eq!(serde_json::from_value::<VendorBoot>(value).unwrap(), boot);
    }
}
