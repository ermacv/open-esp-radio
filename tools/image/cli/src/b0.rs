//! Physical B0 procedure over the product's H4 boundary and pinned DTM peer.
use crate::{command, image};
use oer_bluetooth_hci::{
    VendorShutdownObservationEvent,
    bt_hci::{
        PacketKind,
        cmd::{Opcode, OpcodeGroup},
    },
    h4,
};
use oer_hil_link::peer::{Expected, PeerConsole, SerialLink};
use oer_image_build::{Check, Enforced, Result, esp_idf::catalog};
use oer_lab_arbiter::{Arbiter, Request, claims::Claim};
use oer_lab_board::stand::{Board, Via};
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::ExitCode,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RESET: u16 = 0x0c03;
const RAND: u16 = 0x2018;
const RX: u16 = 0x201d;
const TX: u16 = 0x201e;
const END: u16 = 0x201f;
const RX2: u16 = 0x2033;
const STOP: u16 = 0xfc00;
const CHANNEL: u8 = 19;
const TIMEOUT: Duration = Duration::from_secs(10);
const AIR_INTERVAL: Duration = Duration::from_millis(500);

struct H4<S> {
    serial: S,
    log: fs::File,
    shutdown_pending: Vec<[u8; 2]>,
}
impl<S: Read + Write> H4<S> {
    fn command(&mut self, opcode: u16, parameters: &[u8]) -> Result<Vec<u8>> {
        let mut command = vec![0; h4::COMMAND_FRAME_CAPACITY];
        let length = h4::write_command(
            Opcode::new(OpcodeGroup::new((opcode >> 10) as u8), opcode & 0x03ff),
            parameters,
            &mut command,
        )
        .map_err(|error| format!("H4 command {opcode:#06x}: {error:?}"))?;
        command.truncate(length);
        writeln!(self.log, "tx {}", hex::encode(&command))?;
        self.serial.write_all(&command)?;
        self.serial.flush()?;
        let deadline = Instant::now() + TIMEOUT;
        let mut event = Vec::new();
        loop {
            oer_os_process::check_cancelled()?;
            if Instant::now() >= deadline {
                return Err(format!("HCI {opcode:#06x} timed out").into());
            }
            let mut byte = [0];
            match self.serial.read(&mut byte) {
                Ok(0) => continue,
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
            writeln!(self.log, "rx {:02x}", byte[0])?;
            if event.is_empty() && byte[0] != h4::indicator(PacketKind::Event) {
                continue;
            } // ROM text before H4.
            event.push(byte[0]);
            let extent =
                h4::frame_length(&event).map_err(|error| format!("H4 event framing: {error:?}"))?;
            if extent == Some(event.len()) {
                if let Some(completed) = self.receive_event(&event, opcode)? {
                    return Ok(completed);
                }
                event.clear();
            }
        }
    }
    fn receive_event(&mut self, event: &[u8], opcode: u16) -> Result<Option<Vec<u8>>> {
        if let Some(pending) = shutdown_pending_from_h4(event) {
            writeln!(
                self.log,
                "LP shutdown pending: +0x24={} +0x2c={}",
                pending[0], pending[1]
            )?;
            self.shutdown_pending.push(pending);
            return Ok(None);
        }
        completion(event, opcode).map(Some)
    }

    fn end(&mut self) -> Result<u16> {
        let returned = self.command(END, &[])?;
        let bytes: [u8; 2] = returned
            .try_into()
            .map_err(|_| "LE Test End did not return a packet count")?;
        Ok(u16::from_le_bytes(bytes))
    }
}
/// The Controller's LP pending observation, decoded only from a complete
/// vendor event frame with the H4 event indicator.
fn shutdown_pending_from_h4(frame: &[u8]) -> Option<[u8; 2]> {
    let Ok((PacketKind::Event, body)) = h4::split(frame) else {
        return None;
    };
    VendorShutdownObservationEvent::from_hci_bytes(body).map(|event| event.pending())
}
fn completion(frame: &[u8], opcode: u16) -> Result<Vec<u8>> {
    let event = match h4::split(frame) {
        Ok((PacketKind::Event, event)) => event,
        _ => return Err("HCI completion has a wrong kind, extent, credit or opcode".into()),
    };
    if let [0x10, 1, code] = event {
        return Err(
            format!("Controller Hardware Error {code} while awaiting HCI {opcode:#06x}").into(),
        );
    }
    let [low, high] = opcode.to_le_bytes();
    if event.len() < 6
        || event.first() != Some(&0x0e)
        || usize::from(event[1]) + 2 != event.len()
        || event[2..5] != [1, low, high]
    {
        return Err("HCI completion has a wrong kind, extent, credit or opcode".into());
    }
    if event[5] != 0 {
        return Err(format!("HCI {opcode:#06x} status={:#04x}", event[5]).into());
    }
    Ok(event[6..].to_vec())
}
fn peer_end<L: oer_hil_link::peer::PeerLink>(peer: &mut PeerConsole<L>) -> Result<u64> {
    let reports = peer.command("END", "END", TIMEOUT, &["END"])?;
    let [report] = reports.as_slice() else {
        return Err("peer did not return one END report".into());
    };
    Ok(report
        .field("packets")
        .ok_or("peer END has no packet count")?
        .parse()?)
}

fn sram_accounting(base: &Path, head: &Path) -> Result<serde_json::Value> {
    use serde_json::Value;
    let base: Value = serde_json::from_slice(&fs::read(base)?)?;
    let head: Value = serde_json::from_slice(&fs::read(head)?)?;
    sram_accounting_values(&base, &head)
}

fn sram_accounting_values(
    base: &serde_json::Value,
    head: &serde_json::Value,
) -> Result<serde_json::Value> {
    use serde_json::Value;
    let runtime = |value: &Value| value["elfs"]["runtime"].clone();
    let a = runtime(&base);
    let b = runtime(&head);
    let value = |v: &Value, path: &str| -> Result<i64> {
        v.pointer(path)
            .and_then(Value::as_i64)
            .filter(|value| *value >= 0)
            .ok_or_else(|| format!("sizes.json has no {path}").into())
    };
    let span = |v: &Value| -> Result<i64> {
        let sections = v["sections"].as_object().ok_or("runtime has no sections")?;
        let mut low = i64::MAX;
        let mut high = 0;
        for (name, section) in sections {
            if section["memory"] != "sram" || name == ".rtc_fast.persistent" {
                continue;
            }
            let bytes = value(section, "/bytes")?;
            if bytes == 0 {
                continue;
            }
            let address = value(section, "/address")?;
            low = low.min(address);
            high = high.max(address.checked_add(bytes).ok_or("SRAM extent overflow")?);
        }
        if high <= low {
            return Err("no nonempty valid SRAM span".into());
        }
        high.checked_sub(low)
            .ok_or_else(|| "SRAM span overflow".into())
    };
    let difference = |head: i64, base: i64| -> Result<i64> {
        head.checked_sub(base)
            .ok_or_else(|| "SRAM accounting overflow".into())
    };
    let text = difference(
        value(&b, "/totals/sram/code")?,
        value(&a, "/totals/sram/code")?,
    )?;
    let owned = difference(
        value(&b, "/totals/sram/ram")?,
        value(&a, "/totals/sram/ram")?,
    )?;
    let irq = difference(
        value(&b, "/sections/.irq_stacks/bytes")?,
        value(&a, "/sections/.irq_stacks/bytes")?,
    )?;
    let full = difference(span(&b)?, span(&a)?)?;
    let rw = difference(difference(owned, text)?, irq)?;
    let alignment = difference(full, owned)?;
    Ok(
        json!({"base_provenance":base["provenance"],"head_provenance":head["provenance"],
        "sram_text":text,"sram_rw":rw,"allocated_irq_stacks":irq,"alignment":alignment,
        "full_sram":full,"limit":7360,"passed":full<=7360}),
    )
}

fn dma_snapshot(
    access: &oer_lab_board::lock::DeviceAccess,
    bundle: &Path,
    output: &Path,
) -> Result<()> {
    let sizes: serde_json::Value = serde_json::from_slice(&fs::read(bundle.join("sizes.json"))?)?;
    let section = &sizes["elfs"]["runtime"]["sections"][".dma.bss"];
    let address = section["address"].as_u64().ok_or("DMA section address")?;
    let bytes = section["bytes"].as_u64().ok_or("DMA section size")?;
    fs::create_dir_all(output.join("post-mortem"))?;
    let path = output.join("post-mortem/dma.bin");
    let path = path.to_str().ok_or("DMA snapshot path encoding")?;
    if path.contains(['{', '}']) {
        return Err("DMA snapshot path contains Tcl braces".into());
    }
    let openocd = oer_lab_board::openocd::Openocd::locate()?;
    let mut arguments = openocd.register_arguments("esp32s31", access.id(), &[]);
    *arguments.last_mut().ok_or("OpenOCD command argument")? =
        format!("init; halt; dump_image {{{path}}} {address:#x} {bytes}; resume; shutdown");
    let operation = access.operation()?;
    let mut command = oer_os_process::command(&openocd.program);
    command.args(arguments);
    operation.lifetime().pin(&mut command)?;
    let result = oer_os_process::output(&mut command, Some(Duration::from_secs(30)))?;
    let status = result.status;
    fs::write(
        output.join("post-mortem/dma-openocd.log"),
        [result.stdout, result.stderr].concat(),
    )?;
    if !status.success() || fs::metadata(path)?.len() != bytes {
        return Err("OpenOCD did not capture the complete DMA section".into());
    }
    Ok(())
}

pub fn run(
    root: &Path,
    selector: &str,
    peer_selector: &str,
    base_sizes: &Path,
) -> Result<ExitCode> {
    let _signals = oer_os_process::install_signal_handlers()?;
    let stand = oer_lab_model::Stand::load(&oer_lab_model::path()?)?;
    let selected = stand.resolve(selector)?;
    let peer_selected = stand.resolve(peer_selector)?;
    if selected.chip != "esp32s31" || peer_selected.chip != "esp32c5" {
        return Err("B0 requires an S31 DUT and the pinned C5 DTM peer".into());
    }
    // This stand procedure uses an explicit 0 dBm cap and the actual lab
    // router's regulatory country, just as the Wi-Fi image procedures do.
    let (_, country) = command::w0_country(&stand)?;
    let bundle = image::build(
        root,
        "b0",
        Some(&image::Stand::B0 {
            country,
            cap_dbm: 0,
        }),
        false,
        Enforced::Only([Check::Interrupts].into()),
        None,
    )?;
    let budget = sram_accounting(base_sizes, &bundle.directory.join("sizes.json"))?;
    eprintln!("fw: B0 full SRAM {}", serde_json::to_string(&budget)?);
    if budget["passed"] != true {
        return Err("B0 exceeds the owner's full 7360 B SRAM ceiling; stop before RF".into());
    }
    let peer_build = catalog::build(root, "ble-dtm-peer")?;
    let entries = catalog::entries(root)?;
    let entry = catalog::entry(&entries, "ble-dtm-peer")?;
    let peer_bundle = catalog::bundle(root, entry, &peer_build, &root.join("target/fw/b0-peer"))?;
    let id = format!(
        "b0-{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        std::process::id()
    );
    let output =
        oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Data, "evidence/371/b0-runs")?
            .join(&id);
    fs::create_dir_all(&output)?;
    let mut report = json!({"schema":1,"scope":"B0 physical H4/DTM","run_id":id,"board":selected.id,
        "peer":peer_selected.id,"country":String::from_utf8(country.to_vec())?,"cap_dbm":0,"channel":CHANNEL,
        "phy":"1M","interval_ms":AIR_INTERVAL.as_millis(),"minimum_packets":10,"boots":[],"passed":false,
        "source_commit":oer_os_process::git::text(root,["rev-parse","HEAD"])?,
        "source_dirty":!oer_os_process::git::text(root,["status","--porcelain"])?.is_empty(),
        "firmware":serde_json::from_slice::<serde_json::Value>(&fs::read(bundle.directory.join("bundle.json"))?)?,
        "peer_build":peer_build,"budget":budget});
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    let mut request = Request::from_environment("cargo fw b0: two-boot 1M DTM")?;
    request.run = Some(id.clone());
    request.claims = vec![
        Claim::board(&selected.mac),
        Claim::board(&peer_selected.mac),
        Claim::exclusive("air"),
    ];
    eprintln!("fw: {id}; waiting for both boards and exclusive air");
    let mut grant = Arbiter::open()?.acquire(&request)?;
    grant.supervise_self(false);
    let target = Board::attached(root, &stand, selector)?
        .lease(grant.device(&selected.device_id()?).ok_or("no DUT lease")?)?;
    let peer_board = Board::attached(root, &stand, peer_selector)?.lease(
        grant
            .device(&peer_selected.device_id()?)
            .ok_or("no peer lease")?,
    )?;
    let store = oer_lab_board::image::Store::open()?;
    target.flash(&store, &bundle, "b0", Via::Usb, &id)?;
    peer_board.flash(&store, &peer_bundle, "ble-dtm-peer", Via::Usb, &id)?;
    let transcript = oer_hil_link::peer::PeerTranscript::default();
    let link = SerialLink::new(peer_board.port(), Box::new(peer_board.open_console()?));
    let mut peer = PeerConsole::open_recorded(
        link,
        &transcript,
        Expected {
            peer: "C5 DTM",
            protocol: 1,
            stack: None,
            restore: "cargo hil firmware flash ble-dtm-peer --board c5-a",
        },
        TIMEOUT,
    )?;
    let result = (|| -> Result<()> {
        for boot in 0..2 {
            let serial = target.reset_into_console()?;
            let mut hci = H4 {
                serial,
                log: fs::File::create(output.join(format!("boot-{boot}-h4.log")))?,
                shutdown_pending: Vec::new(),
            };
            std::thread::sleep(Duration::from_millis(250));
            hci.command(RESET, &[])?;
            let random = hci.command(RAND, &[])?;
            if random.len() != 8 {
                return Err("LE Rand returned a wrong extent".into());
            }
            peer.command("RX", &format!("RX 1 {CHANNEL} 1M"), TIMEOUT, &[])?;
            std::thread::sleep(AIR_INTERVAL);
            let peer_idle = peer_end(&mut peer)?;
            hci.command(RX, &[CHANNEL])?;
            std::thread::sleep(AIR_INTERVAL);
            let dut_idle = hci.end()?;
            peer.command("RX", &format!("RX 1 {CHANNEL} 1M"), TIMEOUT, &[])?;
            hci.command(TX, &[CHANNEL, 37, 0])?;
            std::thread::sleep(AIR_INTERVAL);
            hci.end()?;
            let to_peer = peer_end(&mut peer)?;
            hci.command(RX2, &[CHANNEL, 1, 0])?;
            peer.command("TX", &format!("TX 2 {CHANNEL} 37 0 1M"), TIMEOUT, &[])?;
            std::thread::sleep(AIR_INTERVAL);
            let from_peer = hci.end()?;
            peer_end(&mut peer)?;
            hci.command(RESET, &[])?;
            hci.command(RAND, &[])?;
            hci.command(STOP, &[])?;
            report["boots"].as_array_mut().ok_or("boot report array")?.push(json!({"boot":boot,
                "peer_idle":peer_idle,"dut_idle":dut_idle,"to_peer":to_peer,"from_peer":from_peer,"ordinary_stop":true,"lp_timer_pending":hci.shutdown_pending.iter().map(|[software, rollover]| json!({"state_0024":software,"state_002c":rollover})).collect::<Vec<_>>()}));
            fs::write(
                output.join("manifest.json"),
                serde_json::to_vec_pretty(&report)?,
            )?;
            if peer_idle != 0 || dut_idle != 0 || to_peer < 10 || from_peer < 10 {
                return Err("B0 quiet/count gate failed".into());
            }
        }
        Ok(())
    })();
    let cleanup = peer.command("RESET", "RESET", TIMEOUT, &[]);
    transcript.save(&output.join("peer.log"))?;
    if result.is_err() {
        let elf = fs::read(bundle.runtime_elf()).ok();
        if let Some(access) = grant.device(&selected.device_id()?) {
            oer_hil_lab::post_mortem::jtag_snapshot_through_stand_openocd(
                "esp32s31",
                access,
                &output,
                elf.as_deref(),
            );
            if let Err(error) = dma_snapshot(access, &bundle.directory, &output) {
                eprintln!("fw: no DMA post-mortem: {error}");
            }
        }
        let _ = target.reset_into_console();
    }
    report["error"] = result.as_ref().err().map(|e| e.to_string()).into();
    report["cleanup_error"] = cleanup.as_ref().err().map(|e| e.to_string()).into();
    report["passed"] = (result.is_ok() && cleanup.is_ok()).into();
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    eprintln!("fw: B0 evidence {}", output.display());
    result?;
    cleanup?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completions_require_the_original_opcode_single_credit_and_exact_extent() {
        let good = [4, 0x0e, 4, 1, 3, 0x0c, 0];
        assert!(completion(&good, RESET).is_ok());
        for (index, value) in [(0, 1), (1, 0x0f), (2, 3), (3, 2), (4, 4), (5, 0x20), (6, 1)] {
            let mut bad = good;
            bad[index] = value;
            assert!(completion(&bad, RESET).is_err());
        }
        assert!(completion(&good[..6], RESET).is_err());
    }

    #[test]
    fn shutdown_pending_is_retained_before_the_command_completion() {
        let directory = tempfile::tempdir().expect("own H4 log");
        let log_path = directory.path().join("h4.log");
        let mut hci = H4 {
            serial: std::io::Cursor::new(Vec::<u8>::new()),
            log: fs::File::create(&log_path).unwrap(),
            shutdown_pending: Vec::new(),
        };
        assert!(
            hci.receive_event(&[4, 0xff, 3, 1, 7, 3], STOP)
                .unwrap()
                .is_none()
        );
        assert_eq!(hci.shutdown_pending, [[7, 3]]);
        assert_eq!(
            hci.receive_event(&[4, 0x0e, 4, 1, 0, 0xfc, 0], STOP)
                .unwrap(),
            Some(Vec::new())
        );
        assert!(
            fs::read_to_string(log_path)
                .unwrap()
                .contains("+0x24=7 +0x2c=3")
        );
    }

    #[test]
    fn nonzero_shutdown_pending_survives_only_a_complete_event_frame() {
        assert_eq!(
            shutdown_pending_from_h4(&[4, 0xff, 3, 1, 7, 3]),
            Some([7, 3])
        );
        for malformed in [
            &[4, 0xff, 3, 1, 7][..],
            &[4, 0xff, 2, 1, 7, 3],
            &[4, 0xff, 3, 2, 7, 3],
            &[1, 0xff, 3, 1, 7, 3],
            &[4, 0xff, 3, 1, 7, 3, 0],
            &[],
        ] {
            assert_eq!(shutdown_pending_from_h4(malformed), None);
        }
    }

    #[test]
    fn a_hardware_error_is_not_reported_as_a_command_timeout() {
        let error = completion(&[4, 0x10, 1, 9], RAND).expect_err("physical clock failure");
        assert!(error.to_string().contains("Hardware Error 9"));
    }

    #[test]
    fn the_full_sram_limit_includes_alignment_between_owned_sections() {
        let directory = tempfile::tempdir().expect("own image summaries");
        let base = directory.path().join("base.json");
        let head = directory.path().join("head.json");
        let fixture = |gap| {
            json!({"elfs":{"runtime":{
            "totals":{"sram":{"code":16,"ram":48}},
            "sections":{
                ".text":{"memory":"sram","address":1000,"bytes":16},
                ".irq_stacks":{"memory":"sram","address":1016+gap,"bytes":32}
            }}}})
        };
        fs::write(
            &base,
            serde_json::to_vec(&fixture(0)).expect("base summary"),
        )
        .expect("write base");
        for (gap, passed) in [(7360, true), (7361, false)] {
            fs::write(
                &head,
                serde_json::to_vec(&fixture(gap)).expect("head summary"),
            )
            .expect("write head");
            let report = sram_accounting(&base, &head).expect("full accounting");
            assert_eq!(report["full_sram"], gap);
            assert_eq!(report["alignment"], gap);
            assert_eq!(report["passed"], passed);
        }
    }
    #[test]
    fn malformed_sram_summaries_are_errors_instead_of_passes_or_panics() {
        let valid = json!({"elfs":{"runtime":{
            "totals":{"sram":{"code":1,"ram":2}},
            "sections":{".irq_stacks":{"memory":"sram","address":100,"bytes":1}}
        }}});
        let mut cases = Vec::new();
        for sections in [
            json!({}),
            json!({".flash":{"memory":"flash","address":1,"bytes":1}}),
        ] {
            let mut bad = valid.clone();
            bad["elfs"]["runtime"]["sections"] = sections;
            cases.push(bad);
        }
        for (key, value) in [
            ("bytes", 0),
            ("bytes", -1),
            ("address", -1),
            ("address", i64::MAX),
        ] {
            let mut bad = valid.clone();
            bad["elfs"]["runtime"]["sections"][".irq_stacks"][key] = json!(value);
            cases.push(bad);
        }
        for bad in cases {
            assert!(sram_accounting_values(&valid, &bad).is_err());
            assert!(sram_accounting_values(&bad, &valid).is_err());
        }
        let mut base = valid.clone();
        let mut head = valid.clone();
        base["elfs"]["runtime"]["totals"]["sram"]["code"] = json!(i64::MAX);
        head["elfs"]["runtime"]["totals"]["sram"]["ram"] = json!(i64::MAX);
        // Individually representable nonnegative inputs overflow decomposition.
        assert!(sram_accounting_values(&base, &head).is_err());
    }
}
